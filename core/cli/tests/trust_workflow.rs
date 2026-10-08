//! Synthetic Git evidence for stamp, drift, freshness and ledger. No network or wall clock.
mod common;

use std::fs;
use std::path::PathBuf;

use common::{Sandbox, json, write};
use serde_json::{Value, json as value};

struct World {
    sb: Sandbox,
    kb: PathBuf,
    host: PathBuf,
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
        let commit = sb.commit_all(&kb, "synthetic KB");
        sb.git(&kb, &["remote", "add", "origin", kb.to_str().unwrap()]);
        sb.git(&kb, &["update-ref", "refs/remotes/origin/main", &commit]);
        sb.init_repo(&host);
        write(&host.join(".kbw.toml"), "schema = 1\nrepo = \"mobile\"\n");
        write(
            &host.join("app/auth/Store.rs"),
            "// Synthetic source\npub fn persist() -> u32 {\n    1\n}\n",
        );
        let base = sb.commit_all(&host, "synthetic baseline");
        Self { sb, kb, host, base }
    }

    fn run(&self, args: &[&str], exit: i32) -> Value {
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
            Some(exit),
            "{}\n{}",
            common::stdout(&out),
            common::stderr(&out)
        );
        json(&out)
    }

    fn policy(&self, symbol: &str) -> PathBuf {
        let path = self.kb.join("project/knowledge/policies/evidence.md");
        write(
            &path,
            &format!(
                r#"+++
# Synthetic reviewer comment.
schema = 2
id = "acme.policy.evidence"
kind = "policy"
title = "Synthetic persistence contract"
status = "accepted"
owner = "arch"
[scope]
modules = ["mobile.auth"]
[[rules]]
id = "persist"
level = "must"
text = "Persist synthetic state."
[[anchors]]
kind = "source"
repo = "mobile"
path = "app/auth/Store.rs"
symbol = "{symbol}"
+++

## Evidence

Keep this author-written body.
"#
            ),
        );
        path
    }

    fn stamp(&self) -> Value {
        self.run(
            &[
                "anchors",
                "stamp",
                "--id",
                "acme.policy.evidence",
                "--verified-at",
                "2026-01-01",
                "--review-by",
                "2026-02-01",
                "--apply",
            ],
            0,
        )
    }
}

#[test]
fn stamp_is_previewable_idempotent_and_preserves_status_comments_body_and_dirty_host() {
    let w = World::new();
    let path = w.policy("persist");
    let before = fs::read(&path).unwrap();
    let kb_head = w.sb.git(&w.kb, &["rev-parse", "HEAD"]);
    write(
        &w.host.join("app/auth/Store.rs"),
        "dirty host content that must not be stamped\n",
    );
    let preview = w.run(&["anchors", "stamp", "--id", "acme.policy.evidence"], 0);
    assert_eq!(preview["result"]["written"], 0);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        preview["result"]["plan"]["changes"][0]["anchors"][0]["stamp"]["commit"],
        w.base
    );
    assert_eq!(w.stamp()["result"]["written"], 1);
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("# Synthetic reviewer comment.")
            && text.contains("Keep this author-written body.")
    );
    let record = kb::parse::parse_record("record", text.as_bytes())
        .unwrap()
        .record;
    assert_eq!(record.status(), kb::model::Status::Accepted);
    assert_eq!(record.common().verified_at, Some("2026-01-01"));
    assert_eq!(
        record.common().anchors[0].stamp.as_ref().unwrap().end_line,
        4
    );
    assert_eq!(w.stamp()["result"]["written"], 0);
    assert_eq!(w.sb.git(&w.kb, &["rev-parse", "HEAD"]), kb_head);
    assert_eq!(
        fs::read_to_string(w.host.join("app/auth/Store.rs")).unwrap(),
        "dirty host content that must not be stamped\n"
    );
    let check = w.run(
        &[
            "anchors",
            "check",
            "--id",
            "acme.policy.evidence",
            "--strict",
        ],
        0,
    );
    assert_eq!(check["result"]["anchors"]["verified"], 1);
}

#[test]
fn committed_drift_is_grouped_by_owner_and_changes_ledger_support() {
    let w = World::new();
    w.policy("persist");
    w.stamp();
    let before = w.run(&["ledger", "--id", "acme.policy.evidence", "--check"], 0);
    assert_eq!(
        before["result"]["ledger"]["accepted_counts"]["supported"],
        1
    );
    write(
        &w.host.join("app/auth/Store.rs"),
        "// Synthetic changed source\npub fn persist() -> u32 {\n    2\n}\n",
    );
    w.sb.commit_all(&w.host, "synthetic behavior change");
    w.run(&["anchors", "check", "--id", "acme.policy.evidence"], 42);
    let drift = w.run(&["drift", "--id", "acme.policy.evidence", "--check"], 42);
    assert_eq!(
        drift["result"]["drift"]["by_owner"]["arch"][0]["state"],
        "changed"
    );
    assert_eq!(
        drift["result"]["drift"]["by_owner"]["arch"][0]["anchors"][0]["from"],
        w.base
    );
    let ledger = w.run(&["ledger", "--id", "acme.policy.evidence", "--check"], 42);
    assert_eq!(ledger["result"]["ledger"]["accepted_counts"]["stale"], 1);
}

#[test]
fn freshness_is_date_bound_visible_in_context_and_never_drops_the_rule() {
    let w = World::new();
    w.policy("persist");
    w.stamp();
    let validation = w.run(
        &[
            "validate",
            "--stale",
            "10",
            "--on",
            "2026-03-01",
            "--strict",
        ],
        40,
    );
    let diagnostics = validation["result"]["validation"]["diagnostics"]
        .as_array()
        .unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|d| d["record"] == "acme.policy.evidence" && d["code"] == "KNOWLEDGE_STALE")
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d["record"] == "acme.policy.evidence" && d["code"] == "REVIEW_OVERDUE")
    );
    let args = [
        "context",
        "--intent",
        "implement",
        "--path",
        "app/auth/Store.rs",
        "--on",
        "2026-03-01",
        "--stale",
        "10",
    ];
    let a = w.run(&args, 30);
    let b = w.run(&args, 30);
    assert_eq!(a["result"], b["result"]);
    assert!(
        a["result"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "REVIEW_OVERDUE")
    );
    assert!(
        a["result"]["units"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["id"] == "acme.policy.evidence" && u["tier"] == "mandatory")
    );
    let invalid = w.run(&["validate", "--stale", "10", "--on", "2026-02-30"], 64);
    assert_eq!(invalid["error"]["code"], "INVALID_INPUT");
}

#[test]
fn qualified_provider_symbol_stamps_exact_definition_and_git_only_check_verifies_it() {
    let w = World::new();
    let path = w.policy("Store.persist");
    let bytes = fs::read(w.host.join("app/auth/Store.rs")).unwrap();
    let span = kb::provenance::line_span(&bytes, 2, 4).unwrap();
    let fixture = w.sb.path().join("provider.json");
    fs::write(&fixture,serde_json::to_vec(&value!({"protocol":"kb.code.v1","repo":"mobile","commit":w.base,"tool":{"name":"synthetic","version":"1"},"capabilities":["symbols"],"complete":true,"symbols":[{"id":"persist","name":"Store.persist","kind":"function","path":"app/auth/Store.rs","start_line":2,"end_line":4,"extent":"definition","sha256":kb::util::sha256_hex(span)}],"refs":[]})).unwrap()).unwrap();
    let result = w.run(
        &[
            "anchors",
            "stamp",
            "--id",
            "acme.policy.evidence",
            "--provider-file",
            fixture.to_str().unwrap(),
            "--apply",
        ],
        0,
    );
    assert_eq!(
        result["result"]["plan"]["changes"][0]["anchors"][0]["extent"],
        "provider-definition"
    );
    let parsed = kb::parse::parse_record("stamped", &fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        parsed.record.common().anchors[0]
            .stamp
            .as_ref()
            .unwrap()
            .start_line,
        2
    );
    w.run(
        &[
            "anchors",
            "check",
            "--id",
            "acme.policy.evidence",
            "--strict",
        ],
        0,
    );
}

#[test]
fn symbol_prefix_and_missing_baseline_remain_unverifiable() {
    let w = World::new();
    w.policy("persis");
    let rejected = w.run(
        &[
            "anchors",
            "stamp",
            "--id",
            "acme.policy.evidence",
            "--apply",
        ],
        64,
    );
    assert!(
        rejected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("symbol")
    );
    let drift = w.run(&["drift", "--id", "acme.policy.evidence", "--check"], 30);
    assert_eq!(drift["result"]["drift"]["unverifiable_records"], 1);
}

#[test]
fn ledger_draft_audit_is_seeded_repeatable_and_never_accepts_a_record() {
    let w = World::new();
    for id in ["one", "two", "three"] {
        write(
            &w.kb.join(format!("project/knowledge/references/{id}.md")),
            &format!(
                "+++\nschema = 2\nid = \"acme.reference.{id}\"\nkind = \"reference\"\ntitle = \"Synthetic {id}\"\nstatus = \"draft\"\nowner = \"arch\"\nsummary = \"Synthetic orientation.\"\n[scope]\nproduct = true\n+++\n"
            ),
        );
    }
    let args = ["ledger", "--sample", "2", "--seed", "synthetic-audit"];
    let a = w.run(&args, 0);
    let b = w.run(&args, 0);
    assert_eq!(a["result"], b["result"]);
    let sample = a["result"]["ledger"]["draft_audit"].as_array().unwrap();
    assert_eq!(sample.len(), 2);
    assert_ne!(sample[0]["id"], sample[1]["id"]);
    for entry in sample {
        let raw = fs::read(w.kb.join(entry["path"].as_str().unwrap())).unwrap();
        assert_eq!(
            kb::parse::parse_record("draft", &raw)
                .unwrap()
                .record
                .status(),
            kb::model::Status::Draft
        );
    }
}
