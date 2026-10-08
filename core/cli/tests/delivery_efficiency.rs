//! Real CLI evidence for explicit delivery reuse, immutable receipts and hard budgets.
mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::{Sandbox, json, write};
use serde_json::Value;

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct World {
    sb: Sandbox,
    kb: PathBuf,
    host: PathBuf,
}

impl World {
    fn new() -> Self {
        let sb = Sandbox::new();
        let host = sb.path().join("host");
        let kb = host.join(".kb");
        sb.init_repo(&host);
        write(&host.join(".gitignore"), ".kb/\n");
        write(&host.join(".kbw.toml"), "schema = 1\nrepo = \"mobile\"\n");
        write(&host.join("app/auth/Client.rs"), "// Synthetic fixture\n");
        sb.commit_all(&host, "synthetic host");
        sb.init_repo(&kb);
        common::write_min_project(&kb);
        write(&kb.join(".gitignore"), ".cache/\n");
        copy_tree(
            &common::repo_root().join("core/skills"),
            &kb.join("core/skills"),
        );
        write(
            &kb.join("project/skill-config/skill.toml"),
            "schema = 1\nharnesses = [\"claude\",\"codex\",\"cursor\",\"grok\",\"copilot\",\"junie\"]\nkb_path = \".kb\"\n",
        );
        let commit = sb.commit_all(&kb, "synthetic knowledge");
        sb.git(&kb, &["remote", "add", "origin", kb.to_str().unwrap()]);
        sb.git(&kb, &["update-ref", "refs/remotes/origin/main", &commit]);
        Self { sb, kb, host }
    }

    fn output(&self, args: &[&str], json_output: bool) -> Output {
        let mut all = vec![
            "--root",
            self.kb.to_str().unwrap(),
            "--host",
            self.host.to_str().unwrap(),
            "--offline",
            "--snapshot",
            "working-tree",
        ];
        if json_output {
            all.push("--json");
        }
        all.extend_from_slice(args);
        if args.iter().any(|a| matches!(*a, "context" | "outline")) && !args.contains(&"--intent") {
            all.extend_from_slice(&["--intent", "implement"]);
        }
        self.sb.kb(&self.host, &all, &[])
    }

    fn run(&self, args: &[&str], exit: i32) -> Value {
        let out = self.output(args, true);
        assert_eq!(
            out.status.code(),
            Some(exit),
            "{}\n{}",
            common::stdout(&out),
            common::stderr(&out)
        );
        json(&out)
    }

    fn context(&self, more: &[&str]) -> Value {
        let mut args = vec![
            "context",
            "--intent",
            "implement",
            "--path",
            "app/auth/Client.rs",
            "--budget",
            "20000",
        ];
        args.extend_from_slice(more);
        self.run(&args, 30)["result"].clone()
    }

    fn always(&self, text: &str) -> PathBuf {
        let path = self.kb.join("project/knowledge/policies/always.md");
        write(
            &path,
            &format!(
                r#"+++
schema = 2
id = "acme.policy.core"
kind = "policy"
title = "Synthetic always-on rule"
status = "accepted"
owner = "arch"
delivery = "always"
[scope]
product = true
[[rules]]
id = "preserve"
level = "must"
text = "{text}"
conditions = ["When changing synthetic data."]
[[rules.exceptions]]
id = "fixture"
text = "Disposable labeled fixtures may be replaced."
+++

## Example

This deferred body is preserved verbatim.
"#
            ),
        );
        path
    }

    fn integrate(&self) -> Value {
        self.run(&["integrate", "--generate", "--apply"], 0);
        self.run(&["integrate", "--apply"], 0);
        self.run(&["integrate", "--probe"], 0)["result"].clone()
    }
}

fn ids(result: &Value) -> BTreeSet<String> {
    result["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn delta_reuses_only_explicitly_delivered_unchanged_content() {
    let w = World::new();
    let full = w.context(&[]);
    let receipt = full["receipt"]["id"].as_str().unwrap();
    assert_eq!(full["receipt"]["protocol"], "kb.receipt.v2");
    let delta = w.context(&["--since-receipt", receipt]);
    assert_eq!(ids(&delta), ids(&full));
    assert!(
        delta["units"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u["delivery"] == "unchanged" && u.get("content").is_none())
    );
    assert!(delta["budget"]["used"].as_u64().unwrap() < full["budget"]["used"].as_u64().unwrap());
    assert!(
        w.context(&[])["units"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u.get("delivery").is_none())
    );
    let contract = w.kb.join("project/knowledge/contracts/token-api.md");
    let text = fs::read_to_string(&contract).unwrap();
    write(
        &contract,
        &text.replace(
            "Rotate the refresh token on every refresh call.",
            "Rotate and persist the refresh token on every refresh call.",
        ),
    );
    let changed = w.context(&["--since-receipt", receipt]);
    assert_ne!(
        changed["snapshot"]["content_digest"],
        full["snapshot"]["content_digest"]
    );
    let contract = changed["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == "acme.contract.token-api")
        .unwrap();
    assert!(contract.get("content").is_some());
    assert!(contract.get("delivery").is_none());
    assert!(
        changed["units"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["delivery"] == "unchanged")
    );
    let chained = w.context(&[
        "--since-receipt",
        changed["receipt"]["id"].as_str().unwrap(),
    ]);
    assert!(
        chained["units"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u["delivery"] == "unchanged")
    );
}

#[test]
fn missing_corrupt_or_different_host_receipts_do_not_suppress_knowledge() {
    let w = World::new();
    let missing = format!("sha256:{}", "a".repeat(64));
    assert_eq!(
        w.run(&["context", "--since-receipt", &missing], 16)["error"]["code"],
        "NOT_FOUND"
    );
    let full = w.context(&[]);
    let id = full["receipt"]["id"].as_str().unwrap();
    let subject_dir = fs::read_dir(w.kb.join(".cache/context-receipts"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let path = subject_dir.join(format!("{}.json", id.strip_prefix("sha256:").unwrap()));
    let mut stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    stored["subject"] = Value::String("different-host".into());
    fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
    assert_eq!(
        w.run(&["context", "--since-receipt", id], 64)["error"]["code"],
        "INVALID_INPUT"
    );
    write(&path, "{malformed");
    assert_eq!(
        w.run(&["context", "--since-receipt", id], 64)["error"]["code"],
        "INVALID_INPUT"
    );
}

#[test]
fn core_is_omitted_only_after_loaded_source_and_current_content_are_verified() {
    let w = World::new();
    let record = w.always("Preserve synthetic state.");
    write(&w.host.join("AGENTS.md"), "Existing team guidance.\n");
    write(
        &w.host.join(".junie/AGENTS.md"),
        "Existing Junie guidance.\n",
    );
    write(
        &w.host.join("AGENTS.override.md"),
        "Existing Codex override.\n",
    );
    let probe = w.integrate();
    assert_eq!(probe["installation_verified"], true);
    assert_eq!(probe["runtime_load_verified"], false);
    let receipt = probe["core_receipt"].as_str().unwrap();
    let source = probe["core_source"].as_str().unwrap();
    let full = w.context(&[]);
    let delivered = w.context(&["--core-receipt", receipt, "--core-source", source]);
    assert_eq!(ids(&full), ids(&delivered));
    let core = delivered["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == "acme.policy.core")
        .unwrap();
    assert_eq!(core["delivery"], "core");
    assert!(core.get("content").is_none());
    let agents = fs::read_to_string(w.host.join("AGENTS.md")).unwrap();
    assert!(agents.starts_with("Existing team guidance.\n"));
    assert!(
        agents.contains("When changing synthetic data.")
            && agents.contains("Disposable labeled fixtures may be replaced.")
    );
    assert!(
        fs::read_to_string(w.host.join("AGENTS.override.md"))
            .unwrap()
            .starts_with("Existing Codex override.\n")
    );
    assert!(
        fs::read_to_string(w.host.join(".junie/AGENTS.md"))
            .unwrap()
            .starts_with("Existing Junie guidance.\n")
    );
    write(
        &record,
        &fs::read_to_string(&record).unwrap().replace(
            "Preserve synthetic state.",
            "Preserve and audit synthetic state.",
        ),
    );
    assert_eq!(
        w.run(
            &[
                "context",
                "--path",
                "app/auth/Client.rs",
                "--core-receipt",
                receipt,
                "--core-source",
                source
            ],
            64
        )["error"]["code"],
        "INVALID_INPUT"
    );
}

#[test]
fn core_references_in_a_receipt_never_become_unchanged_without_core_proof() {
    let w = World::new();
    w.always("Preserve synthetic state.");
    let probe = w.integrate();
    let receipt = probe["core_receipt"].as_str().unwrap();
    let source = probe["core_source"].as_str().unwrap();
    let referenced = w.context(&["--core-receipt", receipt, "--core-source", source]);
    let unit = |result: &Value, id: &str| {
        result["units"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(unit(&referenced, "acme.policy.core")["delivery"], "core");
    let path = w.host.join(source);
    write(
        &path,
        &fs::read_to_string(&path)
            .unwrap()
            .replace("Preserve synthetic state.", "Do anything."),
    );
    assert_eq!(
        w.run(
            &[
                "context",
                "--path",
                "app/auth/Client.rs",
                "--core-receipt",
                receipt,
                "--core-source",
                source
            ],
            64
        )["error"]["code"],
        "INVALID_INPUT"
    );
    let delta = w.context(&[
        "--since-receipt",
        referenced["receipt"]["id"].as_str().unwrap(),
    ]);
    let core = unit(&delta, "acme.policy.core");
    assert!(core.get("delivery").is_none(), "{core}");
    assert!(core.get("content").is_some());
    assert_eq!(
        unit(&delta, "acme.contract.token-api")["delivery"],
        "unchanged"
    );
}

#[test]
fn edited_core_source_and_oversized_core_are_rejected_without_truncation() {
    let w = World::new();
    w.always("Preserve synthetic state.");
    let probe = w.integrate();
    let receipt = probe["core_receipt"].as_str().unwrap();
    let path = w.host.join("AGENTS.md");
    write(
        &path,
        &fs::read_to_string(&path)
            .unwrap()
            .replace("Preserve synthetic state.", "Do anything."),
    );
    assert_eq!(
        w.run(
            &[
                "context",
                "--core-receipt",
                receipt,
                "--core-source",
                "AGENTS.md"
            ],
            64
        )["error"]["code"],
        "INVALID_INPUT"
    );
    let old_manifest = fs::read(w.kb.join("project/skill-config/generated/manifest.toml")).unwrap();
    w.always(&"Preserve this synthetic state. ".repeat(150));
    let rejected = w.run(&["integrate", "--generate", "--apply"], 64);
    assert!(
        rejected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cap is 600")
    );
    assert_eq!(
        fs::read(w.kb.join("project/skill-config/generated/manifest.toml")).unwrap(),
        old_manifest
    );
}

#[test]
fn one_physical_skill_serves_all_harnesses_without_creating_masking_overrides() {
    let w = World::new();
    w.integrate();
    assert!(w.host.join(".agents/skills/kb/SKILL.md").is_file());
    assert!(
        fs::read_to_string(w.host.join(".claude/commands/kb.md"))
            .unwrap()
            .contains(".agents/skills/kb/SKILL.md")
    );
    for path in [
        ".claude/skills/kb/SKILL.md",
        ".grok/skills/kb/SKILL.md",
        ".kbw/skills/kb/SKILL.md",
        ".junie/AGENTS.md",
        "AGENTS.override.md",
    ] {
        assert!(
            !w.host.join(path).exists(),
            "unexpected duplicate or masking file: {path}"
        );
    }
    assert!(
        fs::read_to_string(w.host.join(".cursor/rules/kb.mdc"))
            .unwrap()
            .starts_with("---\n")
    );
    assert!(
        fs::read_to_string(w.host.join(".cursor/rules/kb.mdc"))
            .unwrap()
            .contains("alwaysApply: true")
    );
    assert!(
        fs::read_to_string(w.host.join("CLAUDE.md"))
            .unwrap()
            .contains("Read `AGENTS.md`")
    );
    assert!(
        fs::read_to_string(w.host.join(".github/copilot-instructions.md"))
            .unwrap()
            .contains("Read `AGENTS.md`")
    );
}

#[test]
fn terse_preserves_the_unit_set_and_conditions_while_sections_are_deferred() {
    let w = World::new();
    w.always("Preserve synthetic state.");
    let full = w.context(&[]);
    let out = w.output(
        &[
            "context",
            "--path",
            "app/auth/Client.rs",
            "--format",
            "terse",
            "--budget-unit",
            "bytes",
            "--budget",
            "8000",
        ],
        false,
    );
    assert_eq!(out.status.code(), Some(30), "{}", common::stdout(&out));
    let text = common::stdout(&out);
    for id in ids(&full) {
        assert!(text.contains(&id), "missing {id}");
    }
    assert!(
        text.contains("When changing synthetic data.")
            && text.contains("Disposable labeled fixtures may be replaced.")
    );
    assert!(!text.contains("This deferred body is preserved verbatim."));
    assert!(out.stdout.len() <= 8000);
    let shown = w.run(&["show", "acme.policy.core", "--sections"], 0);
    assert!(
        shown["result"]["sections"]
            .to_string()
            .contains("This deferred body is preserved verbatim.")
    );
    let tiny = w.output(
        &[
            "context",
            "--path",
            "app/auth/Client.rs",
            "--format",
            "terse",
            "--budget",
            "10",
        ],
        false,
    );
    assert_eq!(tiny.status.code(), Some(31));
}

#[test]
fn outline_is_bounded_and_never_saved_as_a_delivery_receipt() {
    let w = World::new();
    let out = w.run(&["outline", "--path", "app/auth/Client.rs"], 30);
    let value = &out["result"];
    assert_eq!(value["protocol"], "kb.outline.v1");
    assert!(value.get("receipt").is_none());
    assert!(
        value["units"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u["mandatory"].is_boolean()
                && u["tokens"].is_u64()
                && u["reason"].is_string())
    );
    let rendered = serde_json::to_string_pretty(value)
        .unwrap()
        .replace('\n', "\n  ");
    assert!(kb::context::estimate_tokens(&rendered) <= 2000);
    assert!(!w.kb.join(".cache/context-receipts").exists());
}

#[test]
fn terse_reduces_repeated_fields_without_losing_obligations_or_lifecycle_labels() {
    let w = World::new();
    let mut text = String::from(
        r#"+++
schema = 2
id = "acme.policy.many-rules"
kind = "policy"
title = "Synthetic independent obligations"
status = "accepted"
owner = "arch"
[scope]
product = true
[links]
requires = ["acme.reference.legacy"]
"#,
    );
    for i in 0..24 {
        text.push_str(&format!(
            r#"
[[rules]]
id = "rule-{i}"
level = "must"
text = "Preserve synthetic value {i}."
conditions = ["When synthetic mode {i} applies."]
[[rules.exceptions]]
id = "backup-{i}"
text = "A verified backup of value {i} is available."
"#
        ));
    }
    text.push_str("+++\n");
    write(&w.kb.join("project/knowledge/policies/many.md"), &text);
    write(
        &w.kb.join("project/knowledge/legacy.md"),
        r#"+++
schema = 2
id = "acme.reference.legacy"
kind = "reference"
title = "Synthetic deprecated dependency"
status = "deprecated"
owner = "arch"
summary = "Retained historical explanation."
[scope]
product = true
+++
"#,
    );
    let render = |format| {
        w.output(
            &[
                "context",
                "--path",
                "app/auth/Client.rs",
                "--format",
                format,
                "--budget",
                "30000",
            ],
            false,
        )
    };
    let compact = render("compact");
    let terse = render("terse");
    assert_eq!(compact.status.code(), Some(30));
    assert_eq!(terse.status.code(), Some(30));
    let text = common::stdout(&terse);
    for i in 0..24 {
        for value in [
            format!("Preserve synthetic value {i}."),
            format!("When synthetic mode {i} applies."),
            format!("A verified backup of value {i} is available."),
        ] {
            assert!(
                text.contains(&value),
                "lost obligation, condition or exception: {value}"
            );
        }
    }
    assert!(
        text.lines().any(|line| line.starts_with('[')
            && line.contains("acme.reference.legacy")
            && line.contains("deprecated")),
        "deprecated dependency needs its own lifecycle label"
    );
    assert!(
        terse.stdout.len() < compact.stdout.len(),
        "terse={} compact={}",
        terse.stdout.len(),
        compact.stdout.len()
    );
}

#[test]
fn usage_log_preserves_calls_omits_text_and_detects_a_final_scope_change() {
    let w = World::new();
    let modules = w.kb.join("project/registry/modules.toml");
    write(
        &modules,
        &format!(
            "{}\n[[module]]\nid = \"mobile.settings\"\nrepo = \"mobile\"\ntitle = \"Synthetic settings\"\npaths = [\"app/settings/**\"]\n",
            fs::read_to_string(&modules).unwrap()
        ),
    );
    write(
        &w.kb.join("project/knowledge/policies/auth.md"),
        "+++\nschema = 2\nid = \"acme.policy.auth\"\nkind = \"policy\"\ntitle = \"Synthetic scoped knowledge\"\nstatus = \"accepted\"\nowner = \"arch\"\n[scope]\nmodules = [\"mobile.auth\"]\n[[rules]]\nid = \"state\"\nlevel = \"must\"\ntext = \"SENSITIVE_RULE_TEXT stays out of the usage log.\"\n+++\n",
    );
    let first = w.context(&["--task", "SENSITIVE_TASK_TEXT"]);
    let id = first["receipt"]["id"].as_str().unwrap();
    let path = w.kb.join(".cache/usage/delivery.v1.jsonl");
    let before = fs::read(&path).unwrap();
    w.context(&["--task", "SENSITIVE_TASK_TEXT", "--since-receipt", id]);
    let after = fs::read(&path).unwrap();
    assert!(after.starts_with(&before));
    let text = String::from_utf8(after).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(!text.contains("SENSITIVE_TASK_TEXT"));
    assert!(!text.contains("SENSITIVE_RULE_TEXT"));
    write(
        &w.host.join("app/settings/Preferences.rs"),
        "// Synthetic final edit in another module\n",
    );
    let result = w.run(&["usage", "report", "--receipt", id], 0);
    let report = &result["result"]["usage"];
    assert_eq!(report["calls"], 1);
    assert_eq!(report["delivered_but_irrelevant"]["acme.policy.auth"], 1);
    assert!(
        report["delivered_but_irrelevant"]
            .get("acme.mobile.token-storage")
            .is_none()
    );
    assert_eq!(
        report["touched_but_undelivered"],
        serde_json::json!(["mobile.settings"])
    );
    assert_eq!(
        report["without_domain_knowledge"],
        serde_json::json!(["mobile.settings"])
    );
    let missing = format!("sha256:{}", "f".repeat(64));
    w.run(&["usage", "report", "--receipt", &missing], 30);
}

#[test]
fn usage_report_scopes_diffs_with_more_paths_than_the_path_option_limit() {
    let w = World::new();
    write(
        &w.kb.join("project/knowledge/policies/auth.md"),
        "+++\nschema = 2\nid = \"acme.policy.auth\"\nkind = \"policy\"\ntitle = \"Synthetic scoped knowledge\"\nstatus = \"accepted\"\nowner = \"arch\"\n[scope]\nmodules = [\"mobile.auth\"]\n[[rules]]\nid = \"state\"\nlevel = \"must\"\ntext = \"Synthetic scoped rule.\"\n+++\n",
    );
    let first = w.context(&[]);
    let id = first["receipt"]["id"].as_str().unwrap();
    for n in 0..520 {
        write(
            &w.host.join(format!("app/auth/generated/G{n}.rs")),
            "// synthetic generated file\n",
        );
    }
    // Diff paths are bounded by the diff, not by the 512 `--path` values of context.
    let result = w.run(&["usage", "report", "--receipt", id], 0);
    let report = &result["result"]["usage"];
    assert_eq!(report["calls"], 1);
    assert!(
        report["delivered_but_irrelevant"]
            .get("acme.policy.auth")
            .is_none(),
        "{report}"
    );
    assert_eq!(
        result["result"]["diff"]["files"].as_array().unwrap().len(),
        520
    );
}

#[test]
fn identical_receipts_are_stored_separately_for_distinct_host_checkouts() {
    let w = World::new();
    let first = w.context(&[]);
    let id = first["receipt"]["id"].as_str().unwrap();
    let other = w.sb.path().join("other-host");
    w.sb.git(
        &w.sb.path(),
        &[
            "clone",
            "--local",
            "--no-hardlinks",
            w.host.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    let args = [
        "--root",
        w.kb.to_str().unwrap(),
        "--host",
        other.to_str().unwrap(),
        "--offline",
        "--snapshot",
        "working-tree",
        "--json",
        "context",
        "--intent",
        "implement",
        "--path",
        "app/auth/Client.rs",
        "--budget",
        "20000",
    ];
    let out = w.sb.kb(&other, &args, &[]);
    assert_eq!(out.status.code(), Some(30), "{}", common::stdout(&out));
    let second = json(&out);
    assert_eq!(second["result"]["receipt"]["id"], id);
    assert!(!common::stderr(&out).contains("RECEIPT_NOT_SAVED"));
    let mut delta_args = args.to_vec();
    delta_args.extend_from_slice(&["--since-receipt", id]);
    let out = w.sb.kb(&other, &delta_args, &[]);
    assert_eq!(out.status.code(), Some(30), "{}", common::stdout(&out));
    assert!(
        json(&out)["result"]["units"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u["delivery"] == "unchanged")
    );
}
