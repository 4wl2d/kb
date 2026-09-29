//! Building one snapshot inside the caller's `BEGIN IMMEDIATE` transaction: reuse parsed
//! documents by content id, parse only new content, record membership, and compute the
//! snapshot diagnostics (source + registry + parse + lints + cross-record validation).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{BuildStats, proposals, text};
use crate::corpus;
use crate::diag::{self, Diagnostic, Severity};
use crate::error::{KbError, Result};
use crate::knowledge::Overlay;
use crate::model::{ProfileLocation, RecordMeta};
use crate::parse::{lint_record, parse_record};
use crate::source::{SourceEntry, SourceTree, verify_content};
use crate::validate::{MetaInput, validate_metas};
use crate::versions::PARSER_VERSION;

/// Files read from a source per `SourceTree::read` call (bounds memory on cold builds).
const READ_BATCH: usize = 256;

pub const ACCEPTED: &str = "accepted";
pub const PROPOSAL: &str = "proposal";

/// One accepted member of the snapshot being built.
pub struct Member {
    pub path: String,
    pub doc: i64,
    pub content_id: String,
    pub record_id: Option<String>,
    pub meta: Option<Arc<RecordMeta>>,
}

/// Inserts content-addressed documents and counts parse work.
pub struct DocWriter<'t> {
    conn: &'t Connection,
    pub parsed: usize,
}

impl<'t> DocWriter<'t> {
    pub fn new(conn: &'t Connection) -> Self {
        DocWriter { conn, parsed: 0 }
    }

    pub fn conn(&self) -> &'t Connection {
        self.conn
    }

    /// Existing document for a content id (current parser version).
    pub fn lookup(&self, content_id: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .prepare_cached("SELECT id FROM docs WHERE content_id = ?1 AND parser = ?2")?
            .query_row(params![content_id, PARSER_VERSION], |r| r.get(0))
            .optional()?)
    }

    /// Existing document or a newly parsed one. Returns (doc id, reused).
    pub fn get_or_insert(
        &mut self,
        content_id: &str,
        path: &str,
        bytes: &[u8],
    ) -> Result<(i64, bool)> {
        match self.lookup(content_id)? {
            Some(id) => Ok((id, true)),
            None => Ok((self.insert(content_id, path, bytes)?, false)),
        }
    }

    /// Parse `bytes` and store the document with its FTS row, path and term rows.
    /// Diagnostics are stored without a path: the same content may live at many paths.
    ///
    /// Documents are reused by content id alone, so bytes that do not hash to a `sha256:`
    /// `content_id` (a file changed between listing and reading) fail the build instead of
    /// being stored under another content's id.
    pub fn insert(&mut self, content_id: &str, path: &str, bytes: &[u8]) -> Result<i64> {
        verify_content(path, content_id, bytes)?;
        self.parsed += 1;
        let raw = String::from_utf8_lossy(bytes);
        let parsed = match parse_record(path, bytes) {
            Ok(p) => p,
            Err(errors) => {
                self.conn
                    .prepare_cached(
                        "INSERT INTO docs (content_id, parser, raw, diagnostics, fts_len)
                         VALUES (?1, ?2, ?3, ?4, 0)",
                    )?
                    .execute(params![
                        content_id,
                        PARSER_VERSION,
                        raw,
                        serde_json::to_string(&detach(errors))?
                    ])?;
                return Ok(self.conn.last_insert_rowid());
            }
        };
        let meta = parsed.meta();
        let fts = text::fts_columns(&parsed);
        self.conn
            .prepare_cached(
                "INSERT INTO docs (content_id, parser, record_id, kind, status, title, meta,
                                   parsed, raw, diagnostics, fts_len)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?
            .execute(params![
                content_id,
                PARSER_VERSION,
                meta.id,
                meta.kind.as_str(),
                meta.status.as_str(),
                meta.title,
                serde_json::to_string(&meta)?,
                serde_json::to_string(&parsed)?,
                raw,
                serde_json::to_string(&detach(lint_record(&parsed)))?,
                i64::try_from(fts.tokens).unwrap_or(i64::MAX),
            ])?;
        let id = self.conn.last_insert_rowid();
        self.conn
            .prepare_cached(
                "INSERT INTO docs_fts (rowid, ids, title, aliases, normative, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                id,
                fts.ids,
                fts.title,
                fts.aliases,
                fts.normative,
                fts.body
            ])?;
        for row in text::path_rows(&parsed) {
            self.conn
                .prepare_cached(
                    "INSERT INTO doc_paths (doc, repo, dir_prefix, glob) VALUES (?1, ?2, ?3, ?4)",
                )?
                .execute(params![id, row.repo, row.dir_prefix, row.glob])?;
        }
        for t in text::term_rows(&parsed) {
            self.conn
                .prepare_cached(
                    "INSERT INTO doc_terms (doc, kind, value, pattern) VALUES (?1, ?2, ?3, ?4)",
                )?
                .execute(params![id, t.kind, t.value, t.pattern])?;
        }
        Ok(id)
    }

    /// Record `doc` as a member of `snapshot` at `path`.
    pub fn add_member(&self, snapshot: i64, origin: &str, path: &str, doc: i64) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO snapshot_docs (snapshot, origin, path, doc, record_id, kind)
                 SELECT ?1, ?2, ?3, id, record_id, kind FROM docs WHERE id = ?4",
            )?
            .execute(params![snapshot, origin, path, doc])?;
        Ok(())
    }

    /// Stored text of a document (the original bytes, decoded lossily as UTF-8).
    pub fn raw(&self, doc: i64) -> Result<String> {
        Ok(self
            .conn
            .prepare_cached("SELECT raw FROM docs WHERE id = ?1")?
            .query_row([doc], |r| r.get(0))?)
    }

    /// Stored record id, metadata and path-less diagnostics of a document.
    pub fn load(&self, doc: i64) -> Result<StoredDoc> {
        let (record_id, meta, diags): (Option<String>, Option<String>, String) = self
            .conn
            .prepare_cached("SELECT record_id, meta, diagnostics FROM docs WHERE id = ?1")?
            .query_row([doc], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let meta = match meta {
            Some(m) => Some(Arc::new(serde_json::from_str::<RecordMeta>(&m)?)),
            None => None,
        };
        Ok(StoredDoc {
            record_id,
            meta,
            diagnostics: serde_json::from_str(&diags)?,
        })
    }
}

/// What a snapshot build needs from a stored document (`None`s for parse failures).
pub struct StoredDoc {
    pub record_id: Option<String>,
    pub meta: Option<Arc<RecordMeta>>,
    /// Parse errors or lints, without a path.
    pub diagnostics: Vec<Diagnostic>,
}

/// Strip paths from content-level diagnostics (re-attached per snapshot member).
fn detach(mut diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    for d in &mut diags {
        d.path = None;
    }
    diags
}

/// Attach a member path to stored content-level diagnostics.
pub fn attach(mut diags: Vec<Diagnostic>, path: &str) -> Vec<Diagnostic> {
    for d in &mut diags {
        d.path = Some(path.to_string());
    }
    diags
}

/// Build snapshot `key` from `source` (and the optional overlay) in transaction `tx`.
pub fn build(
    tx: &Transaction<'_>,
    key: &str,
    source: &dyn SourceTree,
    loc: &ProfileLocation,
    overlay: Option<&Overlay>,
) -> Result<BuildStats> {
    let config = corpus::load_config(source, loc)?;
    let (registry, registry_diags) = corpus::load_registry(source, loc)?;
    let (files, list_diags) = corpus::list_records(source, loc, &config)?;
    let mut docs = DocWriter::new(tx);

    // Reuse documents by content id; read and parse only content the index lacks.
    let mut by_content: HashMap<String, i64> = HashMap::new();
    let mut missing: Vec<SourceEntry> = Vec::new();
    let mut missing_ids: HashSet<&str> = HashSet::new();
    for f in &files {
        if by_content.contains_key(&f.content_id) || missing_ids.contains(f.content_id.as_str()) {
            continue;
        }
        match docs.lookup(&f.content_id)? {
            Some(id) => {
                by_content.insert(f.content_id.clone(), id);
            }
            None => {
                missing_ids.insert(&f.content_id);
                missing.push(f.clone());
            }
        }
    }
    let reused_docs = files
        .iter()
        .filter(|f| by_content.contains_key(&f.content_id))
        .count();
    for chunk in missing.chunks(READ_BATCH) {
        let contents = source.read(chunk)?;
        if contents.len() != chunk.len() {
            return Err(KbError::internal(format!(
                "source `{}` returned {} files for {} requested",
                source.describe(),
                contents.len(),
                chunk.len()
            )));
        }
        for (entry, bytes) in chunk.iter().zip(contents) {
            let id = docs.insert(&entry.content_id, &entry.path, &bytes)?;
            by_content.insert(entry.content_id.clone(), id);
        }
    }

    tx.prepare_cached(
        "INSERT INTO snapshots (key, created, config, registry, diagnostics, stats, fts_docs, fts_avglen)
         VALUES (?1, (SELECT COALESCE(MAX(created), 0) + 1 FROM snapshots), ?2, ?3, '[]', '{}', 0, 0)",
    )?
    .execute(params![
        key,
        serde_json::to_string(&config)?,
        serde_json::to_string(&registry.data)?
    ])?;
    let snapshot = tx.last_insert_rowid();
    for f in &files {
        let doc = by_content[&f.content_id];
        docs.add_member(snapshot, ACCEPTED, &f.path, doc)?;
    }

    // Diagnostics of the accepted content.
    let mut diagnostics = list_diags;
    diagnostics.extend(registry_diags);
    let mut members = Vec::with_capacity(files.len());
    for f in &files {
        let doc = by_content[&f.content_id];
        let stored = docs.load(doc)?;
        diagnostics.extend(attach(stored.diagnostics, &f.path));
        members.push(Member {
            path: f.path.clone(),
            doc,
            content_id: f.content_id.clone(),
            record_id: stored.record_id,
            meta: stored.meta,
        });
    }
    let inputs: Vec<MetaInput> = members
        .iter()
        .filter_map(|m| {
            Some(MetaInput {
                path: m.path.clone(),
                meta: m.meta.clone()?,
            })
        })
        .collect();
    diagnostics.extend(validate_metas(&config, &registry, &inputs));

    let mut proposal_count = 0;
    let mut proposal_reused = 0;
    if let Some(ov) = overlay {
        let roots = loc.knowledge_roots(&config);
        let applied = proposals::apply(
            &mut docs, snapshot, &roots, ov, &members, &config, &registry,
        )?;
        proposal_count = applied.entries;
        proposal_reused = applied.reused_docs;
        diagnostics.extend(applied.warnings);
    }
    mark_shadowed(tx, snapshot)?;
    diag::normalize(&mut diagnostics);

    let count = |s: Severity| diagnostics.iter().filter(|d| d.severity == s).count();
    let stats = BuildStats {
        snapshot_key: key.to_string(),
        reused: false,
        files: files.len(),
        parsed: docs.parsed,
        reused_docs: reused_docs + proposal_reused,
        proposals: proposal_count,
        errors: count(Severity::Error),
        warnings: count(Severity::Warning),
        collected: 0,
        recovery: None,
    };
    let (fts_docs, fts_avglen): (i64, f64) = tx
        .prepare_cached(
            "SELECT COUNT(*), COALESCE(AVG(fts_len), 0) FROM docs WHERE id IN
               (SELECT doc FROM snapshot_docs
                WHERE snapshot = ?1 AND origin = 'accepted' AND record_id IS NOT NULL)",
        )?
        .query_row([snapshot], |r| Ok((r.get(0)?, r.get(1)?)))?;
    tx.prepare_cached(
        "UPDATE snapshots SET diagnostics = ?2, stats = ?3, fts_docs = ?4, fts_avglen = ?5
         WHERE id = ?1",
    )?
    .execute(params![
        snapshot,
        serde_json::to_string(&diagnostics)?,
        serde_json::to_string(&stats)?,
        fts_docs,
        fts_avglen
    ])?;
    Ok(stats)
}

/// Flag every member that is not the first file (by path) of its record id within the same
/// snapshot and origin, so read queries can filter duplicates without a per-row subquery.
/// Duplicated ids are rare (they are DUPLICATE_ID errors), so only their rows are touched.
fn mark_shadowed(conn: &rusqlite::Connection, snapshot: i64) -> Result<()> {
    let dups: Vec<(String, String)> = conn
        .prepare_cached(
            "SELECT origin, record_id FROM snapshot_docs INDEXED BY snapshot_docs_record
             WHERE snapshot = ?1 AND record_id IS NOT NULL
             GROUP BY origin, record_id HAVING COUNT(*) > 1",
        )?
        .query_map([snapshot], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (origin, id) in dups {
        conn.prepare_cached(
            "UPDATE snapshot_docs SET shadowed = 1
             WHERE snapshot = ?1 AND origin = ?2 AND record_id = ?3 AND path > (
               SELECT MIN(path) FROM snapshot_docs
               WHERE snapshot = ?1 AND origin = ?2 AND record_id = ?3)",
        )?
        .execute(rusqlite::params![snapshot, origin, id])?;
    }
    Ok(())
}
