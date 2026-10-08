//! A coordinated format update through the production path (specification sections 7 and
//! 13): `kb update prepare` with the production runner, which runs `<worktree>/kbw`, the
//! real launcher of the target engine. It bootstraps that engine explicitly inside the
//! update worktree and lets it migrate a legacy (schema 0) project and validate the result.
//!
//! The synthetic upstream holds this checkout's real engine files (`engine_paths` of
//! core/release.toml); the downstream project is the synthetic legacy fixture
//! `core/migrations/fixtures/v0`. Everything is local and offline. The only compilation is
//! the source bootstrap with the pinned toolchain, which shares compiled dependencies with
//! the launcher and end-to-end tests through a directory under Cargo's test tmpdir.
#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{Sandbox, isolated_env, kb_bin, repo_root, stderr, stdout};
use kb::versions::ReleaseManifest;
use serde_json::{Value, json};

const TAG: &str = "v0.1.1";
const BRANCH: &str = "kb-update/v0.1.1";
/// Engine-side change of the synthetic upstream release (documentation only).
const UPSTREAM_DOC: &str = "docs/architecture.md";
const UPSTREAM_MARKER: &str = "<!-- synthetic upstream v0.1.1 -->";

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn show(o: &Output) -> String {
    format!(
        "exit {:?}\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        stdout(o),
        stderr(o)
    )
}

/// Cargo target directory shared by source builds across test runs (the same directory as
/// in the launcher and end-to-end tests), so a bootstrap recompiles only the `kb` crate.
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

/// Copy `src` to `dst` (file or directory tree, permissions included), skipping `target/`
/// and `.cache/`.
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

fn fixture(name: &str) -> PathBuf {
    repo_root()
        .join("core/migrations/fixtures")
        .join(name)
        .join("project")
}

/// All files below `dir` (relative path -> bytes), skipping top-level `.git` and `.cache`.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let rel = e
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if rel == ".git" || rel == ".cache" {
                continue;
            }
            if e.file_type().unwrap().is_dir() {
                walk(root, &e.path(), out);
            } else {
                out.insert(rel, fs::read(e.path()).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// `kb --root <root> --json <args>` from the sandbox root; returns the output and envelope.
fn kb_json(sb: &Sandbox, root: &Path, args: &[&str], toolchain: bool) -> (Output, Value) {
    let mut c = Command::new(kb_bin());
    c.current_dir(sb.path())
        .args(["--root", s(root), "--json"])
        .args(args);
    isolated_env(&mut c, &sb.home());
    if toolchain {
        toolchain_env(&mut c);
    }
    let o = c.output().unwrap();
    let v: Value = serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e})\n{}", show(&o)));
    (o, v)
}

/// Everything of the main checkout that `update prepare` must leave alone: HEAD, the
/// checked-out branch, other branches, the index, status and every work-tree file.
fn main_state(sb: &Sandbox, d: &Path) -> String {
    let mut out = String::new();
    for args in [
        &["rev-parse", "HEAD"][..],
        &["symbolic-ref", "HEAD"],
        &["ls-files", "--stage"],
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ],
        &["diff", "HEAD", "--no-ext-diff"],
        &["stash", "list"],
    ] {
        out.push_str(&format!("$ git {args:?}\n{}\n", sb.git(d, args)));
    }
    let branches = sb.git(
        d,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
        ],
    );
    for b in branches
        .lines()
        .filter(|l| !l.starts_with("refs/heads/kb-update/"))
    {
        out.push_str(&format!("branch {b}\n"));
    }
    for (path, bytes) in tree(d) {
        out.push_str(&format!("file {path} {}\n", kb::util::sha256_hex(&bytes)));
    }
    out
}

/// Bytes of `path` at `rev` in the repository `dir`.
fn blob(sb: &Sandbox, dir: &Path, rev: &str, path: &str) -> Vec<u8> {
    let o = sb.git_output(dir, &["cat-file", "blob", &format!("{rev}:{path}")]);
    assert!(o.status.success(), "{rev}:{path}\n{}", show(&o));
    o.stdout
}

#[test]
fn update_prepare_migrates_a_legacy_project_with_the_bootstrapped_target_engine() {
    let sb = Sandbox::new();
    let src = repo_root();

    // Synthetic upstream v0.1.0: the real engine files and the uninitialized project README.
    let up = sb.path().join("upstream");
    let manifest =
        ReleaseManifest::parse(&fs::read_to_string(src.join("core/release.toml")).unwrap())
            .unwrap();
    for p in manifest
        .engine_paths
        .iter()
        .map(String::as_str)
        .chain(["project/README.md"])
    {
        if src.join(p).exists() {
            copy_path(&src.join(p), &up.join(p));
        }
    }
    for required in ["kbw", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml"] {
        assert!(up.join(required).is_file(), "{required} missing");
    }
    assert!(!up.join("project/project.toml").exists());
    sb.init_repo(&up);
    sb.commit_all(&up, "kb engine 0.1.0 (synthetic upstream)");
    sb.git(&up, &["tag", "-a", "v0.1.0", "-m", "kb 0.1.0"]);

    // The downstream adopted the engine before the current format: its project is legacy.
    let down = sb.path().join("downstream");
    sb.git(&sb.path(), &["clone", "-q", s(&up), s(&down)]);
    copy_path(&fixture("v0"), &down.join("project"));
    let head = sb.commit_all(&down, "adopt the synthetic legacy project");

    // Upstream v0.1.1: an engine-side change outside the Rust sources.
    let doc = up.join(UPSTREAM_DOC);
    let text = fs::read_to_string(&doc).unwrap();
    fs::write(&doc, format!("{text}\n{UPSTREAM_MARKER}\n")).unwrap();
    let v011 = sb.commit_all(&up, "kb engine 0.1.1 (synthetic upstream)");
    sb.git(&up, &["tag", "-a", TAG, "-m", "kb 0.1.1"]);
    let upstream_refs = sb.git(&up, &["for-each-ref", "--format=%(refname) %(objectname)"]);

    let before = main_state(&sb, &down);

    // check: the target engine can migrate the legacy project, and must.
    let check = ["update", "check", "--upstream", s(&up), "--ref", TAG];
    let (o, v) = kb_json(&sb, &down, &check, false);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let r = &v["result"];
    assert_eq!(r["upstream"]["commit"], v011.as_str(), "{r:#}");
    assert_eq!(r["head"], head.as_str());
    assert_eq!(r["merge"]["clean"], true, "{r:#}");
    assert_eq!(r["schema"]["project_schemas"], json!([0]), "{r:#}");
    assert_eq!(r["schema"]["migration_required"], true, "{r:#}");
    assert_eq!(r["schema"]["unsupported"], json!([]), "{r:#}");
    assert_eq!(main_state(&sb, &down), before, "check never writes");

    // prepare with the production runner: bootstrap, migrate, integrate, validate.
    let prepare = ["update", "prepare", "--upstream", s(&up), "--ref", TAG];
    let (o, v) = kb_json(&sb, &down, &prepare, true);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(v["ok"], true, "{v:#}");
    let r = &v["result"];
    assert_eq!(r["branch"], BRANCH, "{r:#}");
    assert_eq!(r["base"], head.as_str());
    assert_eq!(r["upstream"]["commit"], v011.as_str());
    assert_eq!(r["schema"]["migration_required"], true);
    let steps: Vec<(&str, &str)> = r["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s["status"].as_str().unwrap()))
        .collect();
    // The legacy project has no skill configuration, so there is nothing to integrate.
    assert_eq!(
        steps,
        [
            ("bootstrap", "ok"),
            ("migrate", "ok"),
            ("integrate", "skipped"),
            ("validate", "ok")
        ],
        "{r:#}"
    );
    let subjects: Vec<&str> = r["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["subject"].as_str().unwrap())
        .collect();
    let merge_subject = format!("kb: merge upstream {TAG} ({})", &v011[..12]);
    let migrate_subject = format!("kb: migrate knowledge for upstream {TAG}");
    assert_eq!(
        subjects,
        [merge_subject.as_str(), migrate_subject.as_str()],
        "{r:#}"
    );

    // The branch: the upstream merge, then the migrate commit made by the target engine.
    let tip = sb.git(&down, &["rev-parse", &format!("refs/heads/{BRANCH}")]);
    assert_eq!(
        sb.git(&down, &["log", "-1", "--format=%s", &tip]),
        migrate_subject
    );
    assert_eq!(
        sb.git(&down, &["log", "-1", "--format=%s", &format!("{tip}^")]),
        merge_subject
    );
    sb.git(&down, &["merge-base", "--is-ancestor", &v011, &tip]);
    sb.git(&down, &["merge-base", "--is-ancestor", &head, &tip]);
    let tip_doc = String::from_utf8(blob(&sb, &down, &tip, UPSTREAM_DOC)).unwrap();
    assert!(tip_doc.contains(UPSTREAM_MARKER));
    let migrated = sb.git(&down, &["diff", "--name-only", &format!("{tip}^"), &tip]);
    assert!(
        migrated.lines().all(|p| p.starts_with("project/")),
        "the migrate commit only touches the project: {migrated}"
    );
    for p in [
        "project/project.toml",
        "project/knowledge/policies/token-storage.md",
        "project/upstream.toml",
    ] {
        assert!(migrated.lines().any(|l| l == p), "{p} not in {migrated}");
    }

    // Migrated project files on the branch equal the expected fixture byte for byte.
    let expected = tree(&fixture("v2-expected"));
    assert!(expected.len() > 10, "{:?}", expected.keys());
    for (path, bytes) in &expected {
        let got = blob(&sb, &down, &tip, &format!("project/{path}"));
        assert_eq!(
            String::from_utf8_lossy(&got),
            String::from_utf8_lossy(bytes),
            "project/{path} on {BRANCH} differs from v2-expected"
        );
    }
    let recorded = String::from_utf8(blob(&sb, &down, &tip, "project/upstream.toml")).unwrap();
    assert!(
        recorded.contains(&format!("revision = \"{v011}\"")),
        "{recorded}"
    );

    // The worktree is clean and holds the target engine bootstrapped in its own cache.
    let wt = PathBuf::from(r["worktree"].as_str().unwrap());
    assert_eq!(wt, down.join(".cache/update/kb-update-v0.1.1"));
    assert_eq!(sb.git(&wt, &["rev-parse", "HEAD"]), tip);
    assert_eq!(
        sb.git(&wt, &["status", "--porcelain", "--untracked-files=all"]),
        ""
    );
    let runtimes: Vec<PathBuf> = fs::read_dir(wt.join(".cache/runtime"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir() && !p.file_name().unwrap().to_string_lossy().starts_with('.'))
        .collect();
    assert_eq!(runtimes.len(), 1, "{runtimes:?}");
    let info = fs::read_to_string(runtimes[0].join("BUILD-INFO")).unwrap();
    let fp = runtimes[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(info.contains(&format!("\nfingerprint={fp}\n")), "{info}");
    assert!(info.contains("\nsource=source-build\n"), "{info}");

    // The main checkout is unchanged, still legacy, and never bootstrapped; nothing was
    // pushed to the upstream.
    assert_eq!(main_state(&sb, &down), before);
    assert!(
        fs::read_to_string(down.join("project/project.toml"))
            .unwrap()
            .contains("schema = 0")
    );
    assert!(!down.join(".cache/runtime").exists());
    assert_eq!(
        sb.git(&up, &["for-each-ref", "--format=%(refname) %(objectname)"]),
        upstream_refs
    );
}
