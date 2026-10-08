use crate::{Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub fn digest(bytes: &[u8]) -> String {
    kb::util::sha256_hex(bytes)
}
pub fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure(
        bytes.len() as u64 <= limit,
        format!("file exceeds {limit} bytes: {}", path.display()),
    )?;
    Ok(bytes)
}
pub fn json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&read(path, 64 * 1024 * 1024)?)?)
}
pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_new(path, &serde_json::to_vec_pretty(value)?)
}
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}
pub fn absolute(base: &Path, path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
    .canonicalize()?)
}
pub fn isolated_env(command: &mut Command, home: &Path) {
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "KB Replay")
        .env("GIT_AUTHOR_EMAIL", "kb-replay@example.invalid")
        .env("GIT_COMMITTER_NAME", "KB Replay")
        .env("GIT_COMMITTER_EMAIL", "kb-replay@example.invalid")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("NO_COLOR", "1")
        .env("DO_NOT_TRACK", "1");
}
pub fn git(dir: &Path, home: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut c = Command::new("/usr/bin/git");
    isolated_env(&mut c, home);
    c.arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "submodule.recurse=false",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args);
    let result = kb::process::capture(
        &mut c,
        &[],
        kb::process::Limits {
            timeout: Duration::from_secs(300),
            stdout: 128 * 1024 * 1024,
            stderr: 8 * 1024 * 1024,
        },
    )?;
    ensure(
        result.exit_code == 0,
        format!(
            "git failed ({}): {}",
            result.exit_code,
            String::from_utf8_lossy(&result.stderr)
        ),
    )?;
    Ok(result.stdout)
}
pub fn checkout(
    source: &Path,
    commit: &str,
    destination: &Path,
    home: &Path,
    history: crate::model::History,
) -> Result<()> {
    crate::model::commit(commit)?;
    fs::create_dir(destination)?;
    let format = if commit.len() == 64 {
        "--object-format=sha256"
    } else {
        "--object-format=sha1"
    };
    git(destination, home, &["init", "-q", format])?;
    let source = source.canonicalize()?;
    let mut fetch = vec!["fetch", "--no-tags", "--no-write-fetch-head"];
    if history == crate::model::History::BaseOnly {
        fetch.push("--depth=1");
    }
    fetch.extend([source.to_str().ok_or("non-UTF-8 repository path")?, commit]);
    git(destination, home, &fetch)?;
    git(
        destination,
        home,
        &["checkout", "--quiet", "--detach", commit],
    )?;
    ensure(
        !destination.join(".git/objects/info/alternates").exists(),
        "replay checkout unexpectedly has alternates",
    )?;
    let head = git(destination, home, &["rev-parse", "HEAD"])?;
    ensure(
        String::from_utf8_lossy(&head).trim() == commit,
        "checkout HEAD differs from its frozen commit",
    )?;
    if history == crate::model::History::Ancestors {
        ensure(
            !destination.join(".git/shallow").exists(),
            "ancestor replay requires complete past ancestry, not an already shallow source",
        )?;
    }
    // Check objects too, not only refs: an unreachable future blob must not be exposed.
    let reachable = git(destination, home, &["rev-list", "--objects", commit])?;
    let objects = git(
        destination,
        home,
        &[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname)",
        ],
    )?;
    let reachable: BTreeSet<_> = std::str::from_utf8(&reachable)?
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    ensure(
        std::str::from_utf8(&objects)?
            .lines()
            .all(|oid| reachable.contains(oid)),
        "checkout contains objects outside the requested historical view",
    )?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileEntry {
    pub sha256: String,
    pub executable: bool,
}
pub type Tree = BTreeMap<String, FileEntry>;

/// Never follows links. Git metadata and explicit treatment directories stay out of patches.
pub fn tree(root: &Path, excluded: &[String]) -> Result<Tree> {
    fn walk(
        root: &Path,
        dir: &Path,
        excluded: &[String],
        out: &mut Tree,
        total: &mut u64,
    ) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            let rel = path
                .strip_prefix(root)?
                .to_str()
                .ok_or("non-UTF-8 path")?
                .replace('\\', "/");
            if e.file_name() == ".git"
                || excluded
                    .iter()
                    .any(|p| rel == *p || rel.starts_with(&format!("{p}/")))
            {
                continue;
            }
            let meta = fs::symlink_metadata(&path)?;
            ensure(
                !meta.file_type().is_symlink(),
                format!("replay tree contains a symlink: {rel}"),
            )?;
            if meta.is_dir() {
                walk(root, &path, excluded, out, total)?;
            } else {
                ensure(meta.is_file(), "unsupported file type in replay tree")?;
                *total += meta.len();
                ensure(
                    *total <= 2 * 1024 * 1024 * 1024 && out.len() < 200_000,
                    "replay tree exceeds bounds",
                )?;
                let bytes = read(&path, 128 * 1024 * 1024)?;
                #[cfg(unix)]
                let executable = {
                    use std::os::unix::fs::PermissionsExt;
                    meta.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let executable = false;
                out.insert(
                    rel,
                    FileEntry {
                        sha256: digest(&bytes),
                        executable,
                    },
                );
            }
        }
        Ok(())
    }
    let mut result = Tree::new();
    walk(root, root, excluded, &mut result, &mut 0)?;
    Ok(result)
}
pub fn tree_digest(root: &Path) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&tree(root, &[])?)?))
}
pub fn tracked_tree(
    trusted_git: &Path,
    work: &Path,
    home: &Path,
    excluded: &[String],
) -> Result<Tree> {
    let listing = git(
        trusted_git,
        home,
        &[
            "--work-tree",
            work.to_str().ok_or("non-UTF-8 work path")?,
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    let mut out = Tree::new();
    let mut total = 0_u64;
    for raw in listing.split(|b| *b == 0).filter(|v| !v.is_empty()) {
        let rel = std::str::from_utf8(raw)?;
        if excluded
            .iter()
            .any(|p| rel == p || rel.starts_with(&format!("{p}/")))
        {
            continue;
        }
        let p = safe_destination(work, rel)?;
        let meta = match fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        ensure(
            meta.is_file() && !meta.file_type().is_symlink(),
            "only regular source files can enter a candidate patch",
        )?;
        total += meta.len();
        ensure(
            total <= 2 * 1024 * 1024 * 1024 && out.len() < 200_000,
            "candidate source exceeds bounds",
        )?;
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        out.insert(
            rel.into(),
            FileEntry {
                sha256: digest(&read(&p, 128 * 1024 * 1024)?),
                executable,
            },
        );
    }
    Ok(out)
}
fn safe_destination(root: &Path, relative: &str) -> Result<PathBuf> {
    kb::util::check_rel_path(relative).map_err(|e| e.to_string())?;
    ensure(
        !relative.split('/').any(|p| p.eq_ignore_ascii_case(".git")),
        "cannot copy Git metadata",
    )?;
    let mut dest = root.to_path_buf();
    for part in relative.split('/') {
        dest.push(part);
        if let Ok(meta) = fs::symlink_metadata(&dest) {
            ensure(
                !meta.file_type().is_symlink(),
                "destination traverses a symlink",
            )?;
        }
    }
    Ok(dest)
}
pub fn copy_file(source: &Path, dest_root: &Path, relative: &str, entry: &FileEntry) -> Result<()> {
    let dest = safe_destination(dest_root, relative)?;
    let bytes = read(&source.join(relative), 128 * 1024 * 1024)?;
    ensure(digest(&bytes) == entry.sha256, "source changed during copy")?;
    fs::create_dir_all(dest.parent().unwrap())?;
    fs::write(&dest, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            dest,
            fs::Permissions::from_mode(if entry.executable { 0o755 } else { 0o644 }),
        )?;
    }
    Ok(())
}
pub fn overlay_files(source: &Path, dest: &Path) -> Result<()> {
    for (p, e) in tree(source, &[])? {
        copy_file(source, dest, &p, &e)?;
    }
    Ok(())
}
pub fn transfer_changes(
    base: &Tree,
    after: &Tree,
    agent: &Path,
    clean: &Path,
) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for (p, e) in after {
        if base.get(p) != Some(e) {
            copy_file(agent, clean, p, e)?;
            changed.push(p.clone());
        }
    }
    for p in base.keys() {
        if !after.contains_key(p) {
            fs::remove_file(safe_destination(clean, p)?)?;
            changed.push(p.clone());
        }
    }
    changed.sort();
    Ok(changed)
}
