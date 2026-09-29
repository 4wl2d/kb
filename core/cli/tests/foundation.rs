//! Sanity checks of the shared test fixtures and foundation loading.
mod common;

use kb::corpus::load_corpus;
use kb::model::{Profile, ProfileLocation};
use kb::source::WorkingTreeSource;

#[test]
fn minimal_project_fixture_loads_without_errors() {
    let sb = common::Sandbox::new();
    let root = sb.path().join("kb");
    common::write_min_project(&root);
    let corpus = load_corpus(
        &WorkingTreeSource::new(&root),
        &ProfileLocation::for_profile(Profile::Project),
    )
    .unwrap();
    assert!(corpus.diagnostics.is_empty(), "{:#?}", corpus.diagnostics);
    assert_eq!(corpus.records().count(), 2);
    assert_eq!(
        corpus
            .registry
            .modules_for_path("mobile", "app/auth/Token.kt")[0]
            .id,
        "mobile.auth"
    );
}

#[test]
fn clean_upstream_is_not_initialized() {
    let loc = ProfileLocation::for_profile(Profile::Project);
    let err = load_corpus(&WorkingTreeSource::new(common::repo_root()), &loc).unwrap_err();
    assert_eq!(err.code, kb::error::ErrorCode::ProjectNotInitialized);
}

#[test]
fn maintainer_profile_parses_without_errors() {
    let loc = ProfileLocation::for_profile(Profile::Maintainer);
    let corpus = load_corpus(&WorkingTreeSource::new(common::repo_root()), &loc).unwrap();
    assert!(corpus.diagnostics.is_empty(), "{:#?}", corpus.diagnostics);
    assert!(corpus.records().count() >= 14);
}
