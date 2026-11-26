//! Release archives: `kb-<version>-<target>.tar.gz` holding one top directory
//! `kb-<version>-<target>/` with `kb` (`kb.exe` for Windows targets), `BUILD-INFO`, `LICENSE`
//! and `README.md`. Archives are written reproducibly (fixed order, times and owners) and
//! read defensively: only regular files and directories inside the top directory, without
//! `..` or absolute paths, are accepted, and nothing is extracted outside a private directory.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;

use crate::error::{ErrorCode, KbError, Result};

/// Upper bound for one extracted file (the engine binary is well below it).
const MAX_ENTRY_BYTES: u64 = 512 << 20;

pub fn top_dir(version: &str, target: &str) -> String {
    format!("kb-{version}-{target}")
}

pub fn file_name(version: &str, target: &str) -> String {
    format!("{}.tar.gz", top_dir(version, target))
}

/// One file of a new archive: its name inside the top directory, source and mode.
pub struct Member<'a> {
    pub name: &'a str,
    pub source: &'a Path,
    pub executable: bool,
}

/// Write `out` (a gzip-compressed tar) reproducibly with `top/` and the members in order.
/// File times are `SOURCE_DATE_EPOCH` when set, else 0.
pub fn create(out: &Path, top: &str, members: &[Member<'_>]) -> Result<()> {
    let mtime = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let io = |e: io::Error| {
        KbError::new(
            ErrorCode::PackageFailed,
            format!("cannot write {}: {e}", out.display()),
        )
    };
    let file = File::create(out).map_err(io)?;
    let gz = flate2::GzBuilder::new()
        .mtime(0)
        .write(file, Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    let mut header = tar::Header::new_ustar();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_mode(0o755);
    header.set_size(0);
    header.set_mtime(mtime);
    header.set_uid(0);
    header.set_gid(0);
    tar.append_data(&mut header, format!("{top}/"), io::empty())
        .map_err(io)?;
    for m in members {
        let bytes = fs::read(m.source).map_err(|e| {
            KbError::new(
                ErrorCode::PackageFailed,
                format!("cannot read {}: {e}", m.source.display()),
            )
        })?;
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(if m.executable { 0o755 } else { 0o644 });
        header.set_size(bytes.len() as u64);
        header.set_mtime(mtime);
        header.set_uid(0);
        header.set_gid(0);
        tar.append_data(&mut header, format!("{top}/{}", m.name), bytes.as_slice())
            .map_err(io)?;
    }
    let gz = tar.into_inner().map_err(io)?;
    let mut file = gz.finish().map_err(io)?;
    file.flush().map_err(io)?;
    file.sync_all().map_err(io)
}

fn invalid(msg: impl Into<String>) -> KbError {
    KbError::new(ErrorCode::ArtifactInvalid, msg)
}

/// The relative path below `top/` of an archive entry name, or an error for names outside
/// it. `top/` itself yields an empty path.
fn member_path(name: &str, top: &str) -> Result<String> {
    let rest = name
        .strip_prefix(top)
        .and_then(|r| {
            if r.is_empty() {
                Some("")
            } else {
                r.strip_prefix('/')
            }
        })
        .ok_or_else(|| {
            invalid(format!(
                "unsafe archive entry '{name}' (entries must stay inside {top}/)"
            ))
        })?;
    let rest = rest.trim_end_matches('/');
    if !rest.is_empty()
        && rest
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == ".." || c.contains('\\') || c.contains(':'))
    {
        return Err(invalid(format!(
            "unsafe archive entry '{name}' (no '..', '.', empty or drive components)"
        )));
    }
    Ok(rest.to_string())
}

/// Extract `archive` into the empty directory `into` and return `into/<top>`. Fails on any
/// entry that is not a regular file or directory inside `top/`.
pub fn extract(archive: &Path, top: &str, into: &Path) -> Result<PathBuf> {
    let file = File::open(archive).map_err(|e| invalid(format!("cannot open the archive: {e}")))?;
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    let entries = tar.entries().map_err(|e| {
        invalid(format!(
            "cannot read the archive (not a gzip-compressed tar?): {e}"
        ))
    })?;
    let mut count = 0usize;
    for entry in entries {
        let mut entry = entry.map_err(|e| {
            invalid(format!(
                "cannot read the archive (not a gzip-compressed tar?): {e}"
            ))
        })?;
        let name = String::from_utf8(entry.path_bytes().into_owned())
            .map_err(|_| invalid("archive entry name is not UTF-8"))?;
        let rel = member_path(&name, top)?;
        let dest = if rel.is_empty() {
            into.join(top)
        } else {
            into.join(top).join(&rel)
        };
        match entry.header().entry_type() {
            tar::EntryType::Directory => {
                fs::create_dir_all(&dest).map_err(|e| KbError::io(dest.display(), e))?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous if !rel.is_empty() => {
                if entry.size() > MAX_ENTRY_BYTES {
                    return Err(invalid(format!("archive entry '{name}' is too large")));
                }
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(|e| KbError::io(parent.display(), e))?;
                }
                if fs::symlink_metadata(&dest).is_ok() {
                    return Err(invalid(format!("archive entry '{name}' appears twice")));
                }
                let mut out = File::create(&dest).map_err(|e| KbError::io(dest.display(), e))?;
                let copied = io::copy(&mut (&mut entry).take(MAX_ENTRY_BYTES + 1), &mut out)
                    .map_err(|e| invalid(format!("cannot extract '{name}': {e}")))?;
                if copied > MAX_ENTRY_BYTES {
                    return Err(invalid(format!("archive entry '{name}' is too large")));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = if entry.header().mode().unwrap_or(0) & 0o111 != 0 {
                        0o755
                    } else {
                        0o644
                    };
                    fs::set_permissions(&dest, fs::Permissions::from_mode(mode))
                        .map_err(|e| KbError::io(dest.display(), e))?;
                }
            }
            other => {
                return Err(invalid(format!(
                    "archive entry is not a regular file or directory: {name} ({other:?})"
                )));
            }
        }
        count += 1;
    }
    if count == 0 {
        return Err(invalid("the archive is empty"));
    }
    let dir = into.join(top);
    if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return Err(invalid(format!("the archive has no {top}/ directory")));
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_names_stay_inside_the_top_directory() {
        assert_eq!(member_path("kb-1-x/", "kb-1-x").unwrap(), "");
        assert_eq!(member_path("kb-1-x", "kb-1-x").unwrap(), "");
        assert_eq!(member_path("kb-1-x/kb", "kb-1-x").unwrap(), "kb");
        assert_eq!(member_path("kb-1-x/a/b/", "kb-1-x").unwrap(), "a/b");
        for bad in [
            "/kb-1-x/kb",
            "kb-1-xy/kb",
            "kb-1-x/../kb",
            "kb-1-x/a/../../kb",
            "kb-1-x//kb",
            "kb-1-x/./kb",
            "kb-1-x/C:/kb",
            "kb-1-x/a\\..\\kb",
            "other/kb",
        ] {
            assert!(member_path(bad, "kb-1-x").is_err(), "{bad}");
        }
    }

    #[test]
    fn archives_are_reproducible_and_round_trip() {
        let t = tempfile::tempdir().unwrap();
        let bin = t.path().join("bin");
        let info = t.path().join("info");
        fs::write(&bin, "binary").unwrap();
        fs::write(&info, "engine_version=1\n").unwrap();
        let members = [
            Member {
                name: "kb",
                source: &bin,
                executable: true,
            },
            Member {
                name: "BUILD-INFO",
                source: &info,
                executable: false,
            },
        ];
        let a = t.path().join("a.tar.gz");
        let b = t.path().join("b.tar.gz");
        create(&a, "kb-1-x", &members).unwrap();
        create(&b, "kb-1-x", &members).unwrap();
        assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
        let into = t.path().join("x");
        fs::create_dir(&into).unwrap();
        let dir = extract(&a, "kb-1-x", &into).unwrap();
        assert_eq!(fs::read_to_string(dir.join("kb")).unwrap(), "binary");
        assert_eq!(
            fs::read_to_string(dir.join("BUILD-INFO")).unwrap(),
            "engine_version=1\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.join("kb")).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
        // The wrong top directory is refused.
        let other = t.path().join("y");
        fs::create_dir(&other).unwrap();
        assert_eq!(
            extract(&a, "kb-2-x", &other).unwrap_err().code,
            ErrorCode::ArtifactInvalid
        );
    }

    #[test]
    fn links_are_refused() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("l.tar.gz");
        let gz = flate2::write::GzEncoder::new(File::create(&path).unwrap(), Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut h = tar::Header::new_ustar();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        h.set_link_name("/etc/passwd").unwrap();
        tar.append_data(&mut h, "kb-1-x/kb", io::empty()).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        let into = t.path().join("x");
        fs::create_dir(&into).unwrap();
        let err = extract(&path, "kb-1-x", &into).unwrap_err();
        assert!(
            err.message.contains("not a regular file"),
            "{}",
            err.message
        );
    }
}
