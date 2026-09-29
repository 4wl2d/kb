//! [`IndexView`]: the SQLite-backed [`KnowledgeView`] of one snapshot and its proposals.
//!
//! A view holds a read transaction for its whole lifetime, so every query observes the
//! same committed state even while other processes build or garbage-collect snapshots.
//! Queries touch only the rows they need; nothing loads the whole corpus.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, params};

use super::Index;
use super::build::{ACCEPTED, PROPOSAL};
use super::text::{
    self, TERM_ALIAS_FIRST, TERM_ALIAS_PREFIX, TERM_CONCEPT, fts_string, query_tokens,
};
use crate::diag::Diagnostic;
use crate::error::{ErrorCode, KbError, Result};
use crate::glob::{RepoGlob, dir_prefixes};
use crate::knowledge::{
    KnowledgeView, MetaEntry, Origin, ProposalChange, ProposalEntry, RawEntry, RecordEntry,
    TaskPath,
};
use crate::model::{Kind, ParsedRecord, ProfileConfig, RecordMeta, Registry, RegistryData};
use crate::normalize::AliasPattern;

/// Maximum distinct query tokens used by [`KnowledgeView::fulltext`].
pub const MAX_FULLTEXT_TERMS: usize = 64;

/// SQL condition keeping, for each record id, only its first member file by path (alias
/// `sd`; the `shadowed` flag is computed once per build by `mark_shadowed`). `MemoryView` and validation resolve duplicate ids the same way (the duplicate is
/// reported as `DUPLICATE_ID`), so an id's metadata and content always come from one file.
macro_rules! first_path {
    () => {
        " AND sd.shadowed = 0"
    };
}

/// Ends the index's shared read transaction when the last view is dropped.
struct ReadGuard<'a>(&'a Index);

impl Drop for ReadGuard<'_> {
    fn drop(&mut self) {
        self.0.end_read();
    }
}

/// Read-only access to one indexed snapshot (accepted content plus its proposal overlay).
pub struct IndexView<'a> {
    guard: ReadGuard<'a>,
    snapshot: i64,
    key: String,
    config: ProfileConfig,
    registry: Registry,
    diagnostics: Vec<Diagnostic>,
    /// Accepted parsed documents and their mean token count (ranking statistics).
    fts_docs: usize,
    fts_avglen: f64,
    /// Parsed metadata by document id (a view may ask for the same records repeatedly).
    metas: RefCell<HashMap<i64, Arc<RecordMeta>>>,
}

fn origin_str(o: Origin) -> &'static str {
    match o {
        Origin::Accepted => ACCEPTED,
        Origin::Proposal => PROPOSAL,
    }
}

impl<'a> IndexView<'a> {
    pub(super) fn open(index: &'a Index, key: &str) -> Result<IndexView<'a>> {
        index.begin_read()?;
        let guard = ReadGuard(index);
        type Row = (i64, String, String, String, i64, f64);
        let row: Option<Row> = index
            .conn()
            .prepare_cached(
                "SELECT id, config, registry, diagnostics, fts_docs, fts_avglen
                 FROM snapshots WHERE key = ?1",
            )?
            .query_row([key], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })
            .optional()?;
        let Some((snapshot, config, registry, diagnostics, fts_docs, fts_avglen)) = row else {
            return Err(KbError::new(
                ErrorCode::IndexError,
                format!("snapshot `{key}` is not in the index"),
            )
            .with_hint("build it first (Index::ensure); it may have been garbage-collected"));
        };
        let registry: RegistryData = serde_json::from_str(&registry)?;
        Ok(IndexView {
            guard,
            snapshot,
            key: key.to_string(),
            config: serde_json::from_str(&config)?,
            registry: Registry::new(registry),
            diagnostics: serde_json::from_str(&diagnostics)?,
            fts_docs: usize::try_from(fts_docs).unwrap_or(0),
            fts_avglen,
            metas: RefCell::new(HashMap::new()),
        })
    }

    /// Snapshot key of this view.
    pub fn key(&self) -> &str {
        &self.key
    }

    fn conn(&self) -> &Connection {
        self.guard.0.conn()
    }

    /// Metadata of a document, deserialized at most once per view.
    fn meta(&self, doc: i64, json: &str) -> Result<Arc<RecordMeta>> {
        if let Some(m) = self.metas.borrow().get(&doc) {
            return Ok(m.clone());
        }
        let m = Arc::new(serde_json::from_str::<RecordMeta>(json)?);
        self.metas.borrow_mut().insert(doc, m.clone());
        Ok(m)
    }

    /// Run a `(doc, path, meta)` query and collect meta entries.
    fn meta_rows(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
        origin: Origin,
        out: &mut Vec<MetaEntry>,
    ) -> Result<()> {
        let mut stmt = self.conn().prepare_cached(sql)?;
        let mut rows = stmt.query(params)?;
        while let Some(r) = rows.next()? {
            let doc: i64 = r.get(0)?;
            let path: String = r.get(1)?;
            let json: String = r.get(2)?;
            out.push(MetaEntry {
                path,
                origin,
                meta: self.meta(doc, &json)?,
            });
        }
        Ok(())
    }

    /// Metas of the given accepted documents at the paths where they are their record id's
    /// first file, sorted.
    fn accepted_metas(&self, docs: &BTreeSet<i64>) -> Result<Vec<MetaEntry>> {
        let mut out = Vec::new();
        for doc in docs {
            self.meta_rows(
                concat!(
                    "SELECT sd.doc, sd.path, d.meta FROM snapshot_docs sd JOIN docs d ON d.id = sd.doc
                     WHERE sd.snapshot = ?1 AND sd.doc = ?2 AND sd.origin = 'accepted'
                       AND d.meta IS NOT NULL",
                    first_path!()
                ),
                params![self.snapshot, doc],
                Origin::Accepted,
                &mut out,
            )?;
        }
        sort_metas(&mut out);
        Ok(out)
    }

    /// `(doc, pattern)` term rows of accepted documents with `kind` (and `value`, if given).
    fn term_rows(&self, kind: &str, value: Option<&str>) -> Result<Vec<(i64, String)>> {
        let row = |r: &rusqlite::Row<'_>| Ok((r.get(0)?, r.get(1)?));
        let rows = match value {
            Some(v) => self
                .conn()
                .prepare_cached(
                    "SELECT DISTINCT t.doc, t.pattern FROM doc_terms t
                     JOIN snapshot_docs sd ON sd.doc = t.doc
                     WHERE t.kind = ?1 AND t.value = ?2
                       AND sd.snapshot = ?3 AND sd.origin = 'accepted'",
                )?
                .query_map(params![kind, v, self.snapshot], row)?
                .collect::<rusqlite::Result<_>>()?,
            None => self
                .conn()
                .prepare_cached(
                    "SELECT DISTINCT t.doc, t.pattern FROM doc_terms t
                     JOIN snapshot_docs sd ON sd.doc = t.doc
                     WHERE t.kind = ?1 AND sd.snapshot = ?2 AND sd.origin = 'accepted'",
                )?
                .query_map(params![kind, self.snapshot], row)?
                .collect::<rusqlite::Result<_>>()?,
        };
        Ok(rows)
    }
}

fn sort_metas(v: &mut [MetaEntry]) {
    v.sort_by(|a, b| (&a.meta.id, &a.path).cmp(&(&b.meta.id, &b.path)));
}

fn dedup_sorted(ids: &[String]) -> BTreeSet<&str> {
    ids.iter().map(String::as_str).collect()
}

impl KnowledgeView for IndexView<'_> {
    fn config(&self) -> &ProfileConfig {
        &self.config
    }

    fn registry(&self) -> &Registry {
        &self.registry
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    fn metas_by_kind(&self, kinds: &[Kind], origin: Origin) -> Result<Vec<MetaEntry>> {
        let kinds: BTreeSet<Kind> = kinds.iter().copied().collect();
        let mut out = Vec::new();
        for kind in kinds {
            self.meta_rows(
                concat!(
                    "SELECT sd.doc, sd.path, d.meta FROM snapshot_docs sd INDEXED BY snapshot_docs_kind
                     JOIN docs d ON d.id = sd.doc
                     WHERE sd.snapshot = ?1 AND sd.origin = ?2 AND sd.kind = ?3
                       AND d.meta IS NOT NULL",
                    first_path!()
                ),
                params![self.snapshot, origin_str(origin), kind.as_str()],
                origin,
                &mut out,
            )?;
        }
        sort_metas(&mut out);
        Ok(out)
    }

    fn metas_by_ids(&self, ids: &[String], origin: Origin) -> Result<Vec<MetaEntry>> {
        let mut out = Vec::new();
        for id in dedup_sorted(ids) {
            self.meta_rows(
                concat!(
                    "SELECT sd.doc, sd.path, d.meta FROM snapshot_docs sd INDEXED BY snapshot_docs_record
                     JOIN docs d ON d.id = sd.doc
                     WHERE sd.snapshot = ?1 AND sd.origin = ?2 AND sd.record_id = ?3
                       AND d.meta IS NOT NULL",
                    first_path!()
                ),
                params![self.snapshot, origin_str(origin), id],
                origin,
                &mut out,
            )?;
        }
        sort_metas(&mut out);
        Ok(out)
    }

    fn path_candidates(&self, paths: &[TaskPath]) -> Result<Vec<MetaEntry>> {
        let mut globs: HashMap<String, Option<RepoGlob>> = HashMap::new();
        let mut docs = BTreeSet::new();
        let mut stmt = self.conn().prepare_cached(
            "SELECT DISTINCT dp.doc, dp.glob FROM doc_paths dp
             JOIN snapshot_docs sd ON sd.doc = dp.doc
             WHERE dp.dir_prefix = ?1 AND (dp.repo IS NULL OR dp.repo = ?2)
               AND sd.snapshot = ?3 AND sd.origin = 'accepted'",
        )?;
        for tp in paths {
            for prefix in dir_prefixes(&tp.path) {
                let mut rows = stmt.query(params![prefix, tp.repo, self.snapshot])?;
                while let Some(r) = rows.next()? {
                    let doc: i64 = r.get(0)?;
                    let spec: String = r.get(1)?;
                    let glob = globs
                        .entry(spec)
                        .or_insert_with_key(|s| RepoGlob::parse(s).ok());
                    if glob
                        .as_ref()
                        .is_some_and(|g| g.matches(tp.repo.as_deref(), &tp.path))
                    {
                        docs.insert(doc);
                    }
                }
            }
        }
        drop(stmt);
        self.accepted_metas(&docs)
    }

    fn term_candidates(&self, concepts: &[String], tokens: &[String]) -> Result<Vec<MetaEntry>> {
        let mut docs = BTreeSet::new();
        for c in dedup_sorted(concepts) {
            for (doc, _) in self.term_rows(TERM_CONCEPT, Some(c))? {
                docs.insert(doc);
            }
        }
        // Alias rows only narrow the candidates; the stored pattern decides.
        let mut verdicts: HashMap<String, bool> = HashMap::new();
        let mut alias_hits = |rows: Vec<(i64, String)>, docs: &mut BTreeSet<i64>| {
            for (doc, key) in rows {
                let hit = *verdicts.entry(key).or_insert_with_key(|k| {
                    AliasPattern::compile(k).is_some_and(|p| p.matches(tokens))
                });
                if hit {
                    docs.insert(doc);
                }
            }
        };
        for t in dedup_sorted(tokens) {
            alias_hits(self.term_rows(TERM_ALIAS_FIRST, Some(t))?, &mut docs);
        }
        if !tokens.is_empty() {
            alias_hits(self.term_rows(TERM_ALIAS_PREFIX, None)?, &mut docs);
        }
        self.accepted_metas(&docs)
    }

    fn fulltext(&self, terms: &[String], limit: usize) -> Result<Vec<String>> {
        let tokens = query_tokens(terms, MAX_FULLTEXT_TERMS);
        if limit == 0 || tokens.is_empty() || self.fts_docs == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn().prepare_cached(concat!(
            "SELECT sd.doc, sd.record_id, d.fts_len
             FROM (SELECT rowid AS doc FROM docs_fts WHERE docs_fts MATCH ?1) m
             JOIN snapshot_docs sd ON sd.doc = m.doc
             JOIN docs d ON d.id = sd.doc
             WHERE sd.snapshot = ?2 AND sd.origin = 'accepted' AND sd.record_id IS NOT NULL",
            first_path!()
        ))?;
        // doc -> (record id, score); terms are processed in sorted order, so the floating
        // point sums are reproducible.
        let mut scores: BTreeMap<i64, (String, f64)> = BTreeMap::new();
        for token in &tokens {
            let mut hits: BTreeMap<i64, (String, i64)> = BTreeMap::new();
            let rows = stmt.query_map(params![fts_string(token), self.snapshot], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?;
            for row in rows {
                let (doc, id, len) = row?;
                hits.insert(doc, (id, len));
            }
            let idf = text::idf(self.fts_docs, hits.len().min(self.fts_docs));
            for (doc, (id, len)) in hits {
                let w = text::term_weight(idf, len as f64, self.fts_avglen);
                scores.entry(doc).or_insert_with(|| (id, 0.0)).1 += w;
            }
        }
        // One entry per record id (each id has a single member document, see `first_path`).
        let mut best: BTreeMap<String, f64> = BTreeMap::new();
        for (id, score) in scores.into_values() {
            let e = best.entry(id).or_insert(score);
            if score > *e {
                *e = score;
            }
        }
        let mut ranked: Vec<(String, f64)> = best.into_iter().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(limit);
        Ok(ranked.into_iter().map(|(id, _)| id).collect())
    }

    fn records(&self, ids: &[String], origin: Origin) -> Result<Vec<RecordEntry>> {
        let mut stmt = self.conn().prepare_cached(concat!(
            "SELECT sd.path, d.parsed FROM snapshot_docs sd INDEXED BY snapshot_docs_record
             JOIN docs d ON d.id = sd.doc
             WHERE sd.snapshot = ?1 AND sd.origin = ?2 AND sd.record_id = ?3
               AND d.parsed IS NOT NULL",
            first_path!()
        ))?;
        let mut out = Vec::new();
        for id in dedup_sorted(ids) {
            let rows = stmt.query_map(params![self.snapshot, origin_str(origin), id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (path, json) = row?;
                out.push(RecordEntry {
                    path,
                    origin,
                    parsed: Arc::new(serde_json::from_str::<ParsedRecord>(&json)?),
                });
            }
        }
        Ok(out)
    }

    fn raw(&self, id: &str, origin: Origin) -> Result<Option<RawEntry>> {
        let row: Option<(String, String)> = self
            .conn()
            .prepare_cached(
                "SELECT sd.path, d.raw FROM snapshot_docs sd INDEXED BY snapshot_docs_record
                 JOIN docs d ON d.id = sd.doc
                 WHERE sd.snapshot = ?1 AND sd.origin = ?2 AND sd.record_id = ?3
                 ORDER BY sd.path LIMIT 1",
            )?
            .query_row(params![self.snapshot, origin_str(origin), id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        Ok(row.map(|(path, text)| RawEntry {
            path,
            origin,
            text: Arc::from(text),
        }))
    }

    fn proposals(&self) -> Result<Vec<ProposalEntry>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT sp.path, sp.record_id, sp.change, sp.stale, sp.diagnostics, sp.doc, d.meta
             FROM snapshot_proposals sp LEFT JOIN docs d ON d.id = sp.doc
             WHERE sp.snapshot = ?1",
        )?;
        type Row = (
            String,
            Option<String>,
            String,
            bool,
            String,
            Option<i64>,
            Option<String>,
        );
        let rows = stmt.query_map([self.snapshot], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (path, record_id, change, stale, diagnostics, doc, meta): Row = row?;
            let meta = match (doc, meta) {
                (Some(doc), Some(json)) => Some(self.meta(doc, &json)?),
                _ => None,
            };
            out.push((
                record_id,
                ProposalEntry {
                    path,
                    change: serde_json::from_str::<ProposalChange>(&change)?,
                    meta,
                    stale,
                    diagnostics: serde_json::from_str(&diagnostics)?,
                },
            ));
        }
        out.sort_by(|a, b| (&a.0, &a.1.path).cmp(&(&b.0, &b.1.path)));
        Ok(out.into_iter().map(|(_, e)| e).collect())
    }
}
