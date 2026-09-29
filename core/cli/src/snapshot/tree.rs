//! [`GitTreeSource`]: an immutable snapshot of files read from Git objects (`ls-tree` +
//! `cat-file --batch`), never from a work tree.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::error::{ErrorCode, KbError, Result};
use crate::git::{Git, check_revision_arg};
use crate::source::{MAX_SOURCE_FILE_BYTES, SourceEntry, SourceIssue, SourceTree};
use crate::util::check_rel_path;

const SYMLINK_MODE: &str = "120000";
const GITLINK_MODE: &str = "160000";

/// Files of one commit in a repository (normally the isolated KB mirror).
#[derive(Debug, Clone)]
pub struct GitTreeSource {
    git: Git,
    commit: String,
    /// Files read ahead by [`GitTreeSource::prefetch`] (`None` = absent). Commit content is
    /// immutable, so the cache never goes stale.
    prefetched: Arc<Mutex<BTreeMap<String, Option<Vec<u8>>>>>,
}

impl GitTreeSource {
    /// A source over `commit` (a full commit id is expected) in the repository of `git`.
    pub fn new(git: Git, commit: impl Into<String>) -> Result<GitTreeSource> {
        let commit = commit.into();
        check_revision_arg(&commit)?;
        Ok(GitTreeSource {
            git,
            commit,
            prefetched: Arc::default(),
        })
    }

    /// The commit this source reads.
    pub fn commit(&self) -> &str {
        &self.commit
    }

    /// Read several small files with a single `cat-file --batch` process over
    /// `<commit>:<path>` names; later [`SourceTree::read_path`] calls for these paths are
    /// answered from memory. `cat-file` never follows symlinks (a symlink yields its link
    /// text), so only plain blobs within the size limit are cached; anything else is left to
    /// `read_path`, which inspects the tree entry and reports it.
    pub fn prefetch(&self, paths: &[&str]) -> Result<()> {
        for p in paths {
            check_rel_path(p).map_err(KbError::unsafe_path)?;
        }
        let names: Vec<String> = paths
            .iter()
            .map(|p| format!("{}:{p}", self.commit))
            .collect();
        let objects = self.git.cat_file_objects(&names)?;
        let mut cache = self.prefetched.lock().unwrap_or_else(|p| p.into_inner());
        for (p, obj) in paths.iter().zip(objects) {
            match obj {
                None => {
                    cache.insert((*p).to_string(), None);
                }
                Some((kind, bytes))
                    if kind == "blob" && bytes.len() as u64 <= MAX_SOURCE_FILE_BYTES =>
                {
                    // A symlink entry also reads as a blob: confirm the mode only for the
                    // unusual case of a blob that could be link text (no line break).
                    if !bytes.contains(&b'\n') && self.is_symlink(p)? {
                        continue;
                    }
                    cache.insert((*p).to_string(), Some(bytes));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    fn is_symlink(&self, path: &str) -> Result<bool> {
        Ok(self
            .ls_tree(false, &[path])?
            .iter()
            .any(|e| e.path == path.as_bytes() && e.mode == SYMLINK_MODE))
    }

    fn ls_tree(&self, recursive: bool, paths: &[&str]) -> Result<Vec<TreeEntry>> {
        let mut args = vec!["--literal-pathspecs", "ls-tree", "-z", "-l", "--full-tree"];
        if recursive {
            args.push("-r");
        }
        args.push(&self.commit);
        args.push("--");
        args.extend_from_slice(paths);
        let out = self.git.run_bytes(&args)?;
        Ok(out
            .split(|b| *b == 0)
            .filter(|e| !e.is_empty())
            .filter_map(TreeEntry::parse)
            .collect())
    }
}

/// One `git ls-tree -l -z` line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeEntry {
    mode: String,
    kind: String,
    oid: String,
    /// Blob size (`None` for trees and gitlinks).
    size: Option<u64>,
    /// Raw path bytes (Git paths need not be UTF-8).
    path: Vec<u8>,
}

impl TreeEntry {
    fn parse(line: &[u8]) -> Option<TreeEntry> {
        let tab = line.iter().position(|b| *b == b'\t')?;
        let meta = std::str::from_utf8(&line[..tab]).ok()?;
        let fields: Vec<&str> = meta.split_ascii_whitespace().collect();
        if fields.len() < 3 {
            return None;
        }
        Some(TreeEntry {
            mode: fields[0].to_string(),
            kind: fields[1].to_string(),
            oid: fields[2].to_string(),
            size: fields.get(3).and_then(|s| s.parse().ok()),
            path: line[tab + 1..].to_vec(),
        })
    }

    fn path_lossy(&self) -> String {
        String::from_utf8_lossy(&self.path).into_owned()
    }
}

fn is_hex_oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn oid_of(entry: &SourceEntry) -> Result<String> {
    entry
        .content_id
        .strip_prefix("git:")
        .filter(|o| is_hex_oid(o))
        .map(str::to_string)
        .ok_or_else(|| {
            KbError::invalid_input(format!(
                "`{}` has content id `{}`, which is not a Git blob id",
                entry.path, entry.content_id
            ))
        })
}

impl SourceTree for GitTreeSource {
    fn describe(&self) -> String {
        format!("git:{}", self.commit)
    }

    fn list(&self, prefixes: &[String]) -> Result<(Vec<SourceEntry>, Vec<SourceIssue>)> {
        let mut paths = Vec::with_capacity(prefixes.len());
        for p in prefixes {
            let p = p.trim_end_matches('/');
            check_rel_path(p).map_err(KbError::unsafe_path)?;
            paths.push(p);
        }
        if paths.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut entries = Vec::new();
        let mut issues = Vec::new();
        for e in self.ls_tree(true, &paths)? {
            let path = match String::from_utf8(e.path.clone()) {
                Ok(p) => p,
                Err(_) => {
                    issues.push(SourceIssue {
                        path: e.path_lossy(),
                        code: "UNSAFE_PATH",
                        message: "path is not valid UTF-8".into(),
                    });
                    continue;
                }
            };
            if let Err(msg) = check_rel_path(&path) {
                issues.push(SourceIssue {
                    path,
                    code: "UNSAFE_PATH",
                    message: msg,
                });
                continue;
            }
            match (e.mode.as_str(), e.kind.as_str()) {
                (SYMLINK_MODE, _) => issues.push(SourceIssue {
                    path,
                    code: "SYMLINK_NOT_ALLOWED",
                    message: "symlinks are not followed inside knowledge sources".into(),
                }),
                (GITLINK_MODE, _) => {}
                (_, "blob") => {
                    let size = e.size.unwrap_or(0);
                    if size > MAX_SOURCE_FILE_BYTES {
                        issues.push(SourceIssue {
                            path,
                            code: "SOURCE_UNREADABLE",
                            message: format!(
                                "{size} bytes, above the limit of {MAX_SOURCE_FILE_BYTES} bytes"
                            ),
                        });
                    } else {
                        entries.push(SourceEntry {
                            path,
                            content_id: format!("git:{}", e.oid),
                            size,
                        });
                    }
                }
                _ => {}
            }
        }
        entries.sort();
        entries.dedup_by(|a, b| a.path == b.path);
        issues.sort_by(|a, b| a.path.cmp(&b.path));
        Ok((entries, issues))
    }

    fn read(&self, entries: &[SourceEntry]) -> Result<Vec<Vec<u8>>> {
        let oids = entries.iter().map(oid_of).collect::<Result<Vec<_>>>()?;
        self.git.cat_file_batch(&oids)
    }

    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>> {
        check_rel_path(path).map_err(KbError::unsafe_path)?;
        if let Some(hit) = self
            .prefetched
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(path)
        {
            return Ok(hit.clone());
        }
        let entry = self
            .ls_tree(false, &[path])?
            .into_iter()
            .find(|e| e.path == path.as_bytes());
        let Some(e) = entry else {
            return Ok(None);
        };
        if e.mode == SYMLINK_MODE {
            return Err(KbError::unsafe_path(format!(
                "refusing to read symlink `{path}` in {}",
                self.describe()
            )));
        }
        if e.kind != "blob" {
            return Err(KbError::invalid_input(format!(
                "`{path}` is not a regular file"
            )));
        }
        let size = e.size.unwrap_or(0);
        if size > MAX_SOURCE_FILE_BYTES {
            return Err(KbError::invalid_input(format!(
                "`{path}` is {size} bytes, above the limit of {MAX_SOURCE_FILE_BYTES} bytes"
            )));
        }
        let mut blobs = self.git.cat_file_batch(std::slice::from_ref(&e.oid))?;
        blobs.pop().map(Some).ok_or_else(|| {
            KbError::new(
                ErrorCode::GitError,
                format!("cat-file returned nothing for `{path}`"),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ls_tree_lines() {
        let e = TreeEntry::parse(
            b"100644 blob 78981922613b2afb6025042ff6bd878ac1994e85       2\td/a b.md",
        )
        .unwrap();
        assert_eq!(e.mode, "100644");
        assert_eq!(e.kind, "blob");
        assert_eq!(e.size, Some(2));
        assert_eq!(e.path, b"d/a b.md");
        let g =
            TreeEntry::parse(b"160000 commit 9e593dbdd9640b37665992cc31fedb00ad6c7f22       -\tkb")
                .unwrap();
        assert_eq!(g.size, None);
        assert!(TreeEntry::parse(b"garbage").is_none());
    }

    #[test]
    fn rejects_non_git_content_ids() {
        let e = SourceEntry {
            path: "a.md".into(),
            content_id: "git:abc\nHEAD".into(),
            size: 1,
        };
        assert!(oid_of(&e).is_err());
        let ok = SourceEntry {
            content_id: "git:78981922613b2afb6025042ff6bd878ac1994e85".into(),
            ..e
        };
        assert_eq!(oid_of(&ok).unwrap().len(), 40);
    }
}
