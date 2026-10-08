//! Behavioral retrieval checks over synthetic repositories with isolated Git config.
mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use common::{Sandbox, json, write};
use serde_json::Value;

struct World {
    sb: Sandbox,
    kb: PathBuf,
    host: PathBuf,
}

impl World {
    fn new() -> Self {
        let sb = Sandbox::new();
        let kb = sb.path().join("kb");
        let host = sb.path().join("host");
        sb.init_repo(&kb);
        common::write_min_project(&kb);
        let types = fs::read_to_string(
            common::repo_root().join("core/templates/project/registry/change-types.toml"),
        )
        .unwrap();
        write(&kb.join("project/registry/change-types.toml"), &types);
        sb.git(&kb, &["remote", "add", "origin", kb.to_str().unwrap()]);
        let revision = sb.commit_all(&kb, "synthetic knowledge");
        sb.git(&kb, &["update-ref", "refs/remotes/origin/main", &revision]);
        sb.init_repo(&host);
        write(&host.join(".kbw.toml"), "schema = 1\nrepo = \"mobile\"\n");
        write(
            &host.join("app/auth/TokenHTTPClient.kt"),
            "// Synthetic tracked fixture.\nclass TokenHTTPClient\n",
        );
        let w = Self { sb, kb, host };
        w.commit_host("2024-01-01T00:00:00Z");
        w
    }

    fn commit_host(&self, date: &str) -> String {
        self.sb.git(&self.host, &["add", "-A"]);
        let mut c = Command::new("git");
        c.arg("-C").arg(&self.host).args([
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "synthetic host change",
        ]);
        common::isolated_env(&mut c, &self.sb.home());
        c.env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date);
        let out = c.output().unwrap();
        assert!(out.status.success(), "{}", common::stderr(&out));
        self.sb.git(&self.host, &["rev-parse", "HEAD"])
    }

    fn record(&self, name: &str, text: &str) {
        write(&self.kb.join(format!("project/knowledge/{name}.md")), text);
    }

    fn run(&self, extra: &[&str]) -> Value {
        let out = self.output(extra);
        assert!(
            matches!(out.status.code(), Some(0 | 30)),
            "{}\n{}",
            common::stdout(&out),
            common::stderr(&out)
        );
        let doc = json(&out);
        assert!(!doc["result"].is_null());
        doc["result"].clone()
    }

    /// The `error` of a failing context call.
    fn fail(&self, extra: &[&str]) -> Value {
        let out = self.output(extra);
        assert!(
            !matches!(out.status.code(), Some(0 | 30)),
            "{}",
            common::stdout(&out)
        );
        json(&out)["error"].clone()
    }

    fn output(&self, extra: &[&str]) -> std::process::Output {
        let mut args = vec![
            "--root",
            self.kb.to_str().unwrap(),
            "--host",
            self.host.to_str().unwrap(),
            "--snapshot",
            "working-tree",
            "--offline",
            "--json",
            "context",
        ];
        args.extend_from_slice(extra);
        self.sb.kb(&self.host, &args, &[])
    }
}

fn ids(result: &Value) -> Vec<&str> {
    result["units"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|u| u["id"].as_str())
        .collect()
}

/// Codes of a result array such as `status_reasons` or `issues`.
fn codes<'a>(result: &'a Value, field: &str) -> Vec<&'a str> {
    result[field]
        .as_array()
        .map(|a| a.iter().filter_map(|r| r["code"].as_str()).collect())
        .unwrap_or_default()
}

fn issue<'a>(result: &'a Value, code: &str) -> &'a str {
    result["issues"]
        .as_array()
        .and_then(|a| a.iter().find(|i| i["code"] == code))
        .and_then(|i| i["message"].as_str())
        .unwrap_or_else(|| panic!("no {code} in {}", result["issues"]))
}

fn module_policy(id: &str, module: &str) -> String {
    format!(
        "+++\nschema = 2\nid = \"acme.policy.{id}\"\nkind = \"policy\"\ntitle = \"Synthetic {id}\"\nstatus = \"accepted\"\nowner = \"arch\"\n[scope]\nmodules = [\"{module}\"]\n[[rules]]\nid = \"preserve\"\nlevel = \"must\"\ntext = \"Preserve the synthetic {id} contract.\"\n+++\n"
    )
}

fn policy(id: &str, category: &str) -> String {
    format!(
        "+++\nschema = 2\nid = \"acme.policy.{id}\"\nkind = \"policy\"\ntitle = \"Synthetic {id}\"\nstatus = \"accepted\"\nowner = \"arch\"\n[scope]\nproduct = true\nchange_types = [\"{category}\"]\n[[rules]]\nid = \"preserve\"\nlevel = \"must\"\ntext = \"Preserve the synthetic {id} contract.\"\n+++\n"
    )
}

fn reference(id: &str, introduced: &str, retired: Option<&str>, requires: Option<&str>) -> String {
    let end = retired
        .map(|x| format!("retired = \"{x}\"\n"))
        .unwrap_or_default();
    let links = requires
        .map(|x| format!("[links]\nrequires = [\"{x}\"]\n"))
        .unwrap_or_default();
    format!(
        "+++\nschema = 2\nid = \"acme.reference.{id}\"\nkind = \"reference\"\ntitle = \"Synthetic {id}\"\nstatus = \"accepted\"\nowner = \"arch\"\nsummary = \"Synthetic historical fact {id}.\"\nintroduced = \"{introduced}\"\n{end}[scope]\nproduct = true\n[selectors]\naliases = [\"time travel\"]\n{links}+++\n"
    )
}

#[test]
fn path_free_diagnosis_delivers_models_but_never_claims_complete_scope() {
    let w = World::new();
    w.record("queue", "+++\nschema = 2\nid = \"acme.feature.queue\"\nkind = \"feature\"\ntitle = \"Synthetic pending queue\"\nstatus = \"accepted\"\nowner = \"arch\"\nfeature = \"login\"\nsummary = \"Synthetic queue.\"\n[scope]\nfeatures = [\"login\"]\n[selectors]\naliases = [\"response stalls\"]\n[[behaviors]]\nid = \"pending\"\ntext = \"Retain pending data.\"\n[[scenarios]]\nid = \"timeout\"\ngiven = \"No response arrives.\"\nexpect = \"Keep pending; do not infer success.\"\n+++\n");
    let result = w.run(&["--intent", "diagnose", "--task", "response stalls"]);
    assert!(ids(&result).contains(&"acme.feature.queue"));
    assert!(
        result["status_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["code"] == "DIAGNOSE_SCOPE_PROVISIONAL")
    );
    assert_eq!(
        result["scope"]["features"]["values"],
        serde_json::json!(["login"])
    );
    assert!(
        result
            .to_string()
            .contains("Keep pending; do not infer success.")
    );
}

#[test]
fn identifiers_use_tracked_files_and_split_acronyms() {
    let w = World::new();
    write(
        &w.host.join("app/auth/GhostStore.kt"),
        "// untracked synthetic source\n",
    );
    let result = w.run(&["--intent", "implement", "--task", "Fix Token HTTP Client"]);
    assert_eq!(
        result["scope"]["inferred_paths"],
        serde_json::json!(["app/auth/TokenHTTPClient.kt"])
    );
    // A discovered file is a candidate: it maps to its module without making the module
    // scope known.
    assert_eq!(result["scope"]["modules"]["state"], "unknown");
    assert!(
        result["scope"]["paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["path"] == "app/auth/TokenHTTPClient.kt"
                && p["modules"] == serde_json::json!(["mobile.auth"]))
    );
    let ghost = w.run(&["--intent", "diagnose", "--task", "Fix GhostStore"]);
    assert!(ghost["scope"]["inferred_paths"].is_null());
    let other = w.run(&["--intent", "diagnose", "--task", "Fix TokenHTTPClientOther"]);
    assert!(other["scope"]["inferred_paths"].is_null());
}

#[test]
fn glossary_meanings_and_scenarios_are_searchable_typed_content() {
    let w = World::new();
    w.record("glossary", "+++\nschema = 2\nid = \"acme.reference.glossary\"\nkind = \"reference\"\ntitle = \"Synthetic glossary\"\nstatus = \"accepted\"\nowner = \"arch\"\nsummary = \"Settled terms.\"\nterms = [{ term = \"Confirmed delivery\", meaning = \"Quasar receipt arrived.\", source = \"docs/synthetic.md\" }]\n[scope]\nproduct = true\n+++\n");
    let result = w.run(&["--intent", "explain", "--task", "quasar"]);
    assert!(ids(&result).contains(&"acme.reference.glossary"));
    let phrase = w.run(&["--intent", "explain", "--task", "confirmed delivery"]);
    assert!(ids(&phrase).contains(&"acme.reference.glossary"));
}

#[test]
fn changed_scope_covers_old_names_deletions_and_extensionless_files() {
    let w = World::new();
    write(&w.host.join("Makefile"), "# synthetic build entry\n");
    let base = w.commit_host("2024-01-02T00:00:00Z");
    fs::create_dir_all(w.host.join("tools")).unwrap();
    fs::rename(
        w.host.join("app/auth/TokenHTTPClient.kt"),
        w.host.join("tools/TokenHTTPClient.kt"),
    )
    .unwrap();
    fs::remove_file(w.host.join("Makefile")).unwrap();
    let result = w.run(&[
        "--intent",
        "review",
        "--changed",
        "--base",
        &base,
        "--working-tree",
    ]);
    let paths = result["request"]["paths"].as_array().unwrap();
    for name in [
        "app/auth/TokenHTTPClient.kt",
        "tools/TokenHTTPClient.kt",
        "Makefile",
    ] {
        assert!(paths.iter().any(|p| p == name), "{paths:?}");
    }
    assert_eq!(
        result["scope"]["modules"]["values"],
        serde_json::json!(["mobile.auth"])
    );
    assert!(
        !result["status_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["code"] == "UNDETERMINED_OBLIGATIONS")
    );
    assert_eq!(result["request"]["change"]["working_tree"], true);
}

#[test]
fn empty_diff_has_known_empty_scope() {
    let w = World::new();
    let result = w.run(&["--intent", "review", "--changed"]);
    assert_eq!(
        result["scope"]["modules"],
        serde_json::json!({"state":"known", "values":[]})
    );
    assert!(
        !result["status_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["code"] == "UNDETERMINED_OBLIGATIONS")
    );
}

#[test]
fn change_hints_include_positive_matches_without_pruning_uncertain_categories() {
    let w = World::new();
    w.record("migration", &policy("migration", "migration"));
    w.record("retry", &policy("retry", "retry"));
    let inferred = w.run(&[
        "--intent",
        "implement",
        "--path",
        "app/auth/db.rs",
        "--task",
        "Add a migration",
    ]);
    assert!(ids(&inferred).contains(&"acme.policy.migration"));
    assert!(
        inferred["undetermined"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == "acme.policy.retry")
    );
    let explicit = w.run(&[
        "--intent",
        "implement",
        "--path",
        "app/auth/db.rs",
        "--change-type",
        "migration",
        "--explain",
    ]);
    assert!(!ids(&explicit).contains(&"acme.policy.retry"));
    assert!(
        explicit["explain"]["excluded"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == "acme.policy.retry" && e["reason"] == "not-applicable")
    );
}

#[test]
fn changed_symbol_text_can_supply_a_change_type_hint() {
    let w = World::new();
    w.record("error-mapping", &policy("error-mapping", "error-mapping"));
    write(
        &w.host.join("app/auth/TokenHTTPClient.kt"),
        "// Synthetic fixture\nval mapper = ErrorMapper()\n",
    );
    let result = w.run(&["--intent", "review", "--changed"]);
    assert!(ids(&result).contains(&"acme.policy.error-mapping"));
    assert!(
        result["scope"]["change_type_hints"]["error-mapping"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "changed text identifier: ErrorMapper")
    );
}

#[test]
fn future_records_do_not_change_historical_lexical_order() {
    let w = World::new();
    w.record("first", &reference("first", "2024-01-01", None, None));
    w.record(
        "second",
        &reference("second", "2024-01-01", None, None).replace(
            "Synthetic historical fact second.",
            "Synthetic historical fact second with extra context and explanatory words.",
        ),
    );
    let args = [
        "--intent",
        "explain",
        "--task",
        "historical fact",
        "--as-of",
        "2024-06-01",
    ];
    let before = w.run(&args);
    for n in 0..12 {
        w.record(
            &format!("future-{n}"),
            &reference(&format!("future-{n}"), "2025-01-01", None, None).replace(
                "Synthetic historical fact",
                "historical historical historical",
            ),
        );
    }
    let after = w.run(&args);
    assert_eq!(ids(&before), ids(&after));
    assert_eq!(
        before["units"], after["units"],
        "future corpus statistics must not affect past delivery"
    );
}

#[test]
fn temporal_dates_filter_aliases_and_ids_and_receipts_are_repeatable() {
    let w = World::new();
    w.record(
        "past",
        &reference("past", "2024-01-01", Some("2024-06-01"), None),
    );
    w.record("future", &reference("future", "2024-06-01", None, None));
    let args = [
        "--intent",
        "explain",
        "--task",
        "time travel acme.reference.future",
        "--as-of",
        "2024-05-31",
        "--explain",
    ];
    let a = w.run(&args);
    let b = w.run(&args);
    assert_eq!(a, b);
    assert!(ids(&a).contains(&"acme.reference.past"));
    assert!(!ids(&a).contains(&"acme.reference.future"));
    assert!(
        a["status_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["code"] == "AS_OF_UNDATED")
    );
    let cutoff = w.run(&[
        "--intent",
        "explain",
        "--task",
        "time travel",
        "--as-of",
        "2024-06-01",
    ]);
    assert!(!ids(&cutoff).contains(&"acme.reference.past"));
    assert!(ids(&cutoff).contains(&"acme.reference.future"));
}

#[test]
fn commit_bounds_use_ancestry_and_historical_identifiers() {
    let w = World::new();
    let old = w.sb.git(&w.host, &["rev-parse", "HEAD"]);
    write(
        &w.host.join("app/auth/FutureClient.kt"),
        "// synthetic future code\n",
    );
    let new = w.commit_host("2024-01-03T00:00:00Z");
    w.record("bounded", &reference("bounded", &old, Some(&new), None));
    let a = w.run(&[
        "--intent",
        "explain",
        "--task",
        "time travel FutureClient",
        "--as-of",
        &old,
    ]);
    assert!(ids(&a).contains(&"acme.reference.bounded"));
    assert!(
        a["scope"]["inferred_paths"].is_null(),
        "future filenames cannot affect old context"
    );
    assert_eq!(a["host"]["head"], old);
    let b = w.run(&[
        "--intent",
        "explain",
        "--task",
        "time travel",
        "--as-of",
        &new,
    ]);
    assert!(!ids(&b).contains(&"acme.reference.bounded"));
}

#[test]
fn temporal_filter_does_not_waive_a_future_required_record() {
    let w = World::new();
    w.record("future", &reference("future", "2025-01-01", None, None));
    w.record("required", "+++\nschema = 2\nid = \"acme.policy.needs-proof\"\nkind = \"policy\"\ntitle = \"Synthetic dependency\"\nstatus = \"accepted\"\nowner = \"arch\"\nintroduced = \"2024-01-01\"\n[scope]\nproduct = true\n[links]\nrequires = [\"acme.reference.future\"]\n[[rules]]\nid = \"proof\"\nlevel = \"must\"\ntext = \"Retain the stated proof.\"\n+++\n");
    let result = w.run(&["--intent", "implement", "--as-of", "2024-06-01"]);
    assert_eq!(result["completeness"], "incomplete");
    assert!(
        result["status_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["code"] == "REQUIRES_MISSING")
    );
    assert!(!ids(&result).contains(&"acme.reference.future"));
}

#[test]
fn identifier_matches_never_make_an_unknown_module_scope_known() {
    let w = World::new();
    write(&w.host.join("LICENSE"), "Synthetic license text.\n");
    w.commit_host("2024-01-02T00:00:00Z");
    w.record("auth", &module_policy("auth", "mobile.auth"));
    let result = w.run(&[
        "--intent",
        "implement",
        "--task",
        "Update the login flow and keep the license header",
    ]);
    assert_eq!(
        result["scope"]["inferred_paths"],
        serde_json::json!(["LICENSE"])
    );
    assert_eq!(result["scope"]["modules"]["state"], "unknown");
    assert!(
        result["undetermined"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == "acme.policy.auth"),
        "a filename in the task must not drop module obligations"
    );
    assert!(codes(&result, "status_reasons").contains(&"UNDETERMINED_OBLIGATIONS"));
    // Beside explicit scope, a discovered file adds the module it names.
    let explicit = w.run(&[
        "--intent",
        "implement",
        "--module",
        "backend.api",
        "--task",
        "Fix TokenHTTPClient",
    ]);
    assert_eq!(
        explicit["scope"]["modules"],
        serde_json::json!({"state": "known", "values": ["backend.api", "mobile.auth"]})
    );
    assert!(ids(&explicit).contains(&"acme.policy.auth"));
}

#[test]
fn ambiguous_filename_stems_are_skipped_beside_explicit_scope() {
    let w = World::new();
    for n in 0..520 {
        write(&w.host.join(format!("pkg/p{n}/__init__.py")), "");
    }
    w.commit_host("2024-01-02T00:00:00Z");
    let explicit = [
        "--module",
        "mobile.auth",
        "--path",
        "app/auth/TokenHTTPClient.kt",
    ];
    for extra in [&[][..], &explicit[..]] {
        let mut args = vec![
            "--intent",
            "implement",
            "--task",
            "Fix init order of the TokenHTTPClient",
        ];
        args.extend_from_slice(extra);
        let result = w.run(&args);
        assert_eq!(
            result["scope"]["inferred_paths"],
            serde_json::json!(["app/auth/TokenHTTPClient.kt"])
        );
        assert!(issue(&result, "IDENTIFIER_AMBIGUOUS").contains("`init` (520 files)"));
    }
}

#[test]
fn discovered_paths_are_capped_and_never_count_against_the_path_limit() {
    let w = World::new();
    let names: Vec<String> = (0..70).map(|n| format!("Synth{n:03}")).collect();
    for name in &names {
        write(
            &w.host.join(format!("app/auth/{name}.kt")),
            "// synthetic\n",
        );
    }
    w.commit_host("2024-01-02T00:00:00Z");
    let task = format!("Rename {}", names.join(" "));
    let explicit: Vec<String> = (0..500)
        .map(|n| format!("app/auth/Explicit{n}.kt"))
        .collect();
    let mut args = vec![
        "--intent",
        "implement",
        "--task",
        &task,
        "--budget",
        "1000000",
    ];
    for path in &explicit {
        args.extend(["--path", path.as_str()]);
    }
    let result = w.run(&args);
    let inferred = result["scope"]["inferred_paths"].as_array().unwrap();
    assert_eq!(inferred.len(), 64);
    assert_eq!(inferred[0], "app/auth/Synth000.kt");
    assert_eq!(inferred[63], "app/auth/Synth063.kt");
    assert!(issue(&result, "INFERRED_PATHS_TRUNCATED").contains("70 identifier paths"));
}

#[test]
fn unusual_tracked_names_do_not_fail_identifier_discovery() {
    let w = World::new();
    write(
        &w.host.join("app/a\\b.txt"),
        "synthetic Windows-style name\n",
    );
    write(&w.host.join("app/Icon\r"), "");
    w.commit_host("2024-01-02T00:00:00Z");
    let result = w.run(&[
        "--intent",
        "implement",
        "--path",
        "app/auth/TokenHTTPClient.kt",
        "--task",
        "retry refresh in TokenHTTPClient",
    ]);
    assert!(issue(&result, "TRACKED_NAMES_SKIPPED").starts_with("2 tracked filename(s)"));
    assert_eq!(
        result["scope"]["inferred_paths"],
        serde_json::json!(["app/auth/TokenHTTPClient.kt"])
    );
}

#[test]
fn commit_bounds_of_other_repositories_are_withheld_not_fatal() {
    let w = World::new();
    let missing = "0123456789abcdef0123456789abcdef01234567";
    let foreign = reference("backend", missing, None, None)
        .replace("[scope]\nproduct = true", "[scope]\nrepos = [\"backend\"]");
    w.record("backend", &foreign);
    w.record("dated", &reference("dated", "2024-01-01", None, None));
    let args = [
        "--intent",
        "explain",
        "--task",
        "time travel",
        "--as-of",
        "2026-06-01",
        "--explain",
    ];
    let result = w.run(&args);
    assert!(codes(&result, "status_reasons").contains(&"AS_OF_BOUND_UNRESOLVED"));
    assert!(ids(&result).contains(&"acme.reference.dated"));
    assert!(
        result["explain"]["excluded"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == "acme.reference.backend" && e["reason"] == "temporal")
    );
    // A missing commit of knowledge that can apply in this host remains an error.
    w.record("backend", &reference("backend", missing, None, None));
    let error = w.fail(&args);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("missing or ambiguous in the host"),
        "{error}"
    );
}

#[test]
fn oversized_patch_text_skips_changed_text_hints() {
    let w = World::new();
    let base = w.sb.git(&w.host, &["rev-parse", "HEAD"]);
    // About 9 MB of generated text: above the 8 MiB lexical-analysis limit.
    let line = format!("{}\n", "x".repeat(99));
    write(
        &w.host.join("app/auth/package-lock.json"),
        &line.repeat(90_000),
    );
    w.commit_host("2024-01-02T00:00:00Z");
    let result = w.run(&["--intent", "review", "--changed", "--base", &base]);
    assert!(codes(&result, "issues").contains(&"CHANGED_TEXT_SKIPPED"));
    assert_eq!(
        result["scope"]["modules"],
        serde_json::json!({"state": "known", "values": ["mobile.auth"]})
    );
}

#[test]
fn diff_paths_are_not_limited_as_path_options() {
    let w = World::new();
    for n in 0..520 {
        write(
            &w.host.join(format!("app/auth/generated/G{n}.kt")),
            "// synthetic generated file\n",
        );
    }
    let result = w.run(&["--intent", "review", "--changed", "--budget", "1000000"]);
    assert_eq!(result["request"]["paths"].as_array().unwrap().len(), 520);
    assert_eq!(
        result["scope"]["modules"],
        serde_json::json!({"state": "known", "values": ["mobile.auth"]})
    );
    // The `--path` option keeps its own limit.
    let many: Vec<String> = (0..513).map(|n| format!("app/auth/P{n}.kt")).collect();
    let mut args = vec!["--intent", "implement"];
    for path in &many {
        args.extend(["--path", path.as_str()]);
    }
    let error = w.fail(&args);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("513 --path values given"),
        "{error}"
    );
}

#[test]
fn historical_full_text_folds_diacritics_like_the_index() {
    let w = World::new();
    w.record(
        "ordering",
        &reference("ordering", "2024-01-01", None, None).replace(
            "title = \"Synthetic ordering\"",
            "title = \"Synthetic café ordering\"",
        ),
    );
    let live = w.run(&["--intent", "explain", "--task", "cafe"]);
    assert!(ids(&live).contains(&"acme.reference.ordering"));
    let past = w.run(&[
        "--intent",
        "explain",
        "--task",
        "cafe",
        "--as-of",
        "2026-06-01",
    ]);
    assert!(
        ids(&past).contains(&"acme.reference.ordering"),
        "the as-of slice must match accented terms like the index"
    );
}
