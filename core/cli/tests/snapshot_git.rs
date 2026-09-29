//! Freshness, snapshot selection, proposal overlays, host detection and `kb sync` against
//! real local Git repositories. Remotes are local bare repositories addressed by file path;
//! the only non-file URL (credential redaction) points at a refused local port.
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;

use common::{Sandbox, write, write_min_project};
use kb::corpus;
use kb::env::Env;
use kb::error::{ErrorCode, KbError};
use kb::host::{self, HostContext, RepoSource};
use kb::index::Index;
use kb::knowledge::{Freshness, KnowledgeView, Origin, OverlayStatus, PinSource, Selection};
use kb::model::{Profile, ProfileLocation};
use kb::snapshot::{self, ResolvedSnapshot, SelectionRequest, SnapshotRequest};
use kb::source::{SourceTree, WorkingTreeSource};
use kb::util::sha256_hex;
use kb::versions::{ENGINE_VERSION, ReleaseManifest};

const TOKEN_STORAGE: &str = "project/knowledge/policies/token-storage.md";
const TOKEN_API: &str = "project/knowledge/contracts/token-api.md";

/// In-process library calls spawn `git` with this process's environment. Isolate it from
/// the user's Git configuration, credentials and proxies once, before any Git runs.
fn isolate_process_env() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("snapshot_git_home");
        fs::create_dir_all(&home).unwrap();
        // SAFETY: every test calls this before spawning any process, and `Once` blocks
        // concurrent callers until the environment is fully set.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            std::env::set_var("GIT_TERMINAL_PROMPT", "0");
            std::env::set_var("NO_PROXY", "*");
            std::env::set_var("no_proxy", "*");
            for k in [
                "KB_ROOT",
                "KB_CACHE_DIR",
                "GIT_DIR",
                "GIT_WORK_TREE",
                "http_proxy",
                "https_proxy",
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "all_proxy",
            ] {
                std::env::remove_var(k);
            }
        }
    });
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// A bare `origin` with one approved commit (the minimal project) and a developer clone.
struct World {
    sb: Sandbox,
    origin: PathBuf,
    kb: PathBuf,
    cache: PathBuf,
    /// The initial approved commit.
    c1: String,
}

impl World {
    fn new() -> World {
        isolate_process_env();
        let sb = Sandbox::new();
        let origin = sb.path().join("origin.git");
        sb.init_bare(&origin);
        let seed = sb.path().join("seed");
        sb.init_repo(&seed);
        write_min_project(&seed);
        write(&seed.join(".gitignore"), ".cache/\n");
        let c1 = sb.commit_all(&seed, "initial knowledge");
        sb.git(&seed, &["push", "-q", s(&origin), "main"]);
        let kb = sb.path().join("kb");
        sb.git(&sb.path(), &["clone", "-q", s(&origin), s(&kb)]);
        let cache = sb.path().join("cache");
        World {
            sb,
            origin,
            kb,
            cache,
            c1,
        }
    }

    fn env(&self) -> Env {
        env_for(&self.kb, &self.kb, &self.cache)
    }

    /// Another developer commits `files` and pushes them to the approved branch.
    fn push_approved(&self, name: &str, files: &[(&str, &str)]) -> String {
        let dev = self.sb.path().join(format!("dev-{name}"));
        self.sb
            .git(&self.sb.path(), &["clone", "-q", s(&self.origin), s(&dev)]);
        for (rel, text) in files {
            write(&dev.join(rel), text);
        }
        let c = self.sb.commit_all(&dev, name);
        self.sb.git(&dev, &["push", "-q", "origin", "main"]);
        c
    }

    /// A host repository with the KB added as a submodule at `.kb` (pinned to origin's tip).
    fn host_with_submodule(&self, name: &str) -> PathBuf {
        let host = self.sb.path().join(name);
        self.sb.init_repo(&host);
        write(&host.join("app/auth/Token.kt"), "class Token\n");
        self.sb.commit_all(&host, "host code");
        self.sb
            .git(&host, &["submodule", "add", "-q", s(&self.origin), ".kb"]);
        self.sb.commit_all(&host, "add kb submodule");
        host
    }
}

fn env_for(kb_root: &Path, cwd: &Path, cache: &Path) -> Env {
    let kb_root = kb_root.canonicalize().unwrap();
    let manifest =
        ReleaseManifest::parse(&fs::read_to_string(kb_root.join("core/release.toml")).unwrap())
            .unwrap();
    Env {
        kb_root,
        cwd: cwd.to_path_buf(),
        cache_dir: cache.to_path_buf(),
        quiet: true,
        manifest,
    }
}

fn loc() -> ProfileLocation {
    ProfileLocation::for_profile(Profile::Project)
}

fn req(selection: SelectionRequest, offline: bool, include_proposals: bool) -> SnapshotRequest {
    SnapshotRequest {
        selection,
        offline,
        include_proposals,
    }
}

fn resolve(
    env: &Env,
    host: Option<&HostContext>,
    selection: SelectionRequest,
    offline: bool,
) -> Result<ResolvedSnapshot, KbError> {
    snapshot::resolve(env, &loc(), host, &req(selection, offline, false))
}

fn err_of(r: Result<ResolvedSnapshot, KbError>) -> KbError {
    match r {
        Ok(snap) => panic!("expected an error, got snapshot {}", snap.info.label()),
        Err(e) => e,
    }
}

fn read_text(src: &dyn SourceTree, path: &str) -> String {
    String::from_utf8(src.read_path(path).unwrap().unwrap()).unwrap()
}

/// Everything a reading command must leave untouched in a checkout: HEAD, current branch,
/// all refs, index entries (including gitlinks), status, stashes, worktrees and file bytes.
fn checkout_state(sb: &Sandbox, dir: &Path) -> String {
    let mut out = String::new();
    let queries: [&[&str]; 7] = [
        &["rev-parse", "HEAD"],
        &["symbolic-ref", "-q", "HEAD"],
        &["for-each-ref", "--format=%(refname) %(objectname)"],
        &["ls-files", "--stage"],
        &["status", "--porcelain=v1", "--untracked-files=all"],
        &["stash", "list"],
        &["worktree", "list", "--porcelain"],
    ];
    for args in queries {
        let o = sb.git_output(dir, args);
        out.push_str(&format!(
            "$ git {args:?} -> {:?}\n{}\n",
            o.status.code(),
            String::from_utf8_lossy(&o.stdout)
        ));
    }
    for (path, digest) in file_digests(dir) {
        out.push_str(&format!("{path} {digest}\n"));
    }
    out
}

/// sha256 of every file below `dir`, skipping `.git` and the generated `.cache`.
fn file_digests(dir: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let name = e.file_name();
            if name == ".git" || name == ".cache" {
                continue;
            }
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().display().to_string();
            let ft = e.file_type().unwrap();
            if ft.is_symlink() {
                out.insert(
                    rel,
                    format!("symlink:{}", fs::read_link(&p).unwrap().display()),
                );
            } else if ft.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(rel, sha256_hex(&fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn mirrors(cache: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(cache.join("git"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "git"))
        .collect();
    v.sort();
    v
}

// ---------------------------------------------------------------------------------------
// Freshness and selection
// ---------------------------------------------------------------------------------------

#[test]
fn latest_is_verified_and_read_from_the_mirror_without_touching_the_checkout() {
    let w = World::new();
    let c2 = w.push_approved(
        "new-gap",
        &[(
            "project/knowledge/gaps/offline.md",
            "+++\nschema = 1\n+++\n",
        )],
    );
    let env = w.env();
    let before = checkout_state(&w.sb, &w.kb);

    let snap = resolve(&env, None, SelectionRequest::Auto, false).unwrap();
    let info = &snap.info;
    assert_eq!(info.selection, Selection::Latest);
    assert_eq!(info.freshness, Freshness::Verified);
    assert_eq!(info.revision.as_deref(), Some(c2.as_str()));
    assert_eq!(info.latest_approved.as_deref(), Some(c2.as_str()));
    assert_eq!(info.approved, Some(true));
    assert_eq!(info.pin, None);
    assert_eq!(info.source, s(&w.origin));
    assert_eq!(info.approved_ref, "refs/heads/main");
    assert_eq!(info.engine_version, ENGINE_VERSION);
    assert_eq!(snap.config.project.namespace, "acme");
    assert!(snap.overlay.is_none());

    // The approved content is read from Git objects; the checkout does not have the file.
    let (entries, issues) = snap
        .source
        .list(&loc().content_prefixes(&snap.config))
        .unwrap();
    assert!(issues.is_empty(), "{issues:?}");
    let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.contains(&"project/knowledge/gaps/offline.md"));
    assert!(paths.windows(2).all(|p| p[0] < p[1]), "sorted: {paths:?}");
    assert!(entries.iter().all(|e| e.content_id.starts_with("git:")));
    assert!(!w.kb.join("project/knowledge/gaps/offline.md").exists());
    assert_eq!(
        snap.source.read(&entries[..1]).unwrap()[0],
        fs::read(w.kb.join(&entries[0].path)).unwrap()
    );

    // Deterministic key; the KB checkout got no refs and no changes.
    let again = resolve(&env, None, SelectionRequest::Latest, false).unwrap();
    assert_eq!(again.info.key, info.key);
    assert_eq!(checkout_state(&w.sb, &w.kb), before);
    assert!(
        !w.sb
            .git(&w.kb, &["for-each-ref", "--format=%(refname)"])
            .contains("refs/kb/")
    );
    assert_eq!(mirrors(&w.cache).len(), 1);
}

#[test]
fn remote_failure_is_freshness_unverified_and_offline_uses_the_last_fetch() {
    let w = World::new();
    let env = w.env();
    resolve(&env, None, SelectionRequest::Auto, false).unwrap();
    w.push_approved(
        "later",
        &[(TOKEN_API, "+++\nschema = 1\n+++\nchanged upstream\n")],
    );
    fs::rename(&w.origin, w.sb.path().join("origin.moved")).unwrap();
    let before = checkout_state(&w.sb, &w.kb);

    let e = err_of(resolve(&env, None, SelectionRequest::Auto, false));
    assert_eq!(e.code, ErrorCode::FreshnessUnverified);
    assert_eq!(e.exit_code(), 20);
    assert!(e.hint.as_deref().unwrap().contains("--offline"));
    assert_eq!(e.details["approved_ref"], "refs/heads/main");

    // No automatic fallback: only an explicit offline request uses the last fetch.
    let snap = resolve(&env, None, SelectionRequest::Auto, true).unwrap();
    assert_eq!(snap.info.freshness, Freshness::Unverified);
    assert_eq!(snap.info.revision.as_deref(), Some(w.c1.as_str()));
    assert_eq!(snap.info.latest_approved.as_deref(), Some(w.c1.as_str()));
    assert_eq!(snap.info.selection, Selection::Latest);
    assert!(snap.info.label().contains("freshness=unverified"));
    assert_eq!(checkout_state(&w.sb, &w.kb), before);
}

#[test]
fn offline_falls_back_to_the_tracking_ref_and_otherwise_finds_no_snapshot() {
    let w = World::new();
    // Fresh cache: nothing fetched yet, so the checkout's refs/remotes/origin/main is used.
    let snap = resolve(&w.env(), None, SelectionRequest::Latest, true).unwrap();
    assert_eq!(snap.info.revision.as_deref(), Some(w.c1.as_str()));
    assert_eq!(snap.info.freshness, Freshness::Unverified);

    // A checkout that never fetched its remote has no approved revision offline.
    let lone = w.sb.path().join("lone");
    w.sb.init_repo(&lone);
    write_min_project(&lone);
    w.sb.commit_all(&lone, "local only");
    w.sb.git(&lone, &["remote", "add", "origin", s(&w.origin)]);
    let env = env_for(&lone, &lone, &w.sb.path().join("cache-lone"));
    let e = err_of(resolve(&env, None, SelectionRequest::Auto, true));
    assert_eq!(e.code, ErrorCode::SnapshotNotFound);
    // Working-tree selection still works offline and is never approved.
    let wt = resolve(&env, None, SelectionRequest::WorkingTree, true).unwrap();
    assert_eq!(wt.info.approved, Some(false));
    assert_eq!(wt.info.latest_approved, None);
}

#[test]
fn uninitialized_project_is_reported_before_any_git_access() {
    let w = World::new();
    fs::remove_file(w.kb.join("project/project.toml")).unwrap();
    let e = err_of(resolve(&w.env(), None, SelectionRequest::Auto, false));
    assert_eq!(e.code, ErrorCode::ProjectNotInitialized);
    assert!(!w.cache.exists());
}

#[test]
fn stale_host_pin_requires_update_under_auto_and_pinned_reports_both_revisions() {
    let w = World::new();
    let host = w.host_with_submodule("host");
    let kb_root = host.join(".kb");
    let c2 = w.push_approved("newer", &[(TOKEN_API, "+++\nschema = 1\n+++\nnewer\n")]);
    let env = env_for(&kb_root, &kb_root, &w.cache);
    let before = (
        checkout_state(&w.sb, &host),
        checkout_state(&w.sb, &kb_root),
    );

    let hc = host::detect(&env, None)
        .unwrap()
        .expect("superproject is the host");
    assert_eq!(hc.root, host.canonicalize().unwrap());
    assert!(!hc.linked_worktree);
    assert_eq!(hc.kb_submodule_path.as_deref(), Some(".kb"));
    let pin = hc.pin.clone().unwrap();
    assert_eq!(pin.revision, w.c1);
    assert_eq!(pin.source, PinSource::Submodule);
    assert_eq!(pin.path.as_deref(), Some(".kb"));

    let e = err_of(resolve(&env, Some(&hc), SelectionRequest::Auto, false));
    assert_eq!(e.code, ErrorCode::UpdateRequired);
    assert_eq!(e.details["pin"]["revision"], w.c1.as_str());
    assert_eq!(e.details["latest_approved"], c2.as_str());

    let pinned = resolve(&env, Some(&hc), SelectionRequest::Pinned, false).unwrap();
    assert_eq!(pinned.info.selection, Selection::Pinned);
    assert_eq!(pinned.info.revision.as_deref(), Some(w.c1.as_str()));
    assert_eq!(pinned.info.latest_approved.as_deref(), Some(c2.as_str()));
    assert_eq!(pinned.info.approved, Some(true));
    assert_eq!(pinned.info.freshness, Freshness::Verified);
    assert!(!read_text(pinned.source.as_ref(), TOKEN_API).contains("newer"));

    let latest = resolve(&env, Some(&hc), SelectionRequest::Latest, false).unwrap();
    assert_eq!(latest.info.selection, Selection::Latest);
    assert_eq!(latest.info.revision.as_deref(), Some(c2.as_str()));
    assert_eq!(latest.info.pin.as_ref().unwrap().revision, w.c1);
    assert!(read_text(latest.source.as_ref(), TOKEN_API).contains("newer"));
    assert_ne!(latest.info.key, pinned.info.key);

    // Offline pinned mode needs no remote and still reports the last known tip.
    let offline = resolve(&env, Some(&hc), SelectionRequest::Pinned, true).unwrap();
    assert_eq!(offline.info.revision.as_deref(), Some(w.c1.as_str()));
    assert_eq!(offline.info.latest_approved.as_deref(), Some(c2.as_str()));
    assert_eq!(offline.info.freshness, Freshness::Unverified);

    assert_eq!(
        (
            checkout_state(&w.sb, &host),
            checkout_state(&w.sb, &kb_root)
        ),
        before
    );
}

#[test]
fn unpushed_pin_is_copied_from_the_checkout_and_marked_not_approved() {
    let w = World::new();
    let host = w.host_with_submodule("host");
    let kb_root = host.join(".kb");
    write(
        &kb_root.join(TOKEN_API),
        "+++\nschema = 1\n+++\nlocal only\n",
    );
    let local = w.sb.commit_all(&kb_root, "unreviewed");
    w.sb.commit_all(&host, "pin unreviewed kb commit");
    let env = env_for(&kb_root, &host, &w.cache);
    let hc = host::detect(&env, None).unwrap().unwrap();
    assert_eq!(hc.pin.as_ref().unwrap().revision, local);
    let before = (
        checkout_state(&w.sb, &host),
        checkout_state(&w.sb, &kb_root),
    );

    let e = err_of(resolve(&env, Some(&hc), SelectionRequest::Auto, false));
    assert_eq!(e.code, ErrorCode::UpdateRequired);
    let snap = resolve(&env, Some(&hc), SelectionRequest::Pinned, false).unwrap();
    assert_eq!(snap.info.revision.as_deref(), Some(local.as_str()));
    assert_eq!(snap.info.approved, Some(false));
    assert!(read_text(snap.source.as_ref(), TOKEN_API).contains("local only"));

    let report = snapshot::sync(&env, &loc(), Some(&hc)).unwrap();
    let hs = report.host.unwrap();
    assert_eq!(hs.status, snapshot::PinStatus::Ahead);
    assert_eq!((hs.ahead, hs.behind), (Some(1), Some(0)));
    assert_eq!(report.local.ahead, Some(1));
    assert!(report.checkout_unchanged);
    assert_eq!(
        (
            checkout_state(&w.sb, &host),
            checkout_state(&w.sb, &kb_root)
        ),
        before
    );
}

#[test]
fn host_binding_file_selection_pin_and_repo_are_honored() {
    let w = World::new();
    let host = w.host_with_submodule("host");
    let kb_root = host.join(".kb");
    let c2 = w.push_approved("newer", &[(TOKEN_API, "+++\nschema = 1\n+++\nnewer\n")]);
    let env = env_for(&kb_root, &kb_root, &w.cache);

    write(
        &host.join(".kbw.toml"),
        "schema = 1\nselection = \"latest\"\n",
    );
    let hc = host::detect(&env, None).unwrap().unwrap();
    let snap = resolve(&env, Some(&hc), SelectionRequest::Auto, false).unwrap();
    assert_eq!(snap.info.selection, Selection::Latest);
    assert_eq!(snap.info.revision.as_deref(), Some(c2.as_str()));

    write(
        &host.join(".kbw.toml"),
        "schema = 1\nselection = \"pinned\"\n",
    );
    let hc = host::detect(&env, None).unwrap().unwrap();
    let snap = resolve(&env, Some(&hc), SelectionRequest::Auto, false).unwrap();
    assert_eq!(snap.info.selection, Selection::Pinned);
    assert_eq!(snap.info.revision.as_deref(), Some(w.c1.as_str()));

    write(&host.join(".kbw.toml"), "schema = 1\nsurprise = true\n");
    assert_eq!(
        host::detect(&env, None).unwrap_err().code,
        ErrorCode::ConfigInvalid
    );
    fs::remove_file(host.join(".kbw.toml")).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(w.kb.join("project/project.toml"), host.join(".kbw.toml"))
            .unwrap();
        assert_eq!(
            host::detect(&env, None).unwrap_err().code,
            ErrorCode::UnsafePath
        );
        fs::remove_file(host.join(".kbw.toml")).unwrap();
    }

    // A separate (non-submodule) checkout pinned by `.kbw.toml`, host given with --host.
    let plain = w.sb.path().join("plain-host");
    w.sb.init_repo(&plain);
    let short: String = w.c1.chars().take(10).collect();
    write(
        &plain.join(".kbw.toml"),
        &format!("schema = 1\nrepo = \"backend\"\npin = \"{short}\"\n"),
    );
    w.sb.commit_all(&plain, "bind kb");
    let env = env_for(&w.kb, &w.kb, &w.cache);
    let hc = host::detect(&env, Some(&plain)).unwrap().unwrap();
    let pin = hc.pin.clone().unwrap();
    assert_eq!(pin.revision, w.c1, "abbreviated pin is expanded");
    assert_eq!(pin.source, PinSource::BindingFile);
    assert_eq!(hc.kb_submodule_path, None);
    let e = err_of(resolve(&env, Some(&hc), SelectionRequest::Auto, false));
    assert_eq!(e.code, ErrorCode::UpdateRequired);
    let snap = resolve(&env, Some(&hc), SelectionRequest::Pinned, false).unwrap();
    assert_eq!(snap.info.revision.as_deref(), Some(w.c1.as_str()));

    let (registry, _) = corpus::load_registry(&WorkingTreeSource::new(&w.kb), &loc()).unwrap();
    assert_eq!(
        host::identify_repo(&hc, &registry),
        Some(("backend".to_string(), RepoSource::BindingFile))
    );
}

#[test]
fn working_tree_selection_reads_local_files_and_is_keyed_by_content() {
    let w = World::new();
    let env = w.env();
    let original = fs::read_to_string(w.kb.join(TOKEN_STORAGE)).unwrap();
    write(
        &w.kb.join(TOKEN_STORAGE),
        &format!("{original}\nlocal edit one\n"),
    );
    let before = checkout_state(&w.sb, &w.kb);

    let one = resolve(&env, None, SelectionRequest::WorkingTree, false).unwrap();
    assert_eq!(one.info.selection, Selection::WorkingTree);
    assert_eq!(one.info.revision, None);
    assert_eq!(one.info.approved, Some(false));
    assert_eq!(one.info.freshness, Freshness::Verified);
    assert_eq!(one.info.latest_approved.as_deref(), Some(w.c1.as_str()));
    assert!(read_text(one.source.as_ref(), TOKEN_STORAGE).contains("local edit one"));
    assert_eq!(
        resolve(&env, None, SelectionRequest::WorkingTree, false)
            .unwrap()
            .info
            .key,
        one.info.key
    );
    assert_eq!(checkout_state(&w.sb, &w.kb), before);

    write(
        &w.kb.join(TOKEN_STORAGE),
        &format!("{original}\nlocal edit two\n"),
    );
    let two = resolve(&env, None, SelectionRequest::WorkingTree, true).unwrap();
    assert_ne!(two.info.key, one.info.key);
    let latest = resolve(&env, None, SelectionRequest::Latest, true).unwrap();
    assert_ne!(latest.info.key, two.info.key);
}

#[test]
fn working_tree_snapshots_are_built_from_the_listing_that_keyed_them() {
    let w = World::new();
    let env = w.env();
    let file = w.kb.join(TOKEN_STORAGE);
    let original = fs::read_to_string(&file).unwrap();
    let id = "acme.mobile.token-storage";
    let snap = resolve(&env, None, SelectionRequest::WorkingTree, true).unwrap();
    // An editor saves between the key computation and the index build.
    write(
        &file,
        &original.replace("title = \"Token storage\"", "title = \"EDITED MID-CALL\""),
    );
    let mut ix = Index::open(&w.cache, Profile::Project).unwrap();
    let err = ix
        .ensure(&snap.info.key, snap.source.as_ref(), &loc(), None)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::IoError, "{err}");
    assert!(err.message.contains(TOKEN_STORAGE), "{err}");
    assert!(!ix.has_snapshot(&snap.info.key).unwrap());
    assert!(snap.source.read_path(TOKEN_STORAGE).is_err());

    // The edit is undone: the same key now gets the original content.
    write(&file, &original);
    let again = resolve(&env, None, SelectionRequest::WorkingTree, true).unwrap();
    assert_eq!(again.info.key, snap.info.key);
    let built = ix
        .ensure(&again.info.key, again.source.as_ref(), &loc(), None)
        .unwrap();
    assert!(!built.reused);
    {
        let v = ix.view(&again.info.key).unwrap();
        let raw = v.raw(id, Origin::Accepted).unwrap().unwrap();
        assert_eq!(&*raw.text, original.as_str());
    }

    // A record added after the listing is not part of the keyed snapshot; the build
    // still matches the key exactly.
    let later = resolve(&env, None, SelectionRequest::WorkingTree, true).unwrap();
    let added = "project/knowledge/refs/added-later.md";
    write(
        &w.kb.join(added),
        &original.replace(id, "acme.mobile.added-later"),
    );
    ix.gc(0).unwrap();
    ix.ensure(&later.info.key, later.source.as_ref(), &loc(), None)
        .unwrap();
    let v = ix.view(&later.info.key).unwrap();
    assert!(
        v.raw("acme.mobile.added-later", Origin::Accepted)
            .unwrap()
            .is_none()
    );
    assert!(v.raw(id, Origin::Accepted).unwrap().is_some());
}

#[test]
fn incompatible_engine_on_the_remote_requires_update_before_reading_knowledge() {
    let w = World::new();
    let manifest = fs::read_to_string(w.kb.join("core/release.toml")).unwrap();
    let bumped = manifest.replace(
        &format!("engine_version = \"{ENGINE_VERSION}\""),
        "engine_version = \"9.9.9\"",
    );
    assert_ne!(bumped, manifest);
    // The project config at that revision is garbage: it must never be read.
    let c2 = w.push_approved(
        "engine-9",
        &[
            ("core/release.toml", bumped.as_str()),
            ("project/project.toml", "this is not toml ["),
        ],
    );
    let env = w.env();
    let e = err_of(resolve(&env, None, SelectionRequest::Auto, false));
    assert_eq!(e.code, ErrorCode::UpdateRequired);
    assert_eq!(e.details["revision"], c2.as_str());
    assert_eq!(e.details["mismatches"][0]["field"], "engine_version");

    // An explicit older revision is still usable and approved (it precedes the tip).
    let old = resolve(&env, None, SelectionRequest::Revision(w.c1.clone()), false).unwrap();
    assert_eq!(old.info.selection, Selection::Revision);
    assert_eq!(old.info.approved, Some(true));
    assert_eq!(old.info.latest_approved.as_deref(), Some(c2.as_str()));
    let by_name = resolve(
        &env,
        None,
        SelectionRequest::Revision("origin/main".into()),
        false,
    )
    .unwrap();
    assert_eq!(by_name.info.revision.as_deref(), Some(w.c1.as_str()));
    let missing = err_of(resolve(
        &env,
        None,
        SelectionRequest::Revision("no-such-branch".into()),
        false,
    ));
    assert_eq!(missing.code, ErrorCode::SnapshotNotFound);
}

// ---------------------------------------------------------------------------------------
// Proposal overlay
// ---------------------------------------------------------------------------------------

#[test]
fn proposal_overlay_covers_every_local_change_kind_and_flags_stale_edits() {
    let w = World::new();
    let kb = &w.kb;
    // committed but not pushed
    write(
        &kb.join("project/knowledge/decisions/local.md"),
        "+++\nschema = 1\n+++\ncommitted locally\n",
    );
    w.sb.commit_all(kb, "local decision");
    let local_head = w.sb.git(kb, &["rev-parse", "HEAD"]);
    // staged
    write(&kb.join(TOKEN_STORAGE), "+++\nschema = 1\n+++\nstaged\n");
    w.sb.git(kb, &["add", TOKEN_STORAGE]);
    // unstaged edit of a file that the approved branch also changes (stale)
    write(&kb.join(TOKEN_API), "+++\nschema = 1\n+++\nunstaged\n");
    // untracked
    write(
        &kb.join("project/knowledge/gaps/new.md"),
        "+++\nschema = 1\n+++\nuntracked\n",
    );
    // deleted (unstaged)
    fs::remove_file(kb.join("project/registry/concepts.toml")).unwrap();
    // outside the profile content prefixes: ignored
    write(&kb.join("NOTES.txt"), "not knowledge\n");
    let c2 = w.push_approved(
        "upstream",
        &[(TOKEN_API, "+++\nschema = 1\n+++\nupstream\n")],
    );

    let env = w.env();
    let before = checkout_state(&w.sb, kb);
    let snap = snapshot::resolve(
        &env,
        &loc(),
        None,
        &req(SelectionRequest::Auto, false, true),
    )
    .unwrap();
    assert_eq!(snap.info.revision.as_deref(), Some(c2.as_str()));
    let ov = snap.overlay.as_ref().unwrap();
    assert_eq!(ov.base.as_deref(), Some(w.c1.as_str()));
    let got: Vec<(&str, OverlayStatus, bool)> = ov
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.status, f.stale))
        .collect();
    assert_eq!(
        got,
        vec![
            (TOKEN_API, OverlayStatus::Modified, true),
            (
                "project/knowledge/decisions/local.md",
                OverlayStatus::Added,
                false
            ),
            ("project/knowledge/gaps/new.md", OverlayStatus::Added, false),
            (TOKEN_STORAGE, OverlayStatus::Modified, false),
            (
                "project/registry/concepts.toml",
                OverlayStatus::Deleted,
                false
            ),
        ]
    );
    for f in &ov.files {
        match f.status {
            OverlayStatus::Deleted => assert!(f.content.is_none() && f.content_id.is_none()),
            _ => {
                let bytes = fs::read(kb.join(&f.path)).unwrap();
                assert_eq!(f.content.as_deref(), Some(bytes.as_slice()));
                assert_eq!(
                    f.content_id.as_deref(),
                    Some(format!("sha256:{}", sha256_hex(&bytes)).as_str())
                );
            }
        }
    }
    let info = snap.info.overlay.as_ref().unwrap();
    assert_eq!((info.files, info.digest.as_str()), (5, ov.digest.as_str()));
    let plain = resolve(&env, None, SelectionRequest::Auto, false).unwrap();
    assert_ne!(
        plain.info.key, snap.info.key,
        "overlay digest is part of the key"
    );
    assert_eq!(checkout_state(&w.sb, kb), before);

    // Without an approved tip the base is the local HEAD: the local commit is not a proposal.
    let local = kb::overlay::compute(&env, &loc(), &snap.config, None).unwrap();
    assert_eq!(local.base.as_deref(), Some(local_head.as_str()));
    assert!(local.files.iter().all(|f| !f.stale));
    assert!(
        !local
            .files
            .iter()
            .any(|f| f.path == "project/knowledge/decisions/local.md")
    );
    assert_eq!(local.files.len(), 4);
    assert_eq!(checkout_state(&w.sb, kb), before);
}

// ---------------------------------------------------------------------------------------
// Host detection
// ---------------------------------------------------------------------------------------

#[test]
fn linked_worktrees_of_the_host_carry_their_own_pins() {
    let w = World::new();
    let host = w.host_with_submodule("host");
    let kb_root = host.join(".kb");
    let c2 = w.push_approved("newer", &[(TOKEN_API, "+++\nschema = 1\n+++\nnewer\n")]);
    let wt = w.sb.path().join("host-feature");
    w.sb.git(&host, &["worktree", "add", "-q", s(&wt), "-b", "feature"]);
    w.sb.git(
        &wt,
        &["update-index", "--cacheinfo", &format!("160000,{c2},.kb")],
    );
    w.sb.git(&wt, &["commit", "-q", "-m", "bump kb pin"]);
    let before = (
        checkout_state(&w.sb, &host),
        checkout_state(&w.sb, &wt),
        checkout_state(&w.sb, &kb_root),
    );

    // From the linked worktree: its own HEAD pins c2.
    let env = env_for(&kb_root, &wt, &w.cache);
    let hc = host::detect(&env, None).unwrap().unwrap();
    assert_eq!(hc.root, wt.canonicalize().unwrap());
    assert!(hc.linked_worktree);
    assert_eq!(hc.git_common_dir, host.join(".git").canonicalize().unwrap());
    assert_eq!(hc.pin.as_ref().unwrap().revision, c2);
    let snap = resolve(&env, Some(&hc), SelectionRequest::Auto, false).unwrap();
    assert_eq!(snap.info.selection, Selection::Latest, "pin equals the tip");
    let pinned = resolve(&env, Some(&hc), SelectionRequest::Pinned, false).unwrap();
    assert_eq!(pinned.info.revision.as_deref(), Some(c2.as_str()));
    // --host selects the same worktree from anywhere.
    let elsewhere = env_for(&kb_root, &w.kb, &w.cache);
    let by_flag = host::detect(&elsewhere, Some(&wt)).unwrap().unwrap();
    assert_eq!(by_flag.pin, hc.pin);

    // From the main worktree: its HEAD still pins c1.
    let env = env_for(&kb_root, &kb_root, &w.cache);
    let main = host::detect(&env, None).unwrap().unwrap();
    assert!(!main.linked_worktree);
    assert_eq!(main.pin.as_ref().unwrap().revision, w.c1);
    let e = err_of(resolve(&env, Some(&main), SelectionRequest::Auto, false));
    assert_eq!(e.code, ErrorCode::UpdateRequired);

    assert_eq!(
        (
            checkout_state(&w.sb, &host),
            checkout_state(&w.sb, &wt),
            checkout_state(&w.sb, &kb_root),
        ),
        before
    );
}

#[test]
fn host_detection_uses_git_context_remotes_and_version_files() {
    let w = World::new();
    // A standalone KB checkout run from inside itself has no host.
    assert!(host::detect(&w.env(), None).unwrap().is_none());

    let app = w.sb.path().join("mobile-app");
    w.sb.init_repo(&app);
    w.sb.git(
        &app,
        &[
            "remote",
            "add",
            "origin",
            "https://token@example.invalid/acme/mobile.git",
        ],
    );
    write(&app.join("VERSION"), "v2.3.4\nrest ignored\n");
    w.sb.commit_all(&app, "app");
    let sub = app.join("src");
    fs::create_dir_all(&sub).unwrap();
    let env = env_for(&w.kb, &sub, &w.cache);
    let hc = host::detect(&env, None).unwrap().unwrap();
    assert_eq!(hc.root, app.canonicalize().unwrap());
    assert_eq!(hc.pin, None);
    assert_eq!(hc.binding, None);
    assert!(hc.head.is_some());
    assert_eq!(hc.remotes.len(), 1);
    // Remote URLs are never serialized.
    assert!(!serde_json::to_string(&hc).unwrap().contains("token@"));

    let repos = w.kb.join("project/registry/repos.toml");
    let text = fs::read_to_string(&repos).unwrap();
    let with_version = text.replace(
        "remotes = [\"example.invalid/acme/mobile\"]\n",
        "remotes = [\"example.invalid/acme/mobile\"]\nversion_file = \"VERSION\"\n",
    );
    assert_ne!(with_version, text);
    write(&repos, &with_version);
    let (registry, diags) = corpus::load_registry(&WorkingTreeSource::new(&w.kb), &loc()).unwrap();
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(
        host::identify_repo(&hc, &registry),
        Some(("mobile".to_string(), RepoSource::Remote))
    );
    assert_eq!(
        host::host_version(&hc, &registry, "mobile"),
        Some(semver::Version::new(2, 3, 4))
    );
    #[cfg(unix)]
    {
        fs::remove_file(app.join("VERSION")).unwrap();
        std::os::unix::fs::symlink(w.sb.path().join("elsewhere"), app.join("VERSION")).unwrap();
        write(&w.sb.path().join("elsewhere"), "1.0.0\n");
        assert_eq!(host::host_version(&hc, &registry, "mobile"), None);
    }

    let not_git = w.sb.path().join("not-git");
    fs::create_dir_all(&not_git).unwrap();
    assert_eq!(
        host::detect(&env, Some(&not_git)).unwrap_err().code,
        ErrorCode::InvalidInput
    );
}

// ---------------------------------------------------------------------------------------
// Cache isolation and transport safety
// ---------------------------------------------------------------------------------------

#[test]
fn caches_of_projects_with_identical_record_ids_stay_isolated() {
    let a = World::new();
    let b = World::new();
    b.push_approved(
        "b-edit",
        &[(TOKEN_STORAGE, "+++\nschema = 1\n+++\nproject b\n")],
    );
    let shared = a.sb.path().join("shared-cache");
    let env_a = env_for(&a.kb, &a.kb, &shared);
    let env_b = env_for(&b.kb, &b.kb, &shared);

    let ra = resolve(&env_a, None, SelectionRequest::Auto, false).unwrap();
    let rb = resolve(&env_b, None, SelectionRequest::Auto, false).unwrap();
    assert_ne!(ra.info.key, rb.info.key);
    assert_eq!(mirrors(&shared).len(), 2);
    assert!(!read_text(ra.source.as_ref(), TOKEN_STORAGE).contains("project b"));
    assert!(read_text(rb.source.as_ref(), TOKEN_STORAGE).contains("project b"));
    // Interleaved calls keep their identities.
    assert_eq!(
        resolve(&env_a, None, SelectionRequest::Auto, false)
            .unwrap()
            .info
            .key,
        ra.info.key
    );
}

#[test]
fn transport_policy_comes_from_the_local_config_and_blocks_other_protocols() {
    let w = World::new();
    let cfg = w.kb.join("project/project.toml");
    let text = fs::read_to_string(&cfg).unwrap();
    write(
        &cfg,
        &text.replace(
            "allowed_protocols = [\"file\"]",
            "allowed_protocols = [\"https\"]",
        ),
    );
    let e = err_of(resolve(&w.env(), None, SelectionRequest::Auto, false));
    assert_eq!(e.code, ErrorCode::FreshnessUnverified);
    assert!(e.message.contains("not allowed"), "{}", e.message);
}

#[test]
fn credentials_in_remote_urls_never_reach_errors_or_output() {
    let w = World::new();
    let cfg = w.kb.join("project/project.toml");
    let text = fs::read_to_string(&cfg).unwrap();
    write(
        &cfg,
        &text.replace(
            "allowed_protocols = [\"file\"]",
            "allowed_protocols = [\"https\", \"file\"]",
        ),
    );
    // Port 9 on the loopback interface refuses connections: no network is involved.
    w.sb.git(
        &w.kb,
        &[
            "remote",
            "set-url",
            "origin",
            "https://user:secret@127.0.0.1:9/x.git",
        ],
    );
    let e = err_of(resolve(&w.env(), None, SelectionRequest::Auto, false));
    assert_eq!(e.code, ErrorCode::FreshnessUnverified);
    let all = format!("{} {} {:?}", e.message, e.details, e.hint);
    assert!(!all.contains("secret"), "{all}");
    assert!(
        e.details["source"]
            .as_str()
            .unwrap()
            .contains("***@127.0.0.1:9")
    );

    let cache = w.cache.to_str().unwrap();
    let envs = [
        ("KB_CACHE_DIR", cache),
        ("NO_PROXY", "*"),
        ("no_proxy", "*"),
    ];
    for format in ["--json", "--format=human"] {
        let o = w.sb.kb(&w.kb, &["--root", s(&w.kb), format, "sync"], &envs);
        assert_eq!(o.status.code(), Some(20), "{}", common::stderr(&o));
        let out = format!("{}{}", common::stdout(&o), common::stderr(&o));
        assert!(!out.contains("secret"), "{out}");
        assert!(out.contains("FRESHNESS_UNVERIFIED"), "{out}");
    }
}

#[cfg(unix)]
#[test]
fn git_tree_source_rejects_symlinks_and_skips_gitlinks() {
    let w = World::new();
    let dev = w.sb.path().join("dev-links");
    w.sb.git(&w.sb.path(), &["clone", "-q", s(&w.origin), s(&dev)]);
    let link = "project/knowledge/policies/link.md";
    std::os::unix::fs::symlink("../contracts/token-api.md", dev.join(link)).unwrap();
    w.sb.git(&dev, &["add", link]);
    let gitlink = format!("160000,{},project/knowledge/vendored", w.c1);
    w.sb.git(&dev, &["update-index", "--add", "--cacheinfo", &gitlink]);
    w.sb.git(&dev, &["commit", "-q", "-m", "links"]);
    w.sb.git(&dev, &["push", "-q", "origin", "main"]);
    let c2 = w.sb.git(&dev, &["rev-parse", "HEAD"]);

    let snap = resolve(&w.env(), None, SelectionRequest::Latest, false).unwrap();
    let src = snap.source.as_ref();
    assert_eq!(src.describe(), format!("git:{c2}"));
    let (entries, issues) = src.list(&["project/knowledge/".into()]).unwrap();
    let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec![TOKEN_API, TOKEN_STORAGE]);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(
        (issues[0].path.as_str(), issues[0].code),
        (link, "SYMLINK_NOT_ALLOWED")
    );
    assert_eq!(src.read_path(link).unwrap_err().code, ErrorCode::UnsafePath);
    assert_eq!(
        src.read_path("project/knowledge").unwrap_err().code,
        ErrorCode::InvalidInput
    );
    assert!(
        src.read_path("project/knowledge/missing.md")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        src.list(&["../escape".into()]).unwrap_err().code,
        ErrorCode::UnsafePath
    );
    assert_eq!(
        src.read_path("project/../core/release.toml")
            .unwrap_err()
            .code,
        ErrorCode::UnsafePath
    );
}

#[test]
fn concurrent_resolves_share_one_mirror() {
    let w = World::new();
    let c2 = w.push_approved("newer", &[(TOKEN_API, "+++\nschema = 1\n+++\nnewer\n")]);
    let env = w.env();
    let revisions: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..6)
            .map(|_| {
                scope.spawn(|| {
                    resolve(&env, None, SelectionRequest::Latest, false).map(|r| r.info.revision)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for r in revisions {
        assert_eq!(r.unwrap().as_deref(), Some(c2.as_str()));
    }
    assert_eq!(mirrors(&w.cache).len(), 1);
}

#[test]
fn unusable_mirror_is_set_aside_and_rebuilt() {
    let w = World::new();
    let env = w.env();
    resolve(&env, None, SelectionRequest::Auto, false).unwrap();
    let mirror = mirrors(&w.cache).remove(0);
    fs::rename(&mirror, w.sb.path().join("stolen")).unwrap();
    write(&mirror.join("garbage"), "not a repository");
    let snap = resolve(&env, None, SelectionRequest::Auto, false).unwrap();
    assert_eq!(snap.info.revision.as_deref(), Some(w.c1.as_str()));
    let names: Vec<String> = fs::read_dir(w.cache.join("git"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n.contains(".git.unusable-")),
        "{names:?}"
    );
}

// ---------------------------------------------------------------------------------------
// `kb sync` through the real executable
// ---------------------------------------------------------------------------------------

#[test]
fn sync_cli_fetches_and_reports_without_modifying_the_checkout_or_host() {
    let w = World::new();
    let host = w.host_with_submodule("host");
    let kb_root = host.join(".kb");
    let c2 = w.push_approved("newer", &[(TOKEN_API, "+++\nschema = 1\n+++\nnewer\n")]);
    write(
        &kb_root.join("project/knowledge/gaps/draft.md"),
        "+++\nschema = 1\n+++\ndraft\n",
    );
    let before = (
        checkout_state(&w.sb, &host),
        checkout_state(&w.sb, &kb_root),
    );
    let cache = w.cache.to_str().unwrap();
    let envs = [("KB_CACHE_DIR", cache)];
    let kb = |args: &[&str]| {
        let mut all = vec!["--root", s(&kb_root)];
        all.extend_from_slice(args);
        w.sb.kb(&host, &all, &envs)
    };

    let o = kb(&["--json", "sync"]);
    assert_eq!(o.status.code(), Some(0), "{}", common::stderr(&o));
    let v = common::json(&o);
    assert_eq!(v["protocol"], "kb.cli.v1");
    assert_eq!(v["command"], "sync");
    assert_eq!(v["ok"], true);
    let r = &v["result"];
    assert_eq!(r["freshness"], "verified");
    assert_eq!(r["latest_approved"], c2.as_str());
    assert_eq!(r["previous_approved"], serde_json::Value::Null);
    assert_eq!(r["source"], s(&w.origin));
    assert_eq!(r["local"]["head"], w.c1.as_str());
    assert_eq!(r["local"]["ahead"], 0);
    assert_eq!(r["local"]["behind"], 1);
    assert_eq!(r["local"]["dirty"], true);
    assert_eq!(r["local"]["changes"], 1);
    assert_eq!(r["host"]["pin"]["revision"], w.c1.as_str());
    assert_eq!(r["host"]["pin"]["source"], "submodule");
    assert_eq!(r["host"]["status"], "behind");
    assert_eq!(r["host"]["behind"], 1);
    assert_eq!(r["checkout_unchanged"], true);

    let o = kb(&["sync"]);
    assert_eq!(o.status.code(), Some(0));
    let text = common::stdout(&o);
    assert!(text.contains("(unchanged)"), "{text}");
    assert!(text.contains("1 commits behind approved"), "{text}");
    assert!(text.contains("nothing modified"), "{text}");

    let o = kb(&["--json", "--offline", "sync"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(common::json(&o)["result"]["freshness"], "unverified");

    fs::rename(&w.origin, w.sb.path().join("origin.gone")).unwrap();
    let o = kb(&["--json", "sync"]);
    assert_eq!(o.status.code(), Some(20));
    let v = common::json(&o);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "FRESHNESS_UNVERIFIED");

    assert_eq!(
        (
            checkout_state(&w.sb, &host),
            checkout_state(&w.sb, &kb_root)
        ),
        before
    );
}
