//! `kb migrate` and `kb update`: real transformations of the synthetic legacy fixtures and
//! update scenarios against synthetic upstream/downstream repositories (local paths only,
//! isolated Git configuration, no network).
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{Sandbox, json, stderr, stdout, write};
use kb::corpus::load_corpus;
use kb::model::{Profile, ProfileLocation};
use kb::source::WorkingTreeSource;

// ---------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------

fn fixture(name: &str) -> PathBuf {
    common::repo_root()
        .join("core/migrations/fixtures")
        .join(name)
        .join("project")
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for e in fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            fs::copy(e.path(), to).unwrap();
        }
    }
}

/// All files below `dir` (relative path → bytes), skipping top-level `.git` and `.cache`.
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

/// A temp KB root holding the engine manifest and a copy of a fixture project.
fn kb_with_fixture(sb: &Sandbox, name: &str) -> PathBuf {
    let root = sb.path().join("kb");
    common::copy_manifest(&root);
    copy_dir(&fixture(name), &root.join("project"));
    root
}

fn kb_json(sb: &Sandbox, root: &Path, args: &[&str]) -> (Output, serde_json::Value) {
    let mut all = vec!["--root", root.to_str().unwrap(), "--json"];
    all.extend_from_slice(args);
    let out = sb.kb(&sb.path(), &all, &[]);
    let v = json(&out);
    (out, v)
}

fn assert_exit(out: &Output, code: i32) {
    assert_eq!(
        out.status.code(),
        Some(code),
        "stdout:\n{}\nstderr:\n{}",
        stdout(out),
        stderr(out)
    );
}

// ---------------------------------------------------------------------------------------
// kb migrate
// ---------------------------------------------------------------------------------------

#[test]
fn fixture_migration_matches_expected_byte_for_byte_and_is_idempotent() {
    let sb = Sandbox::new();
    let root = kb_with_fixture(&sb, "v0");
    let legacy = tree(&root.join("project"));

    let (out, v) = kb_json(&sb, &root, &["migrate"]);
    assert_exit(&out, 0);
    assert_eq!(v["result"]["mode"], "dry-run");
    assert_eq!(v["result"]["pending"], 13);
    let files = v["result"]["files"].as_array().unwrap();
    assert!(files.iter().all(|f| f["status"] == "migrate"));
    let cfg = files.iter().find(|f| f["kind"] == "config").unwrap();
    assert!(
        cfg["diff"]
            .as_str()
            .unwrap()
            .contains("+approved_ref = \"refs/heads/main\"")
    );
    assert_eq!(
        tree(&root.join("project")),
        legacy,
        "dry-run must not write"
    );

    let (out, v) = kb_json(&sb, &root, &["migrate", "--apply"]);
    assert_exit(&out, 0);
    assert_eq!(v["result"]["written"].as_array().unwrap().len(), 13);
    let migrated = tree(&root.join("project"));
    let expected = tree(&fixture("v1-expected"));
    assert_eq!(
        migrated.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>()
    );
    for (path, bytes) in &expected {
        assert_eq!(
            String::from_utf8_lossy(&migrated[path]),
            String::from_utf8_lossy(bytes),
            "{path} differs from the expected fixture"
        );
    }

    let (out, v) = kb_json(&sb, &root, &["migrate", "--apply"]);
    assert_exit(&out, 0);
    assert!(v["result"]["written"].as_array().unwrap().is_empty());
    assert_eq!(v["result"]["up_to_date"], 13);
    assert_eq!(tree(&root.join("project")), expected);
    let text = sb.kb(
        &sb.path(),
        &["--root", root.to_str().unwrap(), "migrate"],
        &[],
    );
    assert_exit(&text, 0);
    assert!(
        stdout(&text).contains("nothing to migrate"),
        "{}",
        stdout(&text)
    );
}

#[test]
fn expected_fixture_loads_under_the_current_engine() {
    let sb = Sandbox::new();
    let root = kb_with_fixture(&sb, "v1-expected");
    let loc = ProfileLocation::for_profile(Profile::Project);
    let corpus = load_corpus(&WorkingTreeSource::new(&root), &loc).unwrap();
    assert!(corpus.diagnostics.is_empty(), "{:#?}", corpus.diagnostics);
    assert_eq!(corpus.records().count(), 7);
    let metas: Vec<_> = corpus
        .records()
        .map(|(e, p)| kb::validate::MetaInput {
            path: e.path.clone(),
            meta: std::sync::Arc::new(p.meta()),
        })
        .collect();
    let problems = kb::validate::validate_metas(&corpus.config, &corpus.registry, &metas);
    assert!(problems.iter().all(|d| !d.is_error()), "{problems:#?}");
}

#[test]
fn legacy_config_is_read_through_an_in_memory_migration() {
    let sb = Sandbox::new();
    let root = kb_with_fixture(&sb, "v0");
    let loc = ProfileLocation::for_profile(Profile::Project);
    let before = tree(&root);
    let err = kb::corpus::load_config(&WorkingTreeSource::new(&root), &loc).unwrap_err();
    assert_eq!(err.code, kb::error::ErrorCode::UnsupportedSchemaVersion);
    let cfg = kb::migrate::effective_config(&root, &loc).unwrap();
    assert_eq!(cfg.project.namespace, "legacy");
    assert_eq!(cfg.source.approved_ref, "refs/heads/main");
    assert_eq!(cfg.source.allowed_protocols, vec!["file".to_string()]);
    assert_eq!(tree(&root), before);
    assert_eq!(
        kb::migrate::project_schemas(&root, &loc)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![0]
    );
}

#[test]
fn unsupported_schema_versions_fail_and_write_nothing() {
    let sb = Sandbox::new();
    let root = kb_with_fixture(&sb, "v0");
    write(
        &root.join("project/knowledge/future.md"),
        "+++\nschema = 99\nid = \"legacy.future\"\n+++\n",
    );
    let before = tree(&root);
    let (out, v) = kb_json(&sb, &root, &["migrate", "--apply"]);
    assert_exit(&out, 13);
    assert_eq!(v["error"]["code"], "UNSUPPORTED_SCHEMA_VERSION");
    let files = v["error"]["details"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "project/knowledge/future.md");
    assert_eq!(files[0]["schema"], 99);
    assert_eq!(tree(&root), before);

    let (out, v) = kb_json(&sb, &root, &["migrate", "--to", "2"]);
    assert_exit(&out, 13);
    assert_eq!(v["error"]["code"], "UNSUPPORTED_SCHEMA_VERSION");
    assert_eq!(tree(&root), before);
}

#[test]
fn invalid_transform_results_fail_and_write_nothing() {
    let sb = Sandbox::new();
    let root = kb_with_fixture(&sb, "v0");
    // Transforms fine, but the result is not a valid schema-1 record (bad id).
    let path = root.join("project/knowledge/policies/logging.md");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replace("\"legacy.policy.logging\"", "\"Legacy Logging\""),
    )
    .unwrap();
    // The transform itself fails (unknown legacy state).
    let path = root.join("project/knowledge/features/login.md");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, text.replace("\"proposed\"", "\"someday\"")).unwrap();
    let before = tree(&root);

    for args in [&["migrate"][..], &["migrate", "--apply"][..]] {
        let (out, v) = kb_json(&sb, &root, args);
        assert_exit(&out, 52);
        assert_eq!(v["error"]["code"], "MIGRATION_FAILED");
        let files = v["error"]["details"]["files"].as_array().unwrap();
        let paths: Vec<_> = files.iter().map(|f| f["path"].as_str().unwrap()).collect();
        assert_eq!(
            paths,
            vec![
                "project/knowledge/features/login.md",
                "project/knowledge/policies/logging.md"
            ]
        );
        assert_eq!(files[0]["step"], "v0-to-v1");
        assert_eq!(files[1]["step"], "verify");
        assert!(
            files[1]["problems"][0]
                .as_str()
                .unwrap()
                .contains("ID_INVALID")
        );
        assert_eq!(tree(&root), before, "nothing may be written");
    }
}

// ---------------------------------------------------------------------------------------
// kb update: synthetic upstream and downstream
// ---------------------------------------------------------------------------------------

/// Engine files copied from this checkout into the synthetic upstream.
const ENGINE_FILES: [&str; 6] = [
    "Cargo.toml",
    "rust-toolchain.toml",
    ".gitignore",
    "core/release.toml",
    "docs/architecture.md",
    "core/migrations/README.md",
];

/// Stand-in launcher of the synthetic upstream: records how it was invoked and fails,
/// like an engine whose bootstrap is broken.
const FAILING_KBW: &str = "#!/bin/sh\n\
{ echo \"args=$*\"; echo \"KB_ROOT=$KB_ROOT\"; echo \"KB_CACHE_DIR=$KB_CACHE_DIR\"; \
echo \"KBW_CARGO_TARGET_DIR=$KBW_CARGO_TARGET_DIR\"; } > \"$KBW_STUB_LOG\"\n\
echo 'synthetic engine bootstrap failure' >&2\n\
exit 7\n";

struct Repos {
    sb: Sandbox,
    upstream: PathBuf,
    downstream: PathBuf,
    /// Upstream commit tagged v0.1.0 (the downstream's base).
    base: String,
    /// Upstream commit tagged v0.2.0 (annotated tag).
    target: String,
}

/// Upstream `v0.1.0` → downstream clone with the legacy (schema 0) project → upstream
/// `v0.2.0`. With `conflict`, both sides edit the first line of docs/architecture.md.
fn repos(conflict: bool) -> Repos {
    let sb = Sandbox::new();
    let up = sb.path().join("upstream");
    sb.init_repo(&up);
    for f in ENGINE_FILES {
        let dst = up.join(f);
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::copy(common::repo_root().join(f), dst).unwrap();
    }
    write(&up.join("kbw"), FAILING_KBW);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(up.join("kbw"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    write(
        &up.join("project/README.md"),
        "Synthetic upstream: the project is not initialized.\n",
    );
    let base = sb.commit_all(&up, "engine 0.1.0 (synthetic upstream)");
    sb.git(&up, &["tag", "v0.1.0"]);

    let down = sb.path().join("downstream");
    sb.git(
        &sb.path(),
        &["clone", "-q", up.to_str().unwrap(), down.to_str().unwrap()],
    );
    copy_dir(&fixture("v0"), &down.join("project"));
    sb.commit_all(&down, "adopt the synthetic legacy project");
    let doc = "docs/architecture.md";
    if conflict {
        edit_first_line(&down.join(doc), "# KB architecture (downstream edit)");
        sb.commit_all(&down, "downstream engine edit");
        edit_first_line(&up.join(doc), "# KB architecture (upstream 0.2.0)");
    } else {
        let text = fs::read_to_string(up.join(doc)).unwrap();
        fs::write(
            up.join(doc),
            format!("{text}\n<!-- synthetic upstream 0.2.0 -->\n"),
        )
        .unwrap();
    }
    write(
        &up.join("core/templates/UPSTREAM-0.2.0.txt"),
        "synthetic upstream change\n",
    );
    sb.commit_all(&up, "engine 0.2.0 (synthetic upstream)");
    sb.git(&up, &["tag", "-a", "v0.2.0", "-m", "synthetic 0.2.0"]);
    let target = sb.git(&up, &["rev-parse", "v0.2.0^{commit}"]);
    Repos {
        sb,
        upstream: up,
        downstream: down,
        base,
        target,
    }
}

fn edit_first_line(path: &Path, line: &str) {
    let text = fs::read_to_string(path).unwrap();
    let rest = text.split_once('\n').map(|(_, r)| r).unwrap_or("");
    fs::write(path, format!("{line}\n{rest}")).unwrap();
}

/// Observable state of the main checkout: HEAD, non-update branches, index bytes,
/// status and every work-tree file (outside .git and .cache).
#[derive(Debug, PartialEq)]
struct MainState {
    head: String,
    branches: Vec<String>,
    index: Vec<u8>,
    status: String,
    files: BTreeMap<String, Vec<u8>>,
}

fn main_state(r: &Repos) -> MainState {
    let d = &r.downstream;
    let branches =
        r.sb.git(
            d,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads",
            ],
        )
        .lines()
        .filter(|l| !l.starts_with("refs/heads/kb-update/"))
        .map(str::to_string)
        .collect();
    MainState {
        head: r.sb.git(d, &["rev-parse", "HEAD"]),
        branches,
        index: fs::read(d.join(".git/index")).unwrap(),
        status: r.sb.git(
            d,
            &[
                "--no-optional-locks",
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
            ],
        ),
        files: tree(d),
    }
}

impl Repos {
    fn kb(&self, args: &[&str], envs: &[(&str, &str)]) -> (Output, serde_json::Value) {
        let mut all = vec!["--root", self.downstream.to_str().unwrap(), "--json"];
        all.extend_from_slice(args);
        let out = self.sb.kb(&self.sb.path(), &all, envs);
        let v = json(&out);
        (out, v)
    }

    fn update_worktree(&self) -> PathBuf {
        self.downstream.join(".cache/update/kb-update-v0.2.0")
    }
}

#[test]
fn update_check_reports_versions_schema_and_predicted_conflicts() {
    let r = repos(true);
    let before = main_state(&r);
    // A relative upstream path is resolved against the working directory.
    let (out, v) = r.kb(
        &[
            "update",
            "check",
            "--upstream",
            "upstream",
            "--ref",
            "v0.2.0",
        ],
        &[],
    );
    assert_exit(&out, 51);
    assert_eq!(v["error"]["code"], "UPDATE_CONFLICT");
    let res = &v["result"];
    assert_eq!(res["upstream"]["commit"], r.target.as_str());
    assert_eq!(res["upstream"]["url"], r.upstream.to_str().unwrap());
    assert_eq!(res["versions"]["engine_version"]["changed"], false);
    assert_eq!(res["versions"]["document_schema"]["target"], "1");
    assert_eq!(res["schema"]["project_schemas"], serde_json::json!([0]));
    assert_eq!(res["schema"]["migration_required"], true);
    assert_eq!(res["merge"]["clean"], false);
    assert_eq!(
        res["merge"]["conflicts"],
        serde_json::json!(["docs/architecture.md"])
    );
    assert!(res["divergence"].is_null(), "no project/upstream.toml yet");
    assert!(res["notes"].as_array().unwrap().iter().any(|n| {
        n.as_str()
            .unwrap()
            .contains("engine divergence not computed")
    }));
    assert_eq!(main_state(&r), before);
    assert!(!r.update_worktree().exists());

    // Already merged revision: nothing to do, exit 0.
    let (out, v) = r.kb(
        &[
            "update",
            "check",
            "--upstream",
            "upstream",
            "--ref",
            "v0.1.0",
        ],
        &[],
    );
    assert_exit(&out, 0);
    assert_eq!(v["result"]["merge"]["up_to_date"], true);
    assert_eq!(v["result"]["upstream"]["commit"], r.base.as_str());
    assert_eq!(main_state(&r), before);
}

#[test]
fn update_check_clean_merge_transport_policy_and_unsupported_schema() {
    let r = repos(false);
    let up = r.upstream.to_str().unwrap();
    // Uncommitted work is reported and left alone.
    write(
        &r.downstream.join("project/knowledge/draft.md"),
        "work in progress\n",
    );
    let before = main_state(&r);
    let (out, v) = r.kb(
        &["update", "check", "--upstream", up, "--ref", "v0.2.0"],
        &[],
    );
    assert_exit(&out, 0);
    assert_eq!(v["result"]["merge"]["clean"], true);
    assert_eq!(v["result"]["merge"]["conflicts"], serde_json::json!([]));
    assert_eq!(
        v["result"]["uncommitted"],
        serde_json::json!([{"status": "??", "path": "project/knowledge/draft.md"}])
    );
    assert_eq!(main_state(&r), before);

    // The legacy config allows only `file`; https is refused before any network access.
    let (out, v) = r.kb(
        &[
            "update",
            "check",
            "--upstream",
            "https://example.invalid/kb.git",
            "--ref",
            "v1",
        ],
        &[],
    );
    assert_exit(&out, 11);
    assert_eq!(v["error"]["code"], "CONFIG_INVALID");

    // A target engine that cannot migrate schema 0.
    let manifest = r.upstream.join("core/release.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        text.replace("migrates_from = [0]", "migrates_from = []"),
    )
    .unwrap();
    r.sb.commit_all(&r.upstream, "drop legacy migrations (synthetic)");
    r.sb.git(&r.upstream, &["tag", "v0.3.0"]);
    let (out, v) = r.kb(
        &["update", "check", "--upstream", up, "--ref", "v0.3.0"],
        &[],
    );
    assert_exit(&out, 13);
    assert_eq!(v["error"]["code"], "UNSUPPORTED_SCHEMA_VERSION");
    assert_eq!(v["result"]["schema"]["unsupported"], serde_json::json!([0]));
    let (out, v) = r.kb(
        &["update", "prepare", "--upstream", up, "--ref", "v0.3.0"],
        &[],
    );
    assert_exit(&out, 13);
    assert_eq!(v["error"]["code"], "UNSUPPORTED_SCHEMA_VERSION");
    let (out, _) = r.kb(
        &["update", "check", "--upstream", up, "--ref", "no-such-tag"],
        &[],
    );
    assert_exit(&out, 16);
    assert_eq!(main_state(&r), before);
    assert!(!r.downstream.join(".cache/update").exists());
}

#[test]
fn update_prepare_conflict_keeps_main_checkout_and_abandon_cleans_up() {
    let r = repos(true);
    let up = r.upstream.to_str().unwrap();
    let before = main_state(&r);
    let (out, v) = r.kb(
        &["update", "prepare", "--upstream", up, "--ref", "v0.2.0"],
        &[],
    );
    assert_exit(&out, 51);
    assert_eq!(v["error"]["code"], "UPDATE_CONFLICT");
    let details = &v["error"]["details"];
    assert_eq!(details["branch"], "kb-update/v0.2.0");
    assert_eq!(
        details["conflicts"],
        serde_json::json!(["docs/architecture.md"])
    );
    let wt = r.update_worktree();
    assert_eq!(details["worktree"], wt.to_str().unwrap());
    assert!(!details["next_steps"].as_array().unwrap().is_empty());
    // The worktree is left conflicted for manual resolution.
    assert_eq!(
        r.sb.git(&wt, &["diff", "--name-only", "--diff-filter=U"]),
        "docs/architecture.md"
    );
    assert!(
        fs::read_to_string(wt.join("docs/architecture.md"))
            .unwrap()
            .contains("<<<<<<<")
    );
    assert_eq!(
        main_state(&r),
        before,
        "the main checkout must be unchanged"
    );
    assert!(
        r.sb.git(&r.downstream, &["branch", "--list", "kb-update/v0.2.0"])
            .contains("kb-update/v0.2.0")
    );

    // A second attempt on the same branch is refused.
    let (out, _) = r.kb(
        &["update", "prepare", "--upstream", up, "--ref", "v0.2.0"],
        &[],
    );
    assert_exit(&out, 64);

    let (out, _) = r.kb(&["update", "abandon", "main"], &[]);
    assert_exit(&out, 64);
    let (out, _) = r.kb(&["update", "abandon", "kb-update/unknown"], &[]);
    assert_exit(&out, 16);
    // A mutating command: dry-run by default, nothing is removed.
    let (out, v) = r.kb(&["update", "abandon", "kb-update/v0.2.0"], &[]);
    assert_exit(&out, 0);
    assert_eq!(v["result"]["mode"], "dry-run");
    assert_eq!(v["result"]["branch"], "kb-update/v0.2.0");
    assert_eq!(v["result"]["worktree"], wt.to_str().unwrap());
    assert!(v["result"]["dirty"].as_u64().unwrap() > 0, "{v:#}");
    assert!(wt.join("docs/architecture.md").is_file());
    assert!(
        r.sb.git(&r.downstream, &["branch", "--list", "kb-update/v0.2.0"])
            .contains("kb-update/v0.2.0")
    );
    // --force needs --apply.
    let (out, _) = r.kb(&["update", "abandon", "kb-update/v0.2.0", "--force"], &[]);
    assert_exit(&out, 2);
    // The conflicted worktree is not discarded without --force.
    let (out, v) = r.kb(&["update", "abandon", "kb-update/v0.2.0", "--apply"], &[]);
    assert_exit(&out, 45);
    assert_eq!(v["error"]["code"], "CONFLICT");
    assert!(
        v["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("--apply --force"),
        "{v:#}"
    );
    assert!(wt.join("docs/architecture.md").is_file());
    let (out, v) = r.kb(
        &[
            "update",
            "abandon",
            "kb-update/v0.2.0",
            "--apply",
            "--force",
        ],
        &[],
    );
    assert_exit(&out, 0);
    assert_eq!(v["result"]["mode"], "apply");
    assert_eq!(v["result"]["branch"], "kb-update/v0.2.0");
    assert!(!wt.exists());
    assert_eq!(
        r.sb.git(&r.downstream, &["branch", "--list", "kb-update/*"]),
        ""
    );
    assert_eq!(main_state(&r), before);
}

#[test]
fn update_prepare_custom_branch_can_always_be_abandoned() {
    let r = repos(true);
    let up = r.upstream.to_str().unwrap();
    let before = main_state(&r);
    let prepare = |branch: &str| {
        r.kb(
            &[
                "update",
                "prepare",
                "--upstream",
                up,
                "--ref",
                "v0.2.0",
                "--branch",
                branch,
            ],
            &[],
        )
    };

    // `abandon` only removes `kb-update/*` branches, so any other name is refused up front
    // instead of leaving a branch and worktree that the printed recovery cannot remove.
    let (out, v) = prepare("review/kb-0.2");
    assert_exit(&out, 64);
    assert_eq!(v["error"]["code"], "INVALID_INPUT");
    assert!(
        v["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("--branch kb-update/review-kb-0.2"),
        "{v:#}"
    );
    assert_eq!(main_state(&r), before);
    assert_eq!(
        r.sb.git(&r.downstream, &["branch", "--list", "review/*"]),
        ""
    );
    assert!(!r.downstream.join(".cache/update").exists());

    // A custom `kb-update/` name: the abandon command printed on conflict works as printed
    // (with --force for the conflicted files, as the printed note says).
    let (out, v) = prepare("kb-update/review-0.2");
    assert_exit(&out, 51);
    assert_eq!(v["error"]["code"], "UPDATE_CONFLICT");
    let wt = r.downstream.join(".cache/update/kb-update-review-0.2");
    assert!(wt.is_dir());
    let printed = v["error"]["details"]["next_steps"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|s| s.as_str().unwrap().split_once("kbw update abandon "))
        .map(|(_, rest)| rest.split(" (").next().unwrap().trim().to_string())
        .expect("next_steps name the abandon command");
    assert_eq!(printed, "kb-update/review-0.2 --apply");
    let mut args = vec!["update", "abandon"];
    args.extend(printed.split(' '));
    let (out, v) = r.kb(&args, &[]);
    assert_exit(&out, 45);
    assert_eq!(v["error"]["code"], "CONFLICT");
    assert!(wt.is_dir());
    args.push("--force");
    let (out, _) = r.kb(&args, &[]);
    assert_exit(&out, 0);
    assert!(!wt.exists());
    assert_eq!(
        r.sb.git(&r.downstream, &["branch", "--list", "kb-update/*"]),
        ""
    );
    assert_eq!(main_state(&r), before);
}

#[cfg(unix)]
#[test]
fn update_prepare_engine_failure_keeps_worktree_and_main_checkout() {
    let r = repos(false);
    let up = r.upstream.to_str().unwrap();
    let before = main_state(&r);
    let log = r.sb.path().join("kbw-stub.log");
    let (out, v) = r.kb(
        &[
            "update",
            "prepare",
            "--upstream",
            up,
            "--ref",
            "v0.2.0",
            "--branch",
            "kb-update/try-1",
        ],
        &[("KBW_STUB_LOG", log.to_str().unwrap())],
    );
    assert_exit(&out, 53);
    assert_eq!(v["error"]["code"], "UPDATE_FAILED");
    let details = &v["error"]["details"];
    // The explicit bootstrap of the target engine is the first engine step.
    assert_eq!(details["step"], "bootstrap");
    assert_eq!(details["exit_code"], 7);
    assert!(
        v["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("kbw update abandon kb-update/try-1 --apply"),
        "{v:#}"
    );
    assert!(
        details["output"]
            .as_str()
            .unwrap()
            .contains("synthetic engine bootstrap failure")
    );
    let wt = r.downstream.join(".cache/update/kb-update-try-1");
    assert_eq!(details["worktree"], wt.to_str().unwrap());
    // The production runner ran `<worktree>/kbw --kbw-bootstrap` with an isolated cache.
    let invocation = fs::read_to_string(&log).unwrap();
    assert!(invocation.contains("args=--kbw-bootstrap"), "{invocation}");
    assert!(
        invocation.contains(&format!("KB_ROOT={}\n", wt.display())),
        "{invocation}"
    );
    assert!(invocation.contains(&format!("KB_CACHE_DIR={}\n", wt.join(".cache").display())));
    assert!(invocation.contains(&format!(
        "KBW_CARGO_TARGET_DIR={}\n",
        r.downstream.join(".cache/cargo-target").display()
    )));
    // The merge commit exists on the kept branch; the main checkout is untouched.
    let parents = r.sb.git(&wt, &["rev-list", "--parents", "-n1", "HEAD"]);
    assert_eq!(parents.split(' ').count(), 3, "HEAD must be a merge commit");
    assert!(wt.join("core/templates/UPSTREAM-0.2.0.txt").is_file());
    assert_eq!(main_state(&r), before);
    assert!(
        !r.downstream
            .join("core/templates/UPSTREAM-0.2.0.txt")
            .exists()
    );
}

#[test]
fn update_divergence_with_and_without_engine_patches() {
    let r = repos(false);
    let (out, v) = r.kb(&["update", "divergence"], &[]);
    assert_exit(&out, 11);
    assert_eq!(v["error"]["code"], "CONFIG_INVALID");

    let upstream_file = r.downstream.join("project/upstream.toml");
    write(
        &upstream_file,
        &format!("schema = 1\nrevision = \"{}\"\nref = \"v0.1.0\"\n", r.base),
    );
    r.sb.commit_all(&r.downstream, "record upstream base");
    let (out, v) = r.kb(&["update", "divergence"], &[]);
    assert_exit(&out, 0);
    assert_eq!(v["result"]["diverged"], serde_json::json!([]));

    // Uncommitted and untracked engine edits diverge; project edits never do.
    let doc = r.downstream.join("docs/architecture.md");
    fs::write(&doc, fs::read_to_string(&doc).unwrap() + "\nlocal note\n").unwrap();
    write(&r.downstream.join("core/local-hack.txt"), "hack\n");
    write(&r.downstream.join("project/knowledge/new.md"), "draft\n");
    let (out, v) = r.kb(&["update", "divergence"], &[]);
    assert_exit(&out, 43);
    assert_eq!(v["error"]["code"], "ENGINE_DIVERGED");
    assert_eq!(
        v["result"]["diverged"],
        serde_json::json!([
            {"status": "??", "path": "core/local-hack.txt"},
            {"status": "M", "path": "docs/architecture.md"}
        ])
    );

    let patches = "\n[[engine_patches]]\npath = \"docs/architecture.md\"\nreason = \"local note\"\n\n\
[[engine_patches]]\npath = \"core/\"\nreason = \"local tooling\"\n\n\
[[engine_patches]]\npath = \"kbw\"\nreason = \"not changed\"\n";
    fs::write(
        &upstream_file,
        fs::read_to_string(&upstream_file).unwrap() + patches,
    )
    .unwrap();
    let (out, v) = r.kb(&["update", "divergence"], &[]);
    assert_exit(&out, 0);
    assert_eq!(v["result"]["diverged"], serde_json::json!([]));
    assert_eq!(v["result"]["patched"].as_array().unwrap().len(), 2);
    assert_eq!(v["result"]["unused_patches"], serde_json::json!(["kbw"]));

    let missing = "0123456789abcdef0123456789abcdef01234567";
    write(
        &upstream_file,
        &format!("schema = 1\nrevision = \"{missing}\"\n"),
    );
    let (out, v) = r.kb(&["update", "divergence"], &[]);
    assert_exit(&out, 60);
    assert_eq!(v["error"]["code"], "GIT_ERROR");
}

// ---------------------------------------------------------------------------------------
// update prepare happy path (library API with an injected runner)
// ---------------------------------------------------------------------------------------

const CHILD_MARKER: &str = "KB_MIGRATE_UPDATE_ISOLATED_CHILD";

/// Re-run the named test in a child process whose environment is isolated like every
/// other Git invocation in the tests (library calls inherit the process environment).
/// Returns true when the caller is that child and should run the test body.
fn in_isolated_child(test: &str) -> bool {
    if std::env::var_os(CHILD_MARKER).is_some() {
        return true;
    }
    let home = tempfile::tempdir().unwrap();
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args([test, "--exact", "--nocapture", "--test-threads=1"]);
    common::isolated_env(&mut c, home.path());
    c.env(CHILD_MARKER, "1");
    let out = c.output().unwrap();
    assert!(
        out.status.success() && stdout(&out).contains("1 passed"),
        "isolated child failed:\n{}\n{}",
        stdout(&out),
        stderr(&out)
    );
    false
}

/// Runs the target engine as the synthetic upstream carries this engine: `migrate` and
/// `integrate` through the built `kb` executable with `--root <worktree>`, and `validate`
/// in-process (strict corpus load plus cross-record validation) so the step does not
/// depend on another command's implementation.
#[cfg(unix)]
struct TestRunner {
    calls: std::cell::RefCell<Vec<String>>,
}

#[cfg(unix)]
impl kb::update::EngineRunner for TestRunner {
    fn run(&self, worktree: &Path, args: &[&str]) -> kb::error::Result<Output> {
        use std::os::unix::process::ExitStatusExt;
        self.calls.borrow_mut().push(args.join(" "));
        if args == ["validate"] {
            let loc = ProfileLocation::for_profile(Profile::Project);
            let problems = match load_corpus(&WorkingTreeSource::new(worktree), &loc) {
                Ok(c) => {
                    let metas: Vec<_> = c
                        .records()
                        .map(|(e, p)| kb::validate::MetaInput {
                            path: e.path.clone(),
                            meta: std::sync::Arc::new(p.meta()),
                        })
                        .collect();
                    let mut d = c.diagnostics.clone();
                    d.extend(kb::validate::validate_metas(&c.config, &c.registry, &metas));
                    d.iter()
                        .filter(|d| d.is_error())
                        .map(|d| format!("{} {:?}: {}", d.code, d.path, d.message))
                        .collect::<Vec<_>>()
                }
                Err(e) => vec![e.to_string()],
            };
            let code = if problems.is_empty() { 0 } else { 40 };
            return Ok(Output {
                status: std::process::ExitStatus::from_raw(code << 8),
                stdout: Vec::new(),
                stderr: problems.join("\n").into_bytes(),
            });
        }
        Ok(Command::new(common::kb_bin())
            .arg("--root")
            .arg(worktree)
            .args(args)
            .output()
            .expect("run kb"))
    }
}

#[cfg(unix)]
#[test]
fn update_prepare_happy_path_produces_a_validated_reviewable_branch() {
    if !in_isolated_child("update_prepare_happy_path_produces_a_validated_reviewable_branch") {
        return;
    }
    let r = repos(false);
    // A recorded base with a declared engine patch; the update must keep the patch.
    write(
        &r.downstream.join("project/upstream.toml"),
        &format!(
            "# Upstream base (synthetic).\nschema = 1\nrevision = \"{}\"\nref = \"v0.1.0\"\n\n\
             [[engine_patches]]\npath = \"kbw\"\nreason = \"local launcher tweak\"\n",
            r.base
        ),
    );
    r.sb.commit_all(&r.downstream, "record upstream base");
    let before = main_state(&r);
    let env = kb::env::Env::discover(Some(&r.downstream), true).unwrap();
    let loc = ProfileLocation::for_profile(Profile::Project);
    let src = kb::update::UpstreamSource {
        upstream: r.upstream.to_str().unwrap().to_string(),
        reference: "v0.2.0".into(),
    };
    let runner = TestRunner {
        calls: Default::default(),
    };
    let report = kb::update::prepare(&env, &loc, &src, None, &runner).unwrap();

    assert_eq!(
        runner.calls.borrow().as_slice(),
        ["migrate --apply", "validate"]
    );
    assert_eq!(report.branch, "kb-update/v0.2.0");
    let wt = r.update_worktree();
    assert_eq!(Path::new(&report.worktree), wt);
    assert_eq!(report.base, before.head);
    assert_eq!(report.upstream.commit, r.target);
    let steps: Vec<_> = report
        .steps
        .iter()
        .map(|s| (s.name.as_str(), s.status.as_str()))
        .collect();
    assert_eq!(
        steps,
        [
            ("migrate", "ok"),
            ("integrate", "skipped"),
            ("validate", "ok")
        ]
    );
    let subjects: Vec<_> = report.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(
        subjects,
        [
            format!("kb: merge upstream v0.2.0 ({})", &r.target[..12]),
            "kb: migrate knowledge for upstream v0.2.0".to_string()
        ]
    );
    assert_eq!(report.changes.engine, 2, "{:#?}", report.changes);
    assert!(
        report
            .next_steps
            .iter()
            .any(|s| s.contains("git diff HEAD...kb-update/v0.2.0"))
    );
    assert!(
        report
            .next_steps
            .iter()
            .any(|s| s.ends_with("kbw update abandon kb-update/v0.2.0 --apply")),
        "{:?}",
        report.next_steps
    );
    // The committed branch is clean: the dry-run reports what --apply would remove.
    let plan = kb::update::abandon(&env, "kb-update/v0.2.0", false, false).unwrap();
    assert_eq!(plan.mode, "dry-run");
    assert_eq!(plan.dirty, Some(0));
    assert!(plan.commits_not_in_head >= 2, "{plan:?}");
    assert!(wt.is_dir());

    // The branch holds the migrated knowledge, byte-identical to the expected fixture.
    let project = tree(&wt.join("project"));
    for (path, bytes) in tree(&fixture("v1-expected")) {
        assert_eq!(project.get(&path), Some(&bytes), "{path}");
    }
    let upstream_text = fs::read_to_string(wt.join("project/upstream.toml")).unwrap();
    let upstream_cfg: kb::model::UpstreamConfig = toml::from_str(&upstream_text).unwrap();
    assert_eq!(upstream_cfg.revision.as_deref(), Some(r.target.as_str()));
    assert_eq!(upstream_cfg.r#ref.as_deref(), Some("v0.2.0"));
    assert_eq!(upstream_cfg.url.as_deref(), r.upstream.to_str());
    assert_eq!(upstream_cfg.engine_patches.len(), 1);
    assert_eq!(upstream_cfg.engine_patches[0].path, "kbw");
    assert!(upstream_text.starts_with("# Upstream base (synthetic).\n"));
    assert!(wt.join("core/templates/UPSTREAM-0.2.0.txt").is_file());
    // Everything is committed and the committed tree loads under the current engine.
    assert_eq!(
        r.sb.git(&wt, &["status", "--porcelain", "--untracked-files=all"]),
        ""
    );
    let corpus = load_corpus(&WorkingTreeSource::new(&wt), &loc).unwrap();
    assert!(corpus.diagnostics.is_empty(), "{:#?}", corpus.diagnostics);
    let reviewed = r.sb.git(
        &r.downstream,
        &["diff", "--name-only", "HEAD...kb-update/v0.2.0"],
    );
    assert!(reviewed.contains("project/upstream.toml"), "{reviewed}");
    assert!(reviewed.contains("project/knowledge/policies/token-storage.md"));

    // The main checkout (HEAD, index, files, other branches) is untouched and still legacy.
    assert_eq!(main_state(&r), before);
    assert!(
        fs::read_to_string(r.downstream.join("project/project.toml"))
            .unwrap()
            .contains("schema = 0")
    );

    // The main checkout still records (and matches) the old base. After review the update
    // merges into main, and divergence against the newly recorded base is clean too.
    let (out, v) = r.kb(&["update", "divergence"], &[]);
    assert_exit(&out, 0);
    assert_eq!(v["result"]["revision"], r.base.as_str());
    r.sb.git(
        &r.downstream,
        &["merge", "-q", "--ff-only", "kb-update/v0.2.0"],
    );
    let (out, v) = r.kb(&["update", "divergence"], &[]);
    assert_exit(&out, 0);
    assert_eq!(v["result"]["revision"], r.target.as_str());
    assert_eq!(v["result"]["unused_patches"], serde_json::json!(["kbw"]));
}
