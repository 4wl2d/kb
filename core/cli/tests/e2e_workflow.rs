//! The end-to-end workflow of the specification (section 13), through the real `kb`
//! executable and the production `kbw` launcher, entirely offline:
//!
//! synthetic upstream -> downstream init -> multi-repository context -> proposal ->
//! update of the approved origin -> host pin refresh -> upstream-format update.
//!
//! Every repository is local: a synthetic upstream (this checkout's engine files) with a
//! bare `upstream.git`, a downstream clone that keeps the upstream history and publishes to
//! a bare `product-origin.git`, and two host repositories (`mobile`, `backend`) that mount
//! the downstream as a submodule at `.kb`. Their `origin` URLs only identify the registry
//! repos (`.invalid` hosts); nothing is ever fetched from them. The only compilation is the
//! explicit source bootstrap of the downstream runtime and of the update worktree, with the
//! pinned toolchain, offline, sharing compiled dependencies through a directory under
//! Cargo's test tmpdir.
#![cfg(unix)]

mod common;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{Sandbox, isolated_env, kb_bin, repo_root, stderr, stdout, write};
use kb::util::sha256_hex;
use kb::versions::ReleaseManifest;
use serde_json::Value;

const MOBILE_TASK: &str = "Retry the token refresh after a network error";
const MOBILE_PATH: &str = "app/auth/TokenRefresher.kt";
const BACKEND_TASK: &str = "Add a retry to the charge call";
const BACKEND_PATH: &str = "src/payments/ChargeService.kt";

/// Mandatory ids of the mobile token-refresh task (scope alone decides them).
const MOBILE_MANDATORY: [&str; 5] = [
    "example.common.code-review",
    "example.contract.error-envelope",
    "example.contract.token-refresh",
    "example.mobile.single-refresh",
    "example.mobile.token-storage",
];
/// Backend-only, UI, checkout, superseded and draft records: never part of the mobile task.
const MOBILE_FORBIDDEN: [&str; 9] = [
    "example.backend.composition-over-inheritance",
    "example.backend.migration-compat",
    "example.backend.payments-review",
    "example.contract.payment-intent",
    "example.decision.token-in-preferences",
    "example.feature.checkout",
    "example.gap.offline-checkout",
    "example.mobile.biometric-unlock",
    "example.mobile.state-hoisting",
];
const BACKEND_MANDATORY: [&str; 4] = [
    "example.backend.payments-review",
    "example.common.code-review",
    "example.contract.payment-intent",
    "example.gap.offline-checkout",
];
/// Reached only through `links.requires` of the payment contract.
const BACKEND_DEPENDENCY: [&str; 1] = ["example.contract.error-envelope"];

const STORAGE_POLICY: &str = "project/knowledge/repos/mobile/token-storage.md";
const ACCEPTED_RULE: &str = "Store refresh tokens only in the platform keystore.";
const PROPOSED_RULE: &str = "Store refresh tokens only in the hardware-backed keystore.";
const PROPOSED_RECORD: &str = "project/knowledge/invariants/refresh-backoff.md";
const PROPOSED_TEXT: &str = r#"+++
schema = 1
id = "example.mobile.refresh-backoff"
kind = "invariant"
title = "Token refresh retries back off exponentially"
status = "accepted"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[[statements]]
id = "exponential-backoff"
level = "must"
text = "Retry a failed token refresh with exponential backoff capped at 60 seconds."
+++
"#;
const APPROVED_RECORD: &str = "project/knowledge/invariants/refresh-timeout.md";
const APPROVED_TEXT: &str = r#"+++
schema = 1
id = "example.mobile.refresh-timeout"
kind = "invariant"
title = "A token refresh gives up after 30 seconds"
status = "accepted"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[selectors]
concepts = ["auth-token"]

[[statements]]
id = "bounded-refresh"
level = "must"
text = "Abort a token refresh that has not completed within 30 seconds and surface a retryable error."
+++
"#;

/// Engine-side change of the synthetic upstream release v0.1.1 (no Rust source changes).
const SKILL_TEMPLATE: &str = "core/skills/kb/references/recovery.md.tmpl";
const GENERATED_SKILL: &str = "project/skill-config/generated/skills/kb/references/recovery.md";
const UPSTREAM_MARKER: &str =
    "Synthetic upstream v0.1.1: re-read this skill after every engine update.";

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn show(o: &Output) -> String {
    format!(
        "exit {:?}\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        stdout(o),
        stderr(o)
    )
}

fn assert_exit(o: &Output, expected: i32) {
    assert_eq!(code(o), expected, "unexpected exit code\n{}", show(o));
}

/// Checks the machine protocol: stdout is exactly one `kb.cli.v1` envelope for `command`
/// (nothing before or after it, so progress went to stderr) whose `ok`, `error` and exit
/// code agree. Returns the envelope.
fn envelope(o: &Output, command: &str) -> Value {
    let v: Value = serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e})\n{}", show(o)));
    let keys: BTreeSet<&str> = v
        .as_object()
        .unwrap_or_else(|| panic!("envelope is not an object\n{}", show(o)))
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["command", "error", "meta", "ok", "protocol", "result"]),
        "{}",
        show(o)
    );
    assert_eq!(v["protocol"], "kb.cli.v1");
    assert_eq!(v["command"], command, "{}", show(o));
    assert!(v["meta"].is_object(), "{}", show(o));
    let ok = v["ok"]
        .as_bool()
        .unwrap_or_else(|| panic!("`ok` is not a boolean\n{}", show(o)));
    if ok {
        assert_eq!(code(o), 0, "{}", show(o));
        assert!(v["error"].is_null(), "{}", show(o));
        assert!(!v["result"].is_null(), "{}", show(o));
    } else {
        let e = &v["error"];
        assert!(
            e["code"].as_str().is_some_and(|c| !c.is_empty()),
            "{}",
            show(o)
        );
        assert!(
            e["message"].as_str().is_some_and(|m| !m.is_empty()),
            "{}",
            show(o)
        );
        assert_eq!(e["exit_code"], code(o), "{}", show(o));
        assert_ne!(code(o), 0, "{}", show(o));
    }
    v
}

/// Record ids of the context units in the given tiers, in output order.
fn unit_ids(result: &Value, tiers: &[&str]) -> Vec<String> {
    result["units"]
        .as_array()
        .unwrap_or_else(|| panic!("no units in {result:#}"))
        .iter()
        .filter(|u| tiers.contains(&u["tier"].as_str().unwrap()))
        .map(|u| u["record"].as_str().unwrap().to_string())
        .collect()
}

/// Record ids of every unit of a context result, whatever its tier.
fn all_ids(result: &Value) -> Vec<String> {
    result["units"]
        .as_array()
        .unwrap_or_else(|| panic!("no units in {result:#}"))
        .iter()
        .map(|u| u["record"].as_str().unwrap().to_string())
        .collect()
}

fn sorted(ids: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

/// The unit of `record` in `tier` (panics when absent).
fn unit<'a>(result: &'a Value, tier: &str, record: &str) -> &'a Value {
    result["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["tier"] == tier && u["record"] == record)
        .unwrap_or_else(|| panic!("no {tier} unit {record} in {:#}", result["units"]))
}

/// The text of rule `id` of a policy unit.
fn rule_text(u: &Value, id: &str) -> String {
    u["content"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .and_then(|r| r["text"].as_str())
        .unwrap_or_else(|| panic!("no rule {id} in {u:#}"))
        .to_string()
}

/// Cargo target directory shared by source builds across test runs (and with the launcher
/// test), so a bootstrap recompiles only the `kb` crate.
fn shared_target_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("kbw-cargo-target")
}

fn real_home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is set"))
}

/// The registry cache of the real user: HOME is isolated but the build must stay offline.
fn cargo_home() -> PathBuf {
    std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| real_home().join(".cargo"))
}

fn rustup_home() -> Option<PathBuf> {
    std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(real_home().join(".rustup")).filter(|p| p.is_dir()))
}

/// Directory holding the cargo (and rustc) that builds these tests.
fn real_toolchain_dir() -> PathBuf {
    Path::new(env!("CARGO")).parent().unwrap().to_path_buf()
}

/// Environment for processes that may run a source bootstrap: the pinned toolchain first on
/// PATH, the real registry cache, no network, and no implicit builds.
fn toolchain_env(c: &mut Command) {
    let mut path = OsString::from(real_toolchain_dir());
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    c.env("PATH", path)
        .env("CARGO_HOME", cargo_home())
        .env("CARGO_NET_OFFLINE", "true")
        .env("KBW_CARGO_TARGET_DIR", shared_target_dir())
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("KBW_AUTO_BOOTSTRAP")
        .env_remove("KBW_FINGERPRINT")
        .env_remove("KBW_ALLOW_TOOLCHAIN_MISMATCH");
    if let Some(r) = rustup_home() {
        c.env("RUSTUP_HOME", r);
    }
}

/// Copy `src` to `dst` (file or directory tree), skipping `target/` and `.cache/`.
fn copy_path(src: &Path, dst: &Path) {
    let meta = fs::symlink_metadata(src).unwrap();
    assert!(
        !meta.file_type().is_symlink(),
        "engine files contain no symlinks: {}",
        src.display()
    );
    if meta.is_dir() {
        fs::create_dir_all(dst).unwrap();
        for e in fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            let name = e.file_name();
            if name == "target" || name == ".cache" {
                continue;
            }
            copy_path(&e.path(), &dst.join(&name));
        }
    } else {
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::copy(src, dst).unwrap();
    }
}

/// Relative path and content digest of every entry under `dir` (empty when it is missing).
fn tree_digest(dir: &Path) -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(root).unwrap().display().to_string();
            let meta = fs::symlink_metadata(&p).unwrap();
            if meta.file_type().is_symlink() {
                out.push((rel, format!("-> {}", fs::read_link(&p).unwrap().display())));
            } else if meta.is_dir() {
                out.push((rel, "dir".into()));
                walk(root, &p, out);
            } else {
                out.push((rel, sha256_hex(&fs::read(&p).unwrap())));
            }
        }
    }
    let mut out = Vec::new();
    if dir.exists() {
        walk(dir, dir, &mut out);
    }
    out.sort();
    out
}

/// Value of `key=value` lines (BUILD-INFO).
fn kv<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
}

/// The scenario's world.
struct World {
    sb: Sandbox,
    /// Synthetic upstream work repository and its bare publication.
    upstream: PathBuf,
    upstream_bare: PathBuf,
    /// The product downstream (maintainer checkout) and its approved origin.
    down: PathBuf,
    origin: PathBuf,
    mobile: PathBuf,
    backend: PathBuf,
}

impl World {
    fn new() -> World {
        let sb = Sandbox::new();
        let p = sb.path();
        World {
            upstream: p.join("upstream"),
            upstream_bare: p.join("upstream.git"),
            down: p.join("downstream-work"),
            origin: p.join("product-origin.git"),
            mobile: p.join("mobile"),
            backend: p.join("backend"),
            sb,
        }
    }

    /// git with the isolated environment and without background maintenance (a detached
    /// auto-maintenance repack can race with local clones of the same repository). No
    /// transport is allowed beyond Git's defaults unless `extra` says so.
    fn git_with(&self, dir: &Path, extra: &[&str], args: &[&str]) -> Output {
        let mut c = Command::new("git");
        c.arg("-C")
            .arg(dir)
            .args([
                "-c",
                "init.defaultBranch=main",
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
            ])
            .args(extra)
            .args(args);
        isolated_env(&mut c, &self.sb.home());
        c.output().unwrap()
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let o = self.git_with(dir, &[], args);
        assert!(
            o.status.success(),
            "git {args:?} in {} failed\n{}",
            dir.display(),
            show(&o)
        );
        stdout(&o).trim_end().to_string()
    }

    fn commit_all(&self, dir: &Path, msg: &str) -> String {
        self.git(dir, &["add", "-A"]);
        self.git(dir, &["commit", "-q", "-m", msg]);
        self.git(dir, &["rev-parse", "HEAD"])
    }

    /// A bare repository whose own config also disables background maintenance: pushes
    /// run receive-pack there with its own configuration.
    fn init_bare(&self, dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        self.git(dir, &["init", "-q", "--bare", "-b", "main"]);
        for (k, v) in [
            ("maintenance.auto", "false"),
            ("gc.auto", "0"),
            ("receive.autogc", "false"),
        ] {
            self.git(dir, &["config", k, v]);
        }
    }

    /// The `kb` executable under test (no toolchain needed).
    fn kb(&self, cwd: &Path, args: &[&str]) -> Output {
        let mut c = Command::new(kb_bin());
        c.current_dir(cwd).args(args);
        isolated_env(&mut c, &self.sb.home());
        c.env_remove("KBW_AUTO_BOOTSTRAP");
        c.output().unwrap()
    }

    /// `kb --root <root> --json <args>` from `cwd`; returns the checked envelope.
    fn kb_json(&self, cwd: &Path, root: &str, command: &str, args: &[&str]) -> (Output, Value) {
        let mut all = vec!["--root", root, "--json", command];
        all.extend_from_slice(args);
        let o = self.kb(cwd, &all);
        let v = envelope(&o, command);
        (o, v)
    }

    /// `kb` with the toolchain environment (commands that bootstrap an engine).
    fn kb_toolchain(&self, cwd: &Path, args: &[&str]) -> Output {
        let mut c = Command::new(kb_bin());
        c.current_dir(cwd).args(args);
        isolated_env(&mut c, &self.sb.home());
        toolchain_env(&mut c);
        c.output().unwrap()
    }

    /// The downstream's own launcher, as its maintainers run it.
    fn kbw(&self, args: &[&str]) -> Output {
        let mut c = Command::new(self.down.join("kbw"));
        c.current_dir(self.sb.path()).args(args);
        isolated_env(&mut c, &self.sb.home());
        toolchain_env(&mut c);
        c.output().unwrap()
    }

    /// Context from a host root with `--root .kb`.
    fn context(&self, host: &Path, args: &[&str]) -> (Output, Value) {
        let mut all = vec!["--intent", "implement"];
        all.extend_from_slice(args);
        self.kb_json(host, ".kb", "context", &all)
    }

    /// Everything a reading command must leave untouched in a checkout: HEAD, branches and
    /// other refs, the index (including gitlinks), status (including submodules), content
    /// changes, stashes, worktrees and untracked files.
    fn checkout_state(&self, dir: &Path) -> String {
        let mut out = String::new();
        let queries: [&[&str]; 8] = [
            &["rev-parse", "HEAD"],
            &["symbolic-ref", "-q", "HEAD"],
            &["for-each-ref", "--format=%(refname) %(objectname)"],
            &["ls-files", "--stage"],
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ],
            &["diff", "HEAD", "--no-ext-diff"],
            &["stash", "list"],
            &["worktree", "list", "--porcelain"],
        ];
        for args in queries {
            out.push_str(&self.query(dir, args));
        }
        out.push_str(&self.untracked(dir));
        out
    }

    fn query(&self, dir: &Path, args: &[&str]) -> String {
        let o = self.git_with(dir, &[], args);
        format!(
            "$ git {args:?} -> {:?}\n{}\n",
            o.status.code(),
            String::from_utf8_lossy(&o.stdout)
        )
    }

    fn untracked(&self, dir: &Path) -> String {
        let mut out = String::new();
        let list = self.git(dir, &["ls-files", "--others", "--exclude-standard"]);
        for f in list.lines().filter(|l| !l.is_empty()) {
            let bytes = fs::read(dir.join(f)).unwrap_or_default();
            out.push_str(&format!(
                "untracked {f}: {}\n",
                String::from_utf8_lossy(&bytes)
            ));
        }
        out
    }

    /// The state of both hosts and their KB checkouts.
    fn hosts_state(&self) -> Vec<String> {
        [&self.mobile, &self.backend]
            .iter()
            .flat_map(|h| [self.checkout_state(h), self.checkout_state(&h.join(".kb"))])
            .collect()
    }

    fn gitlink(&self, host: &Path) -> String {
        let line = self.git(host, &["ls-tree", "HEAD", ".kb"]);
        let mut parts = line.split_whitespace();
        assert_eq!(parts.next(), Some("160000"), "{line}");
        assert_eq!(parts.next(), Some("commit"), "{line}");
        parts.next().unwrap().to_string()
    }
}

// ---------------------------------------------------------------------------------------
// Scenario steps
// ---------------------------------------------------------------------------------------

/// Step 1: the engine files of this checkout (release.toml `engine_paths` plus the
/// uninitialized `project/README.md`) committed as upstream v0.1.0 and published bare.
fn synthetic_upstream(w: &World) -> String {
    let src = repo_root();
    let manifest =
        ReleaseManifest::parse(&fs::read_to_string(src.join("core/release.toml")).unwrap())
            .unwrap();
    let mut copied = Vec::new();
    for p in manifest
        .engine_paths
        .iter()
        .map(String::as_str)
        .chain(["project/README.md"])
    {
        if src.join(p).exists() {
            copy_path(&src.join(p), &w.upstream.join(p));
            copied.push(p);
        }
    }
    for required in [
        "kbw",
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "core",
        ".gitignore",
        "project/README.md",
    ] {
        assert!(copied.contains(&required), "{required} missing: {copied:?}");
    }
    assert!(
        !w.upstream.join("project/project.toml").exists(),
        "upstream holds no project data"
    );
    w.git(&w.upstream, &["init", "-q", "-b", "main"]);
    let v010 = w.commit_all(&w.upstream, "kb engine 0.1.0 (synthetic upstream)");
    w.git(&w.upstream, &["tag", "-a", "v0.1.0", "-m", "kb 0.1.0"]);
    w.init_bare(&w.upstream_bare);
    w.git(
        &w.upstream,
        &[
            "push",
            "-q",
            s(&w.upstream_bare),
            "main",
            "refs/tags/v0.1.0",
        ],
    );
    v010
}

/// Step 2: the product downstream keeps the upstream history, publishes to its own bare
/// origin, bootstraps its runtime explicitly and is initialized from the synthetic
/// multi-repository example through its launcher. Returns the approved commit and the
/// engine fingerprint of the bootstrapped runtime.
fn downstream(w: &World, v010: &str) -> (String, String) {
    let d = &w.down;
    w.git(
        &w.sb.path(),
        &["clone", "-q", "--no-local", s(&w.upstream_bare), s(d)],
    );
    w.git(d, &["remote", "rename", "origin", "upstream"]);
    w.init_bare(&w.origin);
    w.git(d, &["remote", "add", "origin", s(&w.origin)]);
    assert_eq!(w.git(d, &["rev-parse", "HEAD"]), v010);
    // Source builds of this checkout share compiled dependencies with other test runs.
    // `kb update prepare` points the target engine's builds at `<kb root>/.cache/cargo-target`,
    // which lives in the git-ignored cache.
    fs::create_dir_all(d.join(".cache")).unwrap();
    symlink(shared_target_dir(), d.join(".cache/cargo-target")).unwrap();
    fs::create_dir_all(shared_target_dir()).unwrap();

    // Normal commands never build a runtime implicitly; the bootstrap is explicit.
    let o = w.kbw(&["version"]);
    assert_exit(&o, 50);
    assert!(
        stderr(&o).contains("KBW_RUNTIME_NOT_BOOTSTRAPPED"),
        "{}",
        show(&o)
    );
    let o = w.kbw(&["--kbw-bootstrap"]);
    assert_exit(&o, 0);
    let fp = stdout(&w.kbw(&["--kbw-fingerprint"])).trim().to_string();
    let runtime = d.join(".cache/runtime").join(&fp);
    assert_eq!(stdout(&o).trim(), s(&runtime), "{}", show(&o));
    let info = fs::read_to_string(runtime.join("BUILD-INFO")).unwrap();
    assert_eq!(kv(&info, "fingerprint"), Some(fp.as_str()), "{info}");
    assert_eq!(kv(&info, "source"), Some("source-build"), "{info}");

    // Before init the project context does not exist.
    let (o, v) = w.kb_json(&w.sb.path(), s(d), "context", &["--intent", "implement"]);
    assert_exit(&o, 10);
    assert_eq!(v["error"]["code"], "PROJECT_NOT_INITIALIZED");

    // init: dry-run first (nothing written), then --apply.
    let o = w.kbw(&["--json", "init", "--example", "synthetic-multirepo"]);
    let v = envelope(&o, "init");
    assert_eq!(v["result"]["mode"], "dry-run");
    assert_eq!(v["result"]["written"], 0);
    assert_eq!(w.git(d, &["status", "--porcelain"]), "");
    let o = w.kbw(&[
        "--json",
        "init",
        "--example",
        "synthetic-multirepo",
        "--apply",
    ]);
    let v = envelope(&o, "init");
    assert_eq!(v["result"]["mode"], "apply");
    assert!(v["result"]["written"].as_u64().unwrap() > 20, "{v:#}");
    let upstream_toml = fs::read_to_string(d.join("project/upstream.toml")).unwrap();
    assert!(
        upstream_toml.contains(&format!("revision = \"{v010}\"")),
        "{upstream_toml}"
    );

    // validate (records, links, policies, routing fixtures) through the launcher.
    let o = w.kbw(&["--json", "validate"]);
    let v = envelope(&o, "validate");
    assert_exit(&o, 0);
    let r = &v["result"];
    assert_eq!(r["validation"]["errors"], 0, "{r:#}");
    assert_eq!(r["routing"]["failed"], 0, "{r:#}");
    assert_eq!(r["routing"]["passed"], 7, "{r:#}");

    let c1 = w.commit_all(d, "Initialize the synthetic example knowledge base");
    w.git(d, &["push", "-q", "origin", "main"]);
    assert_eq!(
        w.git(&w.origin, &["rev-parse", "refs/heads/main"]),
        c1,
        "the approved ref"
    );
    (c1, fp)
}

/// Step 3: host repositories identified by their remote URLs, mounting the KB at `.kb`.
fn hosts(w: &World, c1: &str) {
    let files: [(&Path, &str, &[&str]); 2] = [
        (
            &w.mobile,
            "mobile",
            &[
                MOBILE_PATH,
                "app/auth/SessionStore.kt",
                "app/ui/ProfileScreen.kt",
            ],
        ),
        (
            &w.backend,
            "backend",
            &[BACKEND_PATH, "src/api/AuthController.kt"],
        ),
    ];
    for (dir, repo, code_files) in files {
        fs::create_dir_all(dir).unwrap();
        w.git(dir, &["init", "-q", "-b", "main"]);
        let url = format!("https://git.example.invalid/example/{repo}.git");
        w.git(dir, &["remote", "add", "origin", &url]);
        write(&dir.join("VERSION"), "1.4.0\n");
        for f in code_files {
            write(&dir.join(f), "// synthetic host code\n");
        }
        w.commit_all(dir, "host code");
        // The file transport is allowed for this one command only.
        let o = w.git_with(
            dir,
            &["-c", "protocol.file.allow=always"],
            &["submodule", "add", "-q", s(&w.origin), ".kb"],
        );
        assert!(o.status.success(), "{}", show(&o));
        w.commit_all(dir, "Pin the knowledge base");
        assert_eq!(w.gitlink(dir), c1, "{repo} pins the approved commit");
        assert_eq!(w.git(&dir.join(".kb"), &["rev-parse", "HEAD"]), c1);
    }
}

/// Step 4: scoped multi-repository context from each host.
fn multi_repo_context(w: &World, c1: &str) {
    let before = w.hosts_state();

    let explained = ["--task", MOBILE_TASK, "--path", MOBILE_PATH, "--explain"];
    let (o, v) = w.context(&w.mobile, &explained);
    assert_exit(&o, 0);
    let r = &v["result"];
    assert_eq!(r["completeness"], "complete", "{r:#}");
    let snap = &r["snapshot"];
    assert_eq!(snap["freshness"], "verified", "{snap:#}");
    assert_eq!(snap["selection"], "latest");
    assert_eq!(snap["approved"], true);
    assert_eq!(snap["revision"], c1);
    assert_eq!(snap["latest_approved"], c1);
    assert_eq!(snap["pin"]["revision"], c1);
    assert_eq!(snap["pin"]["source"], "submodule");
    assert_eq!(r["host"]["repo"], "mobile");
    assert_eq!(r["host"]["repo_source"], "remote");
    assert_eq!(r["host"]["versions"]["mobile"], "1.4.0");
    let mut mandatory = unit_ids(r, &["mandatory", "dependency"]);
    mandatory.sort();
    assert_eq!(mandatory, sorted(&MOBILE_MANDATORY), "{:#}", r["units"]);
    // The refresh contract binds the backend as well: the cross-repository obligation is
    // delivered to the mobile task.
    let contract = unit(r, "mandatory", "example.contract.token-refresh");
    let parties: BTreeSet<&str> = contract["content"]["parties"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["repo"].as_str().unwrap())
        .collect();
    assert_eq!(parties, BTreeSet::from(["backend", "mobile"]));
    assert!(
        unit_ids(r, &["supplementary"]).contains(&"example.decision.secure-token-storage".into()),
        "rationale of the storage policy: {:#}",
        r["units"]
    );
    let all = all_ids(r);
    for id in MOBILE_FORBIDDEN {
        assert!(!all.contains(&id.to_string()), "{id} leaked into {all:?}");
    }
    // The receipt id is the digest of the (explained) deterministic result.
    let receipt = r["receipt"]["id"].as_str().unwrap();
    assert!(receipt.starts_with("sha256:"), "{receipt}");
    assert_eq!(kb::context::receipt_id_of(r), receipt);
    // Same request, same snapshot: identical deterministic result from the warm index.
    let (o2, v2) = w.context(&w.mobile, &explained);
    assert_exit(&o2, 0);
    assert_eq!(v2["result"], v["result"]);
    assert_eq!(v2["meta"]["index"]["reused"], true, "{:#}", v2["meta"]);

    let (o, v) = w.context(
        &w.backend,
        &["--task", BACKEND_TASK, "--path", BACKEND_PATH],
    );
    assert_exit(&o, 0);
    let r = &v["result"];
    assert_eq!(r["completeness"], "complete", "{r:#}");
    assert_eq!(r["snapshot"]["freshness"], "verified");
    assert_eq!(r["snapshot"]["revision"], c1);
    assert_eq!(r["host"]["repo"], "backend");
    let mut mandatory = unit_ids(r, &["mandatory"]);
    mandatory.sort();
    assert_eq!(mandatory, sorted(&BACKEND_MANDATORY), "{:#}", r["units"]);
    // The shared error envelope is outside the payment module's scope but required by
    // the payment contract.
    assert_eq!(unit_ids(r, &["dependency"]), BACKEND_DEPENDENCY);
    let all = all_ids(r);
    for id in &all {
        assert!(
            !id.starts_with("example.mobile."),
            "{id} leaked into {all:?}"
        );
    }
    assert!(!all.contains(&"example.contract.token-refresh".to_string()));
    let min_reviewers = r["effective_settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["setting"] == "min-reviewers")
        .unwrap_or_else(|| panic!("{:#}", r["effective_settings"]));
    assert_eq!(min_reviewers["value"], 2);
    assert_eq!(
        min_reviewers["override_by"],
        "example.backend.payments-review"
    );

    // The authoritative record text is available through show.
    let (o, v) = w.kb_json(
        &w.backend,
        ".kb",
        "show",
        &["example.contract.payment-intent"],
    );
    assert_exit(&o, 0);
    assert_eq!(
        v["result"]["id"], "example.contract.payment-intent",
        "{v:#}"
    );

    assert_eq!(w.hosts_state(), before, "reading changed a checkout");
}

/// Step 5: an uncommitted local edit and a new record in the mobile KB checkout.
fn proposal(w: &World) {
    let kb = w.mobile.join(".kb");
    let storage = kb.join(STORAGE_POLICY);
    let accepted = fs::read_to_string(&storage).unwrap();
    assert!(accepted.contains(ACCEPTED_RULE));
    let edited = accepted.replace(ACCEPTED_RULE, PROPOSED_RULE);
    fs::write(&storage, &edited).unwrap();
    write(&kb.join(PROPOSED_RECORD), PROPOSED_TEXT);
    let before = w.hosts_state();
    let args = ["--task", MOBILE_TASK, "--path", MOBILE_PATH];

    // Without --include-proposals local edits are invisible.
    let (o, v) = w.context(&w.mobile, &args);
    assert_exit(&o, 0);
    let r = &v["result"];
    assert!(r["snapshot"]["overlay"].is_null(), "{:#}", r["snapshot"]);
    assert!(unit_ids(r, &["proposal"]).is_empty());
    assert!(
        !unit_ids(r, &["mandatory", "dependency", "supplementary"])
            .contains(&"example.mobile.refresh-backoff".to_string())
    );
    let policy = unit(r, "mandatory", "example.mobile.token-storage");
    assert_eq!(rule_text(policy, "keystore-only"), ACCEPTED_RULE);

    // With it they are labeled proposals; the accepted text stays authoritative and a new
    // record never becomes mandatory.
    let mut with = vec!["--include-proposals"];
    with.extend_from_slice(&args);
    let (o, v) = w.context(&w.mobile, &with);
    assert!(matches!(code(&o), 0 | 30), "{}", show(&o));
    let r = &v["result"];
    let overlay = &r["snapshot"]["overlay"];
    assert_eq!(overlay["files"], 2, "{overlay:#}");
    assert!(overlay["digest"].is_string(), "{overlay:#}");
    let mut proposals = unit_ids(r, &["proposal"]);
    proposals.sort();
    assert_eq!(
        proposals,
        [
            "example.mobile.refresh-backoff",
            "example.mobile.token-storage"
        ]
    );
    let new = unit(r, "proposal", "example.mobile.refresh-backoff");
    assert_eq!(new["origin"], "proposal");
    assert_eq!(new["labels"], serde_json::json!(["proposal:new"]));
    let modified = unit(r, "proposal", "example.mobile.token-storage");
    assert_eq!(modified["origin"], "proposal");
    assert_eq!(modified["labels"], serde_json::json!(["proposal:modifies"]));
    assert_eq!(rule_text(modified, "keystore-only"), PROPOSED_RULE);
    let policy = unit(r, "mandatory", "example.mobile.token-storage");
    assert_eq!(policy["origin"], "accepted");
    assert_eq!(rule_text(policy, "keystore-only"), ACCEPTED_RULE);
    let mut mandatory = unit_ids(r, &["mandatory", "dependency"]);
    mandatory.sort();
    assert_eq!(mandatory, sorted(&MOBILE_MANDATORY));

    // Reading never resets, stashes or commits the proposal.
    assert_eq!(w.hosts_state(), before, "reading changed a checkout");
    assert_eq!(fs::read_to_string(&storage).unwrap(), edited);
    assert_eq!(
        fs::read_to_string(kb.join(PROPOSED_RECORD)).unwrap(),
        PROPOSED_TEXT
    );
}

/// The proposal of step 5 is still in the mobile KB checkout, byte for byte.
fn assert_proposal_intact(w: &World) {
    let kb = w.mobile.join(".kb");
    let storage = fs::read_to_string(kb.join(STORAGE_POLICY)).unwrap();
    assert!(storage.contains(PROPOSED_RULE), "{storage}");
    assert_eq!(
        fs::read_to_string(kb.join(PROPOSED_RECORD)).unwrap(),
        PROPOSED_TEXT
    );
    let status = w.git(&kb, &["status", "--porcelain", "--untracked-files=all"]);
    assert!(status.contains(&format!(" M {STORAGE_POLICY}")), "{status}");
    assert!(
        status.contains(&format!("?? {PROPOSED_RECORD}")),
        "{status}"
    );
}

/// Step 6: a reviewed change lands on the approved origin; the mobile host pin is behind
/// until it is refreshed through an ordinary host commit.
fn approved_update_and_refresh(w: &World, c1: &str) {
    // A reviewer's separate clone: validate (as the KB's CI would) and publish.
    let reviewer = w.sb.path().join("reviewer");
    w.git(
        &w.sb.path(),
        &["clone", "-q", "--no-local", s(&w.origin), s(&reviewer)],
    );
    write(&reviewer.join(APPROVED_RECORD), APPROVED_TEXT);
    let (o, _) = w.kb_json(&w.sb.path(), s(&reviewer), "validate", &[]);
    assert_exit(&o, 0);
    let c2 = w.commit_all(&reviewer, "Add the refresh timeout invariant");
    w.git(&reviewer, &["push", "-q", "origin", "main"]);
    assert_ne!(c1, c2);

    let before = w.hosts_state();
    let args = ["--task", MOBILE_TASK, "--path", MOBILE_PATH];
    let new_id = "example.mobile.refresh-timeout".to_string();

    // Default selection: the host pin differs from the approved tip.
    let (o, v) = w.context(&w.mobile, &args);
    assert_exit(&o, 21);
    let e = &v["error"];
    assert_eq!(e["code"], "UPDATE_REQUIRED", "{e:#}");
    assert_eq!(e["details"]["pin"]["revision"], c1, "{e:#}");
    assert_eq!(e["details"]["latest_approved"], c2.as_str(), "{e:#}");

    // Explicit latest: the new approved tip, freshness verified, both revisions reported.
    let mut latest = vec!["--snapshot", "latest"];
    latest.extend_from_slice(&args);
    let (o, v) = w.context(&w.mobile, &latest);
    assert_exit(&o, 0);
    let r = &v["result"];
    let snap = &r["snapshot"];
    assert_eq!(snap["selection"], "latest", "{snap:#}");
    assert_eq!(snap["revision"], c2.as_str());
    assert_eq!(snap["latest_approved"], c2.as_str());
    assert_eq!(snap["pin"]["revision"], c1);
    assert_eq!(snap["freshness"], "verified");
    assert_eq!(snap["approved"], true);
    assert_eq!(r["completeness"], "complete");
    assert!(
        unit_ids(r, &["mandatory"]).contains(&new_id),
        "{:#}",
        r["units"]
    );

    // Explicit pinned: the old content, never called latest, the approved tip reported.
    let mut pinned = vec!["--snapshot", "pinned"];
    pinned.extend_from_slice(&args);
    let (o, v) = w.context(&w.mobile, &pinned);
    assert_exit(&o, 0);
    let r = &v["result"];
    let snap = &r["snapshot"];
    assert_eq!(snap["selection"], "pinned", "{snap:#}");
    assert_eq!(snap["revision"], c1);
    assert_eq!(snap["latest_approved"], c2.as_str());
    assert_eq!(snap["freshness"], "verified");
    let all = all_ids(r);
    assert!(!all.contains(&new_id), "{all:?}");
    let mut mandatory = unit_ids(r, &["mandatory", "dependency"]);
    mandatory.sort();
    assert_eq!(mandatory, sorted(&MOBILE_MANDATORY));

    // sync reports the stale pin and changes nothing.
    let (o, v) = w.kb_json(&w.mobile, ".kb", "sync", &[]);
    assert_exit(&o, 0);
    let r = &v["result"];
    assert_eq!(r["freshness"], "verified", "{r:#}");
    assert_eq!(r["latest_approved"], c2.as_str());
    assert_eq!(r["host"]["pin"]["revision"], c1);
    assert_eq!(r["host"]["status"], "behind", "{r:#}");
    assert_eq!(r["host"]["behind"], 1);
    assert_eq!(r["local"]["head"], c1);
    assert_eq!(r["local"]["behind"], 1);
    assert_eq!(r["local"]["dirty"], true, "the proposal is local work");
    assert_eq!(r["checkout_unchanged"], true);

    assert_eq!(w.hosts_state(), before, "reading changed a checkout");
    assert_proposal_intact(w);

    // Refresh the pin as a normal, reviewable host change: move the KB checkout to the
    // approved commit (the local proposal is carried along) and commit the gitlink.
    let kb = w.mobile.join(".kb");
    w.git(&kb, &["fetch", "-q", "origin"]);
    w.git(&kb, &["checkout", "-q", "--detach", &c2]);
    w.git(&w.mobile, &["add", ".kb"]);
    w.git(
        &w.mobile,
        &["commit", "-q", "-m", "Update the knowledge base pin"],
    );
    assert_eq!(w.gitlink(&w.mobile), c2);
    assert_eq!(w.gitlink(&w.backend), c1, "the other host is untouched");
    assert_proposal_intact(w);

    let before = w.hosts_state();
    let (o, v) = w.context(&w.mobile, &args);
    assert_exit(&o, 0);
    let r = &v["result"];
    let snap = &r["snapshot"];
    assert_eq!(snap["selection"], "latest", "{snap:#}");
    assert_eq!(snap["revision"], c2.as_str());
    assert_eq!(snap["pin"]["revision"], c2.as_str());
    assert_eq!(snap["freshness"], "verified");
    assert_eq!(r["completeness"], "complete");
    let unit_new = unit(r, "mandatory", &new_id);
    assert_eq!(unit_new["origin"], "accepted");
    // Pins are per host: the backend still pins the old commit and must choose explicitly.
    let (o, v) = w.context(
        &w.backend,
        &["--task", BACKEND_TASK, "--path", BACKEND_PATH],
    );
    assert_exit(&o, 21);
    assert_eq!(v["error"]["code"], "UPDATE_REQUIRED");
    assert_eq!(v["error"]["details"]["pin"]["revision"], c1);
    let (o, v) = w.kb_json(&w.mobile, ".kb", "sync", &[]);
    assert_exit(&o, 0);
    assert_eq!(
        v["result"]["host"]["status"], "current",
        "{:#}",
        v["result"]
    );
    // The proposal still overlays the new snapshot.
    let mut with = vec!["--include-proposals"];
    with.extend_from_slice(&args);
    let (o, v) = w.context(&w.mobile, &with);
    assert!(matches!(code(&o), 0 | 30), "{}", show(&o));
    assert_eq!(v["result"]["snapshot"]["overlay"]["base"], c2.as_str());
    assert_eq!(unit_ids(&v["result"], &["proposal"]).len(), 2);
    assert_eq!(w.hosts_state(), before, "reading changed a checkout");
    assert_proposal_intact(w);
}

/// State of the downstream maintainer checkout that `update prepare` must not touch.
fn main_checkout_state(w: &World) -> String {
    let d = &w.down;
    let mut out = String::new();
    for args in [
        &["rev-parse", "HEAD"][..],
        &["symbolic-ref", "-q", "HEAD"],
        &["ls-files", "--stage"],
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
        &["diff", "HEAD", "--no-ext-diff"],
        &["stash", "list"],
    ] {
        out.push_str(&w.query(d, args));
    }
    out.push_str(&w.untracked(d));
    out
}

fn refs(w: &World, dir: &Path) -> BTreeSet<String> {
    w.git(dir, &["for-each-ref", "--format=%(refname) %(objectname)"])
        .lines()
        .map(str::to_string)
        .collect()
}

/// Step 7: upstream publishes an engine-side change (a skill template; no Rust sources)
/// as v0.1.1; the downstream checks it and prepares a reviewable update branch with the
/// production runner (`<worktree>/kbw`).
fn upstream_format_update(w: &World, fp: &str) {
    let tmpl = w.upstream.join(SKILL_TEMPLATE);
    let mut text = fs::read_to_string(&tmpl).unwrap();
    text.push_str(&format!("\n## Engine updates\n\n{UPSTREAM_MARKER}\n"));
    fs::write(&tmpl, text).unwrap();
    let v011 = w.commit_all(&w.upstream, "kb engine 0.1.1: skill recovery note");
    w.git(&w.upstream, &["tag", "-a", "v0.1.1", "-m", "kb 0.1.1"]);
    w.git(
        &w.upstream,
        &[
            "push",
            "-q",
            s(&w.upstream_bare),
            "main",
            "refs/tags/v0.1.1",
        ],
    );

    let d = &w.down;
    let head = w.git(d, &["rev-parse", "HEAD"]);
    let state = main_checkout_state(w);
    let refs_before = refs(w, d);
    let runtime_before = tree_digest(&d.join(".cache/runtime"));
    assert!(!runtime_before.is_empty(), "bootstrapped in step 2");
    let upstream = s(&w.upstream_bare);

    let (o, v) = w.kb_json(
        &w.sb.path(),
        s(d),
        "update",
        &["check", "--upstream", upstream, "--ref", "v0.1.1"],
    );
    assert_exit(&o, 0);
    let r = &v["result"];
    assert_eq!(r["upstream"]["commit"], v011.as_str(), "{r:#}");
    assert_eq!(r["head"], head.as_str());
    assert_eq!(r["merge"]["clean"], true, "{r:#}");
    assert_eq!(r["merge"]["up_to_date"], false);
    assert_eq!(r["merge"]["conflicts"], serde_json::json!([]));
    assert_eq!(r["schema"]["migration_required"], false);
    assert_eq!(r["schema"]["unsupported"], serde_json::json!([]));
    assert_eq!(r["divergence"]["diverged"], serde_json::json!([]), "{r:#}");
    assert_eq!(
        refs(w, d),
        refs_before,
        "check never writes to the KB repository"
    );

    let o = w.kb_toolchain(
        &w.sb.path(),
        &[
            "--root",
            s(d),
            "--json",
            "update",
            "prepare",
            "--upstream",
            upstream,
            "--ref",
            "v0.1.1",
        ],
    );
    let v = envelope(&o, "update");
    assert_exit(&o, 0);
    let r = &v["result"];
    let branch = "kb-update/v0.1.1";
    assert_eq!(r["branch"], branch, "{r:#}");
    assert_eq!(r["base"], head.as_str());
    assert_eq!(r["upstream"]["commit"], v011.as_str());
    let steps: Vec<(&str, &str)> = r["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s["status"].as_str().unwrap()))
        .collect();
    assert_eq!(
        steps,
        [
            ("bootstrap", "ok"),
            ("migrate", "ok"),
            ("integrate", "ok"),
            ("validate", "ok")
        ],
        "{r:#}"
    );
    let changed: BTreeSet<&str> = r["changes"]["project"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    assert!(changed.contains("project/upstream.toml"), "{changed:?}");
    assert!(changed.contains(GENERATED_SKILL), "{changed:?}");

    // The reviewable branch: upstream merged, project/upstream.toml recorded, the skill
    // bundle regenerated, knowledge untouched. Nothing was pushed.
    let tip = w.git(d, &["rev-parse", &format!("refs/heads/{branch}")]);
    w.git(d, &["merge-base", "--is-ancestor", &v011, &tip]);
    w.git(d, &["merge-base", "--is-ancestor", &head, &tip]);
    let at = |path: &str| w.git(d, &["show", &format!("{tip}:{path}")]);
    assert!(at(SKILL_TEMPLATE).contains(UPSTREAM_MARKER));
    assert!(at(GENERATED_SKILL).contains(UPSTREAM_MARKER));
    assert!(at("project/upstream.toml").contains(&format!("revision = \"{v011}\"")));
    assert!(
        !fs::read_to_string(d.join(GENERATED_SKILL))
            .unwrap()
            .contains(UPSTREAM_MARKER)
    );
    let knowledge = w.git_with(
        d,
        &[],
        &[
            "diff",
            "--quiet",
            &head,
            &tip,
            "--",
            "project/knowledge",
            "project/registry",
            "project/routing-tests",
        ],
    );
    assert!(knowledge.status.success(), "{}", show(&knowledge));
    assert_eq!(
        w.git(&w.origin, &["for-each-ref", "--format=%(refname)"]),
        "refs/heads/main",
        "kb never pushes"
    );

    // The update worktree is clean, has its own bootstrapped runtime and validates.
    let wt = PathBuf::from(r["worktree"].as_str().unwrap());
    assert!(wt.starts_with(d.join(".cache/update")), "{}", wt.display());
    assert_eq!(w.git(&wt, &["rev-parse", "HEAD"]), tip);
    assert_eq!(
        w.git(&wt, &["status", "--porcelain", "--untracked-files=all"]),
        ""
    );
    // Unchanged engine build inputs give the same fingerprint, but the target engine was
    // bootstrapped inside the worktree's own cache.
    let info = fs::read_to_string(wt.join(".cache/runtime").join(fp).join("BUILD-INFO"))
        .unwrap_or_else(|e| panic!("worktree runtime {fp}: {e}"));
    assert_eq!(kv(&info, "fingerprint"), Some(fp), "{info}");
    assert_eq!(kv(&info, "source"), Some("source-build"), "{info}");
    let (o, v) = w.kb_json(&w.sb.path(), s(&wt), "validate", &[]);
    assert_exit(&o, 0);
    assert_eq!(v["result"]["validation"]["errors"], 0);
    assert_eq!(v["result"]["routing"]["failed"], 0);

    // The maintainer checkout: HEAD, branch, index, work tree and runtime cache unchanged;
    // the only new refs are the update branch and its namespaced upstream ref.
    assert_eq!(main_checkout_state(w), state);
    assert_eq!(tree_digest(&d.join(".cache/runtime")), runtime_before);
    let added: BTreeSet<String> = refs(w, d)
        .difference(&refs_before)
        .map(|l| l.split(' ').next().unwrap().to_string())
        .collect();
    assert_eq!(
        added,
        BTreeSet::from([
            format!("refs/heads/{branch}"),
            "refs/kb/upstream/v0.1.1".to_string()
        ])
    );
    assert!(
        refs_before.is_subset(&refs(w, d)),
        "existing refs never move"
    );
    // The engine in the maintainer checkout still reports no divergence from its base.
    let (o, v) = w.kb_json(&w.sb.path(), s(d), "update", &["divergence"]);
    assert_exit(&o, 0);
    assert_eq!(v["result"]["diverged"], serde_json::json!([]));
}

#[test]
fn synthetic_upstream_to_downstream_update_end_to_end() {
    let w = World::new();
    let v010 = synthetic_upstream(&w);
    let (c1, fp) = downstream(&w, &v010);
    hosts(&w, &c1);
    multi_repo_context(&w, &c1);
    proposal(&w);
    approved_update_and_refresh(&w, &c1);
    upstream_format_update(&w, &fp);
}
