//! Impact analysis against real temporary Git repositories and parsed temporary corpora.
mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};

use common::{Sandbox, write};
use kb::corpus::load_corpus;
use kb::error::ErrorCode;
use kb::impact::{
    ChangeStatus, HostDiff, ImpactReport, KbChangeKind, REASONING_NOTE, UNKNOWN_COVERAGE_NOTE,
    analyze, check, host_diff, parse_statement, render, to_json,
};
use kb::knowledge::{MetaEntry, Origin};
use kb::model::{AnchorKind, Profile, ProfileLocation, Registry};
use kb::output::Format;
use kb::source::WorkingTreeSource;

/// `host_diff` spawns Git with this process's environment; isolate it from the user's
/// global/system configuration and default excludes file once per test process.
fn isolate_git_env() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let absent_home = std::env::temp_dir().join("kb-impact-tests-no-home");
        // SAFETY: std serializes environment reads (including `Command::spawn`) and writes
        // with its internal lock, and these tests run no foreign code that reads the
        // environment concurrently.
        unsafe {
            std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            std::env::set_var("HOME", &absent_home);
            std::env::set_var("XDG_CONFIG_HOME", &absent_home);
            std::env::set_var("GIT_TERMINAL_PROMPT", "0");
        }
    });
}

fn sandbox() -> Sandbox {
    isolate_git_env();
    Sandbox::new()
}

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// A host repo with a `feature` branch touching every change kind; `main` moved on after the
/// branch point (a change that must not appear in the diff).
fn feature_host(sb: &Sandbox) -> PathBuf {
    let host = sb.path().join("host");
    sb.init_repo(&host);
    for (path, text) in [
        ("app/auth/Session.kt", "class Session\n"),
        ("app/legacy/Login.kt", "class LegacyLogin\n"),
        ("app/ui/Screen.kt", "class Screen\n"),
        ("app/misc/Other.kt", "class Other\n"),
        (
            "docs/old.md",
            "# Architecture notes\n\nLong enough to be detected as a rename.\n",
        ),
        ("src/api/Handler.kt", "class Handler\n"),
        ("tools/build.sh", "echo build\n"),
    ] {
        write(&host.join(path), text);
    }
    sb.commit_all(&host, "base");
    sb.git(&host, &["checkout", "-q", "-b", "feature"]);
    write(
        &host.join("app/auth/Session.kt"),
        "class Session { fun refresh() }\n",
    );
    write(
        &host.join("app/auth/test/SessionTest.kt"),
        "class SessionTest\n",
    );
    write(&host.join("app/settings/Prefs.kt"), "class Prefs\n");
    write(&host.join("app/ui/Screen.kt"), "class Screen { }\n");
    write(&host.join("src/api/Handler.kt"), "class Handler { }\n");
    write(&host.join("tools/build.sh"), "echo build all\n");
    sb.git(&host, &["rm", "-q", "app/legacy/Login.kt"]);
    sb.git(&host, &["mv", "docs/old.md", "docs/new.md"]);
    sb.commit_all(&host, "feature work");
    sb.git(&host, &["checkout", "-q", "main"]);
    write(&host.join("app/misc/Other.kt"), "class Other { }\n");
    sb.commit_all(&host, "main moves on");
    sb.git(&host, &["checkout", "-q", "feature"]);
    host
}

fn record(root: &Path, name: &str, text: &str) {
    write(
        &root.join(format!("project/knowledge/extra/{name}.md")),
        text,
    );
}

/// The minimal project plus records linked through every relation kind.
fn knowledge(sb: &Sandbox) -> (Registry, Vec<MetaEntry>) {
    let root = sb.path().join("kb");
    common::write_min_project(&root);
    write(
        &root.join("project/registry/modules.toml"),
        r#"schema = 1
[[module]]
id = "mobile.auth"
repo = "mobile"
title = "Mobile authentication"
paths = ["app/auth/**"]
features = ["login"]
[[module]]
id = "mobile.settings"
repo = "mobile"
title = "Mobile settings"
paths = ["app/settings/**"]
[[module]]
id = "backend.api"
repo = "backend"
title = "Backend API"
paths = ["src/api/**"]
features = ["login"]
"#,
    );
    record(
        &root,
        "session-refresh",
        r#"+++
schema = 1
id = "acme.mobile.session-refresh"
kind = "invariant"
title = "Session refresh is single-flight"
status = "accepted"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[links]
supersedes = ["acme.mobile.old-session"]

[[anchors]]
kind = "source"
repo = "mobile"
path = "app/auth/Session.kt"

[[anchors]]
kind = "test"
repo = "mobile"
path = "app/auth/test/"

[[statements]]
id = "single-flight"
level = "must"
text = "Run at most one token refresh at a time."
+++
"#,
    );
    record(
        &root,
        "old-session",
        r#"+++
schema = 1
id = "acme.mobile.old-session"
kind = "invariant"
title = "Old session rule"
status = "superseded"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[[anchors]]
kind = "source"
repo = "mobile"
path = "app/auth/Session.kt"

[[statements]]
id = "old"
level = "must"
text = "Refresh on every request."
+++
"#,
    );
    record(
        &root,
        "legacy-login",
        r#"+++
schema = 1
id = "acme.mobile.legacy-login"
kind = "decision"
title = "Keep the legacy login screen"
status = "accepted"
owner = "team-mobile"
context = "Old devices still use the legacy screen."
decision = "Keep it until usage drops."
reasons = ["Usage is still significant."]

[scope]
repos = ["mobile"]

[[anchors]]
kind = "source"
repo = "mobile"
path = "app/legacy/Login.kt"

[[anchors]]
kind = "doc"
repo = "mobile"
path = "docs/old.md"
+++
"#,
    );
    record(
        &root,
        "ui-guidelines",
        r#"+++
schema = 1
id = "acme.mobile.ui-guidelines"
kind = "reference"
title = "UI guidelines"
status = "accepted"
owner = "team-mobile"
summary = "Screen composition conventions."

[scope]
repos = ["mobile"]

[selectors]
paths = ["mobile:app/ui/**"]
+++
"#,
    );
    record(
        &root,
        "backend-api",
        r#"+++
schema = 1
id = "acme.backend.api-guidelines"
kind = "reference"
title = "Backend API guidelines"
status = "accepted"
owner = "team-backend"
summary = "Handler conventions."

[scope]
repos = ["backend"]

[selectors]
paths = ["src/api/**"]
+++
"#,
    );
    record(
        &root,
        "login-feature",
        r#"+++
schema = 1
id = "acme.feature.login"
kind = "feature"
title = "Login"
status = "accepted"
owner = "arch"
feature = "login"
summary = "Users sign in and receive tokens."

[scope]
features = ["login"]

[[behaviors]]
id = "sign-in"
text = "A successful sign-in stores a refresh token."
+++
"#,
    );
    record(
        &root,
        "tool-scripts",
        r#"+++
schema = 1
id = "acme.tools.scripts"
kind = "reference"
title = "Build scripts"
status = "accepted"
owner = "arch"
summary = "Scripts are POSIX sh."

[scope]
product = true

[selectors]
paths = ["tools/**"]
+++
"#,
    );
    let corpus = load_corpus(
        &WorkingTreeSource::new(&root),
        &ProfileLocation::for_profile(Profile::Project),
    )
    .unwrap();
    assert!(corpus.diagnostics.is_empty(), "{:#?}", corpus.diagnostics);
    let metas = corpus
        .records()
        .map(|(entry, parsed)| MetaEntry {
            path: entry.path.clone(),
            origin: Origin::Accepted,
            meta: Arc::new(parsed.meta()),
        })
        .collect();
    (corpus.registry, metas)
}

fn statuses(diff: &HostDiff) -> Vec<(String, Option<String>, ChangeStatus)> {
    diff.files
        .iter()
        .map(|f| (f.path.clone(), f.old_path.clone(), f.status))
        .collect()
}

fn links_of(report: &ImpactReport, path: &str) -> BTreeSet<(String, String)> {
    let file = report
        .files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} is not in the report"));
    file.links
        .iter()
        .map(|l| (l.record.clone(), l.relation.as_str().to_string()))
        .collect()
}

fn pairs(v: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

fn s(v: &str) -> String {
    v.to_string()
}

// ---------------------------------------------------------------------------------------
// host_diff
// ---------------------------------------------------------------------------------------

#[test]
fn commit_diff_reports_every_change_kind_relative_to_the_merge_base() {
    let sb = sandbox();
    let host = feature_host(&sb);
    let diff = host_diff(&host, "main", Some("feature"), false, None).unwrap();

    let main = sb.git(&host, &["rev-parse", "main"]);
    let branch_point = sb.git(&host, &["merge-base", "main", "feature"]);
    assert_eq!(diff.base, main);
    assert_eq!(diff.merge_base, branch_point);
    assert_ne!(diff.base, diff.merge_base);
    assert_eq!(diff.head, Some(sb.git(&host, &["rev-parse", "feature"])));
    assert!(diff.kb_pointer.is_none());

    use ChangeStatus::*;
    assert_eq!(
        statuses(&diff),
        vec![
            (s("app/auth/Session.kt"), None, Modified),
            (s("app/auth/test/SessionTest.kt"), None, Added),
            (s("app/legacy/Login.kt"), None, Deleted),
            (s("app/settings/Prefs.kt"), None, Added),
            (s("app/ui/Screen.kt"), None, Modified),
            (s("docs/new.md"), Some(s("docs/old.md")), Renamed),
            (s("src/api/Handler.kt"), None, Modified),
            (s("tools/build.sh"), None, Modified),
        ],
        "main-only change app/misc/Other.kt must not appear"
    );
    // Reading never touches the work tree or the index.
    assert_eq!(sb.git(&host, &["status", "--porcelain"]), "");
}

#[test]
fn working_tree_mode_includes_staged_unstaged_and_untracked_files() {
    let sb = sandbox();
    let host = sb.path().join("wt-host");
    sb.init_repo(&host);
    write(&host.join(".gitignore"), "*.log\n");
    write(&host.join("a.txt"), "a\n");
    write(&host.join("b.txt"), "b\n");
    let head = sb.commit_all(&host, "base");

    write(&host.join("a.txt"), "a changed, unstaged\n");
    fs::remove_file(host.join("b.txt")).unwrap();
    write(&host.join("s.txt"), "staged\n");
    sb.git(&host, &["add", "s.txt"]);
    write(&host.join("u.txt"), "untracked\n");
    write(
        &host.join("nested/dir/x.txt"),
        "untracked in a new directory\n",
    );
    write(&host.join("debug.log"), "ignored\n");

    let diff = host_diff(&host, "HEAD", None, true, None).unwrap();
    assert_eq!(diff.head, None);
    assert_eq!(diff.base, head);
    assert_eq!(diff.merge_base, head);
    use ChangeStatus::*;
    assert_eq!(
        statuses(&diff),
        vec![
            (s("a.txt"), None, Modified),
            (s("b.txt"), None, Deleted),
            (s("nested/dir/x.txt"), None, Added),
            (s("s.txt"), None, Added),
            (s("u.txt"), None, Added),
        ]
    );

    // Commit mode ignores local changes entirely.
    let committed = host_diff(&host, "HEAD", None, false, None).unwrap();
    assert!(committed.files.is_empty());
    assert_eq!(committed.head, Some(head));
    // The index is untouched: `s.txt` is still the only staged path.
    assert_eq!(sb.git(&host, &["diff", "--cached", "--name-only"]), "s.txt");
}

#[test]
fn kb_submodule_pointer_change_is_reported_separately_from_files() {
    let sb = sandbox();
    let kbsrc = sb.path().join("kbsrc");
    sb.init_repo(&kbsrc);
    write(&kbsrc.join("README.md"), "v1\n");
    let c1 = sb.commit_all(&kbsrc, "kb v1");
    write(&kbsrc.join("README.md"), "v2\n");
    let c2 = sb.commit_all(&kbsrc, "kb v2");

    let host = sb.path().join("sub-host");
    sb.init_repo(&host);
    write(&host.join("app/a.txt"), "a\n");
    sb.git(
        &host,
        &["submodule", "add", "-q", kbsrc.to_str().unwrap(), ".kb"],
    );
    sb.git(&host.join(".kb"), &["checkout", "-q", &c1]);
    sb.git(&host, &["add", ".kb", "app/a.txt"]);
    sb.git(&host, &["commit", "-q", "-m", "base with kb at v1"]);
    sb.git(&host, &["checkout", "-q", "-b", "feature"]);
    sb.git(&host.join(".kb"), &["checkout", "-q", &c2]);
    write(&host.join("app/a.txt"), "a changed\n");
    sb.git(&host, &["add", ".kb", "app/a.txt"]);
    sb.git(&host, &["commit", "-q", "-m", "bump kb"]);
    // `ignore = all` hides the gitlink from plain `git diff`; impact analysis must still
    // see the pointer change.
    sb.git(
        &host,
        &["config", "-f", ".gitmodules", "submodule..kb.ignore", "all"],
    );
    sb.git(&host, &["add", ".gitmodules"]);
    sb.git(&host, &["commit", "-q", "-m", "ignore kb in status"]);
    assert_eq!(
        sb.git(&host, &["diff", "--name-only", "main", "feature"]),
        ".gitmodules\napp/a.txt"
    );

    let diff = host_diff(&host, "main", Some("feature"), false, Some(".kb")).unwrap();
    let pointer = diff.kb_pointer.clone().expect("pointer change");
    assert_eq!(pointer.old.as_deref(), Some(c1.as_str()));
    assert_eq!(pointer.new.as_deref(), Some(c2.as_str()));
    assert_eq!(diff.kb_submodule.as_deref(), Some(".kb"));
    assert_eq!(
        statuses(&diff),
        vec![
            (s(".gitmodules"), None, ChangeStatus::Modified),
            (s("app/a.txt"), None, ChangeStatus::Modified),
        ]
    );

    // Without a configured KB path the gitlink is an ordinary changed file.
    let plain = host_diff(&host, "main", Some("feature"), false, None).unwrap();
    assert!(plain.kb_pointer.is_none());
    assert!(
        plain
            .files
            .iter()
            .any(|f| f.path == ".kb" && f.status == ChangeStatus::Modified)
    );

    // Working-tree mode reads the submodule checkout (Git reports a null id for that side).
    sb.git(&host, &["checkout", "-q", "main"]);
    assert_eq!(sb.git(&host.join(".kb"), &["rev-parse", "HEAD"]), c2);
    let local = host_diff(&host, "main", None, true, Some(".kb")).unwrap();
    let pointer = local.kb_pointer.clone().expect("local pointer change");
    assert_eq!(pointer.old.as_deref(), Some(c1.as_str()));
    assert_eq!(pointer.new.as_deref(), Some(c2.as_str()));
    assert!(local.files.is_empty(), "{:?}", local.files);

    // A pointer change needs no statement; declaring `none` contradicts the diff.
    let (registry, metas) = knowledge(&sb);
    let report = analyze(&diff, Some("mobile"), &registry, &metas);
    assert!(report.kb_change);
    let verdict = check(&report, None);
    assert!(
        verdict.ok && !verdict.acknowledgement_required,
        "{verdict:?}"
    );
    let none = parse_statement("<!-- kb-impact:v1\nkb_change = \"none\"\nreason = \"x\"\n-->")
        .unwrap()
        .unwrap();
    assert!(!check(&report, Some(&none)).ok);

    // `included` is checked against the diff when the KB submodule is known.
    let same = host_diff(&host, "main", Some("main"), false, Some(".kb")).unwrap();
    let empty = analyze(&same, Some("mobile"), &registry, &metas);
    let included =
        parse_statement("<!-- kb-impact:v1\nkb_change = \"included\"\nreason = \"x\"\n-->")
            .unwrap()
            .unwrap();
    let verdict = check(&empty, Some(&included));
    assert!(!verdict.ok, "{verdict:?}");
    assert!(verdict.reasons[0].contains("does not update the KB pointer"));
}

#[test]
fn malicious_and_invalid_inputs_are_rejected() {
    let sb = sandbox();
    let host = feature_host(&sb);
    let pwned = sb.path().join("pwned");
    let evil = format!("--output={}", pwned.display());

    for (base, head) in [
        (evil.as_str(), Some("feature")),
        ("main", Some(evil.as_str())),
        ("main..feature", None),
        ("main feature", None),
        ("-p", None),
        ("", None),
    ] {
        let err = host_diff(&host, base, head, false, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{base:?} {head:?}");
    }
    assert!(
        !pwned.exists(),
        "a revision argument was interpreted as an option"
    );

    let err = host_diff(&host, "main", Some("feature"), true, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    let err = host_diff(&host, "no-such-branch", None, false, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    for kb_path in ["../escape", "/abs", "a/../../b"] {
        let err = host_diff(&host, "main", None, false, Some(kb_path)).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsafePath, "{kb_path}");
    }

    let unrelated = sb.path().join("unrelated");
    sb.init_repo(&unrelated);
    sb.git(
        &unrelated,
        &["commit", "-q", "--allow-empty", "-m", "root a"],
    );
    sb.git(&unrelated, &["checkout", "-q", "--orphan", "other"]);
    sb.git(
        &unrelated,
        &["commit", "-q", "--allow-empty", "-m", "root b"],
    );
    let err = host_diff(&unrelated, "main", Some("other"), false, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(
        err.message.contains("no common ancestor"),
        "{}",
        err.message
    );

    let not_git = sb.path().join("plain-dir");
    fs::create_dir_all(&not_git).unwrap();
    let err = host_diff(&not_git, "main", None, false, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::GitError);
}

// ---------------------------------------------------------------------------------------
// analyze
// ---------------------------------------------------------------------------------------

#[test]
fn analysis_links_changed_files_to_knowledge() {
    let sb = sandbox();
    let host = feature_host(&sb);
    let (registry, metas) = knowledge(&sb);
    let diff = host_diff(&host, "main", Some("feature"), false, None).unwrap();
    let report = analyze(&diff, Some("mobile"), &registry, &metas);

    assert_eq!(report.repo.as_deref(), Some("mobile"));
    assert!(!report.kb_change);
    let token_api = ("acme.contract.token-api", "contract-repo-party");
    assert_eq!(
        links_of(&report, "app/auth/Session.kt"),
        pairs(&[
            token_api,
            ("acme.feature.login", "feature-record"),
            ("acme.feature.login", "feature-scope"),
            ("acme.mobile.session-refresh", "module-scope"),
            ("acme.mobile.session-refresh", "source-anchor"),
        ]),
        "superseded acme.mobile.old-session must be ignored"
    );
    assert!(
        links_of(&report, "app/auth/test/SessionTest.kt")
            .contains(&(s("acme.mobile.session-refresh"), s("test-anchor")))
    );
    assert_eq!(
        links_of(&report, "app/legacy/Login.kt"),
        pairs(&[token_api, ("acme.mobile.legacy-login", "source-anchor")])
    );
    assert_eq!(
        links_of(&report, "docs/new.md"),
        pairs(&[token_api, ("acme.mobile.legacy-login", "doc-anchor")]),
        "renames are linked through their old path"
    );
    assert_eq!(
        links_of(&report, "app/ui/Screen.kt"),
        pairs(&[token_api, ("acme.mobile.ui-guidelines", "path-selector")])
    );
    assert_eq!(
        links_of(&report, "src/api/Handler.kt"),
        pairs(&[token_api]),
        "a backend-scoped unqualified selector must not match a mobile path"
    );
    assert_eq!(
        links_of(&report, "tools/build.sh"),
        pairs(&[token_api, ("acme.tools.scripts", "path-selector")])
    );

    let session = report
        .files
        .iter()
        .find(|f| f.path == "app/auth/Session.kt")
        .unwrap();
    assert_eq!(session.modules, vec!["mobile.auth"]);
    assert_eq!(session.features, vec!["login"]);
    assert!(session.covered);
    let rename = report
        .files
        .iter()
        .find(|f| f.path == "docs/new.md")
        .unwrap();
    let doc_link = rename
        .links
        .iter()
        .find(|l| l.relation.as_str() == "doc-anchor")
        .unwrap();
    assert_eq!(doc_link.path.as_deref(), Some("docs/old.md"));
    assert_eq!(doc_link.via, "docs/old.md");

    // Unknown coverage: a repo-wide contract party alone does not describe a changed area,
    // and belonging to a declared module is not coverage either (the module is reported).
    let unknown: Vec<(&str, Vec<String>)> = report
        .unknown_coverage
        .iter()
        .map(|u| (u.path.as_str(), u.modules.clone()))
        .collect();
    assert_eq!(
        unknown,
        vec![
            ("app/settings/Prefs.kt", vec![s("mobile.settings")]),
            ("src/api/Handler.kt", vec![]),
        ]
    );

    let affected: Vec<&str> = report.affected.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(
        affected,
        vec![
            "acme.contract.token-api",
            "acme.feature.login",
            "acme.mobile.legacy-login",
            "acme.mobile.session-refresh",
            "acme.mobile.ui-guidelines",
            "acme.tools.scripts",
        ]
    );
    let contract = &report.affected[0];
    assert_eq!(contract.files.len(), report.files.len());
    assert_eq!(
        contract.record_path,
        "project/knowledge/contracts/token-api.md"
    );
    let session_rec = &report.affected[3];
    assert_eq!(
        session_rec
            .relations
            .iter()
            .map(|r| r.as_str())
            .collect::<Vec<_>>(),
        vec!["source-anchor", "test-anchor", "module-scope"]
    );
    assert_eq!(
        session_rec.files,
        vec!["app/auth/Session.kt", "app/auth/test/SessionTest.kt"]
    );

    let stale: Vec<(&str, AnchorKind, &str, ChangeStatus, Option<&str>)> = report
        .stale_anchors
        .iter()
        .map(|a| {
            (
                a.record.as_str(),
                a.kind,
                a.path.as_str(),
                a.change,
                a.renamed_to.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        stale,
        vec![
            (
                "acme.mobile.legacy-login",
                AnchorKind::Source,
                "app/legacy/Login.kt",
                ChangeStatus::Deleted,
                None
            ),
            (
                "acme.mobile.legacy-login",
                AnchorKind::Doc,
                "docs/old.md",
                ChangeStatus::Renamed,
                Some("docs/new.md")
            ),
        ]
    );
}

#[test]
fn analysis_is_independent_of_record_order() {
    let sb = sandbox();
    let host = feature_host(&sb);
    let (registry, metas) = knowledge(&sb);
    let diff = host_diff(&host, "main", Some("feature"), false, None).unwrap();
    let a = analyze(&diff, Some("mobile"), &registry, &metas);
    let mut reversed = metas.clone();
    reversed.reverse();
    let b = analyze(&diff, Some("mobile"), &registry, &reversed);
    assert_eq!(a, b);
    for f in &a.files {
        let mut sorted = f.links.clone();
        sorted.sort();
        assert_eq!(f.links, sorted);
    }
}

#[test]
fn unknown_host_repo_evaluates_only_repo_independent_links() {
    let sb = sandbox();
    let host = feature_host(&sb);
    let (registry, metas) = knowledge(&sb);
    let diff = host_diff(&host, "main", Some("feature"), false, None).unwrap();
    let report = analyze(&diff, None, &registry, &metas);

    assert!(report.repo.is_none());
    assert!(report.stale_anchors.is_empty());
    let covered: Vec<&str> = report
        .files
        .iter()
        .filter(|f| f.covered)
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(covered, vec!["tools/build.sh"]);
    assert_eq!(report.unknown_coverage.len(), report.files.len() - 1);
    assert!(report.files.iter().all(|f| f.modules.is_empty()));
    let text = render(&report, None, Format::Compact);
    assert!(text.contains("repo unknown"), "{text}");
    assert!(text.contains("host repo not identified"), "{text}");
}

// ---------------------------------------------------------------------------------------
// Statements and verdicts
// ---------------------------------------------------------------------------------------

fn block(body: &str) -> String {
    format!("## Summary\n\nCode change.\n\n<!-- kb-impact:v1\n{body}\n-->\n\nTrailing text.\n")
}

#[test]
fn statement_parsing_accepts_valid_blocks() {
    assert_eq!(
        parse_statement("No block in this description.").unwrap(),
        None
    );
    assert_eq!(
        parse_statement("<!-- an unrelated comment --> text").unwrap(),
        None
    );

    let none = parse_statement(&block(
        "kb_change = \"none\"\nreason = \"  Pure refactoring; no behavior change.  \"",
    ))
    .unwrap()
    .unwrap();
    assert_eq!(none.kb_change, KbChangeKind::None);
    assert_eq!(none.reason, "Pure refactoring; no behavior change.");
    assert_eq!(none.kb_revision, None);

    let linked = parse_statement(&block(
        "kb_change = \"linked\"\nreason = \"Knowledge lands first.\"\nkb_revision = \"0123abcd\"\nchange_id = \"PROJ-123\"",
    ))
    .unwrap()
    .unwrap();
    assert_eq!(linked.kb_change, KbChangeKind::Linked);
    assert_eq!(linked.kb_revision.as_deref(), Some("0123abcd"));
    assert_eq!(linked.change_id.as_deref(), Some("PROJ-123"));

    let by_change_id = parse_statement(&block(
        "kb_change = \"linked\"\nreason = \"Shared id.\"\nchange_id = \"PROJ-9\"",
    ))
    .unwrap()
    .unwrap();
    assert_eq!(by_change_id.kb_revision, None);

    let included = parse_statement(
        "<!--kb-impact:v1 kb_change = \"included\"\nreason = \"KB pointer bumped.\" -->",
    )
    .unwrap()
    .unwrap();
    assert_eq!(included.kb_change, KbChangeKind::Included);
}

#[test]
fn statement_parsing_rejects_malformed_blocks() {
    let cases: [(String, &str); 11] = [
        (block("kb_change = \"none\""), "reason"),
        (block("kb_change = \"none\"\nreason = \"   \""), "reason"),
        (
            block("kb_change = \"none\"\nreason = \"x\"\napproved = true"),
            "approved",
        ),
        (
            block("kb_change = \"none\"\nreason = \"x\"\nreason = \"y\""),
            "",
        ),
        (block("kb_change = \"maybe\"\nreason = \"x\""), ""),
        (
            block("kb_change = \"linked\"\nreason = \"Separate MR.\""),
            "requires `kb_revision` or `change_id`",
        ),
        (
            block("kb_change = \"linked\"\nreason = \"x\"\nkb_revision = \"--output=/tmp/x\""),
            "kb_revision",
        ),
        (
            block("kb_change = \"none\"\nreason = \"x\"\nkb_revision = \"0123abcd\""),
            "contradicts",
        ),
        (
            format!(
                "{}{}",
                block("kb_change = \"none\"\nreason = \"a\""),
                block("kb_change = \"none\"\nreason = \"b\"")
            ),
            "exactly one",
        ),
        (
            "<!-- kb-impact:v2\nkb_change = \"none\"\nreason = \"x\"\n-->".into(),
            "unsupported",
        ),
        (
            "<!-- kb-impact:v1\nkb_change = \"none\"\nreason = \"x\"\n".into(),
            "unterminated",
        ),
    ];
    for (text, needle) in cases {
        let err = parse_statement(&text).expect_err(&text);
        assert!(err.contains(needle), "{err:?} should mention {needle:?}");
    }
}

#[test]
fn check_requires_an_acknowledgement_without_judging_it() {
    let sb = sandbox();
    let host = feature_host(&sb);
    let (registry, metas) = knowledge(&sb);
    let diff = host_diff(&host, "main", Some("feature"), false, None).unwrap();
    let report = analyze(&diff, Some("mobile"), &registry, &metas);

    let missing = check(&report, None);
    assert!(!missing.ok);
    assert!(missing.acknowledgement_required);
    assert!(
        missing.reasons[0].contains("not acknowledged"),
        "{missing:?}"
    );

    for body in [
        "kb_change = \"none\"\nreason = \"Build tooling only.\"",
        "kb_change = \"linked\"\nreason = \"KB MR !42.\"\nchange_id = \"PROJ-1\"",
        "kb_change = \"included\"\nreason = \"KB lives outside the host.\"",
    ] {
        let statement = parse_statement(&block(body)).unwrap().unwrap();
        let verdict = check(&report, Some(&statement));
        assert!(verdict.ok, "{body}: {verdict:?}");
        assert!(verdict.acknowledgement_required);
        assert_eq!(verdict.statement.as_ref(), Some(&statement));
    }
    let linked = parse_statement(&block(
        "kb_change = \"linked\"\nreason = \"x\"\nkb_revision = \"abcdef1\"",
    ))
    .unwrap()
    .unwrap();
    assert!(check(&report, Some(&linked)).reasons[0].contains("not verified"));

    // Nothing changed: nothing to acknowledge.
    let same = host_diff(&host, "feature", Some("feature"), false, None).unwrap();
    let empty = analyze(&same, Some("mobile"), &registry, &metas);
    assert!(empty.files.is_empty());
    let verdict = check(&empty, None);
    assert!(verdict.ok && !verdict.acknowledgement_required);
}

#[test]
fn rendering_is_deterministic_and_states_its_limits() {
    let sb = sandbox();
    let host = feature_host(&sb);
    let (registry, metas) = knowledge(&sb);
    let diff = host_diff(&host, "main", Some("feature"), false, None).unwrap();
    let report = analyze(&diff, Some("mobile"), &registry, &metas);
    let verdict = check(&report, None);

    let value = to_json(&report, Some(&verdict));
    assert_eq!(value, to_json(&report, Some(&verdict)));
    assert_eq!(value["summary"]["files"], 8);
    assert_eq!(value["summary"]["covered"], 6);
    assert_eq!(value["summary"]["unknown_coverage"], 2);
    assert_eq!(value["summary"]["affected"], 6);
    assert_eq!(value["summary"]["stale_anchors"], 2);
    assert_eq!(value["diff"]["working_tree"], false);
    assert_eq!(value["files"][5]["status"], "renamed");
    assert_eq!(value["files"][5]["old_path"], "docs/old.md");
    assert_eq!(value["stale_anchors"][1]["renamed_to"], "docs/new.md");
    assert_eq!(value["check"]["ok"], false);
    assert_eq!(value["check"]["reasoning_evaluated"], false);
    assert_eq!(value["check"]["note"], REASONING_NOTE);
    assert_eq!(value["notes"][0], UNKNOWN_COVERAGE_NOTE);
    assert!(to_json(&report, None)["check"].is_null());

    let json_text = render(&report, Some(&verdict), Format::Json);
    let reparsed: serde_json::Value = serde_json::from_str(&json_text).unwrap();
    assert_eq!(reparsed, value);

    let compact = render(&report, Some(&verdict), Format::Compact);
    assert_eq!(compact, render(&report, Some(&verdict), Format::Compact));
    for needle in [
        "summary files=8 covered=6 unknown=2 affected=6 stale-anchors=2",
        "file R docs/new.md <- docs/old.md",
        "unknown A app/settings/Prefs.kt modules=mobile.settings",
        "stale acme.mobile.legacy-login doc-anchor mobile:docs/old.md renamed -> docs/new.md",
        "check failed acknowledgement-required=true",
        REASONING_NOTE,
        UNKNOWN_COVERAGE_NOTE,
    ] {
        assert!(
            compact.contains(needle),
            "missing {needle:?} in:\n{compact}"
        );
    }

    let human = render(&report, Some(&verdict), Format::Human);
    for needle in [
        "Changed files (8)",
        "Affected knowledge (6)",
        "Unknown coverage (2)",
        "Stale anchors (2)",
        "Acknowledgement: FAILED (required)",
        REASONING_NOTE,
    ] {
        assert!(human.contains(needle), "missing {needle:?} in:\n{human}");
    }
}
