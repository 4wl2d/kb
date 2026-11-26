//! Warm path for the engine fingerprint: `<root>/.cache/engine-stamp` records the
//! fingerprint together with the size, modification time (and on Unix change time and inode)
//! of every build input. While the listed inputs and their metadata are unchanged, the
//! fingerprint is taken from the stamp instead of hashing the sources. Metadata is collected
//! before hashing, so an edit made while hashing changes it and is noticed by the next run.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

use super::fingerprint;

const STAMP: &str = ".cache/engine-stamp";
const HEADER: &str = "kb-engine-stamp 1";

fn meta_line(rel: &str, md: &fs::Metadata) -> String {
    let nanos = |t: io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos())
    };
    #[cfg(unix)]
    let (ctime, ino) = {
        use std::os::unix::fs::MetadataExt;
        (
            i128::from(md.ctime()) * 1_000_000_000 + i128::from(md.ctime_nsec()),
            md.ino(),
        )
    };
    #[cfg(not(unix))]
    let (ctime, ino) = (0i128, 0u64);
    format!(
        "{}\t{}\t{}\t{}\t{rel}",
        md.len(),
        nanos(md.modified()),
        ctime,
        ino
    )
}

fn inventory(root: &Path) -> io::Result<(Vec<String>, String)> {
    let mut paths = fingerprint::input_paths(root)?;
    paths.sort_unstable();
    let mut lines = String::new();
    for rel in &paths {
        let md = fs::symlink_metadata(root.join(rel))?;
        let _ = writeln!(lines, "{}", meta_line(rel, &md));
    }
    Ok((paths, lines))
}

/// The engine fingerprint of `root`, from the stamp when it is still valid.
pub fn fingerprint(root: &Path) -> io::Result<String> {
    let (paths, lines) = inventory(root)?;
    let stamp = root.join(STAMP);
    if let Ok(text) = fs::read_to_string(&stamp)
        && let Some(rest) = text.strip_prefix(HEADER).and_then(|r| r.strip_prefix('\n'))
        && let Some((fp, recorded)) = rest.split_once('\n')
        && fp.len() == 64
        && fp.bytes().all(|b| b.is_ascii_hexdigit())
        && recorded == lines
    {
        return Ok(fp.to_string());
    }
    let fp = fingerprint::digest(root, &paths)?;
    write_stamp(&stamp, &format!("{HEADER}\n{fp}\n{lines}"));
    Ok(fp)
}

/// Best effort: a checkout that cannot be written is hashed on every run.
fn write_stamp(path: &Path, text: &str) {
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = dir.join(format!(".engine-stamp.{}.tmp", std::process::id()));
    if fs::write(&tmp, text).is_err() || fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(root: &Path) {
        for (rel, text) in [
            ("Cargo.toml", "[workspace]\n"),
            ("Cargo.lock", "# lock\n"),
            ("rust-toolchain.toml", "[toolchain]\nchannel = \"1.0.0\"\n"),
            (
                "core/release.toml",
                "manifest = 1\nengine_version = \"0.0.1\"\nrust_toolchain = \"1.0.0\"\n",
            ),
            ("core/cli/Cargo.toml", "[package]\nname = \"kb\"\n"),
            ("core/cli/src/main.rs", "fn main() {}\n"),
            ("core/cli/src/a/b.rs", "// b\n"),
            ("core/cli/src/.hidden.rs", "ignored"),
            ("core/cli/src/notes.rs~", "ignored"),
            ("core/cli/src/.dir/kept.rs", "kept\n"),
        ] {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
    }

    /// The value the retired POSIX launcher (`kbw --kbw-fingerprint`) printed for this tree:
    /// runtimes it built keep their fingerprint.
    const KBW_FINGERPRINT: &str =
        "9ea2f70e8273fb602a04588f9a929e3a3015782b1c41fdfd65f98f359185fe21";

    #[test]
    fn matches_the_retired_launcher_and_ignores_line_endings() {
        let t = tempfile::tempdir().unwrap();
        tree(t.path());
        assert_eq!(fingerprint::compute(t.path()).unwrap(), KBW_FINGERPRINT);
        fs::write(t.path().join("core/cli/src/main.rs"), "fn main() {}\r\n").unwrap();
        assert_eq!(fingerprint::compute(t.path()).unwrap(), KBW_FINGERPRINT);
        // Knowledge and other files are not engine inputs.
        fs::create_dir_all(t.path().join("project")).unwrap();
        fs::write(t.path().join("project/x.md"), "x").unwrap();
        fs::write(t.path().join("README.md"), "x").unwrap();
        assert_eq!(fingerprint::compute(t.path()).unwrap(), KBW_FINGERPRINT);
        // A build script is an input once it exists.
        fs::write(t.path().join("core/cli/build.rs"), "fn main() {}\n").unwrap();
        assert_ne!(fingerprint::compute(t.path()).unwrap(), KBW_FINGERPRINT);
    }

    #[test]
    fn stamp_is_reused_until_an_input_changes() {
        let t = tempfile::tempdir().unwrap();
        tree(t.path());
        let first = fingerprint(t.path()).unwrap();
        assert_eq!(first, KBW_FINGERPRINT);
        let stamp = fs::read_to_string(t.path().join(STAMP)).unwrap();
        assert!(stamp.starts_with(&format!("{HEADER}\n{first}\n")));
        // A forged stamp with matching metadata is believed: proof that hashing is skipped.
        let forged = stamp.replacen(&first, &"0".repeat(64), 1);
        fs::write(t.path().join(STAMP), &forged).unwrap();
        assert_eq!(fingerprint(t.path()).unwrap(), "0".repeat(64));
        // New, removed or edited inputs invalidate it.
        fs::write(t.path().join("core/cli/src/new.rs"), "// new\n").unwrap();
        let second = fingerprint(t.path()).unwrap();
        assert_ne!(second, first);
        fs::remove_file(t.path().join("core/cli/src/new.rs")).unwrap();
        assert_eq!(fingerprint(t.path()).unwrap(), first);
        fs::write(t.path().join("core/cli/src/a/b.rs"), "// changed size\n").unwrap();
        assert_ne!(fingerprint(t.path()).unwrap(), first);
    }
}
