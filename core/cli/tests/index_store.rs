//! Derived SQLite index: incremental reuse, snapshot isolation, proposals, concurrency,
//! interrupted builds, corruption recovery, FTS safety and garbage collection.
mod common;

use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use kb::context::memory::MemoryView;
use kb::corpus;
use kb::error::{ErrorCode, KbError, Result};
use kb::index::{BuildStats, DEFAULT_KEEP, Index, RecoveryKind};
use kb::knowledge::{
    KnowledgeView, MetaEntry, Origin, Overlay, OverlayFile, OverlayStatus, ProposalChange, TaskPath,
};
use kb::model::{Kind, Profile, ProfileLocation};
use kb::normalize::tokens;
use kb::source::{SourceEntry, SourceIssue, SourceTree, WorkingTreeSource};
use kb::util::sha256_hex;

fn loc() -> ProfileLocation {
    ProfileLocation::for_profile(Profile::Project)
}

/// A temporary KB checkout (minimal project) plus a separate cache directory.
struct Fixture {
    _sb: common::Sandbox,
    root: PathBuf,
    cache: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let sb = common::Sandbox::new();
        let root = sb.path().join("kb");
        common::write_min_project(&root);
        let cache = sb.path().join("cache");
        Fixture {
            _sb: sb,
            root,
            cache,
        }
    }

    fn write(&self, rel: &str, text: &str) {
        common::write(&self.root.join(rel), text);
    }

    fn remove(&self, rel: &str) {
        fs::remove_file(self.root.join(rel)).unwrap();
    }

    fn source(&self) -> WorkingTreeSource {
        WorkingTreeSource::new(&self.root)
    }

    fn open(&self) -> Index {
        Index::open(&self.cache, Profile::Project).unwrap()
    }

    fn build(&self, ix: &mut Index, key: &str) -> BuildStats {
        ix.ensure(key, &self.source(), &loc(), None).unwrap()
    }

    fn db(&self) -> PathBuf {
        self.cache.join("index/project.sqlite")
    }
}

/// A product-wide reference record; `extra` holds additional TOML tables.
fn reference(id: &str, title: &str, summary: &str, extra: &str) -> String {
    format!(
        "+++\nschema = 1\nid = \"{id}\"\nkind = \"reference\"\ntitle = \"{title}\"\n\
         status = \"accepted\"\nowner = \"arch\"\nsummary = \"{summary}\"\n\n\
         [scope]\nproduct = true\n{extra}+++\n"
    )
}

fn ref_path(name: &str) -> String {
    format!("project/knowledge/refs/{name}.md")
}

fn ids(entries: &[kb::knowledge::MetaEntry]) -> Vec<String> {
    entries.iter().map(|e| e.meta.id.clone()).collect()
}

fn all_accepted(v: &impl KnowledgeView) -> Vec<String> {
    accepted_ids(v).unwrap()
}

fn accepted_ids(v: &impl KnowledgeView) -> Result<Vec<String>> {
    Ok(ids(&v.metas_by_kind(&Kind::ALL, Origin::Accepted)?))
}

fn toks(s: &str) -> Vec<String> {
    tokens(s)
}

#[test]
fn incremental_builds_parse_only_new_content() {
    let fx = Fixture::new();
    fx.write(
        &ref_path("r1"),
        &reference("acme.ref.r1", "One", "first", ""),
    );
    fx.write(
        &ref_path("r2"),
        &reference("acme.ref.r2", "Two", "second", ""),
    );
    let mut ix = fx.open();

    let s1 = fx.build(&mut ix, "k1");
    assert!(!s1.reused);
    assert_eq!((s1.files, s1.parsed, s1.reused_docs), (4, 4, 0));
    assert_eq!(s1.errors, 0, "{s1:?}");

    let again = fx.build(&mut ix, "k1");
    assert!(again.reused);
    assert_eq!((again.files, again.parsed), (4, 0));

    // add, modify, delete and rename (identical bytes at a new path)
    fx.write(
        &ref_path("r3"),
        &reference("acme.ref.r3", "Three", "third", ""),
    );
    fx.write(
        &ref_path("r1"),
        &reference("acme.ref.r1", "One v2", "first", ""),
    );
    fx.remove(&ref_path("r2"));
    let api = fs::read_to_string(fx.root.join("project/knowledge/contracts/token-api.md")).unwrap();
    fx.remove("project/knowledge/contracts/token-api.md");
    fx.write("project/knowledge/contracts/token-api-v2.md", &api);

    let s2 = fx.build(&mut ix, "k2");
    assert_eq!((s2.files, s2.parsed, s2.reused_docs), (4, 2, 2), "{s2:?}");

    let v2 = ix.view("k2").unwrap();
    assert_eq!(
        all_accepted(&v2),
        vec![
            "acme.contract.token-api",
            "acme.mobile.token-storage",
            "acme.ref.r1",
            "acme.ref.r3"
        ]
    );
    let api_meta = v2
        .metas_by_ids(&["acme.contract.token-api".into()], Origin::Accepted)
        .unwrap();
    assert_eq!(
        api_meta[0].path,
        "project/knowledge/contracts/token-api-v2.md"
    );
    let r1 = v2
        .records(&["acme.ref.r1".into()], Origin::Accepted)
        .unwrap();
    assert_eq!(r1[0].parsed.record.common().title, "One v2");
    drop(v2);

    // the old snapshot is untouched
    let v1 = ix.view("k1").unwrap();
    assert!(all_accepted(&v1).contains(&"acme.ref.r2".to_string()));
    let r1_old = v1
        .records(&["acme.ref.r1".into()], Origin::Accepted)
        .unwrap();
    assert_eq!(r1_old[0].parsed.record.common().title, "One");
    let raw = v1.raw("acme.ref.r2", Origin::Accepted).unwrap().unwrap();
    assert!(raw.text.contains("summary = \"second\""));
    assert!(v1.raw("acme.ref.r3", Origin::Accepted).unwrap().is_none());
}

/// Counts every access to the wrapped source.
struct CountingSource {
    inner: WorkingTreeSource,
    calls: Cell<usize>,
}

impl SourceTree for CountingSource {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn list(&self, prefixes: &[String]) -> Result<(Vec<SourceEntry>, Vec<SourceIssue>)> {
        self.calls.set(self.calls.get() + 1);
        self.inner.list(prefixes)
    }
    fn read(&self, entries: &[SourceEntry]) -> Result<Vec<Vec<u8>>> {
        self.calls.set(self.calls.get() + 1);
        self.inner.read(entries)
    }
    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.calls.set(self.calls.get() + 1);
        self.inner.read_path(path)
    }
}

#[test]
fn warm_path_touches_no_source_files() {
    let fx = Fixture::new();
    let src = CountingSource {
        inner: fx.source(),
        calls: Cell::new(0),
    };
    let mut ix = fx.open();
    ix.ensure("k", &src, &loc(), None).unwrap();
    assert!(src.calls.get() > 0);
    src.calls.set(0);
    let mut other = fx.open();
    let s = other.ensure("k", &src, &loc(), None).unwrap();
    assert!(s.reused);
    assert_eq!(
        src.calls.get(),
        0,
        "warm ensure must not list or read files"
    );
}

#[test]
fn parse_errors_and_lints_are_snapshot_diagnostics() {
    let fx = Fixture::new();
    fx.write("project/knowledge/broken.md", "no front matter here\n");
    fx.write(
        &ref_path("loud"),
        &(reference("acme.ref.loud", "Loud", "x", "") + "## Notes\nYou MUST do this.\n"),
    );
    fx.write("project/knowledge/notes.txt", "ignored");
    let mut ix = fx.open();
    let s = fx.build(&mut ix, "k");
    assert_eq!(s.errors, 1, "{s:?}");
    assert!(s.warnings >= 2, "{s:?}");
    let v = ix.view("k").unwrap();
    let d = v.diagnostics();
    let find = |code: &str| d.iter().find(|x| x.code == code).cloned();
    let fm = find("FRONT_MATTER_MISSING").expect("parse error");
    assert_eq!(fm.path.as_deref(), Some("project/knowledge/broken.md"));
    let lint = find("NORMATIVE_LANGUAGE_IN_BODY").expect("lint");
    assert_eq!(lint.path.as_deref(), Some("project/knowledge/refs/loud.md"));
    assert_eq!(lint.record.as_deref(), Some("acme.ref.loud"));
    assert!(find("NON_RECORD_FILE").is_some());
    // the broken file is a member but has no record
    assert_eq!(all_accepted(&v).len(), 3);
    drop(v);

    // the same broken bytes at another path report the new path (diagnostics are per member)
    fx.remove("project/knowledge/broken.md");
    fx.write(
        "project/knowledge/moved/broken.md",
        "no front matter here\n",
    );
    let s2 = fx.build(&mut ix, "k2");
    assert_eq!(s2.parsed, 0);
    let v2 = ix.view("k2").unwrap();
    let fm2 = v2
        .diagnostics()
        .iter()
        .find(|x| x.code == "FRONT_MATTER_MISSING")
        .unwrap();
    assert_eq!(
        fm2.path.as_deref(),
        Some("project/knowledge/moved/broken.md")
    );
}

#[test]
fn alias_and_registry_changes_are_visible_only_in_new_snapshots() {
    let fx = Fixture::new();
    let sel = "[selectors]\naliases = [\"token refresh\"]\n";
    fx.write(
        &ref_path("refresh"),
        &reference("acme.ref.refresh", "Refresh flow", "x", sel),
    );
    let mut ix = fx.open();
    fx.build(&mut ix, "k1");

    let sel2 = "[selectors]\naliases = [\"token refresh\", \"обновлени*\"]\n";
    fx.write(
        &ref_path("refresh"),
        &reference("acme.ref.refresh", "Refresh flow", "x", sel2),
    );
    let concepts = fs::read_to_string(fx.root.join("project/registry/concepts.toml")).unwrap();
    fx.write(
        "project/registry/concepts.toml",
        &concepts.replace(
            "aliases = [\"token\", \"токен*\"]",
            "aliases = [\"token\", \"jwt\"]",
        ),
    );
    let s2 = fx.build(&mut ix, "k2");
    assert_eq!(s2.parsed, 1);

    let ru = toks("Обновление сессии");
    let en = toks("please fix the token refresh");
    let v1 = ix.view("k1").unwrap();
    assert_eq!(
        ids(&v1.term_candidates(&[], &en).unwrap()),
        vec!["acme.ref.refresh"]
    );
    assert!(v1.term_candidates(&[], &ru).unwrap().is_empty());
    assert!(!v1.registry().alias_table().contains_key("jwt"));
    assert!(v1.registry().alias_table().contains_key("токен*"));
    drop(v1);

    let v2 = ix.view("k2").unwrap();
    assert_eq!(
        ids(&v2.term_candidates(&[], &ru).unwrap()),
        vec!["acme.ref.refresh"]
    );
    assert_eq!(v2.registry().alias_table()["jwt"], vec!["auth-token"]);
    assert!(!v2.registry().alias_table().contains_key("токен*"));
    drop(v2);

    // A registry-only change is a new snapshot without any re-parsing.
    fx.write("project/registry/concepts.toml", &concepts);
    let s3 = fx.build(&mut ix, "k3");
    assert_eq!((s3.parsed, s3.reused_docs), (0, 3));
    let v3 = ix.view("k3").unwrap();
    assert!(v3.registry().alias_table().contains_key("токен*"));
}

#[test]
fn path_and_term_candidates_follow_selectors() {
    let fx = Fixture::new();
    fx.write(
        &ref_path("auth"),
        &reference(
            "acme.ref.auth",
            "Auth paths",
            "x",
            "[selectors]\npaths = [\"app/auth/**\"]\n",
        ),
    );
    fx.write(
        &ref_path("api"),
        &reference(
            "acme.ref.api",
            "Api paths",
            "x",
            "[selectors]\npaths = [\"backend:src/api/**/*.rs\"]\n",
        ),
    );
    fx.write(
        &ref_path("anywhere"),
        &reference(
            "acme.ref.anywhere",
            "Token dirs",
            "x",
            "[selectors]\npaths = [\"**/token/**\"]\n",
        ),
    );
    let mut ix = fx.open();
    fx.build(&mut ix, "k");
    let v = ix.view("k").unwrap();
    let tp = |repo: Option<&str>, path: &str| TaskPath {
        repo: repo.map(str::to_string),
        path: path.to_string(),
    };
    let hits = |paths: &[TaskPath]| ids(&v.path_candidates(paths).unwrap());

    assert_eq!(
        hits(&[tp(Some("mobile"), "app/auth/ui/Login.kt")]),
        vec!["acme.ref.auth"]
    );
    assert_eq!(
        hits(&[tp(Some("backend"), "src/api/v1/users.rs")]),
        vec!["acme.ref.api"]
    );
    assert!(hits(&[tp(Some("mobile"), "src/api/v1/users.rs")]).is_empty());
    assert!(hits(&[tp(None, "src/api/v1/users.rs")]).is_empty());
    assert!(hits(&[tp(Some("backend"), "src/api/v1/users.kt")]).is_empty());
    assert_eq!(
        hits(&[
            tp(Some("backend"), "src/api/v1/users.rs"),
            tp(None, "lib/token/store.rs"),
            tp(Some("mobile"), "app/auth/Token.kt"),
        ]),
        vec!["acme.ref.anywhere", "acme.ref.api", "acme.ref.auth"]
    );

    // concept selectors (the fixture policy selects `auth-token`)
    assert_eq!(
        ids(&v.term_candidates(&["auth-token".into()], &[]).unwrap()),
        vec!["acme.mobile.token-storage"]
    );
    assert!(
        v.term_candidates(&["unknown".into()], &[])
            .unwrap()
            .is_empty()
    );
}

fn overlay_file(path: &str, status: OverlayStatus, text: Option<&str>) -> OverlayFile {
    OverlayFile {
        path: path.to_string(),
        status,
        content: text.map(|t| t.as_bytes().to_vec()),
        content_id: text.map(|t| format!("sha256:{}", sha256_hex(t.as_bytes()))),
        stale: false,
    }
}

#[test]
fn proposals_are_separate_and_classified() {
    let fx = Fixture::new();
    fx.write(
        &ref_path("r1"),
        &reference("acme.ref.r1", "One", "first", ""),
    );
    fx.write(
        &ref_path("r2"),
        &reference("acme.ref.r2", "Two", "second", ""),
    );
    let api_path = "project/knowledge/contracts/token-api.md";
    let api = fs::read_to_string(fx.root.join(api_path)).unwrap();
    let mut ix = fx.open();
    fx.build(&mut ix, "base");

    let mut modified = overlay_file(
        api_path,
        OverlayStatus::Modified,
        Some(&api.replace("Token refresh API", "Token refresh API zeppelin")),
    );
    modified.stale = true;
    let files = vec![
        overlay_file(
            "project/knowledge/new/fresh.md",
            OverlayStatus::Added,
            Some(&reference("acme.ref.fresh", "Fresh zeppelin", "new", "")),
        ),
        modified,
        overlay_file(&ref_path("r2"), OverlayStatus::Deleted, None),
        overlay_file(&ref_path("r1"), OverlayStatus::Deleted, None),
        overlay_file(
            &ref_path("r1-moved"),
            OverlayStatus::Added,
            Some(&reference("acme.ref.r1", "One moved", "first", "")),
        ),
        overlay_file(
            "project/knowledge/bad.md",
            OverlayStatus::Added,
            Some("+++\nschema = 1\n"),
        ),
        overlay_file(
            "project/knowledge/README.md",
            OverlayStatus::Added,
            Some("docs"),
        ),
        overlay_file(
            "project/registry/concepts.toml",
            OverlayStatus::Modified,
            Some("schema = 1\n"),
        ),
    ];
    let ov = Overlay {
        base: Some("abc".into()),
        digest: "d1".into(),
        files,
    };
    let s = ix
        .ensure("base+ov", &fx.source(), &loc(), Some(&ov))
        .unwrap();
    assert_eq!(s.proposals, 5, "{s:?}");
    assert_eq!(
        s.errors, 0,
        "proposal errors never taint the accepted snapshot: {s:?}"
    );

    let v = ix.view("base+ov").unwrap();
    // accepted content is exactly the base snapshot
    assert_eq!(
        all_accepted(&v),
        vec![
            "acme.contract.token-api",
            "acme.mobile.token-storage",
            "acme.ref.r1",
            "acme.ref.r2"
        ]
    );
    let accepted_api = v
        .records(&["acme.contract.token-api".into()], Origin::Accepted)
        .unwrap();
    assert_eq!(
        accepted_api[0].parsed.record.common().title,
        "Token refresh API"
    );
    let proposed_api = v
        .records(&["acme.contract.token-api".into()], Origin::Proposal)
        .unwrap();
    assert_eq!(
        proposed_api[0].parsed.record.common().title,
        "Token refresh API zeppelin"
    );
    assert!(
        v.raw("acme.ref.fresh", Origin::Proposal)
            .unwrap()
            .unwrap()
            .text
            .contains("Fresh zeppelin")
    );
    assert!(v.raw("acme.ref.fresh", Origin::Accepted).unwrap().is_none());
    assert_eq!(
        ids(&v.metas_by_kind(&Kind::ALL, Origin::Proposal).unwrap()),
        vec!["acme.contract.token-api", "acme.ref.fresh", "acme.ref.r1"]
    );
    assert!(v.fulltext(&["zeppelin".into()], 10).unwrap().is_empty());

    let props = v.proposals().unwrap();
    let summary: Vec<(String, ProposalChange, bool)> = props
        .iter()
        .map(|p| (p.path.clone(), p.change.clone(), p.stale))
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "project/knowledge/bad.md".into(),
                ProposalChange::Invalid,
                false
            ),
            (
                api_path.into(),
                ProposalChange::Modifies {
                    id: "acme.contract.token-api".into()
                },
                true
            ),
            (
                "project/knowledge/new/fresh.md".into(),
                ProposalChange::New {
                    id: "acme.ref.fresh".into()
                },
                false
            ),
            (
                ref_path("r1-moved"),
                ProposalChange::Modifies {
                    id: "acme.ref.r1".into()
                },
                false
            ),
            (
                ref_path("r2"),
                ProposalChange::Removes {
                    id: "acme.ref.r2".into()
                },
                false
            ),
        ]
    );
    assert!(props[0].meta.is_none());
    assert!(props[0].diagnostics.iter().any(|d| d.is_error()));
    assert_eq!(props[4].meta.as_ref().unwrap().title, "Two");
    let not_applied: Vec<_> = v
        .diagnostics()
        .iter()
        .filter(|d| d.code == "PROPOSAL_NOT_APPLIED")
        .map(|d| d.path.clone().unwrap())
        .collect();
    assert_eq!(not_applied, vec!["project/registry/concepts.toml"]);
    // the registry change was not applied
    assert!(v.registry().concept("auth-token").is_some());
    drop(v);

    let base = ix.view("base").unwrap();
    assert!(base.proposals().unwrap().is_empty());
    assert!(
        base.metas_by_kind(&Kind::ALL, Origin::Proposal)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn proposed_state_is_validated_per_entry() {
    let fx = Fixture::new();
    let mut ix = fx.open();
    let unowned = reference("acme.ref.unowned", "Unowned", "x", "")
        .replace("owner = \"arch\"", "owner = \"nobody\"");
    let ov = Overlay {
        base: None,
        digest: "d2".into(),
        files: vec![overlay_file(
            &ref_path("unowned"),
            OverlayStatus::Added,
            Some(&unowned),
        )],
    };
    let s = ix.ensure("ov", &fx.source(), &loc(), Some(&ov)).unwrap();
    assert_eq!((s.proposals, s.errors), (1, 0), "{s:?}");
    let v = ix.view("ov").unwrap();
    let p = v.proposals().unwrap();
    assert_eq!(
        p[0].change,
        ProposalChange::New {
            id: "acme.ref.unowned".into()
        }
    );
    assert!(
        p[0].diagnostics
            .iter()
            .any(|d| d.is_error() && d.path.as_deref() == Some(p[0].path.as_str())),
        "{:#?}",
        p[0].diagnostics
    );
    assert!(v.diagnostics().iter().all(|d| !d.is_error()));
}

#[test]
fn concurrent_writers_with_same_and_different_keys() {
    let fx = Fixture::new();
    let threads = 6;
    let barrier = Barrier::new(threads * 2);
    let results: Vec<(String, BuildStats)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads * 2)
            .map(|i| {
                let fx = &fx;
                let barrier = &barrier;
                s.spawn(move || {
                    barrier.wait();
                    let mut ix = fx.open();
                    let key = if i % 2 == 0 {
                        "shared".to_string()
                    } else {
                        format!("own-{i}")
                    };
                    let stats = fx.build(&mut ix, &key);
                    (key, stats)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let shared_builds = results
        .iter()
        .filter(|(k, s)| k == "shared" && !s.reused)
        .count();
    assert_eq!(shared_builds, 1, "exactly one builder of the shared key");
    assert!(
        results
            .iter()
            .filter(|(k, _)| k != "shared")
            .all(|(_, s)| !s.reused && s.files == 2)
    );
    let ix = fx.open();
    let stats = ix.stats().unwrap();
    assert_eq!(stats.snapshots.len(), 1 + threads);
    // every snapshot shares the same two documents
    assert_eq!(stats.docs, 2);
    for (key, _) in &results {
        assert_eq!(all_accepted(&ix.view(key).unwrap()).len(), 2);
    }
}

#[test]
fn readers_see_complete_snapshots_while_others_write() {
    let fx = Fixture::new();
    let mut ix = fx.open();
    fx.build(&mut ix, "w0");
    let rounds = 10;
    let done = AtomicBool::new(false);
    let checked = AtomicUsize::new(0);
    std::thread::scope(|s| {
        s.spawn(|| {
            let mut writer = fx.open();
            for i in 1..=rounds {
                fx.write(
                    &ref_path(&format!("w{i}")),
                    &reference(&format!("acme.ref.w{i}"), "W", "w", ""),
                );
                fx.build(&mut writer, &format!("w{i}"));
            }
            done.store(true, Ordering::SeqCst);
        });
        s.spawn(|| {
            let reader = fx.open();
            while !done.load(Ordering::SeqCst) {
                for i in 0..=rounds {
                    // A view either does not exist yet (or was collected) or is complete.
                    if let Ok(v) = reader.view(&format!("w{i}")) {
                        assert_eq!(all_accepted(&v).len(), 2 + i);
                        checked.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        });
    });
    assert!(checked.load(Ordering::SeqCst) > 0);

    // A live view keeps its state even when another process deletes the snapshot.
    let reader = fx.open();
    let latest = format!("w{rounds}");
    let v = reader.view(&latest).unwrap();
    let mut other = fx.open();
    assert!(other.gc(0).unwrap() > 0);
    assert_eq!(other.stats().unwrap().docs, 0);
    assert_eq!(all_accepted(&v).len(), 2 + rounds);
    assert_eq!(
        v.fulltext(&["rotate".into()], 5).unwrap(),
        vec!["acme.contract.token-api"]
    );
    drop(v);
    assert!(reader.view(&latest).is_err());
    assert!(!reader.has_snapshot(&latest).unwrap());
}

/// Fails on the second `read` call (after the first batch was stored).
struct FailingSource {
    inner: WorkingTreeSource,
    reads: Cell<usize>,
}

impl SourceTree for FailingSource {
    fn describe(&self) -> String {
        "failing".into()
    }
    fn list(&self, prefixes: &[String]) -> Result<(Vec<SourceEntry>, Vec<SourceIssue>)> {
        self.inner.list(prefixes)
    }
    fn read(&self, entries: &[SourceEntry]) -> Result<Vec<Vec<u8>>> {
        self.reads.set(self.reads.get() + 1);
        if self.reads.get() == 2 {
            return Err(KbError::internal("simulated read failure"));
        }
        self.inner.read(entries)
    }
    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.inner.read_path(path)
    }
}

#[test]
fn interrupted_build_persists_nothing() {
    let fx = Fixture::new();
    for i in 0..300 {
        fx.write(
            &ref_path(&format!("g{i:03}")),
            &reference(&format!("acme.ref.g{i:03}"), "Generated", "g", ""),
        );
    }
    let mut ix = fx.open();
    let failing = FailingSource {
        inner: fx.source(),
        reads: Cell::new(0),
    };
    let err = ix.ensure("k", &failing, &loc(), None).unwrap_err();
    assert!(err.message.contains("simulated"), "{err}");
    assert_eq!(
        failing.reads.get(),
        2,
        "the failure happened after a stored batch"
    );
    assert!(!ix.has_snapshot("k").unwrap());
    let stats = ix.stats().unwrap();
    assert!(stats.snapshots.is_empty());
    assert_eq!(stats.docs, 0, "no partially built documents survive");

    let ok = fx.build(&mut ix, "k");
    assert_eq!((ok.files, ok.parsed), (302, 302));
}

#[test]
fn corrupt_database_is_moved_aside_and_rebuilt() {
    let fx = Fixture::new();
    {
        let mut ix = fx.open();
        fx.build(&mut ix, "k");
        assert!(ix.recovery().is_none());
    }
    let garbage = b"this is not an SQLite database\n".repeat(200);
    fs::write(fx.db(), &garbage).unwrap();
    let mut ix = fx.open();
    let rec = ix.recovery().cloned().expect("recovery reported");
    assert_eq!(rec.kind, RecoveryKind::Corrupt);
    let moved = rec.moved_to.clone().unwrap();
    assert!(
        moved
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("project.sqlite.corrupt-")
    );
    assert_eq!(
        fs::read(&moved).unwrap(),
        garbage,
        "the damaged file is kept"
    );
    assert_eq!(rec.diagnostic().code, "INDEX_RECOVERED");
    assert!(!ix.has_snapshot("k").unwrap());
    let s = fx.build(&mut ix, "k");
    assert_eq!(s.parsed, 2);
    assert_eq!(
        s.recovery.as_ref().map(|r| r.kind),
        Some(RecoveryKind::Corrupt)
    );
    assert_eq!(ix.stats().unwrap().recovery, Some(rec));

    // Reopening the repaired file needs no recovery.
    drop(ix);
    let ix = fx.open();
    assert!(ix.recovery().is_none());
    assert!(ix.has_snapshot("k").unwrap());
}

#[test]
fn corruption_found_by_a_build_is_recovered() {
    let fx = Fixture::new();
    let (page_size, root) = {
        let mut ix = fx.open();
        fx.build(&mut ix, "k");
        drop(ix);
        let c = rusqlite::Connection::open(fx.db()).unwrap();
        let page_size: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        let root: i64 = c
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE name = 'snapshots'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        (page_size, root)
    };
    // Damage only the `snapshots` table: the header and metadata stay readable, so the
    // problem surfaces during `ensure`, not when opening.
    let mut bytes = fs::read(fx.db()).unwrap();
    let start = usize::try_from((root - 1) * page_size).unwrap();
    let end = start + usize::try_from(page_size).unwrap();
    bytes[start..end].fill(0xA5);
    fs::write(fx.db(), &bytes).unwrap();

    let mut ix = fx.open();
    assert!(
        ix.recovery().is_none(),
        "opening does not read the damaged page"
    );
    let s = fx.build(&mut ix, "k2");
    assert!(!s.reused);
    let rec = s.recovery.expect("recovery reported by the build");
    assert_eq!(rec.kind, RecoveryKind::Corrupt);
    assert!(rec.moved_to.as_ref().is_some_and(|p| p.exists()));
    assert!(ix.has_snapshot("k2").unwrap());
    assert!(!ix.has_snapshot("k").unwrap());
}

#[test]
fn foreign_and_outdated_databases_are_replaced() {
    // A valid SQLite file that is not a kb index is moved aside.
    let fx = Fixture::new();
    fs::create_dir_all(fx.db().parent().unwrap()).unwrap();
    {
        let c = rusqlite::Connection::open(fx.db()).unwrap();
        c.execute_batch("CREATE TABLE unrelated (x); INSERT INTO unrelated VALUES (1);")
            .unwrap();
    }
    let ix = fx.open();
    let rec = ix.recovery().unwrap();
    assert_eq!(rec.kind, RecoveryKind::Corrupt);
    assert!(rec.reason.contains("metadata"), "{}", rec.reason);
    drop(ix);

    // An index from another index schema version is rebuilt in place.
    let mut ix = fx.open();
    fx.build(&mut ix, "k");
    drop(ix);
    {
        let c = rusqlite::Connection::open(fx.db()).unwrap();
        c.execute("UPDATE meta SET value = '0' WHERE key = 'index_schema'", [])
            .unwrap();
    }
    let ix = fx.open();
    let rec = ix.recovery().unwrap();
    assert_eq!(rec.kind, RecoveryKind::Outdated);
    assert!(
        rec.reason
            .contains(&format!("index_schema 0 -> {}", kb::versions::INDEX_SCHEMA)),
        "{}",
        rec.reason
    );
    assert!(rec.moved_to.is_none());
    assert!(!ix.has_snapshot("k").unwrap());
    drop(ix);

    // Explicit rebuild empties the cache.
    let mut ix = fx.open();
    fx.build(&mut ix, "k");
    drop(ix);
    let ix = Index::rebuild(&fx.cache, Profile::Project).unwrap();
    assert_eq!(ix.recovery().unwrap().kind, RecoveryKind::Requested);
    assert_eq!(ix.stats().unwrap().docs, 0);
    assert!(!ix.has_snapshot("k").unwrap());
}

#[test]
fn index_missing_a_metadata_key_is_rebuilt_in_place() {
    // An index written before a metadata key existed (here the table `layout` revision) is
    // outdated derived data: it is rebuilt in place and reported as INDEX_REBUILT, never
    // moved aside as a corrupt `.corrupt-*` file.
    let fx = Fixture::new();
    {
        let mut ix = fx.open();
        fx.build(&mut ix, "k");
    }
    {
        let c = rusqlite::Connection::open(fx.db()).unwrap();
        let removed = c
            .execute("DELETE FROM meta WHERE key = 'layout'", [])
            .unwrap();
        assert_eq!(removed, 1, "the index records its layout revision");
    }
    let mut ix = fx.open();
    let rec = ix.recovery().cloned().expect("recovery reported");
    assert_eq!(rec.kind, RecoveryKind::Outdated, "{rec:?}");
    assert!(rec.reason.contains("layout missing -> "), "{}", rec.reason);
    assert!(rec.moved_to.is_none());
    let diag = rec.diagnostic();
    assert_eq!(diag.code, "INDEX_REBUILT");
    assert_eq!(diag.severity, kb::diag::Severity::Info);
    let corrupt: Vec<String> = fs::read_dir(fx.db().parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("corrupt"))
        .collect();
    assert!(corrupt.is_empty(), "nothing is moved aside: {corrupt:?}");
    assert!(!ix.has_snapshot("k").unwrap());
    let s = fx.build(&mut ix, "k");
    assert_eq!(s.parsed, 2);
    assert_eq!(
        s.recovery.as_ref().map(|r| r.kind),
        Some(RecoveryKind::Outdated)
    );

    // The rebuilt file carries every key again and reopens without recovery.
    drop(ix);
    let ix = fx.open();
    assert!(ix.recovery().is_none());
    assert!(ix.has_snapshot("k").unwrap());
}

#[test]
fn fulltext_quotes_user_input_and_ranks_within_the_snapshot() {
    let fx = Fixture::new();
    fx.write(
        &ref_path("rare"),
        &reference(
            "acme.ref.rare",
            "Quokka habitat",
            "The quokka lives near drop zones",
            "",
        ),
    );
    fx.write(
        &ref_path("common-a"),
        &reference("acme.ref.common-a", "Habitat a", "habitat notes", ""),
    );
    fx.write(
        &ref_path("common-b"),
        &reference("acme.ref.common-b", "Habitat b", "habitat notes", ""),
    );
    let mut ix = fx.open();
    fx.build(&mut ix, "k");
    let v = ix.view("k").unwrap();
    for hostile in [
        "\" OR * NEAR( -- ;DROP",
        "NEAR(",
        "*",
        "AND OR NOT",
        "title:quokka",
        "\"",
        "'); DROP TABLE docs; --",
        "^quokka",
        "{ids title}: x",
        "\u{0}",
    ] {
        v.fulltext(&[hostile.to_string()], 10)
            .unwrap_or_else(|e| panic!("{hostile:?}: {e}"));
    }
    assert_eq!(
        v.fulltext(&["\" OR * NEAR( -- ;DROP".into()], 10).unwrap(),
        vec!["acme.ref.rare"]
    );
    // rarer term first, ties broken by id, limit respected
    assert_eq!(
        v.fulltext(&toks("quokka habitat"), 10).unwrap(),
        vec!["acme.ref.rare", "acme.ref.common-a", "acme.ref.common-b"]
    );
    assert_eq!(v.fulltext(&toks("habitat"), 2).unwrap().len(), 2);
    assert!(v.fulltext(&toks("nonexistentword"), 10).unwrap().is_empty());
    assert!(v.fulltext(&toks("quokka"), 0).unwrap().is_empty());
    let docs_before = ix.stats().unwrap().docs;
    drop(v);
    assert_eq!(ix.stats().unwrap().docs, docs_before);

    // Documents of other snapshots never appear.
    fx.write(
        &ref_path("later"),
        &reference("acme.ref.later", "Quokka later", "quokka", ""),
    );
    fx.build(&mut ix, "k2");
    let v = ix.view("k").unwrap();
    assert!(
        !v.fulltext(&toks("quokka"), 10)
            .unwrap()
            .contains(&"acme.ref.later".into())
    );
}

#[test]
fn ranking_does_not_depend_on_cache_history() {
    // `a` and `c` are symmetric within the snapshot, so they tie and sort by id. Table-wide
    // statistics (FTS5 bm25) would rank `c` first in a cache that also holds many
    // `quokka` documents from another snapshot.
    let fx = Fixture::new();
    fx.write(
        &ref_path("a"),
        &reference("acme.ref.a", "Doc", "quokka", ""),
    );
    fx.write(
        &ref_path("c"),
        &reference("acme.ref.c", "Doc", "habitat", ""),
    );
    let query = toks("quokka habitat");
    let fresh = {
        let mut ix = fx.open();
        fx.build(&mut ix, "target");
        let v = ix.view("target").unwrap();
        v.fulltext(&query, 10).unwrap()
    };
    assert_eq!(fresh, vec!["acme.ref.a", "acme.ref.c"]);
    // Another cache first indexes a snapshot where `quokka` is everywhere.
    let other_cache = fx.cache.with_file_name("cache-2");
    let mut ix = Index::open(&other_cache, Profile::Project).unwrap();
    for i in 0..30 {
        fx.write(
            &ref_path(&format!("noise{i}")),
            &reference(&format!("acme.ref.noise{i}"), "Noise", "quokka quokka", ""),
        );
    }
    ix.ensure("noise", &fx.source(), &loc(), None).unwrap();
    for i in 0..30 {
        fx.remove(&ref_path(&format!("noise{i}")));
    }
    ix.ensure("target", &fx.source(), &loc(), None).unwrap();
    let v = ix.view("target").unwrap();
    assert_eq!(v.fulltext(&query, 10).unwrap(), fresh);
}

#[test]
fn gc_removes_old_snapshots_and_orphaned_documents() {
    let fx = Fixture::new();
    let mut ix = fx.open();
    for i in 0..4 {
        fx.write(
            &ref_path("changing"),
            &reference("acme.ref.changing", &format!("Version {i}"), "x", ""),
        );
        fx.build(&mut ix, &format!("g{i}"));
    }
    assert_eq!(ix.stats().unwrap().docs, 2 + 4);
    assert_eq!(ix.gc(2).unwrap(), 2);
    let stats = ix.stats().unwrap();
    let keys: Vec<_> = stats.snapshots.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, vec!["g3", "g2"]);
    assert_eq!(stats.docs, 2 + 2);
    assert!(ix.view("g0").is_err());
    let v = ix.view("g3").unwrap();
    let r = v
        .records(&["acme.ref.changing".into()], Origin::Accepted)
        .unwrap();
    assert_eq!(r[0].parsed.record.common().title, "Version 3");
    drop(v);
    // rebuilding a collected snapshot re-parses only what was dropped
    fx.write(
        &ref_path("changing"),
        &reference("acme.ref.changing", "Version 0", "x", ""),
    );
    assert_eq!(fx.build(&mut ix, "g0").parsed, 1);

    // automatic retention keeps DEFAULT_KEEP snapshots
    let mut collected = 0;
    for i in 0..DEFAULT_KEEP + 2 {
        fx.write(
            &ref_path("changing"),
            &reference("acme.ref.changing", &format!("Auto {i}"), "x", ""),
        );
        collected += fx.build(&mut ix, &format!("auto{i}")).collected;
    }
    let stats = ix.stats().unwrap();
    assert_eq!(stats.snapshots.len(), DEFAULT_KEEP);
    assert_eq!(collected, 3 + DEFAULT_KEEP + 2 - DEFAULT_KEEP);
    assert_eq!(stats.docs, 2 + DEFAULT_KEEP);
    assert_eq!(ix.gc(0).unwrap(), DEFAULT_KEEP);
    assert_eq!(ix.stats().unwrap().docs, 0);
}

#[test]
fn warm_query_timing_for_generated_corpus() {
    let fx = Fixture::new();
    let n = 2000;
    let words = [
        "cache",
        "token",
        "session",
        "payment",
        "retry",
        "backoff",
        "ledger",
        "invoice",
        "render",
        "layout",
        "migration",
        "schema",
        "profile",
        "search",
        "upload",
        "queue",
    ];
    for i in 0..n {
        let w1 = words[i % words.len()];
        let w2 = words[(i * 7 + 3) % words.len()];
        let extra = format!(
            "[selectors]\npaths = [\"src/m{}/**\"]\naliases = [\"{w1} {w2}\"]\n",
            i % 50
        );
        fx.write(
            &format!("project/knowledge/gen/{:02}/r{i:05}.md", i % 40),
            &(reference(
                &format!("acme.gen.r{i:05}"),
                &format!("Generated {w1} {w2} {i}"),
                &format!("Record {i} about {w1} and {w2} handling"),
                &extra,
            ) + "## Details\nLonger explanatory text for the generated record.\n"),
        );
    }
    let mut ix = fx.open();
    let t = Instant::now();
    let cold = fx.build(&mut ix, "big");
    let cold_ms = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(cold.files, n + 2);

    let t = Instant::now();
    let mut warm_ix = fx.open();
    assert!(fx.build(&mut warm_ix, "big").reused);
    let v = warm_ix.view("big").unwrap();
    let mandatory = v.metas_by_kind(&Kind::MANDATORY, Origin::Accepted).unwrap();
    let paths = v
        .path_candidates(&[TaskPath {
            repo: Some("mobile".into()),
            path: "src/m7/feature/File.kt".into(),
        }])
        .unwrap();
    let terms = v
        .term_candidates(
            &["auth-token".into()],
            &toks("fix payment retry for the ledger"),
        )
        .unwrap();
    let hits = v
        .fulltext(&toks("payment retry backoff ledger handling"), 20)
        .unwrap();
    let records = v.records(&hits, Origin::Accepted).unwrap();
    let warm_ms = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(mandatory.len(), 2);
    assert_eq!(paths.len(), n / 50);
    assert!(!terms.is_empty() && hits.len() == 20 && records.len() == 20);
    eprintln!(
        "index timing ({n} generated records, debug build): cold build {cold_ms:.1} ms; \
         warm open+ensure+view+queries {warm_ms:.1} ms"
    );
}

#[test]
fn caches_of_different_profiles_are_separate_files() {
    let fx = Fixture::new();
    let project = fx.open();
    let maintainer = Index::open(&fx.cache, Profile::Maintainer).unwrap();
    assert_ne!(project.path(), maintainer.path());
    assert!(maintainer.path().ends_with("index/maintainer.sqlite"));
}

/// Rewrites one listed file just before the first `read`, like an editor saving while the
/// index is being built.
struct RacingSource {
    inner: WorkingTreeSource,
    file: PathBuf,
    text: String,
    raced: Cell<bool>,
}

impl SourceTree for RacingSource {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn list(&self, prefixes: &[String]) -> Result<(Vec<SourceEntry>, Vec<SourceIssue>)> {
        self.inner.list(prefixes)
    }
    fn read(&self, entries: &[SourceEntry]) -> Result<Vec<Vec<u8>>> {
        if !self.raced.replace(true) {
            fs::write(&self.file, &self.text).unwrap();
        }
        self.inner.read(entries)
    }
    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.inner.read_path(path)
    }
}

#[test]
fn a_file_changed_while_indexing_never_poisons_the_cache() {
    let fx = Fixture::new();
    let path = ref_path("r1");
    let original = reference("acme.ref.r1", "Original X", "x", "");
    fx.write(&path, &original);
    let mut ix = fx.open();
    let racing = RacingSource {
        inner: fx.source(),
        file: fx.root.join(&path),
        text: reference("acme.ref.r1", "Edited Y", "y", ""),
        raced: Cell::new(false),
    };
    let err = ix.ensure("k1", &racing, &loc(), None).unwrap_err();
    assert_eq!(err.code, ErrorCode::IoError, "{err}");
    assert!(err.message.contains(&path), "{err}");
    assert!(err.hint.is_some(), "the error says to retry");
    assert!(!ix.has_snapshot("k1").unwrap());
    assert_eq!(
        ix.stats().unwrap().docs,
        0,
        "nothing is stored under a wrong id"
    );

    // The edit is undone (editor undo, `git checkout -- file`): the original bytes are
    // indexed as themselves.
    fx.write(&path, &original);
    let s = fx.build(&mut ix, "k2");
    assert_eq!(s.errors, 0, "{s:?}");
    let v = ix.view("k2").unwrap();
    let r1 = v
        .records(&["acme.ref.r1".into()], Origin::Accepted)
        .unwrap();
    assert_eq!(r1[0].parsed.record.common().title, "Original X");
    let raw = v.raw("acme.ref.r1", Origin::Accepted).unwrap().unwrap();
    assert_eq!(&*raw.text, original.as_str());
}

/// Lists honestly but serves other bytes for one path (a source that does not verify).
struct LyingSource {
    inner: WorkingTreeSource,
    path: String,
    text: String,
}

impl SourceTree for LyingSource {
    fn describe(&self) -> String {
        "lying".into()
    }
    fn list(&self, prefixes: &[String]) -> Result<(Vec<SourceEntry>, Vec<SourceIssue>)> {
        self.inner.list(prefixes)
    }
    fn read(&self, entries: &[SourceEntry]) -> Result<Vec<Vec<u8>>> {
        let mut out = self.inner.read(entries)?;
        for (e, bytes) in entries.iter().zip(out.iter_mut()) {
            if e.path == self.path {
                *bytes = self.text.clone().into_bytes();
            }
        }
        Ok(out)
    }
    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.inner.read_path(path)
    }
}

#[test]
fn documents_are_stored_only_under_their_own_content_id() {
    let fx = Fixture::new();
    let path = ref_path("r1");
    fx.write(&path, &reference("acme.ref.r1", "Listed", "x", ""));
    let lying = LyingSource {
        inner: fx.source(),
        path: path.clone(),
        text: reference("acme.ref.r1", "Served", "y", ""),
    };
    let mut ix = fx.open();
    let err = ix.ensure("k", &lying, &loc(), None).unwrap_err();
    assert_eq!(err.code, ErrorCode::IoError, "{err}");
    assert!(err.message.contains(&path), "{err}");
    assert!(!ix.has_snapshot("k").unwrap());
    assert_eq!(ix.stats().unwrap().docs, 0);
}

/// Serves the working tree with Git-style content ids, like a Git snapshot does.
struct GitIdSource {
    inner: WorkingTreeSource,
}

impl SourceTree for GitIdSource {
    fn describe(&self) -> String {
        "git:test".into()
    }
    fn list(&self, prefixes: &[String]) -> Result<(Vec<SourceEntry>, Vec<SourceIssue>)> {
        let (mut entries, issues) = self.inner.list(prefixes)?;
        for e in &mut entries {
            let hex = e.content_id.strip_prefix("sha256:").unwrap();
            e.content_id = format!("git:{}", &hex[..40]);
        }
        Ok((entries, issues))
    }
    fn read(&self, entries: &[SourceEntry]) -> Result<Vec<Vec<u8>>> {
        self.inner.read(entries)
    }
    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.inner.read_path(path)
    }
}

#[test]
fn proposals_identical_to_git_snapshot_files_are_skipped() {
    let fx = Fixture::new();
    let r1 = reference("acme.ref.r1", "One", "first", "");
    fx.write(&ref_path("r1"), &r1);
    let api_path = "project/knowledge/contracts/token-api.md";
    let api = fs::read_to_string(fx.root.join(api_path)).unwrap();
    let git = GitIdSource { inner: fx.source() };
    let ov = Overlay {
        base: Some("abc".into()),
        digest: "d-git".into(),
        files: vec![
            // e.g. a local edit that was squash-merged upstream: same bytes as accepted
            overlay_file(&ref_path("r1"), OverlayStatus::Modified, Some(&r1)),
            overlay_file(
                api_path,
                OverlayStatus::Modified,
                Some(&api.replace("Token refresh API", "Token refresh API v2")),
            ),
        ],
    };
    let mut ix = fx.open();
    let s = ix.ensure("git+ov", &git, &loc(), Some(&ov)).unwrap();
    assert_eq!(s.proposals, 1, "{s:?}");
    let v = ix.view("git+ov").unwrap();
    let props: Vec<(String, ProposalChange)> = v
        .proposals()
        .unwrap()
        .into_iter()
        .map(|p| (p.path, p.change))
        .collect();
    assert_eq!(
        props,
        vec![(
            api_path.to_string(),
            ProposalChange::Modifies {
                id: "acme.contract.token-api".into()
            }
        )]
    );
}

fn id_paths(entries: Vec<MetaEntry>) -> Vec<(String, String)> {
    entries
        .into_iter()
        .map(|e| (e.meta.id.clone(), e.path))
        .collect()
}

#[test]
fn duplicate_ids_resolve_to_the_first_path_like_the_memory_view() {
    let fx = Fixture::new();
    let first = ref_path("a-first");
    fx.write(
        &first,
        &reference("acme.ref.dup", "First copy", "alpha", ""),
    );
    fx.write(
        &ref_path("b-second"),
        &reference(
            "acme.ref.dup",
            "Second copy",
            "zeppelin",
            "[selectors]\npaths = [\"app/dup/**\"]\n",
        ),
    );
    let mut ix = fx.open();
    let s = fx.build(&mut ix, "k");
    assert!(s.errors > 0, "DUPLICATE_ID is reported: {s:?}");
    let v = ix.view("k").unwrap();
    let mem = MemoryView::from_corpus(
        &corpus::load_corpus(&fx.source(), &loc()).unwrap(),
        Vec::new(),
    );
    let dup = vec!["acme.ref.dup".to_string()];

    let by_kind = id_paths(v.metas_by_kind(&Kind::ALL, Origin::Accepted).unwrap());
    assert_eq!(
        by_kind,
        id_paths(mem.metas_by_kind(&Kind::ALL, Origin::Accepted).unwrap())
    );
    assert_eq!(
        by_kind
            .iter()
            .filter(|(id, _)| id == "acme.ref.dup")
            .count(),
        1
    );
    let by_id = id_paths(v.metas_by_ids(&dup, Origin::Accepted).unwrap());
    assert_eq!(by_id, vec![("acme.ref.dup".to_string(), first.clone())]);
    assert_eq!(
        by_id,
        id_paths(mem.metas_by_ids(&dup, Origin::Accepted).unwrap())
    );

    // Metadata and content always come from the same file.
    let records = v.records(&dup, Origin::Accepted).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].path, first);
    assert_eq!(records[0].parsed.record.common().title, "First copy");
    assert_eq!(
        v.raw("acme.ref.dup", Origin::Accepted)
            .unwrap()
            .unwrap()
            .path,
        first
    );

    // The shadowed copy is not searchable or selectable, as in the memory view.
    assert!(v.fulltext(&toks("zeppelin"), 10).unwrap().is_empty());
    assert!(mem.fulltext(&toks("zeppelin"), 10).unwrap().is_empty());
    assert_eq!(v.fulltext(&toks("alpha"), 10).unwrap(), dup);
    let tp = [TaskPath {
        repo: Some("mobile".into()),
        path: "app/dup/x.kt".into(),
    }];
    assert!(v.path_candidates(&tp).unwrap().is_empty());
    assert!(mem.path_candidates(&tp).unwrap().is_empty());
}

/// Overwrite the first page of table `name` in the (checkpointed) database file.
fn damage_table(fx: &Fixture, name: &str) {
    let (page_size, root) = {
        let c = rusqlite::Connection::open(fx.db()).unwrap();
        let page_size: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        let root: i64 = c
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE name = ?1",
                [name],
                |r| r.get(0),
            )
            .unwrap();
        (page_size, root)
    };
    let mut bytes = fs::read(fx.db()).unwrap();
    let start = usize::try_from((root - 1) * page_size).unwrap();
    let end = start + usize::try_from(page_size).unwrap();
    bytes[start..end].fill(0xA5);
    fs::write(fx.db(), &bytes).unwrap();
}

#[test]
fn corruption_found_by_a_query_is_recovered() {
    let fx = Fixture::new();
    for i in 0..50 {
        fx.write(
            &ref_path(&format!("q{i:02}")),
            &reference(&format!("acme.ref.q{i:02}"), "Query", "q", ""),
        );
    }
    {
        let mut ix = fx.open();
        fx.build(&mut ix, "k");
    }
    // Only `docs` is damaged: opening and the warm `ensure` never read it.
    damage_table(&fx, "docs");
    let mut ix = fx.open();
    assert!(ix.recovery().is_none());
    let warm = fx.build(&mut ix, "k");
    assert!(warm.reused && warm.recovery.is_none(), "{warm:?}");
    let err = ix
        .view("k")
        .and_then(|v| v.metas_by_kind(&Kind::ALL, Origin::Accepted))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::IndexError, "{err}");

    let src = fx.source();
    let run = ix
        .with_view("k", &src, &loc(), None, |v| accepted_ids(v))
        .unwrap();
    assert_eq!(run.value.len(), 52);
    let rebuilt = run.rebuilt.expect("the snapshot was rebuilt");
    assert!(!rebuilt.reused);
    let rec = rebuilt.recovery.expect("recovery is reported");
    assert_eq!(rec.kind, RecoveryKind::Corrupt);
    assert!(rec.moved_to.as_ref().is_some_and(|p| p.exists()), "{rec:?}");
    assert_eq!(rec.diagnostic().code, "INDEX_RECOVERED");
    assert_eq!(ix.recovery(), Some(&rec));

    // The replacement is healthy and serves later calls directly.
    assert!(ix.recover_if_damaged().unwrap().is_none());
    let again = ix
        .with_view("k", &src, &loc(), None, |v| Ok(accepted_ids(v)?.len()))
        .unwrap();
    assert_eq!((again.value, again.rebuilt), (52, None));
    drop(ix);
    let ix = fx.open();
    assert!(ix.recovery().is_none());
    assert_eq!(all_accepted(&ix.view("k").unwrap()).len(), 52);
}

#[test]
fn recover_if_damaged_replaces_only_a_damaged_database() {
    let fx = Fixture::new();
    {
        let mut ix = fx.open();
        fx.build(&mut ix, "k");
        assert!(ix.recover_if_damaged().unwrap().is_none());
        assert!(ix.has_snapshot("k").unwrap(), "a healthy index is kept");
    }
    damage_table(&fx, "docs");
    let mut ix = fx.open();
    let rec = ix.recover_if_damaged().unwrap().expect("damage is found");
    assert_eq!(rec.kind, RecoveryKind::Corrupt);
    assert!(rec.moved_to.is_some());
    assert!(!ix.has_snapshot("k").unwrap());
    let s = fx.build(&mut ix, "k");
    assert_eq!(s.recovery.map(|r| r.kind), Some(RecoveryKind::Corrupt));
}

#[test]
fn with_view_rebuilds_a_collected_snapshot_and_passes_other_errors_through() {
    let fx = Fixture::new();
    let mut ix = fx.open();
    fx.build(&mut ix, "k");
    // Another process collects the snapshot between `ensure` and the view.
    fx.open().gc(0).unwrap();
    let src = fx.source();
    let run = ix
        .with_view("k", &src, &loc(), None, |v| accepted_ids(v))
        .unwrap();
    assert_eq!(run.value.len(), 2);
    let rebuilt = run.rebuilt.expect("rebuilt");
    assert!(!rebuilt.reused && rebuilt.recovery.is_none(), "{rebuilt:?}");

    let calls = Cell::new(0);
    let err = ix
        .with_view("k", &src, &loc(), None, |_| -> Result<()> {
            calls.set(calls.get() + 1);
            Err(KbError::invalid_input("callback failed"))
        })
        .unwrap_err();
    assert_eq!((err.code, calls.get()), (ErrorCode::InvalidInput, 1));
    // An index error on a healthy database with the snapshot present is not retried.
    let err = ix
        .with_view("k", &src, &loc(), None, |_| -> Result<()> {
            calls.set(calls.get() + 1);
            Err(KbError::new(ErrorCode::IndexError, "query failed"))
        })
        .unwrap_err();
    assert_eq!((err.code, calls.get()), (ErrorCode::IndexError, 2));
}
