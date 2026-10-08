//! Controlled Git command runner.
//!
//! All invocations use argument arrays (never a shell), a fixed environment
//! (`GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, `LC_ALL=C`), disable fsmonitor, never
//! recurse into submodules, and redact credentials from any error text. Network operations
//! pass an explicit transport policy (`protocol.allow=never` + allowed protocols).
//! Inherited repository variables (`GIT_DIR`, `GIT_INDEX_FILE`, ...) are removed; only an
//! explicit [`Git::with_index_file`] names another index.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{ErrorCode, KbError, Result};

/// Protocols accepted in transport policies.
pub const KNOWN_PROTOCOLS: [&str; 5] = ["https", "ssh", "git", "file", "http"];

/// Deadline of [`Git::run_bytes_limited`].
pub const LIMITED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Diagnostic (stderr) bytes accepted by [`Git::run_bytes_limited`].
pub const LIMITED_STDERR_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Git {
    /// Directory passed with `-C`.
    pub dir: PathBuf,
    /// Explicit `--git-dir` (for bare mirrors).
    pub git_dir: Option<PathBuf>,
    /// Explicit `GIT_INDEX_FILE` (the pending index a commit hook checks).
    pub index_file: Option<PathBuf>,
}

/// Output of a Git command that may legitimately fail.
#[derive(Debug, Clone)]
pub struct GitOutput {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl GitOutput {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

impl Git {
    pub fn new(dir: impl Into<PathBuf>) -> Git {
        Git {
            dir: dir.into(),
            git_dir: None,
            index_file: None,
        }
    }

    pub fn bare(git_dir: impl Into<PathBuf>) -> Git {
        let g = git_dir.into();
        Git {
            dir: g.clone(),
            git_dir: Some(g),
            index_file: None,
        }
    }

    /// Read the index at `index_file` instead of the repository's default index. Used only
    /// for host staged reads; KB and mirror commands never inherit a caller's index.
    pub fn with_index_file(mut self, index_file: Option<&Path>) -> Git {
        self.index_file = index_file.map(Path::to_path_buf);
        self
    }

    fn command(&self, extra_config: &[String]) -> Command {
        let mut c = Command::new("git");
        c.arg("-C").arg(&self.dir);
        if let Some(gd) = &self.git_dir {
            c.arg("--git-dir").arg(gd);
        }
        for kv in [
            "core.fsmonitor=false",
            "core.quotepath=false",
            "submodule.recurse=false",
            "color.ui=false",
            "advice.detachedHead=false",
        ] {
            c.arg("-c").arg(kv);
        }
        for kv in extra_config {
            c.arg("-c").arg(kv);
        }
        c.env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
            .env_remove("GIT_NAMESPACE")
            .env_remove("GIT_CEILING_DIRECTORIES")
            .stdin(Stdio::null());
        if let Some(index) = &self.index_file {
            c.env("GIT_INDEX_FILE", index);
        }
        c
    }

    /// Run and capture; never fails on non-zero exit (only on spawn failure).
    pub fn output(&self, args: &[&str]) -> Result<GitOutput> {
        self.output_with(args, &[])
    }

    /// Like [`Git::output`] with extra `-c key=value` settings.
    pub fn output_with(&self, args: &[&str], config: &[String]) -> Result<GitOutput> {
        let out = self
            .command(config)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| KbError::new(ErrorCode::GitError, format!("cannot run git: {e}")))?;
        Ok(GitOutput {
            code: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: redact(&String::from_utf8_lossy(&out.stderr)),
        })
    }

    /// Run and return stdout (trailing newline trimmed); non-zero exit → `GIT_ERROR`.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let o = self.output(args)?;
        if !o.ok() {
            return Err(git_failure(args, &o));
        }
        Ok(o.stdout_str().trim_end_matches('\n').to_string())
    }

    /// Run and return raw stdout bytes.
    pub fn run_bytes(&self, args: &[&str]) -> Result<Vec<u8>> {
        let o = self.output(args)?;
        if !o.ok() {
            return Err(git_failure(args, &o));
        }
        Ok(o.stdout)
    }

    /// Read bounded command output for potentially large patches and history exports, within
    /// [`LIMITED_TIMEOUT`] and [`LIMITED_STDERR_BYTES`].
    pub fn run_bytes_limited(&self, args: &[&str], max_bytes: usize) -> Result<Vec<u8>> {
        let mut command = self.command(&[]);
        command.args(args);
        let output = crate::process::capture(
            &mut command,
            &[],
            crate::process::Limits {
                timeout: LIMITED_TIMEOUT,
                stdout: max_bytes,
                stderr: LIMITED_STDERR_BYTES,
            },
        )?;
        if output.exit_code != 0 {
            return Err(git_failure(
                args,
                &GitOutput {
                    code: output.exit_code,
                    stdout: output.stdout,
                    stderr: redact(&String::from_utf8_lossy(&output.stderr)),
                },
            ));
        }
        Ok(output.stdout)
    }

    /// Run a network operation (fetch/ls-remote) under an explicit transport policy.
    pub fn run_network(&self, args: &[&str], allowed_protocols: &[String]) -> Result<GitOutput> {
        let cfg = transport_policy(allowed_protocols)?;
        self.output_with(args, &cfg)
    }

    /// Resolve a revision to a full commit id (`None` if it does not resolve).
    pub fn resolve_commit(&self, rev: &str) -> Result<Option<String>> {
        check_revision_arg(rev)?;
        let spec = format!("{rev}^{{commit}}");
        let o = self.output(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ])?;
        if !o.ok() {
            return Ok(None);
        }
        let s = o.stdout_str().trim().to_string();
        Ok(if s.is_empty() { None } else { Some(s) })
    }

    /// Is `ancestor` an ancestor of (or equal to) `descendant`?
    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        let o = self.output(&["merge-base", "--is-ancestor", ancestor, descendant])?;
        match o.code {
            0 => Ok(true),
            1 => Ok(false),
            _ => Err(git_failure(&["merge-base", "--is-ancestor"], &o)),
        }
    }

    /// Read many blobs with one `git cat-file --batch` process.
    pub fn cat_file_batch(&self, oids: &[String]) -> Result<Vec<Vec<u8>>> {
        self.cat_file_objects(oids)?
            .into_iter()
            .zip(oids)
            .map(|(obj, oid)| match obj {
                Some((kind, bytes)) if kind == "blob" => Ok(bytes),
                _ => Err(KbError::new(
                    ErrorCode::GitError,
                    format!("cat-file: object {oid} is missing or not a blob"),
                )),
            })
            .collect()
    }

    /// Read many objects by name (object ids or `<commit>:<path>`) with one
    /// `git cat-file --batch` process. Missing or ambiguous names yield `None`; found objects
    /// yield (type, content). Names must not contain line breaks.
    pub fn cat_file_objects(&self, names: &[String]) -> Result<Vec<NamedObject>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        if names.iter().any(|n| n.contains(['\n', '\r'])) {
            return Err(KbError::invalid_input("object names must be single lines"));
        }
        let mut child = self
            .command(&[])
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| KbError::new(ErrorCode::GitError, format!("cannot run git: {e}")))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let input: String = names.iter().map(|o| format!("{o}\n")).collect();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
        let stdout = child.stdout.take().expect("piped stdout");
        let mut reader = BufReader::new(stdout);
        let io = |e: std::io::Error| KbError::new(ErrorCode::GitError, format!("cat-file: {e}"));
        let mut out = Vec::with_capacity(names.len());
        for _ in names {
            let mut header = String::new();
            reader.read_line(&mut header).map_err(io)?;
            let parts: Vec<&str> = header.split_whitespace().collect();
            // `<name> missing` / `<name> ambiguous` carry no content.
            if parts.len() != 3 {
                if parts.len() == 2 && matches!(parts[1], "missing" | "ambiguous") {
                    out.push(None);
                    continue;
                }
                let _ = child.kill();
                return Err(KbError::new(
                    ErrorCode::GitError,
                    format!("cat-file: unexpected header `{}`", header.trim()),
                ));
            }
            let size: usize = parts[2]
                .parse()
                .map_err(|_| KbError::new(ErrorCode::GitError, "cat-file: bad size"))?;
            let mut buf = vec![0u8; size];
            reader.read_exact(&mut buf).map_err(io)?;
            let mut nl = [0u8; 1];
            reader.read_exact(&mut nl).map_err(io)?;
            out.push(Some((parts[1].to_string(), buf)));
        }
        let _ = writer.join();
        let _ = child.wait();
        Ok(out)
    }
}

/// A named object read by [`Git::cat_file_objects`]: (type, content), or `None` when missing.
pub type NamedObject = Option<(String, Vec<u8>)>;

/// Build `-c` settings for an explicit transport policy.
pub fn transport_policy(allowed: &[String]) -> Result<Vec<String>> {
    let mut cfg = vec!["protocol.allow=never".to_string()];
    for p in allowed {
        if !KNOWN_PROTOCOLS.contains(&p.as_str()) {
            return Err(KbError::new(
                ErrorCode::ConfigInvalid,
                format!("unknown protocol `{p}` in allowed_protocols"),
            ));
        }
        cfg.push(format!("protocol.{p}.allow=always"));
    }
    Ok(cfg)
}

/// Reject revision arguments that could be parsed as options or contain odd characters.
pub fn check_revision_arg(rev: &str) -> Result<()> {
    if rev.is_empty()
        || rev.len() > 256
        || rev.starts_with('-')
        || rev.chars().any(|c| c.is_control() || c.is_whitespace())
        || rev.contains("..")
    {
        return Err(KbError::invalid_input(format!(
            "invalid revision `{}`",
            redact(rev)
        )));
    }
    Ok(())
}

pub fn git_failure(args: &[&str], o: &GitOutput) -> KbError {
    let what = args.first().copied().unwrap_or("git");
    KbError::new(
        ErrorCode::GitError,
        format!("git {what} failed (exit {}): {}", o.code, o.stderr.trim()),
    )
}

/// Remove credentials from URLs and common token shapes.
pub fn redact(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    // userinfo in scheme://user:pass@host
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| c == '/' || c.is_whitespace() || c == '\'' || c == '"')
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        if let Some(at) = authority.rfind('@') {
            out.push_str("***@");
            out.push_str(&authority[at + 1..]);
        } else {
            out.push_str(authority);
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    // token-looking words
    let prefixes = [
        "ghp_",
        "gho_",
        "ghs_",
        "ghu_",
        "github_pat_",
        "glpat-",
        "xoxb-",
        "xoxp-",
    ];
    let mut result = String::with_capacity(out.len());
    for (i, word) in out.split(' ').enumerate() {
        if i > 0 {
            result.push(' ');
        }
        if prefixes.iter().any(|p| word.contains(p)) {
            result.push_str("***");
        } else {
            result.push_str(word);
        }
    }
    result
}

/// Is `path` inside a Git work tree? Returns the top-level directory. Results are memoized
/// per process (a work tree root does not move while a command runs), which saves repeated
/// `rev-parse` processes on the warm query path.
pub fn toplevel(path: &Path) -> Option<PathBuf> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Mutex::default);
    if let Some(hit) = memo.lock().unwrap_or_else(|p| p.into_inner()).get(path) {
        return hit.clone();
    }
    let top = toplevel_uncached(path);
    memo.lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(path.to_path_buf(), top.clone());
    top
}

fn toplevel_uncached(path: &Path) -> Option<PathBuf> {
    let g = Git::new(path);
    let o = g.output(&["rev-parse", "--show-toplevel"]).ok()?;
    if !o.ok() {
        return None;
    }
    let s = o.stdout_str().trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_credentials() {
        assert_eq!(
            redact("fatal: https://user:s3cret@example.com/a.git not found"),
            "fatal: https://***@example.com/a.git not found"
        );
        assert_eq!(redact("token ghp_abcdef here"), "token *** here");
        assert_eq!(redact("ssh://git@host/x"), "ssh://***@host/x");
        assert_eq!(redact("plain /tmp/x"), "plain /tmp/x");
    }

    #[test]
    fn revision_args() {
        assert!(check_revision_arg("--upload-pack=evil").is_err());
        assert!(check_revision_arg("a b").is_err());
        assert!(check_revision_arg("refs/heads/main").is_ok());
        assert!(check_revision_arg("HEAD~1").is_ok());
    }

    #[test]
    fn transport_policy_rejects_unknown() {
        assert!(transport_policy(&["ext".into()]).is_err());
        let p = transport_policy(&["file".into()]).unwrap();
        assert_eq!(
            p,
            vec!["protocol.allow=never", "protocol.file.allow=always"]
        );
    }
}
