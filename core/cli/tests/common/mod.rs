//! Shared helpers for integration tests. Every Git invocation is isolated from the user's
//! global/system configuration and credentials; no test touches the network.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Absolute path of the repository root (the upstream checkout under test).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The `kb` executable built by Cargo for this test run.
pub fn kb_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kb"))
}

/// Environment applied to every child process in tests.
pub fn isolated_env(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "KB Test")
        .env("GIT_AUTHOR_EMAIL", "kb-test@example.invalid")
        .env("GIT_COMMITTER_NAME", "KB Test")
        .env("GIT_COMMITTER_EMAIL", "kb-test@example.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .env_remove("KB_ROOT")
        .env_remove("KB_CACHE_DIR")
        // Launcher settings of the developer's shell must not leak into tests; tests that
        // need them set them explicitly after this call.
        .env_remove("KBW_CARGO_TARGET_DIR")
        .env_remove("KBW_AUTO_BOOTSTRAP")
        .env_remove("KBW_FINGERPRINT")
        .env_remove("KBW_ALLOW_TOOLCHAIN_MISMATCH")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
}

/// A scratch area with an isolated HOME.
pub struct Sandbox {
    pub dir: tempfile::TempDir,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("home")).unwrap();
        Sandbox { dir }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    pub fn home(&self) -> PathBuf {
        self.path().join("home")
    }

    /// Run git in `dir`; panics with stderr on failure.
    pub fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.git_output(dir, args);
        assert!(
            out.status.success(),
            "git {:?} in {} failed: {}",
            args,
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    pub fn git_output(&self, dir: &Path, args: &[&str]) -> Output {
        let mut c = Command::new("git");
        c.arg("-C")
            .arg(dir)
            .args([
                "-c",
                "protocol.file.allow=always",
                "-c",
                "init.defaultBranch=main",
                // Detached auto-maintenance (repack) can race with local clones that
                // hardlink objects of the same repository; tests never need it.
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
            ])
            .args(args);
        isolated_env(&mut c, &self.home());
        c.output().unwrap()
    }

    /// Run the kb binary with `cwd`, isolated env, extra env vars.
    pub fn kb(&self, cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut c = Command::new(kb_bin());
        c.current_dir(cwd).args(args);
        isolated_env(&mut c, &self.home());
        for (k, v) in envs {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    /// `git init` a work tree at `dir` (created) with branch `main`.
    pub fn init_repo(&self, dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        self.git(dir, &["init", "-q", "-b", "main"]);
    }

    /// `git init --bare` at `dir`.
    pub fn init_bare(&self, dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        self.git(dir, &["init", "-q", "--bare", "-b", "main"]);
        // `-c` settings do not reach receive-pack on local pushes: configure the repository.
        for (k, v) in [
            ("maintenance.auto", "false"),
            ("gc.auto", "0"),
            ("receive.autogc", "false"),
        ] {
            self.git(dir, &["config", k, v]);
        }
    }

    /// Stage everything and commit; returns the new HEAD.
    pub fn commit_all(&self, dir: &Path, msg: &str) -> String {
        self.git(dir, &["add", "-A"]);
        self.git(dir, &["commit", "-q", "--allow-empty", "-m", msg]);
        self.git(dir, &["rev-parse", "HEAD"])
    }
}

pub fn write(path: &Path, text: &str) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, text).unwrap();
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

pub fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}):\n{}\nstderr:\n{}",
            stdout(o),
            stderr(o)
        )
    })
}

/// Copy the engine manifest so a temp KB passes runtime/snapshot compatibility checks.
pub fn copy_manifest(kb_root: &Path) {
    let src = repo_root().join("core/release.toml");
    let dst = kb_root.join("core/release.toml");
    fs::create_dir_all(dst.parent().unwrap()).unwrap();
    fs::copy(src, dst).unwrap();
}

/// Minimal valid project (namespace `acme`, repos mobile + backend) with a few records.
/// Uses `allowed_protocols = ["file"]` so tests can use local bare remotes.
pub fn write_min_project(kb_root: &Path) {
    copy_manifest(kb_root);
    let p = kb_root.join("project");
    write(
        &p.join("project.toml"),
        r#"schema = 1
[project]
name = "Acme (synthetic test)"
namespace = "acme"
[source]
remote = "origin"
approved_ref = "refs/heads/main"
allowed_protocols = ["file"]
"#,
    );
    write(
        &p.join("registry/owners.toml"),
        r#"schema = 1
[[owner]]
id = "arch"
title = "Architecture"
product = true
[[owner]]
id = "team-mobile"
title = "Mobile team"
repos = ["mobile"]
[[owner]]
id = "team-backend"
title = "Backend team"
repos = ["backend"]
"#,
    );
    write(
        &p.join("registry/repos.toml"),
        r#"schema = 1
[[repo]]
id = "mobile"
title = "Mobile app"
remotes = ["example.invalid/acme/mobile"]
[[repo]]
id = "backend"
title = "Backend"
remotes = ["example.invalid/acme/backend"]
"#,
    );
    write(
        &p.join("registry/modules.toml"),
        r#"schema = 1
[[module]]
id = "mobile.auth"
repo = "mobile"
title = "Mobile authentication"
paths = ["app/auth/**"]
features = ["login"]
[[module]]
id = "backend.api"
repo = "backend"
title = "Backend API"
paths = ["src/api/**"]
features = ["login"]
"#,
    );
    write(
        &p.join("registry/features.toml"),
        r#"schema = 1
[[feature]]
id = "login"
title = "Login"
repos = ["mobile", "backend"]
"#,
    );
    write(
        &p.join("registry/concepts.toml"),
        r#"schema = 1
[[concept]]
id = "auth-token"
title = "Authentication token"
aliases = ["token", "токен*"]
"#,
    );
    write(
        &p.join("knowledge/policies/token-storage.md"),
        r#"+++
schema = 1
id = "acme.mobile.token-storage"
kind = "policy"
title = "Token storage"
status = "accepted"
owner = "team-mobile"

[scope]
repos = ["mobile"]

[selectors]
concepts = ["auth-token"]

[links]
requires = ["acme.contract.token-api"]

[[rules]]
id = "no-plaintext"
level = "must-not"
text = "Store refresh tokens in plaintext storage."
+++
"#,
    );
    write(
        &p.join("knowledge/contracts/token-api.md"),
        r#"+++
schema = 1
id = "acme.contract.token-api"
kind = "contract"
title = "Token refresh API"
status = "accepted"
owner = "arch"

[scope]
repos = ["mobile", "backend"]

[[parties]]
id = "provider"
repo = "backend"
role = "Issues tokens"

[[parties]]
id = "consumer"
repo = "mobile"
role = "Refreshes tokens"

[[obligations]]
id = "rotate"
party = "provider"
level = "must"
text = "Rotate the refresh token on every refresh call."
+++
"#,
    );
}
