//! Context assembly, routing, search and show over the synthetic multi-repository fixture
//! in `core/tests/fixtures/context` (see its README).
//!
//! Golden text: `core/tests/fixtures/context/golden/*.txt` pins the rendered output of one
//! representative request. Regenerate intentionally with `KB_UPDATE_GOLDEN=1 cargo test -p kb
//! --test context_golden` and review the diff.
mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kb::context::memory::MemoryView;
use kb::context::{
    self, Completeness, ContextRequest, ContextResult, DimScope, SectionsMode, SettingSource,
    TaskEnv, Tier, UnitBody, canonical_json, estimate_tokens, receipt_id_of,
};
use kb::corpus::{Corpus, load_corpus};
use kb::diag::Diagnostic;
use kb::error::ErrorCode;
use kb::knowledge::{
    Freshness, KnowledgeView, ProposalChange, ProposalEntry, Selection, SnapshotInfo,
};
use kb::model::{BudgetUnit, Intent, Kind, Profile, ProfileLocation, SettingValue};
use kb::normalize;
use kb::output::{CommandOutput, Format, envelope};
use kb::parse::parse_record;
use kb::source::WorkingTreeSource;
use kb::util::sha256_hex;
use serde_json::{Map, Value, json};

const FORMATS: [Format; 3] = [Format::Compact, Format::Human, Format::Json];
const AUTH_PATH: &str = "mobile:app/src/auth/storage/TokenStore.kt";
const UI_PATH: &str = "mobile:app/src/ui/catalog/CatalogScreen.kt";

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/context")
}

fn loc() -> ProfileLocation {
    ProfileLocation::for_profile(Profile::Project)
}

fn corpus_at(root: &Path) -> Corpus {
    load_corpus(&WorkingTreeSource::new(root), &loc()).expect("fixture loads")
}

fn view_at(root: &Path) -> MemoryView {
    MemoryView::from_corpus(&corpus_at(root), Vec::new())
}

fn view() -> MemoryView {
    view_at(&fixture_root())
}

fn snapshot() -> SnapshotInfo {
    let rev = "1111111111111111111111111111111111111111".to_string();
    SnapshotInfo {
        profile: "project".into(),
        remote: "origin".into(),
        source: "file:///synthetic/acme-kb".into(),
        approved_ref: "refs/heads/main".into(),
        selection: Selection::Latest,
        freshness: Freshness::Verified,
        revision: Some(rev.clone()),
        latest_approved: Some(rev),
        approved: Some(true),
        pin: None,
        overlay: None,
        engine_version: kb::versions::ENGINE_VERSION.into(),
        key: "sha256:2222222222222222222222222222222222222222222222222222222222222222".into(),
        content_digest: None,
    }
}

fn env() -> TaskEnv {
    TaskEnv::new(snapshot())
}

/// An environment whose host checkout is identified as registry repo `repo`.
fn host_env(repo: &str) -> TaskEnv {
    let mut e = env();
    e.host_repo = Some(repo.into());
    e.host_repo_source = Some("argument".into());
    e
}

fn known(ids: &[&str]) -> DimScope {
    DimScope::Known(ids.iter().map(|s| s.to_string()).collect())
}

fn owned(ids: Vec<&str>) -> Vec<String> {
    ids.into_iter().map(str::to_string).collect()
}

fn version(s: &str) -> semver::Version {
    semver::Version::parse(s).unwrap()
}

fn auth_request(mobile_version: Option<&str>) -> ContextRequest {
    let mut req = ContextRequest::new(Intent::Implement);
    req.paths = vec![AUTH_PATH.into()];
    if let Some(v) = mobile_version {
        req.host_versions = vec![("mobile".into(), version(v))];
    }
    req
}

fn assemble_with(req: &ContextRequest, view: &dyn KnowledgeView, format: Format) -> ContextResult {
    context::assemble(req, &env(), view, format).expect("context assembles")
}

fn assemble(req: &ContextRequest) -> ContextResult {
    assemble_with(req, &view(), Format::Compact)
}

fn unit_ids(r: &ContextResult, tier: Tier) -> Vec<&str> {
    r.units
        .iter()
        .filter(|u| u.tier == tier)
        .map(|u| u.id.as_str())
        .collect()
}

fn record_ids(r: &ContextResult) -> BTreeSet<&str> {
    r.units.iter().map(|u| u.record_id.as_str()).collect()
}

fn reason_codes(r: &ContextResult) -> Vec<&str> {
    r.header.reasons.iter().map(|x| x.code.as_str()).collect()
}

/// Copy the fixture into a fresh temporary directory (for scenario-specific records).
fn copy_fixture() -> (tempfile::TempDir, PathBuf) {
    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for e in fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let target = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_dir(&e.path(), &target);
            } else {
                fs::copy(e.path(), &target).unwrap();
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("kb");
    copy_dir(&fixture_root().join("project"), &root.join("project"));
    (dir, root)
}

fn check_golden(name: &str, actual: &str) {
    let path = fixture_root().join("golden").join(name);
    if std::env::var_os("KB_UPDATE_GOLDEN").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden file {} ({e}); run with KB_UPDATE_GOLDEN=1",
            path.display()
        )
    });
    if actual != expected {
        let (a, e): (Vec<&str>, Vec<&str>) = (actual.lines().collect(), expected.lines().collect());
        let line = (0..a.len().max(e.len()))
            .find(|&i| a.get(i) != e.get(i))
            .unwrap_or(0);
        panic!(
            "output differs from {} at line {}:\n  actual:   {:?}\n  expected: {:?}\n\
             review and regenerate with KB_UPDATE_GOLDEN=1",
            path.display(),
            line + 1,
            a.get(line),
            e.get(line)
        );
    }
}

// ---------------------------------------------------------------------------------------
// Fixture and routing
// ---------------------------------------------------------------------------------------

#[test]
fn fixture_loads_without_diagnostics() {
    let corpus = corpus_at(&fixture_root());
    assert!(corpus.diagnostics.is_empty(), "{:#?}", corpus.diagnostics);
    assert_eq!(corpus.records().count(), 28);
    let kinds: BTreeSet<Kind> = corpus.records().map(|(_, p)| p.record.kind()).collect();
    assert_eq!(
        kinds.len(),
        Kind::ALL.len(),
        "every record kind is represented"
    );
}

#[test]
fn routing_fixtures_pass() {
    let (cases, diags) =
        context::routing::load_cases(&WorkingTreeSource::new(fixture_root()), &loc()).unwrap();
    assert!(diags.is_empty(), "{diags:#?}");
    assert_eq!(cases.len(), 19);
    let report = context::routing::run_cases(&view(), &cases, &env());
    assert!(
        report.ok(),
        "{}",
        context::routing::render_report(&report, Format::Human)
    );
    assert!(report.failure().is_none());
    let json = context::routing::report_to_json(&report);
    assert_eq!(json["passed"], 19);
    assert_eq!(json["failed"], 0);
}

#[test]
fn routing_reports_failures_and_invalid_cases() {
    let (_tmp, root) = copy_fixture();
    common::write(
        &root.join("project/routing-tests/broken.toml"),
        r#"schema = 1
[[case]]
name = "wrong expectation"
intent = "implement"
paths = ["mobile:app/src/auth/storage/TokenStore.kt"]
host_versions = ["mobile=2.3.0"]
expect_mandatory = ["acme.backend.rate-limit"]
forbid = ["acme.mobile.token-storage"]
expect_status = "partial"

[[case]]
name = "bad version"
intent = "implement"
host_versions = ["mobile=two"]
expect_status = "complete"
"#,
    );
    common::write(
        &root.join("project/routing-tests/notes.txt"),
        "not a fixture",
    );
    let (cases, diags) =
        context::routing::load_cases(&WorkingTreeSource::new(&root), &loc()).unwrap();
    let codes: Vec<&str> = diags.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"ROUTING_TEST_INVALID"), "{diags:#?}");
    assert!(codes.contains(&"NON_ROUTING_FILE"), "{diags:#?}");
    assert_eq!(cases.len(), 20, "the invalid case is skipped");
    let report = context::routing::run_cases(&view_at(&root), &cases, &env());
    assert_eq!(report.failed(), 1);
    let failed = report.cases.iter().find(|c| !c.passed).unwrap();
    assert_eq!(failed.failures.len(), 3, "{:?}", failed.failures);
    let err = report.failure().unwrap();
    assert_eq!(err.code, ErrorCode::RoutingTestsFailed);
    assert!(context::routing::render_report(&report, Format::Compact).contains("FAIL"));
}

// ---------------------------------------------------------------------------------------
// Routing semantics
// ---------------------------------------------------------------------------------------

#[test]
fn cross_repo_contract_is_reachable_without_irrelevant_records() {
    let r = assemble(&auth_request(Some("2.3.0")));
    assert_eq!(r.status(), Completeness::Complete, "{:?}", r.header.reasons);
    assert!(r.failure().is_none());
    // Kind order policy, invariant, contract, gap; id order within a kind.
    assert_eq!(
        unit_ids(&r, Tier::Mandatory),
        vec![
            "acme.mobile.auth-network",
            "acme.mobile.network",
            "acme.mobile.token-storage",
            "acme.product.logging",
            "acme.product.network",
            "acme.contract.token-refresh",
        ]
    );
    // The mobile/backend contract applies through its mobile party module although the
    // task touches no backend path.
    let contract = r
        .units
        .iter()
        .find(|u| u.id == "acme.contract.token-refresh")
        .unwrap();
    assert_eq!(
        contract.why,
        "applies (modules, version); mobile 2.3.0 satisfies >=2.0.0; \
         also required by acme.mobile.token-storage"
    );
    // A backend-only invariant is reached through `requires`, ignoring repo filters.
    assert_eq!(
        unit_ids(&r, Tier::Dependency),
        vec!["acme.backend.refresh-rotation"]
    );
    let dep = r
        .units
        .iter()
        .find(|u| u.id == "acme.backend.refresh-rotation")
        .unwrap();
    assert_eq!(dep.labels, vec!["outside-task-scope"]);
    assert_eq!(dep.why, "required by acme.mobile.token-storage");
    assert!(r.requires.contains(&(
        "acme.mobile.token-storage".to_string(),
        "acme.backend.refresh-rotation".to_string()
    )));
    // Irrelevant backend and UI knowledge stays out; `related` is followed one hop only.
    let ids = record_ids(&r);
    for id in [
        "acme.backend.rate-limit",
        "acme.backend.sql-parameters",
        "acme.backend.money-minor-units",
        "acme.mobile.ui-accessibility",
        "acme.decision.compose-ui",
        "acme.reference.oauth",
    ] {
        assert!(!ids.contains(id), "{id} must not be included");
    }
    assert_eq!(
        unit_ids(&r, Tier::Supplementary),
        vec![
            "acme.feature.login",
            "acme.decision.encrypted-storage",
            "acme.procedure.debug-token-refresh",
        ]
    );
    let text = context::render(&r, Format::Compact, false);
    assert!(text.contains(
        "- consumer MUST [retry-once] Retry the original request at most once after a successful refresh that followed HTTP 401."
    ));
    assert!(text.contains(
        "  except [offline]: When the device is offline the request is queued instead of retried."
    ));
    assert!(text.contains(
        "  if: The user signs out explicitly or the account is removed from the device."
    ));
}

#[test]
fn obligation_is_included_without_keyword_overlap() {
    let mut req = ContextRequest::new(Intent::Implement);
    req.task = Some("Сделать кнопку на экране каталога зелёной".into());
    req.paths = vec![UI_PATH.into()];
    req.host_versions = vec![("mobile".into(), version("2.3.0"))];
    let r = assemble(&req);
    let mandatory = unit_ids(&r, Tier::Mandatory);
    assert!(mandatory.contains(&"acme.mobile.ui-accessibility"));
    assert!(mandatory.contains(&"acme.gap.offline-catalog"));
    // Prove there is no keyword overlap between the task and the obligation.
    let unit = r
        .units
        .iter()
        .find(|u| u.id == "acme.mobile.ui-accessibility")
        .unwrap();
    let UnitBody::Record(p) = &unit.body else {
        panic!("record unit expected")
    };
    let mut record_text = p.record.common().title.to_string();
    for n in p.record.normative() {
        record_text.push(' ');
        record_text.push_str(n.text);
    }
    let task: BTreeSet<String> = normalize::tokens(req.task.as_deref().unwrap())
        .into_iter()
        .collect();
    let overlap: Vec<String> = normalize::tokens(&record_text)
        .into_iter()
        .filter(|t| task.contains(t))
        .collect();
    assert!(overlap.is_empty(), "{overlap:?}");
    assert_eq!(r.status(), Completeness::Complete, "{:?}", r.header.reasons);
}

#[test]
fn multilingual_aliases_route_to_concepts() {
    let mut req = ContextRequest::new(Intent::Debug);
    req.task = Some("Бесконечный 401 после обновления токена".into());
    req.repos = vec!["mobile".into()];
    req.host_versions = vec![("mobile".into(), version("2.3.0"))];
    let r = assemble(&req);
    let sources = &r.header.scope.concepts["auth-token"];
    assert!(sources.contains("alias `токен*`"), "{sources:?}");
    let proc_unit = r
        .units
        .iter()
        .find(|u| u.id == "acme.procedure.debug-token-refresh")
        .expect("procedure routed by Cyrillic alias");
    assert_eq!(proc_unit.tier, Tier::Supplementary);
    assert!(
        proc_unit
            .signals
            .iter()
            .any(|s| s.detail == "alias `бесконечный 401`")
    );
    // The relevant but undetermined obligation is offered, labeled, and listed.
    let storage = r
        .units
        .iter()
        .find(|u| u.id == "acme.mobile.token-storage")
        .unwrap();
    assert_eq!(storage.tier, Tier::Supplementary);
    assert!(
        storage
            .labels
            .contains(&"applicability-undetermined".to_string())
    );
    assert!(
        r.header
            .undetermined
            .iter()
            .any(|u| u.id == "acme.mobile.token-storage")
    );
    assert_eq!(r.status(), Completeness::Partial);

    // Case and `ё` normalization; multi-token Cyrillic alias.
    let mut req = ContextRequest::new(Intent::Explain);
    req.task = Some("ОБНОВЛЕНИЕ ТОКЁНА".into());
    let r = assemble(&req);
    assert!(r.header.scope.concepts.contains_key("auth-token"));
}

#[test]
fn ambiguous_concepts_are_reported_with_candidates() {
    let mut req = ContextRequest::new(Intent::Explain);
    req.task = Some("Объясни, как устроена композиция".into());
    let r = assemble(&req);
    assert_eq!(r.header.scope.ambiguities.len(), 1);
    let a = &r.header.scope.ambiguities[0];
    assert_eq!(a.phrase, "композиция");
    assert_eq!(a.aliases, vec!["композици*"]);
    assert_eq!(a.candidates, vec!["object-composition", "ui-composition"]);
    assert_eq!(
        a.offered,
        vec![
            (
                "object-composition".to_string(),
                "acme.decision.domain-composition".to_string()
            ),
            (
                "ui-composition".to_string(),
                "acme.decision.compose-ui".to_string()
            ),
        ]
    );
    let compose = r
        .units
        .iter()
        .find(|u| u.id == "acme.decision.compose-ui")
        .unwrap();
    assert!(
        compose
            .labels
            .contains(&"ambiguity-candidate:ui-composition".to_string())
    );
    assert!(r.header.scope.concepts.is_empty(), "nothing was guessed");
    let text = context::render(&r, Format::Compact, false);
    assert!(text.contains("ambiguous: \"композиция\""), "{text}");

    // Explicit concept resolves it.
    req.concepts = vec!["ui-composition".into()];
    let r = assemble(&req);
    assert!(r.header.scope.ambiguities.is_empty());
    assert!(r.header.scope.concepts.contains_key("ui-composition"));
    assert!(!record_ids(&r).contains("acme.decision.domain-composition"));

    // A path hint resolves it too.
    let mut req = ContextRequest::new(Intent::Refactor);
    req.task = Some("Упростить композицию экрана".into());
    req.paths = vec![UI_PATH.into()];
    let r = assemble(&req);
    assert!(r.header.scope.ambiguities.is_empty());
    let src = &r.header.scope.concepts["ui-composition"];
    assert!(
        src.iter().any(|s| s.contains("concept path hint")),
        "{src:?}"
    );
}

#[test]
fn version_applicability_selects_and_conflicts() {
    let old = assemble(&auth_request(Some("1.9.0")));
    assert!(unit_ids(&old, Tier::Mandatory).contains(&"acme.mobile.legacy-keystore"));
    assert_eq!(old.status(), Completeness::Conflict);
    assert!(reason_codes(&old).contains(&"INCOMPATIBLE_DEPENDENCY"));
    let dep = old
        .units
        .iter()
        .find(|u| u.id == "acme.contract.token-refresh")
        .unwrap();
    assert!(dep.labels.contains(&"version-incompatible".to_string()));
    assert!(
        old.header
            .issues
            .iter()
            .any(|d| d.code == "INCOMPATIBLE_DEPENDENCY"
                && d.message.contains("mobile 1.9.0 does not satisfy >=2.0.0"))
    );

    let current = assemble(&auth_request(Some("2.3.0")));
    assert!(!record_ids(&current).contains("acme.mobile.legacy-keystore"));

    let unknown = assemble(&auth_request(None));
    assert_eq!(unknown.status(), Completeness::Partial);
    assert!(
        unknown
            .header
            .undetermined
            .iter()
            .any(|u| u.id == "acme.mobile.legacy-keystore")
    );
    assert!(reason_codes(&unknown).contains(&"DEPENDENCY_VERSION_UNDETERMINED"));
}

#[test]
fn directory_paths_resolve_to_the_modules_they_contain() {
    let v = view();
    let run = |path: &str| {
        let mut req = ContextRequest::new(Intent::Implement);
        req.paths = vec![path.into()];
        req.host_versions = vec![("mobile".into(), version("2.3.0"))];
        context::assemble(&req, &host_env("mobile"), &v, Format::Compact).unwrap()
    };
    let file = run("app/src/auth/storage/TokenStore.kt");
    assert_eq!(
        file.status(),
        Completeness::Complete,
        "{:?}",
        file.header.reasons
    );
    let expected = owned(file.mandatory_ids());
    assert!(expected.contains(&"acme.mobile.token-storage".to_string()));
    // The module root as a directory, in every spelling the CLI passes through.
    for dir in [
        "app/src/auth",
        "app/src/auth/",
        "./app/src/auth",
        "mobile:app/src/auth",
    ] {
        let r = run(dir);
        assert_eq!(r.header.scope.modules, known(&["mobile.auth"]), "{dir}");
        assert_eq!(r.header.scope.features, known(&["login"]), "{dir}");
        assert_eq!(owned(r.mandatory_ids()), expected, "{dir}");
        assert_eq!(
            r.status(),
            Completeness::Complete,
            "{dir}: {:?}",
            r.header.reasons
        );
    }
    // A parent directory contains every module below it.
    let parent = run("app/src");
    assert_eq!(
        parent.header.scope.modules,
        known(&[
            "mobile.analytics",
            "mobile.auth",
            "mobile.payments",
            "mobile.ui"
        ])
    );
    let ids = owned(parent.mandatory_ids());
    for id in expected.iter().map(String::as_str).chain([
        "acme.mobile.ui-accessibility",
        "acme.gap.offline-catalog",
        "acme.contract.checkout-api",
        "acme.mobile.analytics-consent",
    ]) {
        assert!(ids.iter().any(|x| x == id), "{id} missing: {ids:?}");
    }
    // `mobile.analytics` (`app/src/ui/**/analytics/**`) has a wildcard before its directory
    // part. A directory below its literal prefix may contain it, although another module
    // (`mobile.ui`, by the directory form) resolves for the same directory; a file there that
    // the glob does not match stays outside it.
    for dir in ["app/src/ui/catalog", "app/src/ui/catalog/", "app/src/ui"] {
        let r = run(dir);
        assert_eq!(
            r.header.scope.modules,
            known(&["mobile.analytics", "mobile.ui"]),
            "{dir}"
        );
        assert!(
            r.mandatory_ids().contains(&"acme.mobile.analytics-consent"),
            "{dir}"
        );
        assert_eq!(
            r.status(),
            Completeness::Complete,
            "{dir}: {:?}",
            r.header.reasons
        );
    }
    let screen = run("app/src/ui/catalog/CatalogScreen.kt");
    assert_eq!(screen.header.scope.modules, known(&["mobile.ui"]));
    assert!(
        !screen
            .mandatory_ids()
            .contains(&"acme.mobile.analytics-consent")
    );
    // A directory that contains no registry module leaves the module and feature scope
    // unknown: module-scoped obligations are undetermined (partial), never dropped.
    let outside = run("app/src/checkout");
    assert_eq!(outside.header.scope.modules, DimScope::Unknown);
    assert_eq!(outside.header.scope.features, DimScope::Unknown);
    assert_eq!(outside.status(), Completeness::Partial);
    assert!(reason_codes(&outside).contains(&"UNDETERMINED_OBLIGATIONS"));
    assert!(
        outside
            .header
            .undetermined
            .iter()
            .any(|u| u.id == "acme.mobile.token-storage")
    );
    assert!(
        outside
            .header
            .issues
            .iter()
            .any(|d| d.code == "PATH_SCOPE_UNKNOWN" && d.message.contains("app/src/checkout"))
    );
    // A file path in no module keeps the module scope known (and empty).
    let stray = run("app/src/Main.kt");
    assert_eq!(stray.header.scope.modules, DimScope::Known(BTreeSet::new()));
    assert!(!owned(stray.mandatory_ids()).contains(&"acme.mobile.token-storage".to_string()));
}

#[test]
fn explicit_modules_and_single_repo_features_make_their_repo_known() {
    let v = view();
    let run = |req: &ContextRequest, host: &str| {
        context::assemble(req, &host_env(host), &v, Format::Compact).unwrap()
    };
    let mut by_module = ContextRequest::new(Intent::Implement);
    by_module.modules = vec!["backend.api".into()];
    let m = run(&by_module, "mobile");
    let mut by_path = ContextRequest::new(Intent::Implement);
    by_path.paths = vec!["backend:src/api/token/RefreshHandler.kt".into()];
    let p = run(&by_path, "mobile");
    assert_eq!(m.header.scope.repos, known(&["backend", "mobile"]));
    assert_eq!(m.header.scope.repos, p.header.scope.repos);
    for id in [
        "acme.backend.rate-limit",
        "acme.backend.money-minor-units",
        "acme.backend.refresh-rotation",
        "acme.backend.sql-parameters",
    ] {
        assert!(m.mandatory_ids().contains(&id), "{id}");
    }
    assert_eq!(m.mandatory_ids(), p.mandatory_ids());

    // `catalog` is declared in the mobile repo only.
    let mut by_feature = ContextRequest::new(Intent::Implement);
    by_feature.features = vec!["catalog".into()];
    let f = run(&by_feature, "backend");
    assert_eq!(f.header.scope.repos, known(&["backend", "mobile"]));
    assert!(f.mandatory_ids().contains(&"acme.mobile.network"));
    // `login` spans two repos and says nothing about which one the task touches.
    let mut multi = ContextRequest::new(Intent::Implement);
    multi.features = vec!["login".into()];
    assert_eq!(
        run(&multi, "backend").header.scope.repos,
        known(&["backend"])
    );
}

#[test]
fn version_constraints_on_other_repos_are_checked() {
    let (_tmp, root) = copy_fixture();
    let edit = |rel: &str, from: &str, to: &str| {
        let path = root.join("project/knowledge").join(rel);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(from), "{rel}");
        fs::write(&path, text.replacen(from, to, 1)).unwrap();
    };
    edit(
        "invariants/backend-refresh-rotation.md",
        "[scope]",
        "[applicability]\nversions = { backend = \">=3.0.0\" }\n\n[scope]",
    );
    let corpus = corpus_at(&root);
    assert!(corpus.diagnostics.is_empty(), "{:#?}", corpus.diagnostics);
    let report = kb::validate::validate_corpus(&corpus);
    assert_eq!(report.errors, 0, "{:#?}", report.diagnostics);
    let v = MemoryView::from_corpus(&corpus, Vec::new());
    let run = |backend: Option<&str>| {
        let mut req = auth_request(Some("2.3.0"));
        if let Some(b) = backend {
            req.host_versions.push(("backend".into(), version(b)));
        }
        assemble_with(&req, &v, Format::Compact)
    };
    let dep_labels = |r: &ContextResult| {
        r.units
            .iter()
            .find(|u| u.id == "acme.backend.refresh-rotation")
            .expect("the required dependency is never dropped")
            .labels
            .clone()
    };

    // An explicit version of the dependency's repo is checked although the repo is outside
    // the task repos.
    let old = run(Some("2.0.0"));
    assert_eq!(
        old.status(),
        Completeness::Conflict,
        "{:?}",
        old.header.reasons
    );
    assert!(reason_codes(&old).contains(&"INCOMPATIBLE_DEPENDENCY"));
    assert_eq!(
        dep_labels(&old),
        vec!["version-incompatible", "outside-task-scope"]
    );
    assert!(
        old.header
            .issues
            .iter()
            .any(|d| d.code == "INCOMPATIBLE_DEPENDENCY"
                && d.message.contains("backend 2.0.0 does not satisfy >=3.0.0"))
    );
    let new = run(Some("3.1.0"));
    assert_eq!(
        new.status(),
        Completeness::Complete,
        "{:?}",
        new.header.reasons
    );
    assert_eq!(dep_labels(&new), vec!["outside-task-scope"]);
    // A required dependency whose version cannot be checked is not assumed compatible.
    let unknown = run(None);
    assert_eq!(unknown.status(), Completeness::Partial);
    assert!(reason_codes(&unknown).contains(&"DEPENDENCY_VERSION_UNDETERMINED"));
    assert!(dep_labels(&unknown).contains(&"version-undetermined".to_string()));

    // Mandatory selection: a constraint on another repo is checked once its version is known.
    edit(
        "contracts/token-refresh.md",
        "versions = { mobile = \">=2.0.0\" }",
        "versions = { mobile = \">=2.0.0\", backend = \">=3.0.0\" }",
    );
    let v = view_at(&root);
    let mut req = auth_request(Some("2.3.0"));
    req.host_versions.push(("backend".into(), version("2.0.0")));
    let r = assemble_with(&req, &v, Format::Compact);
    assert_eq!(r.status(), Completeness::Conflict, "{:?}", r.header.reasons);
    let contract = r
        .units
        .iter()
        .find(|u| u.id == "acme.contract.token-refresh")
        .unwrap();
    assert_eq!(contract.tier, Tier::Dependency);
    assert!(
        contract
            .labels
            .contains(&"version-incompatible".to_string())
    );
}

/// A record that is mandatory by its own scope and also reached through `requires` checks
/// every version constraint, like a record reached only through `requires`: the outcome must
/// not depend on whether the dependency also matches the task scope.
#[test]
fn mandatory_record_reached_through_requires_checks_every_version_constraint() {
    let (_tmp, root) = copy_fixture();
    let path = root.join("project/knowledge/contracts/token-refresh.md");
    let text = fs::read_to_string(&path).unwrap();
    let from = "versions = { mobile = \">=2.0.0\" }";
    assert!(text.contains(from));
    fs::write(
        &path,
        text.replacen(
            from,
            "versions = { mobile = \">=2.0.0\", backend = \">=3.0.0\" }",
            1,
        ),
    )
    .unwrap();
    let corpus = corpus_at(&root);
    let report = kb::validate::validate_corpus(&corpus);
    assert_eq!(report.errors, 0, "{:#?}", report.diagnostics);
    let v = MemoryView::from_corpus(&corpus, Vec::new());
    let run = |backend: Option<&str>| {
        let mut req = auth_request(Some("2.3.0"));
        if let Some(b) = backend {
            req.host_versions.push(("backend".into(), version(b)));
        }
        assemble_with(&req, &v, Format::Compact)
    };
    let contract = |r: &ContextResult| {
        r.units
            .iter()
            .find(|u| u.id == "acme.contract.token-refresh")
            .expect("the contract is included")
            .clone()
    };

    let unknown = run(None);
    assert_eq!(
        unknown.status(),
        Completeness::Partial,
        "{:?}",
        unknown.header.reasons
    );
    assert!(reason_codes(&unknown).contains(&"DEPENDENCY_VERSION_UNDETERMINED"));
    let c = contract(&unknown);
    assert_eq!(c.tier, Tier::Mandatory, "it still applies by its own scope");
    assert_eq!(c.labels, vec!["version-undetermined"]);
    assert!(unknown.header.issues.iter().any(|d| {
        d.code == "DEPENDENCY_VERSION_UNDETERMINED"
            && d.message.contains("acme.contract.token-refresh")
            && d.message
                .contains("backend version unknown (requires >=3.0.0)")
    }));

    let known = run(Some("3.1.0"));
    assert_eq!(
        known.status(),
        Completeness::Complete,
        "{:?}",
        known.header.reasons
    );
    assert!(contract(&known).labels.is_empty());
}

/// Validation only warns when an accepted record requires a deprecated one
/// (`REQUIRES_DEPRECATED`); context includes such a dependency, labeled and with a warning,
/// without making the result incomplete. Draft and superseded targets stay incomplete.
#[test]
fn deprecated_dependencies_are_included_with_a_warning() {
    let (_tmp, root) = copy_fixture();
    common::write(
        &root.join("project/knowledge/invariants/mobile-session-cookie.md"),
        "+++\nschema = 1\nid = \"acme.mobile.session-cookie\"\nkind = \"invariant\"\n\
         title = \"Session cookie\"\nstatus = \"deprecated\"\nowner = \"team-mobile\"\n\n\
         [scope]\nmodules = [\"mobile.ui\"]\n\n[[statements]]\nid = \"cookie\"\n\
         level = \"must\"\ntext = \"Send the session cookie with every request.\"\n+++\n",
    );
    let extra = |requires: &str| {
        common::write(
            &root.join("project/knowledge/policies/extra.md"),
            &format!(
                "+++\nschema = 1\nid = \"acme.mobile.extra\"\nkind = \"policy\"\n\
                 title = \"Extra\"\nstatus = \"accepted\"\nowner = \"team-mobile\"\n\n\
                 [scope]\nrepos = [\"mobile\"]\n\n[links]\nrequires = [{requires}]\n\n\
                 [[rules]]\nid = \"x\"\nlevel = \"must\"\ntext = \"Do the extra thing.\"\n+++\n"
            ),
        );
    };
    extra("\"acme.mobile.session-cookie\"");
    let corpus = corpus_at(&root);
    let report = kb::validate::validate_corpus(&corpus);
    assert_eq!(report.errors, 0, "{:#?}", report.diagnostics);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "REQUIRES_DEPRECATED")
    );

    let req = auth_request(Some("2.3.0"));
    let r = assemble_with(
        &req,
        &MemoryView::from_corpus(&corpus, Vec::new()),
        Format::Compact,
    );
    assert_eq!(r.status(), Completeness::Complete, "{:?}", r.header.reasons);
    assert!(r.failure().is_none());
    let dep = r
        .units
        .iter()
        .find(|u| u.id == "acme.mobile.session-cookie")
        .expect("the deprecated dependency is included");
    assert_eq!(dep.tier, Tier::Dependency);
    assert_eq!(dep.status, kb::model::Status::Deprecated);
    assert_eq!(dep.labels, vec!["deprecated", "outside-task-scope"]);
    assert_eq!(dep.why, "required by acme.mobile.extra");
    let warning = r
        .header
        .issues
        .iter()
        .find(|d| d.code == "REQUIRES_DEPRECATED")
        .expect("a warning names the deprecated dependency");
    assert_eq!(warning.severity, kb::diag::Severity::Warning);
    assert_eq!(warning.record.as_deref(), Some("acme.mobile.extra"));
    assert!(warning.message.contains("acme.mobile.session-cookie"));
    assert!(!reason_codes(&r).contains(&"REQUIRES_NOT_ACCEPTED"));
    let text = context::render(&r, Format::Compact, false);
    assert!(
        text.contains("### dependency acme.mobile.session-cookie"),
        "{text}"
    );
    assert!(
        text.contains("labels: deprecated, outside-task-scope"),
        "{text}"
    );

    // Superseded (and draft) targets are not accepted knowledge: incomplete, not included.
    extra("\"acme.mobile.session-cookie\", \"acme.contract.legacy-auth\"");
    let r = assemble_with(&req, &view_at(&root), Format::Compact);
    assert_eq!(r.status(), Completeness::Incomplete);
    assert!(reason_codes(&r).contains(&"REQUIRES_NOT_ACCEPTED"));
    assert!(!record_ids(&r).contains("acme.contract.legacy-auth"));
    assert!(record_ids(&r).contains("acme.mobile.session-cookie"));
}

// ---------------------------------------------------------------------------------------
// Settings and dependencies
// ---------------------------------------------------------------------------------------

fn setting<'a>(r: &'a ContextResult, name: &str) -> &'a context::EffectiveSetting {
    r.header
        .settings
        .iter()
        .find(|s| s.setting.name == name)
        .unwrap_or_else(|| panic!("setting {name}"))
}

#[test]
fn overrides_resolve_to_the_most_specific_value() {
    let auth = assemble(&auth_request(Some("2.3.0")));
    let t = setting(&auth, "request-timeout-ms");
    assert_eq!(t.value, Some(SettingValue::Integer(3000)));
    assert!(
        matches!(&t.source, SettingSource::Override { by, .. } if by == "acme.mobile.auth-network")
    );
    assert_eq!(
        setting(&auth, "retry-count").source,
        SettingSource::Base,
        "no override"
    );

    let mut ui = ContextRequest::new(Intent::Implement);
    ui.paths = vec![UI_PATH.into()];
    let ui = assemble(&ui);
    let t = setting(&ui, "request-timeout-ms");
    assert_eq!(t.value, Some(SettingValue::Integer(5000)));
    assert!(matches!(&t.source, SettingSource::Override { by, .. } if by == "acme.mobile.network"));

    let mut backend = ContextRequest::new(Intent::Implement);
    backend.paths = vec!["backend:src/api/token/RefreshHandler.kt".into()];
    let backend = assemble(&backend);
    assert_eq!(
        setting(&backend, "request-timeout-ms").value,
        Some(SettingValue::Integer(10000))
    );
    let text = context::render(&auth, Format::Compact, false);
    assert!(text.contains(
        "- acme.product.network#request-timeout-ms = 3000 (override by acme.mobile.auth-network"
    ));
}

#[test]
fn conflicting_overrides_make_the_result_conflict() {
    let (_tmp, root) = copy_fixture();
    let policy = |id: &str, scope: &str, value: i64| {
        format!(
            "+++\nschema = 1\nid = \"{id}\"\nkind = \"policy\"\ntitle = \"Retry override {value}\"\n\
             status = \"accepted\"\nowner = \"arch\"\n\n[scope]\n{scope}\n\n[[overrides]]\n\
             target = \"acme.product.network#retry-count\"\nvalue = {value}\nreason = \"synthetic\"\n+++\n"
        )
    };
    common::write(
        &root.join("project/knowledge/policies/payments-retry.md"),
        &policy(
            "acme.mobile.payments-retry",
            "modules = [\"mobile.payments\"]",
            5,
        ),
    );
    common::write(
        &root.join("project/knowledge/policies/checkout-retry.md"),
        &policy("acme.mobile.checkout-retry", "features = [\"checkout\"]", 1),
    );
    let mut req = ContextRequest::new(Intent::Implement);
    req.paths = vec!["mobile:app/src/payments/Pay.kt".into()];
    req.host_versions = vec![("mobile".into(), version("2.3.0"))];
    let r = assemble_with(&req, &view_at(&root), Format::Compact);
    let s = setting(&r, "retry-count");
    assert_eq!(s.value, None);
    let SettingSource::Conflict { candidates } = &s.source else {
        panic!("conflict expected: {:?}", s.source)
    };
    assert_eq!(candidates.len(), 2);
    assert_eq!(r.status(), Completeness::Conflict);
    assert!(reason_codes(&r).contains(&"SETTING_CONFLICT"));
    let text = context::render(&r, Format::Compact, false);
    assert!(text.contains("acme.product.network#retry-count = CONFLICT between"));
    assert_eq!(r.failure().unwrap().code, ErrorCode::ContextIncomplete);
}

#[test]
fn equal_overrides_with_incomparable_scopes_agree() {
    let (_tmp, root) = copy_fixture();
    let policy = |id: &str, scope: &str, value: &str| {
        format!(
            "+++\nschema = 1\nid = \"{id}\"\nkind = \"policy\"\ntitle = \"Override {id}\"\n\
             status = \"accepted\"\nowner = \"arch\"\n\n[scope]\n{scope}\n\n[[overrides]]\n\
             target = \"{target}\"\nvalue = {value}\nreason = \"synthetic\"\n+++\n",
            target = "acme.product.network#retry-count",
        )
    };
    common::write(
        &root.join("project/knowledge/policies/payments-retry.md"),
        &policy(
            "acme.mobile.payments-retry",
            "modules = [\"mobile.payments\"]",
            "1",
        ),
    );
    common::write(
        &root.join("project/knowledge/policies/checkout-retry.md"),
        &policy(
            "acme.mobile.checkout-retry",
            "features = [\"checkout\"]",
            "1",
        ),
    );
    // Validation accepts equal values (only different values are OVERRIDE_AMBIGUOUS), so
    // context assembly must not turn them into a conflict.
    let corpus = corpus_at(&root);
    let report = kb::validate::validate_corpus(&corpus);
    assert_eq!(report.errors, 0, "{:#?}", report.diagnostics);
    let mut req = ContextRequest::new(Intent::Implement);
    req.paths = vec!["mobile:app/src/payments/Pay.kt".into()];
    req.host_versions = vec![("mobile".into(), version("2.3.0"))];
    let r = assemble_with(
        &req,
        &MemoryView::from_corpus(&corpus, Vec::new()),
        Format::Compact,
    );
    let s = setting(&r, "retry-count");
    assert_eq!(s.value, Some(SettingValue::Integer(1)));
    assert!(
        matches!(&s.source, SettingSource::Override { by, .. } if by == "acme.mobile.checkout-retry"),
        "{:?}",
        s.source
    );
    assert!(!reason_codes(&r).contains(&"SETTING_CONFLICT"));
    assert_eq!(r.status(), Completeness::Complete, "{:?}", r.header.reasons);
}

/// A broader override next to two incomparable, more specific overrides: the most specific
/// applicable overrides (none strictly more specific) decide. Agreeing ones give the value
/// (validation accepts that corpus); disagreeing ones are the conflict, without the broader
/// override among the candidates.
#[test]
fn most_specific_overrides_decide_next_to_a_broader_override() {
    let (_tmp, root) = copy_fixture();
    let policy = |id: &str, scope: &str, value: i64| {
        format!(
            "+++\nschema = 1\nid = \"{id}\"\nkind = \"policy\"\ntitle = \"Override {id}\"\n\
             status = \"accepted\"\nowner = \"arch\"\n\n[scope]\n{scope}\n\n[[overrides]]\n\
             target = \"acme.product.network#retry-count\"\nvalue = {value}\n\
             reason = \"synthetic\"\n+++\n"
        )
    };
    let write = |name: &str, id: &str, scope: &str, value: i64| {
        common::write(
            &root.join("project/knowledge/policies").join(name),
            &policy(id, scope, value),
        );
    };
    write(
        "retry-a.md",
        "acme.mobile.retry-a",
        "repos = [\"mobile\"]",
        2,
    );
    write(
        "retry-b.md",
        "acme.mobile.retry-b",
        "modules = [\"mobile.payments\"]",
        1,
    );
    write(
        "retry-c.md",
        "acme.mobile.retry-c",
        "repos = [\"mobile\"]\nfeatures = [\"checkout\"]",
        1,
    );
    let mut req = ContextRequest::new(Intent::Implement);
    req.paths = vec!["mobile:app/src/payments/Pay.kt".into()];
    req.host_versions = vec![("mobile".into(), version("2.3.0"))];

    let corpus = corpus_at(&root);
    let report = kb::validate::validate_corpus(&corpus);
    assert_eq!(report.errors, 0, "{:#?}", report.diagnostics);
    let r = assemble_with(
        &req,
        &MemoryView::from_corpus(&corpus, Vec::new()),
        Format::Compact,
    );
    let s = setting(&r, "retry-count");
    assert_eq!(s.value, Some(SettingValue::Integer(1)), "{:?}", s.source);
    assert!(
        matches!(&s.source, SettingSource::Override { by, .. } if by == "acme.mobile.retry-b"),
        "{:?}",
        s.source
    );
    assert!(!reason_codes(&r).contains(&"SETTING_CONFLICT"));
    assert_eq!(r.status(), Completeness::Complete, "{:?}", r.header.reasons);

    // Disagreeing most specific overrides conflict; the broader one is not a candidate.
    write(
        "retry-c.md",
        "acme.mobile.retry-c",
        "repos = [\"mobile\"]\nfeatures = [\"checkout\"]",
        4,
    );
    let r = assemble_with(&req, &view_at(&root), Format::Compact);
    let s = setting(&r, "retry-count");
    assert_eq!(s.value, None);
    assert_eq!(
        s.source,
        SettingSource::Conflict {
            candidates: vec![
                ("acme.mobile.retry-b".to_string(), SettingValue::Integer(1)),
                ("acme.mobile.retry-c".to_string(), SettingValue::Integer(4)),
            ]
        }
    );
    assert_eq!(r.status(), Completeness::Conflict);

    // A strictly more specific override still wins over broader ones with other values.
    write(
        "retry-c.md",
        "acme.mobile.retry-c",
        "modules = [\"mobile.payments\"]\nfeatures = [\"checkout\"]",
        4,
    );
    let r = assemble_with(&req, &view_at(&root), Format::Compact);
    let s = setting(&r, "retry-count");
    assert_eq!(s.value, Some(SettingValue::Integer(4)), "{:?}", s.source);
    assert!(
        matches!(&s.source, SettingSource::Override { by, .. } if by == "acme.mobile.retry-c"),
        "{:?}",
        s.source
    );
}

#[test]
fn missing_and_unaccepted_requirements_make_the_result_incomplete() {
    let (_tmp, root) = copy_fixture();
    common::write(
        &root.join("project/knowledge/policies/extra.md"),
        "+++\nschema = 1\nid = \"acme.mobile.extra\"\nkind = \"policy\"\ntitle = \"Extra\"\n\
         status = \"accepted\"\nowner = \"team-mobile\"\n\n[scope]\nrepos = [\"mobile\"]\n\n\
         [links]\nrequires = [\"acme.contract.does-not-exist\", \"acme.mobile.biometric-login\"]\n\n\
         [[rules]]\nid = \"x\"\nlevel = \"must\"\ntext = \"Do the extra thing.\"\n+++\n",
    );
    let mut req = ContextRequest::new(Intent::Implement);
    req.repos = vec!["mobile".into()];
    let r = assemble_with(&req, &view_at(&root), Format::Compact);
    assert_eq!(r.status(), Completeness::Incomplete);
    let codes = reason_codes(&r);
    assert!(codes.contains(&"REQUIRES_MISSING"), "{codes:?}");
    assert!(codes.contains(&"REQUIRES_NOT_ACCEPTED"), "{codes:?}");
    assert!(!record_ids(&r).contains("acme.mobile.biometric-login"));
    let err = r.failure().unwrap();
    assert_eq!(err.code, ErrorCode::ContextIncomplete);
    assert_eq!(err.details["completeness"], "incomplete");
}

// ---------------------------------------------------------------------------------------
// Budget and packing
// ---------------------------------------------------------------------------------------

#[test]
fn budget_exceeded_reports_the_required_amount() {
    for unit in [BudgetUnit::TokensEst, BudgetUnit::Bytes] {
        for format in FORMATS {
            let mut req = auth_request(Some("2.3.0"));
            req.budget = Some(60);
            req.budget_unit = Some(unit);
            let err = context::assemble(&req, &env(), &view(), format).unwrap_err();
            assert_eq!(err.code, ErrorCode::ContextBudgetExceeded);
            assert_eq!(err.details["limit"], 60);
            assert_eq!(err.details["unit"], unit.as_str());
            let required = err.details["required"].as_u64().unwrap();
            assert!(required > 60);

            req.budget = Some(required);
            let r = assemble_with(&req, &view(), format);
            assert!(r.footer.budget.used <= required);
            assert_eq!(r.footer.counts.supplementary, 0);
            assert_eq!(
                r.footer.excluded_budget.len(),
                3,
                "all optional units excluded"
            );
            assert_eq!(unit_ids(&r, Tier::Dependency).len(), 1);

            req.budget = Some(required - 1);
            let err = context::assemble(&req, &env(), &view(), format).unwrap_err();
            assert_eq!(err.code, ErrorCode::ContextBudgetExceeded);
            assert_eq!(err.details["required"].as_u64(), Some(required));
        }
    }
}

#[test]
fn packing_keeps_whole_units_and_measures_bytes_exactly() {
    let mut req = auth_request(Some("2.3.0"));
    req.budget_unit = Some(BudgetUnit::Bytes);
    req.budget = Some(1);
    let err = context::assemble(&req, &env(), &view(), Format::Compact).unwrap_err();
    let required = err.details["required"].as_u64().unwrap();
    let full = assemble(&auth_request(Some("2.3.0")));
    let full_text = context::render(&full, Format::Compact, false);
    let mut partial = None;
    for budget in (required..required + 8000).step_by(25) {
        req.budget = Some(budget);
        let r = assemble_with(&req, &view(), Format::Compact);
        let text = context::render(&r, Format::Compact, false);
        assert_eq!(text.len() as u64, r.footer.budget.used, "bytes are exact");
        assert!(r.footer.budget.used <= budget);
        if r.footer.counts.supplementary > 0 && !r.footer.excluded_budget.is_empty() {
            partial = Some(r);
            break;
        }
    }
    let r = partial.expect("some budget includes part of the optional units");
    let text = context::render(&r, Format::Compact, false);
    // Every included unit appears whole (identical to the unconstrained rendering).
    for u in &r.units {
        let header = format!("### {} {} ", u.tier.as_str(), u.id);
        let start = full_text.find(&header).expect("unit in full output");
        let end = full_text[start + 1..]
            .find("\n### ")
            .or_else(|| full_text[start + 1..].find("\n-- receipt"))
            .map(|i| start + 1 + i + 1)
            .unwrap();
        assert!(
            text.contains(&full_text[start..end]),
            "unit {} is not whole",
            u.id
        );
    }
    for id in &r.footer.excluded_budget {
        assert!(!text.contains(&format!("### supplementary {id} ")));
        assert!(
            r.excluded
                .iter()
                .any(|e| &e.id == id && e.reason.as_str() == "budget")
        );
    }
}

#[test]
fn token_estimates_sum_per_piece() {
    for format in [Format::Compact, Format::Human] {
        let mut req = auth_request(Some("2.3.0"));
        req.sections = SectionsMode::All;
        let r = assemble_with(&req, &view(), format);
        let text = context::render(&r, format, false);
        assert!(r.footer.budget.used >= estimate_tokens(&text));
        assert!(r.footer.budget.used <= r.footer.budget.limit);
        assert_eq!(r.footer.budget.unit, BudgetUnit::TokensEst);
        assert!(text.contains("(estimate, not a tokenizer count)"));
    }
}

#[test]
fn sections_are_optional_verbatim_units() {
    let mut req = auth_request(Some("2.3.0"));
    let without = assemble(&req);
    assert!(unit_ids(&without, Tier::Section).is_empty());
    req.sections = SectionsMode::Mandatory;
    let r = assemble(&req);
    assert_eq!(
        unit_ids(&r, Tier::Section),
        vec![
            "acme.mobile.token-storage#intro",
            "acme.mobile.token-storage#background"
        ]
    );
    let text = context::render(&r, Format::Compact, false);
    assert!(text.contains(
        "```kotlin\nval store = EncryptedTokenStore(keystore)\nstore.save(refreshToken)\n```"
    ));
}

// ---------------------------------------------------------------------------------------
// Determinism, JSON, explain, receipts
// ---------------------------------------------------------------------------------------

#[test]
fn output_is_stable_across_runs_and_file_order() {
    let mut req = ContextRequest::new(Intent::Explain);
    req.task = Some("Объясни, как устроена композиция и обновление токена".into());
    req.paths = vec![AUTH_PATH.into(), UI_PATH.into()];
    req.sections = SectionsMode::All;
    let mut shuffled = corpus_at(&fixture_root());
    shuffled.entries.reverse();
    let n = shuffled.entries.len();
    shuffled.entries.swap(0, n / 2);
    let shuffled = MemoryView::from_corpus(&shuffled, Vec::new());
    for format in FORMATS {
        let a = assemble_with(&req, &view(), format);
        let b = assemble_with(&req, &view(), format);
        let c = assemble_with(&req, &shuffled, format);
        for explain in [false, true] {
            let ra = context::render(&a, format, explain);
            assert_eq!(ra, context::render(&b, format, explain));
            assert_eq!(ra, context::render(&c, format, explain));
        }
        assert_eq!(a.receipt_id(), c.receipt_id());
    }
}

fn keys(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                out.push(k.clone());
                keys(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| keys(x, out)),
        _ => {}
    }
}

#[test]
fn json_result_is_stable_and_receipt_is_verifiable() {
    let req = auth_request(Some("2.3.0"));
    let a = assemble_with(&req, &view(), Format::Json);
    let b = assemble_with(&req, &view(), Format::Json);
    assert_eq!(context::to_json(&a, true), context::to_json(&b, true));
    let full = context::to_json(&a, true);
    assert_eq!(receipt_id_of(&full), a.receipt_id());
    assert!(a.receipt_id().starts_with("sha256:") && a.receipt_id().len() == 71);
    let plain = context::to_json(&a, false);
    assert!(plain.get("explain").is_none());
    assert_eq!(full["explain"]["counted_in_budget"], false);
    assert_eq!(plain["engine_version"], kb::versions::ENGINE_VERSION);
    assert_eq!(plain["skill_protocol"], kb::versions::SKILL_PROTOCOL);
    assert_eq!(plain["completeness"], "complete");
    assert_eq!(plain["budget"]["not_counted"][0], "explain");
    let mut all = Vec::new();
    keys(&full, &mut all);
    for k in &all {
        let k = k.to_ascii_lowercase();
        assert!(
            !(k.contains("elapsed") || k.contains("timing") || k.contains("duration")),
            "non-deterministic key {k}"
        );
    }
    // Changing the request changes the receipt.
    let other = assemble_with(&auth_request(Some("2.4.0")), &view(), Format::Json);
    assert_ne!(other.receipt_id(), a.receipt_id());
    let rendered = context::render(&a, Format::Json, false);
    let parsed: Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(parsed, plain);
}

/// The receipt is verifiable from the default output (no `--explain`) with the documented
/// formula: sha256 of the canonical JSON of `result` without `receipt.id`.
#[test]
fn receipt_is_verifiable_from_the_default_result() {
    for format in FORMATS {
        let r = assemble_with(&auth_request(Some("2.3.0")), &view(), format);
        let mut plain = context::to_json(&r, false);
        assert_eq!(plain["receipt"]["id"], r.receipt_id());
        plain["receipt"].as_object_mut().unwrap().remove("id");
        let expected = format!("sha256:{}", sha256_hex(canonical_json(&plain).as_bytes()));
        assert_eq!(r.receipt_id(), expected, "{format:?}");
        assert_eq!(receipt_id_of(&context::to_json(&r, false)), expected);
        // `--explain` output verifies too once its (uncounted) `explain` part is dropped.
        assert_eq!(receipt_id_of(&context::to_json(&r, true)), expected);
    }
}

/// The `result` member exactly as `kb context --json` prints it: the `kb.cli.v1` envelope
/// built by `output::envelope` and pretty-printed like `output::emit`, cut at the bytes that
/// differ from the same envelope with a `null` result.
fn printed_result(result: Value) -> String {
    let meta = Map::from_iter([("elapsed_ms".to_string(), json!(1234))]);
    let print = |result: Value| {
        let out = CommandOutput::new(result, String::new());
        serde_json::to_string_pretty(&envelope("context", Some(&out), None, &meta)).unwrap()
    };
    let (full, null) = (print(result), print(Value::Null));
    let (a, b) = (full.as_bytes(), null.as_bytes());
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let printed = full[prefix..full.len() - suffix].to_string();
    assert!(printed.starts_with('{') && printed.ends_with('}'));
    printed
}

#[test]
fn json_budget_measures_the_printed_result() {
    let mut req = auth_request(Some("2.3.0"));
    req.budget_unit = Some(BudgetUnit::Bytes);
    req.sections = SectionsMode::All;
    req.budget = Some(1);
    let err = context::assemble(&req, &env(), &view(), Format::Json).unwrap_err();
    let required = err.details["required"].as_u64().unwrap();
    let mut saw_partial = false;
    for budget in (required..required + 30_000).step_by(997) {
        req.budget = Some(budget);
        let r = assemble_with(&req, &view(), Format::Json);
        let printed = printed_result(context::to_json(&r, false));
        assert_eq!(
            printed.len() as u64,
            r.footer.budget.used,
            "bytes are exact"
        );
        assert!(r.footer.budget.used <= budget);
        saw_partial |= !r.footer.excluded_budget.is_empty();
        // The uncounted explain part adds exactly one member of the result object.
        let explained = printed_result(context::to_json(&r, true));
        assert!(explained.len() > printed.len());
    }
    assert!(saw_partial, "some budgets exclude optional units");
    let err = {
        req.budget = Some(required - 1);
        context::assemble(&req, &env(), &view(), Format::Json).unwrap_err()
    };
    assert_eq!(err.code, ErrorCode::ContextBudgetExceeded);

    // tokens-est upper-bounds the estimate of the printed result.
    let mut req = auth_request(Some("2.3.0"));
    req.task = Some("Обновление токена".into());
    let r = assemble_with(&req, &view(), Format::Json);
    let printed = printed_result(context::to_json(&r, false));
    assert!(estimate_tokens(&printed) <= r.footer.budget.used);
    assert!(r.footer.budget.used <= r.footer.budget.limit);

    // An empty result prints `"units": []`.
    let (_tmp, root) = copy_fixture();
    let knowledge = root.join("project/knowledge");
    for e in fs::read_dir(&knowledge).unwrap() {
        for f in fs::read_dir(e.unwrap().path()).unwrap() {
            fs::remove_file(f.unwrap().path()).unwrap();
        }
    }
    let mut req = ContextRequest::new(Intent::Implement);
    req.repos = vec!["mobile".into()];
    req.budget_unit = Some(BudgetUnit::Bytes);
    let r = assemble_with(&req, &view_at(&root), Format::Json);
    assert!(r.units.is_empty());
    let printed = printed_result(context::to_json(&r, false));
    assert!(printed.contains("\"units\": []"));
    assert_eq!(printed.len() as u64, r.footer.budget.used);
}

#[test]
fn explain_is_appended_and_not_counted() {
    let mut req = auth_request(Some("2.3.0"));
    req.budget_unit = Some(BudgetUnit::Bytes);
    let r = assemble(&req);
    let plain = context::render(&r, Format::Compact, false);
    let explained = context::render(&r, Format::Compact, true);
    assert!(explained.starts_with(&plain));
    let extra = &explained[plain.len()..];
    assert!(extra.starts_with("\n== explain (not counted in the budget) =="));
    assert!(extra.contains("- acme.mobile.token-storage -> acme.contract.token-refresh"));
    assert!(extra.contains(
        "- acme.mobile.token-storage: source mobile:app/src/auth/storage/TokenStore.kt symbol TokenStore"
    ));
    assert!(extra.contains("signals: rationale +250"), "{extra}");
    assert_eq!(
        context::render(&r, Format::Compact, false).len() as u64,
        r.footer.budget.used,
        "explain does not change the measured payload"
    );
}

#[test]
fn golden_compact_output() {
    let mut req = auth_request(Some("2.3.0"));
    req.task = Some("Keep the refresh token in the keystore".into());
    let r = assemble(&req);
    check_golden(
        "mobile-auth.compact.txt",
        &context::render(&r, Format::Compact, true),
    );
    let h = assemble_with(&req, &view(), Format::Human);
    check_golden(
        "mobile-auth.human.txt",
        &context::render(&h, Format::Human, false),
    );
}

// ---------------------------------------------------------------------------------------
// Completeness sources
// ---------------------------------------------------------------------------------------

#[test]
fn snapshot_provenance_limits_completeness() {
    let req = auth_request(Some("2.3.0"));
    let run = |snap: SnapshotInfo, view: &MemoryView| {
        context::assemble(&req, &TaskEnv::new(snap), view, Format::Compact).unwrap()
    };
    let v = view();

    let mut offline = snapshot();
    offline.freshness = Freshness::Unverified;
    let r = run(offline, &v);
    assert_eq!(r.status(), Completeness::Partial);
    assert_eq!(r.knowledge_status(), Completeness::Complete);
    assert!(reason_codes(&r).contains(&"FRESHNESS_UNVERIFIED"));
    assert_eq!(context::to_json(&r, false)["freshness"], "unverified");

    let mut unapproved = snapshot();
    unapproved.approved = Some(false);
    assert!(reason_codes(&run(unapproved, &v)).contains(&"NOT_APPROVED"));

    let mut wt = snapshot();
    wt.selection = Selection::WorkingTree;
    wt.revision = None;
    wt.approved = None;
    let r = run(wt, &v);
    assert_eq!(reason_codes(&r), vec!["WORKING_TREE"]);
    assert_eq!(r.status(), Completeness::Partial);

    let broken = MemoryView::from_corpus(
        &corpus_at(&fixture_root()),
        vec![Diagnostic::error(
            "DANGLING_LINK",
            "synthetic validation error",
        )],
    );
    let r = run(snapshot(), &broken);
    assert_eq!(r.status(), Completeness::Incomplete);
    assert!(reason_codes(&r).contains(&"SNAPSHOT_INVALID"));
    assert!(context::render(&r, Format::Compact, false).contains("DANGLING_LINK"));
}

#[test]
fn record_text_cannot_inject_terminal_control_sequences() {
    let (_tmp, root) = copy_fixture();
    common::write(
        &root.join("project/knowledge/policies/hostile.md"),
        "+++\nschema = 1\nid = \"acme.mobile.hostile\"\nkind = \"policy\"\n\
         title = \"Hostile text\"\nstatus = \"accepted\"\nowner = \"team-mobile\"\n\n\
         [scope]\nrepos = [\"mobile\"]\n\n[[rules]]\nid = \"x\"\nlevel = \"must\"\n\
         text = \"Clear \\u001B[2J the screen \\u202E reversed\"\n+++\n",
    );
    let v = view_at(&root);
    let mut req = ContextRequest::new(Intent::Implement);
    req.repos = vec!["mobile".into()];
    for format in [Format::Compact, Format::Human] {
        let r = assemble_with(&req, &v, format);
        let text = context::render(&r, format, true);
        assert!(!text.contains('\u{1b}') && !text.contains('\u{202e}'));
        assert!(text.contains("Clear \\u{1b}[2J the screen \\u{202e} reversed"));
    }
    let shown = context::show::show(&v, "acme.mobile.hostile", None, false, false).unwrap();
    assert!(!context::show::render(&shown, Format::Compact).contains('\u{1b}'));
    let json = context::to_json(&assemble(&req), false).to_string();
    assert!(!json.contains('\u{1b}'));
}

#[test]
fn unknown_repo_is_partial_and_empty_results_prove_nothing() {
    let r = assemble(&ContextRequest::new(Intent::Review));
    assert_eq!(r.status(), Completeness::Partial);
    assert!(reason_codes(&r).contains(&"REPO_UNKNOWN"));

    let (_tmp, root) = copy_fixture();
    let knowledge = root.join("project/knowledge");
    for e in fs::read_dir(&knowledge).unwrap() {
        let dir = e.unwrap().path();
        for f in fs::read_dir(&dir).unwrap() {
            fs::remove_file(f.unwrap().path()).unwrap();
        }
    }
    let mut req = ContextRequest::new(Intent::Implement);
    req.repos = vec!["mobile".into()];
    let r = assemble_with(&req, &view_at(&root), Format::Compact);
    assert_eq!(r.status(), Completeness::Complete);
    assert!(r.units.is_empty());
    assert!(r.header.notes[0].contains("proves nothing about the project"));
}

#[test]
fn unknown_scope_and_invalid_input_are_rejected() {
    let code = |req: &ContextRequest| {
        context::assemble(req, &env(), &view(), Format::Compact)
            .unwrap_err()
            .code
    };
    let base = || ContextRequest::new(Intent::Implement);

    let mut r = base();
    r.repos = vec!["mobile".into(), "nope".into()];
    r.concepts = vec!["unheard-of".into()];
    let err = context::assemble(&r, &env(), &view(), Format::Compact).unwrap_err();
    assert_eq!(err.code, ErrorCode::UnknownScope);
    assert_eq!(err.details["repos"][0], "nope");
    assert_eq!(err.details["concepts"][0], "unheard-of");
    let mut r = base();
    r.modules = vec!["mobile.nope".into()];
    assert_eq!(code(&r), ErrorCode::UnknownScope);
    let mut r = base();
    r.features = vec!["nope".into()];
    assert_eq!(code(&r), ErrorCode::UnknownScope);
    let mut r = base();
    r.paths = vec!["nope:src/x.rs".into()];
    assert_eq!(code(&r), ErrorCode::UnknownScope);

    let mut r = base();
    r.task = Some("x".repeat(8 * 1024 + 1));
    assert_eq!(code(&r), ErrorCode::InvalidInput);
    let mut r = base();
    r.paths = (0..513).map(|i| format!("src/{i}.rs")).collect();
    assert_eq!(code(&r), ErrorCode::InvalidInput);
    for bad in ["../etc/passwd", "/abs/path", "mobile:a/../b", "a\\b"] {
        let mut r = base();
        r.paths = vec![bad.into()];
        assert_eq!(code(&r), ErrorCode::InvalidInput, "{bad}");
    }
    let mut r = base();
    r.budget = Some(0);
    assert_eq!(code(&r), ErrorCode::InvalidInput);
    let mut r = base();
    r.max_supplementary = Some(100_000);
    assert_eq!(code(&r), ErrorCode::InvalidInput);
}

// ---------------------------------------------------------------------------------------
// Proposals
// ---------------------------------------------------------------------------------------

fn proposal(
    path: &str,
    text: &str,
    change: ProposalChange,
    stale: bool,
) -> (ProposalEntry, Option<Arc<kb::model::ParsedRecord>>) {
    let parsed = Arc::new(parse_record(path, text.as_bytes()).unwrap());
    (
        ProposalEntry {
            path: path.into(),
            change,
            meta: Some(Arc::new(parsed.meta())),
            stale,
            diagnostics: Vec::new(),
        },
        Some(parsed),
    )
}

#[test]
fn proposals_are_labeled_and_never_override_accepted_records() {
    let storage_path = "project/knowledge/policies/mobile-token-storage.md";
    let accepted_text = fs::read_to_string(fixture_root().join(storage_path)).unwrap();
    let modified = accepted_text.replace(
        "Delete stored tokens when the user signs out.",
        "Delete stored tokens and caches when the user signs out.",
    );
    let new_policy = "+++\nschema = 1\nid = \"acme.mobile.session-timeout\"\nkind = \"policy\"\n\
        title = \"Session timeout\"\nstatus = \"accepted\"\nowner = \"team-mobile\"\n\n[scope]\n\
        repos = [\"mobile\"]\n\n[[overrides]]\ntarget = \"acme.product.network#request-timeout-ms\"\n\
        value = 1000\nreason = \"proposed\"\n+++\n";
    let proposals = vec![
        proposal(
            storage_path,
            &modified,
            ProposalChange::Modifies {
                id: "acme.mobile.token-storage".into(),
            },
            true,
        ),
        proposal(
            "project/knowledge/policies/session-timeout.md",
            new_policy,
            ProposalChange::New {
                id: "acme.mobile.session-timeout".into(),
            },
            false,
        ),
        (
            ProposalEntry {
                path: "project/knowledge/decisions/encrypted-storage.md".into(),
                change: ProposalChange::Removes {
                    id: "acme.decision.encrypted-storage".into(),
                },
                meta: None,
                stale: false,
                diagnostics: Vec::new(),
            },
            None,
        ),
        (
            ProposalEntry {
                path: "project/knowledge/policies/broken.md".into(),
                change: ProposalChange::Invalid,
                meta: None,
                stale: false,
                diagnostics: Vec::new(),
            },
            None,
        ),
    ];
    let v = view().with_proposals(proposals);
    let mut req = auth_request(Some("2.3.0"));
    let without = assemble_with(&req, &v, Format::Compact);
    assert!(unit_ids(&without, Tier::Proposal).is_empty());

    req.include_proposals = true;
    let r = assemble_with(&req, &v, Format::Compact);
    assert_eq!(
        unit_ids(&r, Tier::Proposal),
        vec![
            "proposal:acme.decision.encrypted-storage",
            "proposal:acme.mobile.session-timeout",
            "proposal:acme.mobile.token-storage"
        ]
    );
    let find = |tier: Tier, id: &str| {
        r.units
            .iter()
            .find(|u| u.tier == tier && u.id == id)
            .unwrap()
    };
    assert_eq!(
        find(Tier::Proposal, "proposal:acme.mobile.token-storage").labels,
        vec!["proposal:modifies", "stale"]
    );
    assert_eq!(
        find(Tier::Proposal, "proposal:acme.mobile.session-timeout").labels,
        vec!["proposal:new"]
    );
    assert_eq!(
        find(Tier::Proposal, "proposal:acme.decision.encrypted-storage").labels,
        vec!["proposal:removes"]
    );
    // The accepted version stays mandatory and authoritative; proposals never count as
    // mandatory and never change effective settings.
    let accepted = find(Tier::Mandatory, "acme.mobile.token-storage");
    let UnitBody::Record(p) = &accepted.body else {
        panic!()
    };
    assert!(
        p.record
            .normative()
            .iter()
            .any(|n| n.text == "Delete stored tokens when the user signs out.")
    );
    assert!(!r.mandatory_ids().contains(&"acme.mobile.session-timeout"));
    assert_eq!(
        setting(&r, "request-timeout-ms").value,
        Some(SettingValue::Integer(3000))
    );
    assert!(r.header.issues.iter().any(|d| d.code == "PROPOSAL_INVALID"));
    let text = context::render(&r, Format::Compact, false);
    assert!(text.contains("Delete stored tokens and caches when the user signs out."));
    assert!(text.contains("### proposal acme.mobile.token-storage (policy, accepted)"));

    // Section units of the proposal and of the accepted record stay distinguishable.
    req.sections = SectionsMode::All;
    let r = assemble_with(&req, &v, Format::Compact);
    let sections = unit_ids(&r, Tier::Section);
    assert!(sections.contains(&"acme.mobile.token-storage#background"));
    assert!(sections.contains(&"proposal:acme.mobile.token-storage#background"));
}

// ---------------------------------------------------------------------------------------
// Search and show
// ---------------------------------------------------------------------------------------

#[test]
fn search_ranks_records_and_is_not_a_context_assembly() {
    let v = view();
    let r = context::search::search(&v, "token storage", &[], 10, false).unwrap();
    assert_eq!(r.hits[0].id, "acme.mobile.token-storage");
    assert!(r.hits.iter().any(|h| h.id == "acme.contract.token-refresh"));
    let text = context::search::render(&r, Format::Compact);
    assert!(text.contains("not a context assembly"));
    assert_eq!(context::search::to_json(&r)["context_assembly"], false);

    let procedures = context::search::search(&v, "token", &[Kind::Procedure], 10, false).unwrap();
    assert!(!procedures.hits.is_empty());
    assert!(procedures.hits.iter().all(|h| h.kind == Kind::Procedure));

    let by_id = context::search::search(&v, "acme.contract.legacy-auth", &[], 5, false).unwrap();
    assert_eq!(by_id.hits[0].id, "acme.contract.legacy-auth");
    assert_eq!(by_id.hits[0].status, kb::model::Status::Superseded);

    let cyrillic = context::search::search(&v, "обновление токена", &[], 5, false).unwrap();
    assert!(
        cyrillic
            .hits
            .iter()
            .any(|h| h.id == "acme.procedure.debug-token-refresh")
    );
    let limited = context::search::search(&v, "token", &[], 2, false).unwrap();
    assert_eq!(limited.hits.len(), 2);
    assert!(limited.total > 2);
    assert_eq!(
        context::search::search(&v, "token", &[], 2, false).unwrap(),
        limited,
        "deterministic"
    );

    for (q, limit) in [("!!!", 5), ("token", 0), ("token", 10_000)] {
        let err = context::search::search(&v, q, &[], limit, false).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{q} {limit}");
    }
}

#[test]
fn show_prints_records_sections_and_raw_bytes() {
    let v = view();
    let full = context::show::show(&v, "acme.mobile.token-storage", None, false, false).unwrap();
    let text = context::show::render(&full, Format::Compact);
    assert!(
        text.starts_with(
            "# acme.mobile.token-storage (policy, accepted): Token storage on devices\n"
        )
    );
    assert!(text.contains("selector aliases: keychain, keystore"));
    assert!(text.contains("related: acme.procedure.debug-token-refresh"));
    assert!(text.contains("## Background\n"));
    let json = context::show::to_json(&full);
    assert_eq!(json["record"]["id"], "acme.mobile.token-storage");
    assert_eq!(json["sections"][1]["id"], "background");

    let section = context::show::show(
        &v,
        "acme.mobile.token-storage#background",
        None,
        false,
        false,
    )
    .unwrap();
    let text = context::show::render(&section, Format::Compact);
    assert!(text.starts_with("# acme.mobile.token-storage#background: Background\n"));
    assert!(text.contains("```kotlin\nval store = EncryptedTokenStore(keystore)"));
    let same = context::show::show(
        &v,
        "acme.mobile.token-storage",
        Some("background"),
        false,
        false,
    )
    .unwrap();
    assert_eq!(
        context::show::to_json(&same),
        context::show::to_json(&section)
    );

    let raw = context::show::show(&v, "acme.mobile.token-storage", None, true, false).unwrap();
    let bytes = fs::read_to_string(
        fixture_root().join("project/knowledge/policies/mobile-token-storage.md"),
    )
    .unwrap();
    assert_eq!(context::show::render(&raw, Format::Compact), bytes);
    assert_eq!(context::show::to_json(&raw)["raw"], bytes.as_str());
}

#[test]
fn superseded_records_stay_addressable_and_errors_are_typed() {
    let v = view();
    let old = context::show::show(&v, "acme.contract.legacy-auth", None, false, false).unwrap();
    assert_eq!(old.successors.len(), 1);
    assert_eq!(old.successors[0].id, "acme.contract.token-refresh");
    let text = context::show::render(&old, Format::Human);
    assert!(text.starts_with(
        "note: superseded: kept addressable for history, not current knowledge; superseded by acme.contract.token-refresh\n"
    ));

    let err = context::show::show(&v, "acme.nope.missing", None, false, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err =
        context::show::show(&v, "acme.mobile.token-storage#nope", None, false, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(err.details["available"][1], "background");
    for (target, section, raw) in [
        ("Not An Id", None, false),
        ("acme.mobile.token-storage#intro", Some("background"), false),
        ("acme.mobile.token-storage#intro", None, true),
    ] {
        let err = context::show::show(&v, target, section, raw, false).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{target}");
    }
}
