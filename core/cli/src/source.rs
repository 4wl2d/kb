//! Content sources: a snapshot of files addressed by KB-root-relative paths.
//!
//! `WorkingTreeSource` reads the local filesystem (validate/index/proposals);
//! `FrozenWorkingTree` binds a working-tree snapshot to the one listing its key was
//! computed from; `snapshot::GitTreeSource` reads immutable Git objects without touching a
//! work tree.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, KbError, Result};
use crate::util::{check_rel_path, read_file_limited, rel_string, safe_join, sha256_hex};

/// Max bytes read for any single file through a source.
pub const MAX_SOURCE_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// The content read for `path` is not the content it was listed with: the file changed
/// while the command was reading it. Retryable (`IO_ERROR` with a hint).
pub fn content_changed(path: &str) -> KbError {
    KbError::new(
        ErrorCode::IoError,
        format!("`{path}` changed while it was being read"),
    )
    .with_hint("files changed while the command was reading them; run it again")
}

/// Check `bytes` read for `path` against the content id it was listed with. `sha256:` ids
/// are re-hashed; other schemes (Git object ids) name immutable content and are trusted.
pub fn verify_content(path: &str, content_id: &str, bytes: &[u8]) -> Result<()> {
    match content_id.strip_prefix("sha256:") {
        Some(hex) if hex != sha256_hex(bytes) => Err(content_changed(path)),
        _ => Ok(()),
    }
}

/// Is `path` the prefix itself or inside the directory `prefix`?
fn is_under(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// A file in a source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceEntry {
    /// KB-root-relative, `/`-separated path.
    pub path: String,
    /// Content identity: `git:<oid>` for Git blobs, `sha256:<hex>` for files.
    pub content_id: String,
    pub size: u64,
}

/// Problems found while listing (symlinks, unreadable entries); reported as diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceIssue {
    pub path: String,
    pub code: &'static str,
    pub message: String,
}

pub trait SourceTree {
    /// Human-readable description for provenance (e.g. `git:abc123`, `working-tree`).
    fn describe(&self) -> String;
    /// All regular files under the given prefixes (a prefix may be a file or a directory),
    /// sorted by path, deduplicated. Missing prefixes are skipped.
    fn list(&self, prefixes: &[String]) -> Result<(Vec<SourceEntry>, Vec<SourceIssue>)>;
    /// Read the contents of the given entries (same order).
    fn read(&self, entries: &[SourceEntry]) -> Result<Vec<Vec<u8>>>;
    /// Read a single file by path, `None` when absent.
    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>>;
}

/// Filesystem source rooted at the KB checkout.
///
/// With [`WorkingTreeSource::with_stat_cache`], content ids of unchanged files are reused
/// from a stat cache keyed by (size, mtime, ctime, inode) instead of re-reading and
/// re-hashing every file. Like Git's index, entries whose mtime is within
/// [`RACY_WINDOW_NS`] of the cache write time are never trusted ("racily clean").
pub struct WorkingTreeSource {
    root: PathBuf,
    stat_cache: Option<PathBuf>,
    cache: Mutex<Option<StatCache>>,
}

/// Files modified this close to the cache write time are always re-hashed.
pub const RACY_WINDOW_NS: u128 = 2_000_000_000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct StatCache {
    version: u32,
    written_ns: u128,
    entries: BTreeMap<String, StatEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StatEntry {
    key: StatKey,
    content_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct StatKey {
    size: u64,
    mtime_ns: u128,
    ctime_ns: u128,
    ino: u64,
}

const STAT_CACHE_VERSION: u32 = 1;

fn now_ns() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn stat_key(md: &fs::Metadata) -> StatKey {
    let mtime_ns = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let ctime_ns = (md.ctime().max(0) as u128) * 1_000_000_000 + md.ctime_nsec().max(0) as u128;
        StatKey {
            size: md.len(),
            mtime_ns,
            ctime_ns,
            ino: md.ino(),
        }
    }
    #[cfg(not(unix))]
    {
        StatKey {
            size: md.len(),
            mtime_ns,
            ctime_ns: 0,
            ino: 0,
        }
    }
}

impl WorkingTreeSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        WorkingTreeSource {
            root: root.into(),
            stat_cache: None,
            cache: Mutex::new(None),
        }
    }

    /// A source that reuses content ids through a stat cache file (derived data; a missing
    /// or unreadable cache is simply rebuilt).
    pub fn with_stat_cache(root: impl Into<PathBuf>, cache_file: impl Into<PathBuf>) -> Self {
        WorkingTreeSource {
            root: root.into(),
            stat_cache: Some(cache_file.into()),
            cache: Mutex::new(None),
        }
    }

    fn load_cache(&self) -> StatCache {
        let Some(path) = &self.stat_cache else {
            return StatCache::default();
        };
        fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<StatCache>(&b).ok())
            .filter(|c| c.version == STAT_CACHE_VERSION)
            .unwrap_or_default()
    }

    /// Content id of `path`, from the stat cache when the entry is trustworthy.
    fn content_id(
        &self,
        rel: &str,
        path: &Path,
        old: &StatCache,
        fresh: &mut BTreeMap<String, StatEntry>,
    ) -> Result<(String, u64)> {
        let md = fs::symlink_metadata(path).map_err(|e| KbError::io(path.display(), e))?;
        let key = stat_key(&md);
        if let Some(e) = old.entries.get(rel) {
            let racy = key.mtime_ns.saturating_add(RACY_WINDOW_NS) >= old.written_ns;
            if e.key == key && !racy {
                fresh.insert(rel.to_string(), e.clone());
                return Ok((e.content_id.clone(), key.size));
            }
        }
        let bytes = read_file_limited(path, MAX_SOURCE_FILE_BYTES)?;
        let content_id = format!("sha256:{}", sha256_hex(&bytes));
        if self.stat_cache.is_some() {
            fresh.insert(
                rel.to_string(),
                StatEntry {
                    key,
                    content_id: content_id.clone(),
                },
            );
        }
        Ok((content_id, bytes.len() as u64))
    }

    /// Replace the cache entries under `prefixes` with `fresh` and persist when changed.
    fn store_cache(&self, prefixes: &[String], old: StatCache, fresh: BTreeMap<String, StatEntry>) {
        let Some(path) = &self.stat_cache else {
            return;
        };
        let under = |p: &str| {
            prefixes
                .iter()
                .any(|pre| is_under(p, pre.trim_end_matches('/')))
        };
        let mut entries: BTreeMap<String, StatEntry> = old
            .entries
            .iter()
            .filter(|(p, _)| !under(p))
            .map(|(p, e)| (p.clone(), e.clone()))
            .collect();
        entries.extend(fresh);
        let mut guard = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if entries == old.entries && old.version == STAT_CACHE_VERSION {
            *guard = Some(old);
            return;
        }
        let cache = StatCache {
            version: STAT_CACHE_VERSION,
            written_ns: now_ns(),
            entries,
        };
        if let Ok(text) = serde_json::to_vec(&cache) {
            // Best effort: the cache is derived data and is rebuilt when missing.
            let _ = crate::util::atomic_write(path, &text);
        }
        *guard = Some(cache);
    }

    /// Check bytes read for `rel` against its listed content id. On a mismatch the file
    /// changed after listing, or (rarely) its stat-cache entry was wrong: the entry is
    /// dropped so the next listing re-hashes the file and a retry succeeds.
    fn verify(&self, rel: &str, content_id: &str, bytes: &[u8]) -> Result<()> {
        verify_content(rel, content_id, bytes).inspect_err(|_| self.forget(rel))
    }

    /// Remove `rel` from the stat cache (best effort, like every cache write).
    fn forget(&self, rel: &str) {
        let Some(path) = &self.stat_cache else {
            return;
        };
        let mut guard = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        let mut cache = guard.take().unwrap_or_else(|| self.load_cache());
        if cache.entries.remove(rel).is_some()
            && let Ok(text) = serde_json::to_vec(&cache)
        {
            let _ = crate::util::atomic_write(path, &text);
        }
        *guard = Some(cache);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn walk(
        &self,
        dir: &Path,
        out: &mut Vec<PathBuf>,
        issues: &mut Vec<SourceIssue>,
    ) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .map_err(|e| KbError::io(dir.display(), e))?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let ft = match e.file_type() {
                Ok(ft) => ft,
                Err(err) => {
                    issues.push(SourceIssue {
                        path: rel_string(&self.root, &p).unwrap_or_default(),
                        code: "SOURCE_UNREADABLE",
                        message: err.to_string(),
                    });
                    continue;
                }
            };
            if ft.is_symlink() {
                issues.push(SourceIssue {
                    path: rel_string(&self.root, &p).unwrap_or_default(),
                    code: "SYMLINK_NOT_ALLOWED",
                    message: "symlinks are not followed inside knowledge sources".into(),
                });
            } else if ft.is_dir() {
                self.walk(&p, out, issues)?;
            } else if ft.is_file() {
                out.push(p);
            }
        }
        Ok(())
    }
}

impl SourceTree for WorkingTreeSource {
    fn describe(&self) -> String {
        "working-tree".into()
    }

    fn list(&self, prefixes: &[String]) -> Result<(Vec<SourceEntry>, Vec<SourceIssue>)> {
        let mut files = Vec::new();
        let mut issues = Vec::new();
        for prefix in prefixes {
            let prefix = prefix.trim_end_matches('/');
            check_rel_path(prefix).map_err(KbError::unsafe_path)?;
            let p = match safe_join(&self.root, prefix) {
                Ok(p) => p,
                Err(e) => {
                    issues.push(SourceIssue {
                        path: prefix.to_string(),
                        code: "SYMLINK_NOT_ALLOWED",
                        message: e.message,
                    });
                    continue;
                }
            };
            match fs::symlink_metadata(&p) {
                Ok(md) if md.is_dir() => self.walk(&p, &mut files, &mut issues)?,
                Ok(md) if md.is_file() => files.push(p),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        let old = match &self.stat_cache {
            Some(_) => {
                let guard = self.cache.lock().unwrap_or_else(|p| p.into_inner());
                guard.clone().unwrap_or_else(|| self.load_cache())
            }
            None => StatCache::default(),
        };
        let mut fresh = BTreeMap::new();
        let mut out = Vec::new();
        for f in files {
            let Some(rel) = rel_string(&self.root, &f) else {
                continue;
            };
            match self.content_id(&rel, &f, &old, &mut fresh) {
                Ok((content_id, size)) => out.push(SourceEntry {
                    path: rel,
                    content_id,
                    size,
                }),
                Err(e) => issues.push(SourceIssue {
                    path: rel,
                    code: "SOURCE_UNREADABLE",
                    message: e.message,
                }),
            }
        }
        if self.stat_cache.is_some() {
            self.store_cache(prefixes, old, fresh);
        }
        out.sort();
        out.dedup_by(|a, b| a.path == b.path);
        Ok((out, issues))
    }

    /// Reads are checked against the listed content ids: a file that changed since `list`
    /// fails with [`content_changed`] instead of returning bytes of another content id.
    fn read(&self, entries: &[SourceEntry]) -> Result<Vec<Vec<u8>>> {
        entries
            .iter()
            .map(|e| {
                let p = safe_join(&self.root, &e.path)?;
                let bytes = read_file_limited(&p, MAX_SOURCE_FILE_BYTES).map_err(|err| {
                    match fs::symlink_metadata(&p) {
                        Err(_) => content_changed(&e.path), // deleted since listing
                        Ok(_) => err,
                    }
                })?;
                self.verify(&e.path, &e.content_id, &bytes)?;
                Ok(bytes)
            })
            .collect()
    }

    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let p = safe_join(&self.root, path)?;
        match fs::symlink_metadata(&p) {
            Ok(md) if md.is_file() => Ok(Some(read_file_limited(&p, MAX_SOURCE_FILE_BYTES)?)),
            Ok(_) => Err(KbError::invalid_input(format!(
                "`{path}` is not a regular file"
            ))),
            Err(_) => Ok(None),
        }
    }
}

/// A working-tree source frozen to one listing: the content a working-tree snapshot key is
/// computed from (docs/architecture.md §6).
///
/// `list` answers from the frozen listing and every read is checked against the frozen
/// content ids, so whatever is built from this source is exactly the listed content, or
/// the build fails with the retryable [`content_changed`] error. Files created after the
/// listing are not part of the snapshot. Paths outside the listed prefixes are not keyed
/// content: `list` rejects such prefixes, `read_path` reads them as is.
pub struct FrozenWorkingTree {
    inner: WorkingTreeSource,
    prefixes: Vec<String>,
    entries: BTreeMap<String, SourceEntry>,
    issues: Vec<SourceIssue>,
}

impl FrozenWorkingTree {
    /// List `prefixes` of `inner` once and freeze the result.
    pub fn freeze(inner: WorkingTreeSource, prefixes: &[String]) -> Result<FrozenWorkingTree> {
        let (entries, issues) = inner.list(prefixes)?;
        Ok(FrozenWorkingTree {
            inner,
            prefixes: prefixes
                .iter()
                .map(|p| p.trim_end_matches('/').to_string())
                .collect(),
            entries: entries.into_iter().map(|e| (e.path.clone(), e)).collect(),
            issues,
        })
    }

    /// The frozen files, sorted by path.
    pub fn entries(&self) -> impl Iterator<Item = &SourceEntry> {
        self.entries.values()
    }

    /// Problems found by the listing, in listing order.
    pub fn issues(&self) -> &[SourceIssue] {
        &self.issues
    }

    fn covers(&self, path: &str) -> bool {
        self.prefixes.iter().any(|p| is_under(path, p))
    }
}

impl SourceTree for FrozenWorkingTree {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn list(&self, prefixes: &[String]) -> Result<(Vec<SourceEntry>, Vec<SourceIssue>)> {
        let mut wanted = Vec::with_capacity(prefixes.len());
        for prefix in prefixes {
            let prefix = prefix.trim_end_matches('/');
            check_rel_path(prefix).map_err(KbError::unsafe_path)?;
            // Only a configuration change during the call asks for unlisted content.
            if !self.covers(prefix) {
                return Err(content_changed(prefix));
            }
            wanted.push(prefix);
        }
        let inside = |p: &str| wanted.iter().any(|w| is_under(p, w));
        let entries = self
            .entries
            .values()
            .filter(|e| inside(&e.path))
            .cloned()
            .collect();
        let issues = self
            .issues
            .iter()
            .filter(|i| inside(&i.path))
            .cloned()
            .collect();
        Ok((entries, issues))
    }

    fn read(&self, entries: &[SourceEntry]) -> Result<Vec<Vec<u8>>> {
        if let Some(e) = entries
            .iter()
            .find(|e| self.entries.get(&e.path) != Some(*e))
        {
            return Err(content_changed(&e.path));
        }
        // The inner source verifies the bytes against these (frozen) content ids.
        self.inner.read(entries)
    }

    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let bytes = self.inner.read_path(path)?;
        match (self.entries.get(path), bytes) {
            (Some(e), Some(b)) => {
                self.inner.verify(path, &e.content_id, &b)?;
                Ok(Some(b))
            }
            (None, b) if !self.covers(path) => Ok(b),
            (None, None) => Ok(None),
            // Deleted, or created, since the listing.
            _ => Err(content_changed(path)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_sorted_and_reports_symlinks() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("p/k/sub")).unwrap();
        fs::write(d.path().join("p/k/b.md"), "b").unwrap();
        fs::write(d.path().join("p/k/sub/a.md"), "a").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hosts", d.path().join("p/k/evil.md")).unwrap();
        let s = WorkingTreeSource::new(d.path());
        let (entries, issues) = s.list(&["p/k".into(), "p/missing".into()]).unwrap();
        let paths: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["p/k/b.md", "p/k/sub/a.md"]);
        #[cfg(unix)]
        assert_eq!(issues[0].code, "SYMLINK_NOT_ALLOWED");
        assert_eq!(s.read(&entries).unwrap()[0], b"b");
        assert!(s.read_path("p/none").unwrap().is_none());
        assert!(s.list(&["../x".into()]).is_err());
    }

    #[test]
    fn reads_are_checked_against_the_listing() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("k")).unwrap();
        fs::write(d.path().join("k/a.md"), "listed").unwrap();
        let s = WorkingTreeSource::new(d.path());
        let (entries, _) = s.list(&["k".into()]).unwrap();
        fs::write(d.path().join("k/a.md"), "edited").unwrap();
        let err = s.read(&entries).unwrap_err();
        assert_eq!(err.code, ErrorCode::IoError);
        assert!(err.message.contains("`k/a.md` changed"), "{err}");
        assert!(err.hint.is_some());
        // Content ids of other schemes are immutable object ids and are not re-hashed.
        assert!(verify_content("x", "git:0123", b"anything").is_ok());
        assert!(verify_content("x", &format!("sha256:{}", sha256_hex(b"a")), b"a").is_ok());
    }

    #[test]
    fn frozen_listing_serves_only_the_listed_content() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("p/k/sub")).unwrap();
        fs::write(d.path().join("p/cfg.toml"), "cfg").unwrap();
        fs::write(d.path().join("p/k/a.md"), "a").unwrap();
        fs::write(d.path().join("p/k/sub/b.md"), "b").unwrap();
        fs::write(d.path().join("outside.txt"), "o").unwrap();
        let prefixes = vec!["p/cfg.toml".to_string(), "p/k/".to_string()];
        let plain = WorkingTreeSource::new(d.path());
        let f = FrozenWorkingTree::freeze(WorkingTreeSource::new(d.path()), &prefixes).unwrap();
        assert_eq!(
            f.entries().cloned().collect::<Vec<_>>(),
            plain.list(&prefixes).unwrap().0
        );

        // Created after the listing: not part of it.
        fs::write(d.path().join("p/k/c.md"), "c").unwrap();
        let (sub, _) = f.list(&["p/k/sub".into()]).unwrap();
        assert_eq!(sub.len(), 1);
        let (all, _) = f.list(&["p/k".into()]).unwrap();
        let paths: Vec<_> = all.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["p/k/a.md", "p/k/sub/b.md"]);
        assert_eq!(f.read(&all).unwrap(), vec![b"a".to_vec(), b"b".to_vec()]);
        assert_eq!(f.read_path("p/cfg.toml").unwrap().unwrap(), b"cfg");
        assert!(f.read_path("p/k/missing.md").unwrap().is_none());
        assert_eq!(f.read_path("outside.txt").unwrap().unwrap(), b"o");
        let changed = |r: Result<Option<Vec<u8>>>| r.unwrap_err().code == ErrorCode::IoError;
        assert!(
            changed(f.read_path("p/k/c.md")),
            "created after the listing"
        );

        // Unlisted prefixes, other content ids, edits and deletions are all refused.
        assert_eq!(
            f.list(&["p/other".into()]).unwrap_err().code,
            ErrorCode::IoError
        );
        assert!(f.list(&["../x".into()]).is_err());
        let mut forged = all[0].clone();
        forged.content_id = format!("sha256:{}", sha256_hex(b"forged"));
        assert!(f.read(&[forged]).is_err());
        fs::write(d.path().join("p/cfg.toml"), "CFG").unwrap();
        assert!(changed(f.read_path("p/cfg.toml")));
        fs::write(d.path().join("p/k/a.md"), "A").unwrap();
        assert_eq!(f.read(&all).unwrap_err().code, ErrorCode::IoError);
        fs::remove_file(d.path().join("p/k/sub/b.md")).unwrap();
        let err = f.read(&all[1..]).unwrap_err();
        assert!(err.message.contains("`p/k/sub/b.md` changed"), "{err}");
    }
}

#[cfg(test)]
mod stat_cache_tests {
    use super::*;

    fn backdate(path: &Path, secs_ago: u64) {
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago);
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(t).unwrap();
    }

    #[test]
    fn reuses_content_ids_and_detects_changes() {
        let d = tempfile::tempdir().unwrap();
        let k = d.path().join("p/k");
        fs::create_dir_all(&k).unwrap();
        fs::write(k.join("a.md"), "one").unwrap();
        fs::write(k.join("b.md"), "two").unwrap();
        backdate(&k.join("a.md"), 60);
        backdate(&k.join("b.md"), 60);
        let cache = d.path().join("cache/wt-stat.json");
        let plain = WorkingTreeSource::new(d.path());
        let (expected, _) = plain.list(&["p/k".into()]).unwrap();

        let s = WorkingTreeSource::with_stat_cache(d.path(), &cache);
        assert_eq!(s.list(&["p/k".into()]).unwrap().0, expected);
        assert!(cache.is_file());
        // A new source instance trusts the (non-racy) cached entries.
        let s2 = WorkingTreeSource::with_stat_cache(d.path(), &cache);
        assert_eq!(s2.list(&["p/k".into()]).unwrap().0, expected);

        // A content change (same size, even with an old mtime) is detected via ctime.
        fs::write(k.join("a.md"), "ONE").unwrap();
        backdate(&k.join("a.md"), 60);
        let (after, _) = WorkingTreeSource::with_stat_cache(d.path(), &cache)
            .list(&["p/k".into()])
            .unwrap();
        let (truth, _) = plain.list(&["p/k".into()]).unwrap();
        assert_eq!(after, truth);
        assert_ne!(after[0].content_id, expected[0].content_id);

        // Deleted files drop out of the listing and the cache.
        fs::remove_file(k.join("b.md")).unwrap();
        let (after, _) = WorkingTreeSource::with_stat_cache(d.path(), &cache)
            .list(&["p/k".into()])
            .unwrap();
        assert_eq!(after.len(), 1);
        let text = fs::read_to_string(&cache).unwrap();
        assert!(!text.contains("b.md"));
    }

    #[test]
    fn racily_clean_entries_are_rehashed() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("k")).unwrap();
        let f = d.path().join("k/a.md");
        fs::write(&f, "old").unwrap();
        let cache = d.path().join("wt-stat.json");
        let s = WorkingTreeSource::with_stat_cache(d.path(), &cache);
        let (first, _) = s.list(&["k".into()]).unwrap();
        // Same size, same (fresh) mtime second: only the racy check forces a re-hash.
        fs::write(&f, "new").unwrap();
        let (second, _) = WorkingTreeSource::with_stat_cache(d.path(), &cache)
            .list(&["k".into()])
            .unwrap();
        assert_ne!(first[0].content_id, second[0].content_id);
    }

    #[test]
    fn a_wrong_cached_id_fails_the_read_and_is_forgotten() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("k")).unwrap();
        let f = d.path().join("k/a.md");
        fs::write(&f, "one").unwrap();
        backdate(&f, 60);
        let cache = d.path().join("wt-stat.json");
        let (truth, _) = WorkingTreeSource::with_stat_cache(d.path(), &cache)
            .list(&["k".into()])
            .unwrap();
        // The cache claims other content for an unchanged (non-racy) file.
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
        let wrong = format!("sha256:{}", sha256_hex(b"two"));
        json["entries"]["k/a.md"]["content_id"] = wrong.clone().into();
        fs::write(&cache, serde_json::to_vec(&json).unwrap()).unwrap();

        let s = WorkingTreeSource::with_stat_cache(d.path(), &cache);
        let (listed, _) = s.list(&["k".into()]).unwrap();
        assert_eq!(listed[0].content_id, wrong);
        let err = s.read(&listed).unwrap_err();
        assert_eq!(err.code, ErrorCode::IoError);
        assert!(err.message.contains("k/a.md"), "{err}");
        // A retry re-hashes the file.
        let (again, _) = WorkingTreeSource::with_stat_cache(d.path(), &cache)
            .list(&["k".into()])
            .unwrap();
        assert_eq!(again, truth);
    }

    #[test]
    fn corrupt_cache_is_ignored() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("k")).unwrap();
        fs::write(d.path().join("k/a.md"), "x").unwrap();
        let cache = d.path().join("wt-stat.json");
        fs::write(&cache, "{not json").unwrap();
        let (v, issues) = WorkingTreeSource::with_stat_cache(d.path(), &cache)
            .list(&["k".into()])
            .unwrap();
        assert_eq!(v.len(), 1);
        assert!(issues.is_empty());
    }
}
