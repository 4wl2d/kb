//! Materialize Git blobs without checkout filters, hooks, symlinks or host-tree mutation.
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use kb::error::{KbError, Result};
use kb::model::CodeRequest;
use kb::process::{Limits, capture};

pub(crate) struct Frozen {
    _temp: tempfile::TempDir,
    pub root: PathBuf,
    home: PathBuf,
    pub files: BTreeMap<String, Vec<u8>>,
    pub limitations: Vec<String>,
}

impl Frozen {
    pub fn environment(&self, command: &mut Command) {
        command
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("CODEGRAPH_NO_DAEMON", "1")
            .env("CODEGRAPH_TELEMETRY", "0")
            .env("CODEGRAPH_NO_UPDATE_CHECK", "1")
            .env("DO_NOT_TRACK", "1")
            .env("LC_ALL", "C")
            .env("LANG", "C");
        for name in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_NAMESPACE",
            "GIT_CONFIG_COUNT",
            "AST_INDEX_WALK_UP",
        ] {
            command.env_remove(name);
        }
    }

    fn git(&self, dir: &std::path::Path, args: &[&str]) -> Result<Vec<u8>> {
        self.git_input(dir, args, &[], 32 * 1024 * 1024)
    }

    fn git_input(
        &self,
        dir: &std::path::Path,
        args: &[&str],
        input: &[u8],
        max_output: usize,
    ) -> Result<Vec<u8>> {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "submodule.recurse=false",
            ])
            .args(args);
        self.environment(&mut command);
        let output = capture(
            &mut command,
            input,
            Limits {
                timeout: Duration::from_secs(30),
                stdout: max_output,
                stderr: 1024 * 1024,
            },
        )?;
        if output.exit_code != 0 {
            return Err(KbError::invalid_input(format!(
                "snapshot git failed: {}",
                kb::git::redact(&String::from_utf8_lossy(&output.stderr))
            )));
        }
        Ok(output.stdout)
    }

    pub fn create(request: &CodeRequest) -> Result<Self> {
        let temp =
            tempfile::tempdir().map_err(|e| KbError::io("provider temporary directory", e))?;
        let root = temp.path().join("snapshot");
        let home = temp.path().join("home");
        fs::create_dir_all(&root)
            .and_then(|_| fs::create_dir_all(&home))
            .map_err(|e| KbError::io("provider directories", e))?;
        let mut frozen = Self {
            _temp: temp,
            root,
            home,
            files: BTreeMap::new(),
            limitations: vec![],
        };
        let host = PathBuf::from(&request.root);
        let resolved = frozen.git(
            &host,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{}^{{commit}}", request.commit),
            ],
        )?;
        if String::from_utf8_lossy(&resolved).trim() != request.commit {
            return Err(KbError::invalid_input(
                "provider commit does not resolve exactly",
            ));
        }
        let listing = frozen.git(&host, &["ls-tree", "-r", "-l", "-z", &request.commit])?;
        let mut entries = Vec::new();
        let mut skipped = 0;
        let mut total = 0_u64;
        for entry in listing.split(|b| *b == 0).filter(|s| !s.is_empty()) {
            let entry = std::str::from_utf8(entry)
                .map_err(|_| KbError::invalid_input("non-UTF-8 Git path"))?;
            let (metadata, path) = entry
                .split_once('\t')
                .ok_or_else(|| KbError::invalid_input("invalid tree entry"))?;
            kb::util::check_rel_path(path).map_err(KbError::unsafe_path)?;
            let fields: Vec<_> = metadata.split_whitespace().collect();
            if fields.len() != 4 {
                return Err(KbError::invalid_input("invalid Git tree metadata"));
            }
            if fields[1] != "blob"
                || !matches!(fields[0], "100644" | "100755")
                || path.split('/').any(|p| {
                    matches!(
                        p.to_ascii_lowercase().as_str(),
                        ".git" | ".codegraph" | ".ast-index"
                    )
                })
            {
                skipped += 1;
                continue;
            }
            let size: u64 = fields[3]
                .parse()
                .map_err(|_| KbError::invalid_input("invalid Git blob size"))?;
            if size > kb::provenance::MAX_ANCHOR_BYTES {
                skipped += 1;
                continue;
            }
            total += size;
            if total > 256 * 1024 * 1024 || entries.len() >= kb::code::MAX_SYMBOLS {
                return Err(KbError::invalid_input(
                    "frozen provider tree exceeds 256 MiB or 100000 files",
                ));
            }
            entries.push((path.to_string(), fields[2].to_string(), size as usize));
        }
        // Object sizes were checked before batching; immutable ids cannot change between reads.
        for chunk in entries.chunks(16) {
            let input = chunk
                .iter()
                .map(|e| format!("{}\n", e.1))
                .collect::<String>();
            let output = frozen.git_input(
                &host,
                &["cat-file", "--batch"],
                input.as_bytes(),
                64 * 1024 * 1024,
            )?;
            let mut remaining = output.as_slice();
            for (path, oid, size) in chunk {
                let header_end = remaining
                    .iter()
                    .position(|b| *b == b'\n')
                    .ok_or_else(|| KbError::invalid_input("truncated Git batch header"))?;
                let header = std::str::from_utf8(&remaining[..header_end])
                    .map_err(|_| KbError::invalid_input("invalid Git batch header"))?;
                if header != format!("{oid} blob {size}") || remaining.len() < header_end + size + 2
                {
                    return Err(KbError::invalid_input(
                        "Git batch object disagrees with immutable tree",
                    ));
                }
                let blob = remaining[header_end + 1..header_end + 1 + size].to_vec();
                remaining = &remaining[header_end + size + 2..];
                let dest = frozen.root.join(path);
                fs::create_dir_all(dest.parent().unwrap())
                    .and_then(|_| fs::write(&dest, &blob))
                    .map_err(|e| KbError::io("materialize provider source", e))?;
                frozen.files.insert(path.clone(), blob);
            }
            if !remaining.is_empty() {
                return Err(KbError::invalid_input("unexpected Git batch suffix"));
            }
        }
        let object_format = if request.commit.len() == 64 {
            "--object-format=sha256"
        } else {
            "--object-format=sha1"
        };
        frozen.git(&frozen.root, &["init", "--quiet", object_format])?;
        let common = frozen.git(
            &host,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let common = std::str::from_utf8(&common)
            .map_err(|_| KbError::invalid_input("Git common directory is not UTF-8"))?
            .trim_end_matches('\n');
        if common.contains(['\n', '\r']) {
            return Err(KbError::unsafe_path(
                "Git common directory contains a line break",
            ));
        }
        let alternates = format!("{common}/objects\n");
        fs::write(frozen.root.join(".git/objects/info/alternates"), alternates)
            .and_then(|_| {
                fs::write(
                    frozen.root.join(".git/HEAD"),
                    format!("{}\n", request.commit),
                )
            })
            .map_err(|e| KbError::io("provider Git metadata", e))?;
        frozen.git(&frozen.root, &["read-tree", &request.commit])?;
        if skipped > 0 {
            frozen.limitations.push(format!(
                "Skipped {skipped} symlinks, submodules, tool-state paths or files exceeding 2 MiB."
            ));
        }
        Ok(frozen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &std::path::Path, home: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("HOME", home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Synthetic")
            .env("GIT_COMMITTER_NAME", "Synthetic")
            .env("GIT_AUTHOR_EMAIL", "synthetic@example.invalid")
            .env("GIT_COMMITTER_EMAIL", "synthetic@example.invalid")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().into()
    }

    #[test]
    fn historical_blobs_are_materialized_without_touching_dirty_host() {
        let temp = tempfile::tempdir().unwrap();
        let host = temp.path().join("host");
        let home = temp.path().join("home");
        fs::create_dir_all(&host).unwrap();
        fs::create_dir_all(&home).unwrap();
        git(&host, &home, &["init", "-q", "-b", "main"]);
        fs::write(host.join("source.rs"), "pub fn old() {}\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/must/not/be/read", host.join("link.rs")).unwrap();
        git(&host, &home, &["add", "--all"]);
        git(&host, &home, &["commit", "-qm", "synthetic base"]);
        let base = git(&host, &home, &["rev-parse", "HEAD"]);
        fs::write(host.join("source.rs"), "pub fn later() {}\n").unwrap();
        git(&host, &home, &["add", "--all"]);
        git(&host, &home, &["commit", "-qm", "synthetic later"]);
        fs::write(host.join("source.rs"), "dirty user work\n").unwrap();
        let status = git(&host, &home, &["status", "--porcelain"]);
        let mut request: CodeRequest =
            serde_json::from_str(include_str!("../tests/fixtures/request.json")).unwrap();
        request.root = host.to_str().unwrap().into();
        request.commit = base.clone();
        let frozen = Frozen::create(&request).unwrap();
        assert_eq!(frozen.files["source.rs"], b"pub fn old() {}\n");
        assert_eq!(
            fs::read(frozen.root.join("source.rs")).unwrap(),
            b"pub fn old() {}\n"
        );
        assert_eq!(git(&frozen.root, &home, &["rev-parse", "HEAD"]), base);
        assert_eq!(git(&host, &home, &["status", "--porcelain"]), status);
        assert_eq!(
            fs::read(host.join("source.rs")).unwrap(),
            b"dirty user work\n"
        );
        #[cfg(unix)]
        {
            assert!(!frozen.root.join("link.rs").symlink_metadata().is_ok());
            assert!(!frozen.limitations.is_empty());
        }
    }
}
