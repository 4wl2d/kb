//! Small shared helpers: hashing, safe paths and atomic writes.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{KbError, Result};

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

pub fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

/// Incremental SHA-256 over length-prefixed fields (unambiguous concatenation).
#[derive(Default)]
pub struct FieldHasher(Sha256);

impl FieldHasher {
    pub fn new() -> Self {
        FieldHasher(Sha256::new())
    }
    pub fn field(&mut self, bytes: impl AsRef<[u8]>) -> &mut Self {
        let b = bytes.as_ref();
        self.0.update((b.len() as u64).to_le_bytes());
        self.0.update(b);
        self
    }
    pub fn finish_hex(self) -> String {
        to_hex(&self.0.finalize())
    }
}

/// Validate a repository-relative, `/`-separated path: non-empty, not absolute, no `..`,
/// no `.` segments, no empty segments, no backslashes, no NUL, no control characters.
pub fn check_rel_path(p: &str) -> std::result::Result<(), String> {
    if p.is_empty() {
        return Err("path is empty".into());
    }
    if p.len() > 4096 {
        return Err("path is too long".into());
    }
    if p.starts_with('/') {
        return Err(format!("path `{p}` must be relative"));
    }
    if p.contains('\\') {
        return Err(format!("path `{p}` must use `/` separators"));
    }
    if p.chars().any(|c| c == '\0' || c.is_control()) {
        return Err("path contains control characters".into());
    }
    if p.len() >= 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic() {
        return Err(format!("path `{p}` looks like a drive path"));
    }
    for seg in p.split('/') {
        if seg.is_empty() && !p.ends_with('/') {
            return Err(format!("path `{p}` contains an empty segment"));
        }
        if seg == ".." || seg == "." {
            return Err(format!("path `{p}` must not contain `.` or `..` segments"));
        }
    }
    Ok(())
}

/// Join a validated relative path onto `root`, refusing to traverse symlinks inside `root`.
/// The final component may not exist yet (for writes).
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf> {
    check_rel_path(rel.trim_end_matches('/')).map_err(KbError::unsafe_path)?;
    let mut cur = root.to_path_buf();
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(seg) => {
                cur.push(seg);
                if let Ok(md) = fs::symlink_metadata(&cur)
                    && md.file_type().is_symlink()
                {
                    return Err(KbError::unsafe_path(format!(
                        "refusing to follow symlink `{}`",
                        cur.display()
                    )));
                }
            }
            _ => {
                return Err(KbError::unsafe_path(format!("unsafe path `{rel}`")));
            }
        }
    }
    Ok(cur)
}

/// Read a regular file (never through a symlink) with a size limit.
pub fn read_file_limited(path: &Path, max: u64) -> Result<Vec<u8>> {
    let md = fs::symlink_metadata(path).map_err(|e| KbError::io(path.display(), e))?;
    if md.file_type().is_symlink() {
        return Err(KbError::unsafe_path(format!(
            "refusing to read symlink `{}`",
            path.display()
        )));
    }
    if !md.is_file() {
        return Err(KbError::invalid_input(format!(
            "`{}` is not a regular file",
            path.display()
        )));
    }
    if md.len() > max {
        return Err(KbError::invalid_input(format!(
            "`{}` is {} bytes, above the limit of {} bytes",
            path.display(),
            md.len(),
            max
        )));
    }
    fs::read(path).map_err(|e| KbError::io(path.display(), e))
}

/// Atomically replace `path` with `bytes`: write a sibling temporary file, fsync, rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| KbError::invalid_input(format!("`{}` has no parent", path.display())))?;
    fs::create_dir_all(dir).map_err(|e| KbError::io(dir.display(), e))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}.tmp.{}.{}",
        std::process::id(),
        unique_suffix()
    ));
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| KbError::io(tmp.display(), e))?;
        f.write_all(bytes)
            .map_err(|e| KbError::io(tmp.display(), e))?;
        f.sync_all().map_err(|e| KbError::io(tmp.display(), e))?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        KbError::io(path.display(), e)
    })
}

fn unique_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    t ^ N.fetch_add(1, Ordering::Relaxed).rotate_left(32)
}

/// Convert a filesystem path relative to `root` into a `/`-separated string.
pub fn rel_string(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str()?.to_string()),
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_path_rules() {
        assert!(check_rel_path("a/b.md").is_ok());
        for bad in ["", "/abs", "a/../b", "./a", "a\\b", "a//b", "C:x", "a\0b"] {
            assert!(check_rel_path(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn hashing() {
        assert_eq!(
            sha256_hex(b"x"),
            "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
        );
        let mut a = FieldHasher::new();
        a.field("ab").field("c");
        let mut b = FieldHasher::new();
        b.field("a").field("bc");
        assert_ne!(a.finish_hex(), b.finish_hex());
    }

    #[test]
    fn safe_join_rejects_symlinks_and_traversal() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("real")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        assert!(safe_join(dir.path(), "real/x").is_ok());
        #[cfg(unix)]
        assert!(safe_join(dir.path(), "link/x").is_err());
        assert!(safe_join(dir.path(), "../x").is_err());
    }

    #[test]
    fn atomic_write_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/f.txt");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        assert_eq!(fs::read_dir(dir.path().join("sub")).unwrap().count(), 1);
    }
}
