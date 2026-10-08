//! Provider pinning, source validation, graph traversal and actual CLI composition.
mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use common::{Sandbox, json, write};
use kb::model::*;

// Library adapters also invoke Git. Isolate the child rather than changing the parallel
// test runner's global HOME.
fn isolated(name: &str) -> bool {
    isolated_with(name, |_, _| {})
}

/// `configure` may add environment and files to a sandbox that outlives the child.
fn isolated_with(name: &str, configure: impl FnOnce(&mut std::process::Command, &Path)) -> bool {
    if std::env::var("KB_PROVIDER_TEST").as_deref() == Ok(name) {
        return false;
    }
    let sb = Sandbox::new();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture"])
        .env("KB_PROVIDER_TEST", name);
    common::isolated_env(&mut command, &sb.home());
    configure(&mut command, &sb.path());
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        common::stdout(&out),
        common::stderr(&out)
    );
    true
}

struct World {
    sb: Sandbox,
    host: PathBuf,
    kb: PathBuf,
    base: String,
}

impl World {
    fn new() -> Self {
        let sb = Sandbox::new();
        let kb = sb.path().join("kb");
        let host = sb.path().join("host");
        sb.init_repo(&kb);
        common::write_min_project(&kb);
        write(&kb.join(".gitignore"), ".cache/\n");
        let accepted = sb.commit_all(&kb, "synthetic knowledge");
        sb.git(&kb, &["remote", "add", "origin", kb.to_str().unwrap()]);
        sb.git(&kb, &["update-ref", "refs/remotes/origin/main", &accepted]);
        sb.init_repo(&host);
        write(&host.join(".kbw.toml"), "schema = 1\nrepo = \"mobile\"\n");
        write(&host.join("app/auth/Core.rs"), "pub fn save() {}\n");
        write(
            &host.join("app/auth/Client.rs"),
            "pub fn client() { save(); }\n",
        );
        write(
            &host.join("app/auth/tests/ClientTest.rs"),
            "fn test_client() { client(); }\n",
        );
        let base = sb.commit_all(&host, "synthetic source");
        Self { sb, host, kb, base }
    }

    fn request(&self, commit: &str) -> CodeRequest {
        kb::code::request(&self.host, "mobile", commit, CodeOperation::Symbols).unwrap()
    }

    fn graph(&self, commit: &str) -> CodeResponse {
        let defs = [
            ("save", "app/auth/Core.rs"),
            ("client", "app/auth/Client.rs"),
            ("test_client", "app/auth/tests/ClientTest.rs"),
        ];
        let mut symbols = Vec::new();
        for (id, path) in defs {
            let Some(bytes) = kb::host::facts::blob_at(&self.host, commit, path, 8192).unwrap()
            else {
                continue;
            };
            symbols.push(CodeSymbol {
                id: id.into(),
                name: id.into(),
                kind: "function".into(),
                path: path.into(),
                start_line: 1,
                end_line: 1,
                extent: CodeExtent::Definition,
                sha256: kb::util::sha256_hex(&bytes),
                signature: None,
                test: id.starts_with("test_"),
            });
        }
        let mut refs = vec![CodeRef {
            from: "test_client".into(),
            to: "client".into(),
            kind: CodeRelation::Call,
            confidence: CodeConfidence::Resolved,
            line: 1,
        }];
        if symbols.iter().any(|s| s.id == "save") {
            refs.push(CodeRef {
                from: "client".into(),
                to: "save".into(),
                kind: CodeRelation::Call,
                confidence: CodeConfidence::Possible,
                line: 1,
            });
        }
        CodeResponse {
            protocol: CodeProtocol::V1,
            repo: "mobile".into(),
            commit: commit.into(),
            tool: CodeTool {
                name: "synthetic".into(),
                version: "1".into(),
            },
            capabilities: vec![
                CodeOperation::Symbols,
                CodeOperation::Refs,
                CodeOperation::Dependents,
                CodeOperation::Similar,
            ],
            complete: true,
            limitations: vec![],
            symbols,
            refs,
            similar: vec![],
        }
    }

    fn fixture(&self, graph: &CodeResponse) -> PathBuf {
        let path = self.sb.path().join(format!("{}.json", graph.commit));
        fs::write(&path, serde_json::to_vec(graph).unwrap()).unwrap();
        path
    }

    fn run(&self, args: &[&str], expected: i32) -> serde_json::Value {
        let mut all = vec![
            "--root",
            self.kb.to_str().unwrap(),
            "--host",
            self.host.to_str().unwrap(),
            "--offline",
            "--snapshot",
            "working-tree",
            "--json",
        ];
        all.extend_from_slice(args);
        let out = self.sb.kb(&self.sb.path(), &all, &[]);
        assert_eq!(
            out.status.code(),
            Some(expected),
            "{}\n{}",
            common::stdout(&out),
            common::stderr(&out)
        );
        json(&out)["result"].clone()
    }
}

#[test]
fn rejects_wrong_commit_hash_paths_and_extent_and_normalizes_order() {
    if isolated("rejects_wrong_commit_hash_paths_and_extent_and_normalizes_order") {
        return;
    }
    let w = World::new();
    let request = w.request(&w.base);
    let mut graph = w.graph(&w.base);
    graph.similar = vec![
        CodeSimilar {
            symbol: "save".into(),
            score: 900,
            reason: "z reason".into(),
        },
        CodeSimilar {
            symbol: "client".into(),
            score: 800,
            reason: "another candidate".into(),
        },
        CodeSimilar {
            symbol: "save".into(),
            score: 100,
            reason: "lower score".into(),
        },
        CodeSimilar {
            symbol: "save".into(),
            score: 900,
            reason: "a reason".into(),
        },
    ];
    let expected = kb::code::validate(graph.clone(), &request).unwrap();
    assert_eq!(expected.similar.len(), 2);
    assert_eq!(expected.similar[0].reason, "a reason");
    let mut reversed = graph.clone();
    reversed.symbols.reverse();
    reversed.refs.reverse();
    reversed.capabilities.reverse();
    reversed.similar.reverse();
    assert_eq!(
        serde_json::to_value(expected).unwrap(),
        serde_json::to_value(kb::code::validate(reversed, &request).unwrap()).unwrap()
    );
    for change in [0, 1, 2, 3] {
        let mut bad = graph.clone();
        match change {
            0 => bad.commit = "0".repeat(40),
            1 => bad.symbols[0].sha256 = "0".repeat(64),
            2 => bad.symbols[0].path = "../outside.rs".into(),
            _ => {
                bad.symbols[0].end_line = 2;
                bad.symbols[0].extent = CodeExtent::Line;
            }
        }
        assert!(kb::code::validate(bad, &request).is_err());
    }
}

#[test]
fn uncertainty_propagates_across_resolved_hops_and_depth_is_bounded() {
    if isolated("uncertainty_propagates_across_resolved_hops_and_depth_is_bounded") {
        return;
    }
    let w = World::new();
    let graph = w.graph(&w.base);
    let changed = BTreeSet::from(["app/auth/Core.rs".into()]);
    let one = kb::code::dependents(&graph, &changed, 1);
    assert_eq!(one.len(), 1);
    let two = kb::code::dependents(&graph, &changed, 2);
    assert_eq!(two.len(), 2);
    assert!(two.iter().all(|s| s.confidence == CodeConfidence::Possible));
}

#[test]
fn context_code_is_opt_in_repeatable_budgeted_and_reads_git_not_dirty_source() {
    if isolated("context_code_is_opt_in_repeatable_budgeted_and_reads_git_not_dirty_source") {
        return;
    }
    let w = World::new();
    let fixture = w.fixture(&w.graph(&w.base));
    write(
        &w.host.join("app/auth/Core.rs"),
        "DIRTY WORK MUST NOT BECOME EVIDENCE\n",
    );
    let args = [
        "context",
        "--intent",
        "implement",
        "--path",
        "app/auth/Core.rs",
        "--with-code",
        "--provider-file",
        fixture.to_str().unwrap(),
        "--budget",
        "8000",
    ];
    let a = w.run(&args, 30);
    let b = w.run(&args, 30);
    assert_eq!(a, b);
    let code: Vec<_> = a["units"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|u| u["kind"] == "code")
        .collect();
    assert_eq!(code.len(), 3);
    assert!(code.iter().all(|u| u["tier"] == "supplementary"
        && u["status"] == "observed"
        && u["origin"] == "provider"));
    assert!(!a.to_string().contains("DIRTY WORK"));
    assert!(a.to_string().contains("pub fn save()"));
    let without = w.run(
        &[
            "context",
            "--intent",
            "implement",
            "--path",
            "app/auth/Core.rs",
            "--budget",
            "8000",
        ],
        30,
    );
    assert!(
        !without["units"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["kind"] == "code")
    );
    let budget = (a["budget"]["used"].as_u64().unwrap() - 200).to_string();
    let mut smaller = args;
    smaller[9] = &budget;
    let limited = w.run(&smaller, 30);
    assert!(
        limited["units"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|u| u["kind"] == "code")
            .count()
            < code.len()
    );
    assert_eq!(
        limited["units"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|u| u["tier"] == "mandatory")
            .count(),
        2
    );
    assert!(limited["budget"]["used"].as_u64().unwrap() <= budget.parse::<u64>().unwrap());
}

fn code_units(result: &serde_json::Value) -> Vec<serde_json::Value> {
    result["units"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|u| u["kind"] == "code")
        .cloned()
        .collect()
}

fn code_limitations(result: &serde_json::Value) -> Vec<String> {
    result["code"]["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn non_utf8_code_unit_is_omitted_with_a_limitation_instead_of_failing_context() {
    if isolated("non_utf8_code_unit_is_omitted_with_a_limitation_instead_of_failing_context") {
        return;
    }
    let w = World::new();
    // Synthetic Latin-1 comment byte; the provider stamp still matches the Git blob.
    fs::write(
        w.host.join("app/auth/Client.rs"),
        b"pub fn client() { save(); } // caf\xe9\n",
    )
    .unwrap();
    let head = w.sb.commit_all(&w.host, "synthetic latin-1 comment");
    let fixture = w.fixture(&w.graph(&head));
    let result = w.run(
        &[
            "context",
            "--intent",
            "implement",
            "--path",
            "app/auth/Core.rs",
            "--with-code",
            "--provider-file",
            fixture.to_str().unwrap(),
            "--budget",
            "8000",
        ],
        30,
    );
    let titles: BTreeSet<_> = code_units(&result)
        .iter()
        .map(|u| u["title"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        titles,
        BTreeSet::from(["save".to_string(), "test_client".to_string()])
    );
    assert!(
        code_limitations(&result)
            .contains(&"Omitted 1 code unit(s) whose source is not UTF-8.".into()),
        "{}",
        result["code"]
    );
}

#[test]
fn pinned_response_does_not_replay_task_specific_similar_candidates() {
    if isolated("pinned_response_does_not_replay_task_specific_similar_candidates") {
        return;
    }
    let w = World::new();
    let mut graph = w.graph(&w.base);
    // As generated for another task; `save` is related to the requested path only by it.
    graph.similar = vec![CodeSimilar {
        symbol: "save".into(),
        score: 1000,
        reason: "lexical overlap with the task; inspect before reusing as a precedent".into(),
    }];
    let fixture = w.fixture(&graph);
    let result = w.run(
        &[
            "context",
            "--intent",
            "implement",
            "--path",
            "app/auth/tests/ClientTest.rs",
            "--with-code",
            "--provider-file",
            fixture.to_str().unwrap(),
            "--budget",
            "8000",
        ],
        30,
    );
    let units = code_units(&result);
    assert!(!units.is_empty());
    assert!(
        units.iter().all(
            |u| u["title"] != "save" && !u["why"].as_str().unwrap().contains("lexical overlap")
        ),
        "{units:?}"
    );
    assert!(code_limitations(&result).contains(&kb::code::PINNED_SIMILAR.into()));
    let request = w.request(&w.base);
    let replayed = kb::code::Provider {
        files: vec![fixture],
        ..Default::default()
    }
    .load(&request)
    .unwrap();
    assert!(replayed.similar.is_empty());
}

#[test]
fn validation_reads_files_with_a_constant_number_of_git_processes() {
    let name = "validation_reads_files_with_a_constant_number_of_git_processes";
    let logging = isolated_with(name, |command, dir| {
        // Synthetic shim first on PATH: log each Git invocation, then run the real Git.
        let path = std::env::var_os("PATH").unwrap_or_default();
        let real = std::env::split_paths(&path)
            .map(|d| d.join("git"))
            .find(|p| p.is_file())
            .expect("git on PATH");
        let shim = dir.join("shim");
        write(
            &shim.join("git"),
            &format!(
                "#!/bin/sh\necho \"$*\" >> \"$KB_GIT_LOG\"\nexec '{}' \"$@\"\n",
                real.display()
            ),
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(shim.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut dirs = vec![shim];
        dirs.extend(std::env::split_paths(&path));
        command
            .env("PATH", std::env::join_paths(dirs).unwrap())
            .env("KB_GIT_LOG", dir.join("git.log"));
    });
    if logging {
        return;
    }
    let w = World::new();
    for i in 0..40 {
        write(
            &w.host.join(format!("app/auth/gen/F{i:02}.rs")),
            &format!("pub fn generated_{i}() {{}}\n"),
        );
    }
    let head = w.sb.commit_all(&w.host, "synthetic generated sources");
    let mut graph = w.graph(&head);
    for i in 0..40 {
        graph.symbols.push(CodeSymbol {
            id: format!("generated_{i}"),
            name: format!("generated_{i}"),
            kind: "function".into(),
            path: format!("app/auth/gen/F{i:02}.rs"),
            start_line: 1,
            end_line: 1,
            extent: CodeExtent::Line,
            sha256: kb::util::sha256_hex(format!("pub fn generated_{i}() {{}}\n").as_bytes()),
            signature: None,
            test: false,
        });
    }
    let request = w.request(&head);
    let log = PathBuf::from(std::env::var_os("KB_GIT_LOG").unwrap());
    fs::write(&log, "").unwrap();
    let validated = kb::code::validate(graph, &request).unwrap();
    assert_eq!(validated.symbols.len(), 43);
    let calls = fs::read_to_string(&log).unwrap();
    // Resolve the commit, list the tree, read every blob in one batch.
    assert!(
        calls.lines().count() <= 3 && calls.contains("cat-file --batch"),
        "{calls}"
    );
}

#[test]
fn deep_impact_reads_base_graph_for_deleted_symbol_and_lists_covering_records() {
    if isolated("deep_impact_reads_base_graph_for_deleted_symbol_and_lists_covering_records") {
        return;
    }
    let w = World::new();
    let base_fixture = w.fixture(&w.graph(&w.base));
    fs::remove_file(w.host.join("app/auth/Core.rs")).unwrap();
    let head = w.sb.commit_all(&w.host, "remove synthetic save");
    let head_fixture = w.fixture(&w.graph(&head));
    let result = w.run(
        &[
            "impact",
            "--base",
            &w.base,
            "--deep",
            "--provider-file",
            base_fixture.to_str().unwrap(),
            "--provider-file",
            head_fixture.to_str().unwrap(),
        ],
        0,
    );
    let deep = &result["deep"];
    assert_eq!(deep["sources"].as_array().unwrap().len(), 2);
    let client = deep["dependents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["dependent"]["symbol"] == "client")
        .unwrap();
    assert_eq!(client["at"], w.base);
    assert_eq!(client["modules"], serde_json::json!(["mobile.auth"]));
    assert!(!client["records"].as_array().unwrap().is_empty());
}

#[test]
fn dirty_deep_impact_is_explicitly_incomplete() {
    if isolated("dirty_deep_impact_is_explicitly_incomplete") {
        return;
    }
    let w = World::new();
    let fixture = w.fixture(&w.graph(&w.base));
    write(
        &w.host.join("app/auth/Core.rs"),
        "pub fn newly_added() {}\n",
    );
    let result = w.run(
        &[
            "impact",
            "--changed",
            "--deep",
            "--provider-file",
            fixture.to_str().unwrap(),
        ],
        0,
    );
    assert_eq!(result["deep"]["complete"], false);
    assert!(
        result["deep"]["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().contains("uncommitted"))
    );
}

#[test]
fn filling_contract_consumers_is_reviewable_draft_only_and_preserves_comments() {
    if isolated("filling_contract_consumers_is_reviewable_draft_only_and_preserves_comments") {
        return;
    }
    let w = World::new();
    let fixture = w.fixture(&w.graph(&w.base));
    let input = w.sb.path().join("draft.md");
    write(
        &input,
        &format!(
            r#"+++
# Synthetic author comment must survive enrichment.
schema = 2
id = "acme.contract.save"
kind = "contract"
title = "Synthetic save protocol"
status = "draft"
owner = "arch"
interface = "save"
[scope]
modules = ["mobile.auth"]
[[parties]]
id = "provider"
repo = "mobile"
role = "Saves synthetic state"
[[parties]]
id = "consumer"
repo = "mobile"
role = "Calls save"
[[obligations]]
id = "persist"
party = "provider"
level = "must"
text = "Persist the synthetic state."
[[anchors]]
kind = "source"
repo = "mobile"
path = "app/auth/Core.rs"
symbol = "save"
commit = "{}"
+++

## Synthetic explanation

Keep this body.
"#,
            w.base
        ),
    );
    let result = w.run(
        &[
            "propose",
            "submit",
            input.to_str().unwrap(),
            "--fill-consumers",
            "--provider-file",
            fixture.to_str().unwrap(),
            "--apply",
        ],
        0,
    );
    assert_eq!(result["draft"]["consumer_evidence"]["commit"], w.base);
    let text = fs::read_to_string(w.kb.join(result["draft"]["path"].as_str().unwrap())).unwrap();
    let record = kb::parse::parse_record("draft", text.as_bytes())
        .unwrap()
        .record;
    let Record::Contract(contract) = record else {
        panic!("expected contract");
    };
    assert_eq!(contract.status, Status::Draft);
    assert_eq!(contract.consumers.len(), 1);
    assert_eq!(contract.consumers[0].path, "app/auth/Client.rs");
    assert!(text.contains("Synthetic author comment must survive enrichment."));
    assert!(text.contains("Keep this body."));
}

#[test]
fn coverage_uses_cross_module_caller_files_for_fan_in() {
    if isolated("coverage_uses_cross_module_caller_files_for_fan_in") {
        return;
    }
    let w = World::new();
    let modules_path = w.kb.join("project/registry/modules.toml");
    let mut modules = fs::read_to_string(&modules_path).unwrap();
    modules.push_str("\n[[module]]\nid = \"mobile.settings\"\nrepo = \"mobile\"\ntitle = \"Synthetic settings\"\npaths = [\"app/settings/**\"]\n");
    write(&modules_path, &modules);
    write(
        &w.host.join("app/settings/Screen.rs"),
        "pub fn client() { save(); }\n",
    );
    let head =
        w.sb.commit_all(&w.host, "synthetic caller in another module");
    let mut graph = w.graph(&head);
    graph
        .symbols
        .iter_mut()
        .find(|s| s.id == "client")
        .unwrap()
        .path = "app/settings/Screen.rs".into();
    let fixture = w.fixture(&graph);
    let report = w.run(
        &["coverage", "--provider-file", fixture.to_str().unwrap()],
        0,
    );
    let auth = report["coverage"]["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "mobile.auth")
        .unwrap();
    assert_eq!(auth["fan_in"], 1);
    assert_eq!(report["code"]["commit"], head);
    assert!(
        report["coverage"]["ranking"]
            .as_str()
            .unwrap()
            .contains("fan-in")
    );
}
