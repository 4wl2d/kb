//! Derived SQLite (FTS5) index: content-addressed docs, per-snapshot membership,
//! transactional incremental builds, corruption recovery. See docs/architecture.md §7.
//!
//! * One database per profile: `<cache_dir>/index/<profile>.sqlite` (WAL, busy timeout,
//!   foreign keys). The file is a disposable cache; Git-tracked text is the source of truth.
//! * `docs` are keyed by (content id, parser version); a new snapshot parses only content
//!   that no earlier snapshot had. Snapshots own membership rows only.
//! * A snapshot is built in one `BEGIN IMMEDIATE` transaction, so readers see either no
//!   snapshot or a complete one; an [`IndexView`] holds one read transaction for its whole
//!   lifetime, so it keeps a consistent state even while other processes build or collect.
//! * Corrupt or foreign files are moved aside (`<name>.corrupt-<unique>`) and replaced;
//!   indexes written by another engine/index/parser version are rebuilt in place. Either
//!   way the event is reported through [`Recovery`], never silently. Damage found when
//!   opening, building, or querying ([`Index::with_view`]) is recovered the same way.
//! * A document is stored only under the content id of the bytes it was parsed from.

mod build;
mod proposals;
mod schema;
use crate::context::lexical as text;
mod view;

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::diag::Diagnostic;
use crate::error::{ErrorCode, KbError, Result};
use crate::knowledge::Overlay;
use crate::model::{Profile, ProfileLocation};
use crate::source::SourceTree;
use crate::versions::{INDEX_SCHEMA, PARSER_VERSION};

use schema::{OpenError, OpenProblem};
pub use view::IndexView;

/// Number of snapshots kept by the automatic retention that runs after each new build.
pub const DEFAULT_KEEP: usize = 8;

/// Build statistics for one `ensure` call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildStats {
    pub snapshot_key: String,
    /// True when the snapshot already existed (warm path, nothing listed, read or parsed).
    pub reused: bool,
    /// Accepted record files in the snapshot.
    pub files: usize,
    /// Documents parsed by this call (new content only; accepted and proposals).
    pub parsed: usize,
    /// Member files whose parsed document already existed in the index.
    pub reused_docs: usize,
    /// Proposal overlay entries.
    pub proposals: usize,
    /// Error diagnostics of the snapshot.
    pub errors: usize,
    /// Warning diagnostics of the snapshot.
    pub warnings: usize,
    /// Old snapshots removed by automatic retention during this call.
    #[serde(default)]
    pub collected: usize,
    /// Set when this index instance had to recover the cache file when it was opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<Recovery>,
}

/// Why the cache database was replaced or rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryKind {
    /// Corrupt file, not a database, or an unusable schema: moved aside and recreated.
    Corrupt,
    /// Written by another engine, index schema or parser version: rebuilt in place.
    Outdated,
    /// Explicit rebuild ([`Index::rebuild`]).
    Requested,
}

/// A cache recovery event. Reported to callers; never silent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recovery {
    pub kind: RecoveryKind,
    pub reason: String,
    /// Where a corrupt file was moved (kept for inspection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_to: Option<PathBuf>,
}

impl Recovery {
    /// Diagnostic describing the event (`INDEX_RECOVERED` warning or `INDEX_REBUILT` info).
    pub fn diagnostic(&self) -> Diagnostic {
        match self.kind {
            RecoveryKind::Corrupt => {
                let moved = self
                    .moved_to
                    .as_ref()
                    .map(|p| format!("; the damaged file was moved to {}", p.display()))
                    .unwrap_or_default();
                Diagnostic::warning(
                    "INDEX_RECOVERED",
                    format!("the index cache was unusable ({}){moved}", self.reason),
                )
            }
            RecoveryKind::Outdated => Diagnostic::info(
                "INDEX_REBUILT",
                format!("the index cache was rebuilt ({})", self.reason),
            ),
            RecoveryKind::Requested => {
                Diagnostic::info("INDEX_REBUILT", "the index cache was rebuilt on request")
            }
        }
    }
}

/// Result of [`Index::with_view`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewRun<T> {
    /// What the callback returned.
    pub value: T,
    /// Stats of the rebuild that had to happen first (the snapshot was collected, or the
    /// damaged database was recovered: then `recovery` is set); `None` when the stored
    /// snapshot was used directly.
    pub rebuilt: Option<BuildStats>,
}

/// One snapshot as listed by [`Index::stats`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnapshotSummary {
    pub key: String,
    /// Monotonic build counter (larger = newer).
    pub created: i64,
    pub files: usize,
    pub proposals: usize,
    pub errors: usize,
    pub warnings: usize,
}

/// Cache-level statistics (`kb index`, `kb doctor`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexStats {
    pub path: PathBuf,
    pub index_schema: u32,
    pub parser_version: u32,
    /// Newest first.
    pub snapshots: Vec<SnapshotSummary>,
    /// Distinct parsed documents (content versions) stored.
    pub docs: usize,
    /// Size of the database file plus its write-ahead log.
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<Recovery>,
}

/// The per-profile index database.
pub struct Index {
    path: PathBuf,
    profile: Profile,
    conn: Connection,
    recovery: Option<Recovery>,
    /// Number of live [`IndexView`]s sharing this connection's read transaction.
    readers: Cell<usize>,
}

impl Index {
    /// Open (or create) `<cache_dir>/index/<profile>.sqlite`, recovering from corruption.
    pub fn open(cache_dir: &Path, profile: Profile) -> Result<Index> {
        Index::open_with(cache_dir, profile, false)
    }

    /// Drop all cached content and start from an empty index (`kb index --rebuild`).
    /// Runs as one transaction, so concurrent users never observe a half-dropped index.
    pub fn rebuild(cache_dir: &Path, profile: Profile) -> Result<Index> {
        Index::open_with(cache_dir, profile, true)
    }

    fn open_with(cache_dir: &Path, profile: Profile, rebuild: bool) -> Result<Index> {
        let dir = cache_dir.join("index");
        fs::create_dir_all(&dir).map_err(|e| KbError::io(dir.display(), e))?;
        let path = dir.join(format!("{}.sqlite", profile.as_str()));
        let _lock = InitLock::acquire(&path)?;
        let (conn, recovery) = open_recovering(&path, profile, rebuild)?;
        Ok(Index {
            path,
            profile,
            conn,
            recovery,
            readers: Cell::new(0),
        })
    }

    /// Database file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Recovery performed when this instance opened the cache, if any.
    pub fn recovery(&self) -> Option<&Recovery> {
        self.recovery.as_ref()
    }

    /// Make sure the snapshot `key` exists, building it from `source` (plus the optional
    /// proposal overlay) when missing. The warm path is a single indexed lookup.
    ///
    /// `key` must identify the content exactly (see docs/architecture.md §6); a build
    /// failure (for example a source read error) leaves no partial snapshot behind.
    pub fn ensure(
        &mut self,
        key: &str,
        source: &dyn SourceTree,
        loc: &ProfileLocation,
        overlay: Option<&Overlay>,
    ) -> Result<BuildStats> {
        let mut stats = match self.ensure_once(key, source, loc, overlay) {
            Err(e) if e.code == ErrorCode::IndexError => {
                // Distinguish a damaged database from ordinary failures (e.g. lock timeout).
                if self.recover_if_damaged()?.is_none() {
                    return Err(e);
                }
                self.ensure_once(key, source, loc, overlay)?
            }
            other => other?,
        };
        stats.recovery = self.recovery.clone();
        Ok(stats)
    }

    /// Check the database structure (`PRAGMA quick_check`) and, when it is damaged, move
    /// the file aside and continue with an empty index, as [`Index::open`] does for a file
    /// it cannot open. Returns the recovery when one happened; it is also reported by
    /// [`Index::recovery`] and in the stats of every later [`Index::ensure`]. Snapshots
    /// must then be rebuilt with `ensure`. Intended after a query failed with
    /// `INDEX_ERROR`; [`Index::with_view`] does all of this.
    pub fn recover_if_damaged(&mut self) -> Result<Option<Recovery>> {
        let Some(problem) = schema::health_problem(&self.conn) else {
            return Ok(None);
        };
        self.recover(problem)?;
        Ok(self.recovery.clone())
    }

    /// Run `f` on a view of snapshot `key`, repairing the index when needed.
    ///
    /// When opening the view or `f` fails with `INDEX_ERROR` and the database turns out to
    /// be damaged (a page read only by queries, never by `open` or a warm `ensure`), it is
    /// recovered like in [`Index::recover_if_damaged`]; when the snapshot is gone
    /// (garbage-collected by another process, or never built) nothing is recovered. In
    /// both cases the snapshot is rebuilt from `source`, `loc` and `overlay` (the arguments
    /// of [`Index::ensure`]) and `f` runs once more on a fresh view; the rebuild's stats are
    /// returned in [`ViewRun::rebuilt`] (with `recovery` set after a recovery). Any other
    /// error, or a second failure, is returned as is. `f` must not keep side effects of a
    /// failed run: it may be called twice.
    pub fn with_view<T>(
        &mut self,
        key: &str,
        source: &dyn SourceTree,
        loc: &ProfileLocation,
        overlay: Option<&Overlay>,
        mut f: impl FnMut(&IndexView<'_>) -> Result<T>,
    ) -> Result<ViewRun<T>> {
        let mut rebuilt = None;
        loop {
            let err = match self.view(key).and_then(|view| f(&view)) {
                Ok(value) => return Ok(ViewRun { value, rebuilt }),
                Err(e) => e,
            };
            if rebuilt.is_some() || err.code != ErrorCode::IndexError {
                return Err(err);
            }
            if self.recover_if_damaged()?.is_none() && self.has_snapshot(key)? {
                return Err(err);
            }
            rebuilt = Some(self.ensure(key, source, loc, overlay)?);
        }
    }

    fn ensure_once(
        &mut self,
        key: &str,
        source: &dyn SourceTree,
        loc: &ProfileLocation,
        overlay: Option<&Overlay>,
    ) -> Result<BuildStats> {
        if let Some(stats) = stored_stats(&self.conn, key)? {
            return Ok(stats);
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Another process may have built the same key while we waited for the lock.
        if let Some(stats) = stored_stats(&tx, key)? {
            return Ok(stats);
        }
        let mut stats = build::build(&tx, key, source, loc, overlay)?;
        stats.collected = collect(&tx, DEFAULT_KEEP, false)?;
        tx.commit()?;
        Ok(stats)
    }

    /// Replace a database found damaged after opening: move it aside and start fresh.
    /// On error the instance is unusable and must be reopened.
    fn recover(&mut self, reason: String) -> Result<()> {
        let _lock = InitLock::acquire(&self.path)?;
        drop(std::mem::replace(
            &mut self.conn,
            Connection::open_in_memory()?,
        ));
        // Another process may have replaced the file while we waited for the lock.
        let (conn, _) = open_recovering(&self.path, self.profile, false)?;
        let moved_to = if schema::health_problem(&conn).is_some() {
            drop(conn);
            let moved = move_aside(&self.path)?;
            self.conn = open_recovering(&self.path, self.profile, false)?.0;
            Some(moved)
        } else {
            self.conn = conn;
            None
        };
        self.recovery = Some(Recovery {
            kind: RecoveryKind::Corrupt,
            reason,
            moved_to,
        });
        Ok(())
    }

    /// Does the index already hold snapshot `key`?
    pub fn has_snapshot(&self, key: &str) -> Result<bool> {
        Ok(self
            .conn
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM snapshots WHERE key = ?1)")?
            .query_row([key], |r| r.get(0))?)
    }

    /// Read-only view of snapshot `key` (see [`IndexView`]). Fails with `INDEX_ERROR` when
    /// the snapshot is absent (never built, or garbage-collected since `ensure`).
    pub fn view(&self, key: &str) -> Result<IndexView<'_>> {
        IndexView::open(self, key)
    }

    /// Delete all but the `keep` most recent snapshots, then every document no remaining
    /// snapshot references. Returns the number of snapshots removed.
    pub fn gc(&mut self, keep: usize) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = collect(&tx, keep, true)?;
        tx.commit()?;
        Ok(removed)
    }

    /// Cache statistics.
    pub fn stats(&self) -> Result<IndexStats> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT key, created, stats FROM snapshots ORDER BY created DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut snapshots = Vec::new();
        for row in rows {
            let (key, created, stats) = row?;
            let s: BuildStats = serde_json::from_str(&stats)?;
            snapshots.push(SnapshotSummary {
                key,
                created,
                files: s.files,
                proposals: s.proposals,
                errors: s.errors,
                warnings: s.warnings,
            });
        }
        let docs: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM docs", [], |r| r.get(0))?;
        let size = |p: &Path| fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let wal = PathBuf::from(format!("{}-wal", self.path.display()));
        Ok(IndexStats {
            path: self.path.clone(),
            index_schema: INDEX_SCHEMA,
            parser_version: PARSER_VERSION,
            snapshots,
            docs: usize::try_from(docs).unwrap_or(0),
            bytes: size(&self.path) + size(&wal),
            recovery: self.recovery.clone(),
        })
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Enter the shared read transaction used by views (nested views share it).
    pub(crate) fn begin_read(&self) -> Result<()> {
        if self.readers.get() == 0 {
            self.conn.execute_batch("BEGIN DEFERRED")?;
        }
        self.readers.set(self.readers.get() + 1);
        Ok(())
    }

    /// Leave the shared read transaction; the last reader ends it.
    pub(crate) fn end_read(&self) {
        let n = self.readers.get().saturating_sub(1);
        self.readers.set(n);
        if n == 0 && !self.conn.is_autocommit() {
            // A read-only transaction has nothing to commit; failure cannot lose data.
            let _ = self.conn.execute_batch("COMMIT");
        }
    }
}

/// Stored statistics of an existing snapshot, marked as reused.
fn stored_stats(conn: &Connection, key: &str) -> Result<Option<BuildStats>> {
    let json: Option<String> = conn
        .prepare_cached("SELECT stats FROM snapshots WHERE key = ?1")?
        .query_row([key], |r| r.get(0))
        .optional()?;
    let Some(json) = json else {
        return Ok(None);
    };
    let mut stats: BuildStats = serde_json::from_str(&json)?;
    stats.reused_docs += stats.parsed;
    stats.parsed = 0;
    stats.reused = true;
    stats.collected = 0;
    Ok(Some(stats))
}

/// Delete snapshots beyond the `keep` newest and, when anything was deleted (or `always`),
/// the documents no snapshot references any more.
fn collect(tx: &rusqlite::Transaction<'_>, keep: usize, always: bool) -> Result<usize> {
    let keep = i64::try_from(keep).unwrap_or(i64::MAX);
    let removed = tx
        .prepare_cached(
            "DELETE FROM snapshots WHERE id NOT IN
               (SELECT id FROM snapshots ORDER BY created DESC, id DESC LIMIT ?1)",
        )?
        .execute([keep])?;
    if removed == 0 && !always {
        return Ok(0);
    }
    let orphans: Vec<i64> = {
        let mut stmt = tx.prepare_cached(
            "SELECT id FROM docs d
             WHERE NOT EXISTS (SELECT 1 FROM snapshot_docs sd WHERE sd.doc = d.id)
               AND NOT EXISTS (SELECT 1 FROM snapshot_proposals sp WHERE sp.doc = d.id)",
        )?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for id in orphans {
        tx.prepare_cached("DELETE FROM docs_fts WHERE rowid = ?1")?
            .execute([id])?;
        tx.prepare_cached("DELETE FROM docs WHERE id = ?1")?
            .execute([id])?;
    }
    Ok(removed)
}

/// Open the database, handling outdated (rebuild in place) and unusable (move aside)
/// files. Must run with the initialization lock held.
fn open_recovering(
    path: &Path,
    profile: Profile,
    rebuild: bool,
) -> Result<(Connection, Option<Recovery>)> {
    match open_checked(path, profile, rebuild) {
        Ok(opened) => Ok(opened),
        Err(OpenError::Problem(OpenProblem::Unusable(reason))) => {
            let moved = move_aside(path)?;
            let (conn, _) = open_checked(path, profile, false).map_err(open_error)?;
            let recovery = Recovery {
                kind: RecoveryKind::Corrupt,
                reason,
                moved_to: Some(moved),
            };
            Ok((conn, Some(recovery)))
        }
        Err(e) => Err(open_error(e)),
    }
}

fn open_checked(
    path: &Path,
    profile: Profile,
    rebuild: bool,
) -> Result<(Connection, Option<Recovery>), OpenError> {
    let mut conn = schema::connect(path)?;
    if rebuild {
        schema::reset(&mut conn, profile)?;
        let recovery = Recovery {
            kind: RecoveryKind::Requested,
            reason: "rebuild requested".into(),
            moved_to: None,
        };
        return Ok((conn, Some(recovery)));
    }
    match schema::check_or_init(&mut conn, profile) {
        Ok(()) => Ok((conn, None)),
        Err(OpenError::Problem(OpenProblem::Outdated(reason))) => {
            schema::reset(&mut conn, profile)?;
            let recovery = Recovery {
                kind: RecoveryKind::Outdated,
                reason,
                moved_to: None,
            };
            Ok((conn, Some(recovery)))
        }
        Err(e) => Err(e),
    }
}

fn open_error(e: OpenError) -> KbError {
    let msg = match e {
        OpenError::Problem(OpenProblem::Unusable(m) | OpenProblem::Outdated(m)) => m,
        OpenError::Sql(e) => e.to_string(),
    };
    KbError::new(
        ErrorCode::IndexError,
        format!("cannot open the index: {msg}"),
    )
    .with_hint("check permissions of the cache directory, or remove it (it is derived data)")
}

/// Move a damaged database and its WAL/SHM files aside; returns the new database path.
fn move_aside(path: &Path) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let suffix = format!("corrupt-{}-{nanos}", std::process::id());
    let base = path.display().to_string();
    let target = PathBuf::from(format!("{base}.{suffix}"));
    fs::rename(path, &target).map_err(|e| KbError::io(path.display(), e))?;
    for side in ["-wal", "-shm"] {
        let from = PathBuf::from(format!("{base}{side}"));
        if from.exists() {
            let to = PathBuf::from(format!("{base}{side}.{suffix}"));
            fs::rename(&from, &to).map_err(|e| KbError::io(from.display(), e))?;
        }
    }
    Ok(target)
}

/// Advisory lock serializing database initialization and recovery across processes
/// (`<db>.lock`). Normal reads and writes rely on SQLite's own locking.
struct InitLock {
    _file: fs::File,
}

impl InitLock {
    fn acquire(db: &Path) -> Result<InitLock> {
        let path = PathBuf::from(format!("{}.lock", db.display()));
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| KbError::io(path.display(), e))?;
        file.lock().map_err(|e| KbError::io(path.display(), e))?;
        Ok(InitLock { _file: file })
    }
}
