//! Execute shipped CI glue against offline platform fixtures and the actual engine.
mod common;
use common::{Sandbox, write};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn script(sb: &Sandbox, cwd: &Path, name: &str, args: &[&str]) -> Output {
    let mut cmd = Command::new("sh");
    cmd.current_dir(cwd)
        .arg(common::repo_root().join(format!("core/templates/ci/{name}")))
        .args(args);
    common::isolated_env(&mut cmd, &sb.home());
    cmd.env("KB_REVIEW_DATE", "2026-01-01");
    cmd.output().unwrap()
}

fn engine_checkout(sb: &Sandbox, root: &Path) {
    sb.init_repo(root);
    common::write_min_project(root);
    write(&root.join(".gitignore"), "kbw\n.cache/\n");
    sb.commit_all(root, "synthetic knowledge");
    fs::copy(common::kb_bin(), root.join("kbw")).unwrap();
}

#[test]
fn platform_exports_preserve_review_text_and_reject_unmerged_changes() {
    let sb = Sandbox::new();
    let raw = sb.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    let body = "Synthetic review text; $(touch should-not-exist); `echo ignored`";
    for platform in ["github", "gitlab"] {
        let change = if platform == "github" {
            json!({"merged":true,"title":"Synthetic change","base":{"sha":"a".repeat(40)},"head":{"sha":"b".repeat(40)},"merge_commit_sha":"c".repeat(40)})
        } else {
            json!({"state":"merged","title":"Synthetic change","diff_refs":{"base_sha":"a".repeat(40),"head_sha":"b".repeat(40)},"squash_commit_sha":"c".repeat(40)})
        };
        let comments = if platform == "github" {
            json!([{"user":{"login":"reviewer"},"body":body,"path":"src/lib.rs","commit_id":"b".repeat(40)}])
        } else {
            json!([{"author":{"username":"reviewer"},"body":body,"position":{"new_path":"src/lib.rs","head_sha":"b".repeat(40)}},{"system":true,"body":"synthetic system event"}])
        };
        write(&raw.join("change.json"), &change.to_string());
        write(&raw.join("comments.json"), &comments.to_string());
        let out = script(
            &sb,
            &sb.path(),
            "normalize-change.sh",
            &[platform, "mobile", raw.to_str().unwrap()],
        );
        assert!(out.status.success(), "{}", common::stderr(&out));
        let export: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(export["review_comments"].as_array().unwrap().len(), 1);
        assert_eq!(export["review_comments"][0]["body"], body);
        assert_eq!(export["base"], "a".repeat(40));
        assert_eq!(export["head"], "b".repeat(40));
        assert!(!sb.path().join("should-not-exist").exists());
        let mut unmerged = change;
        unmerged["merged"] = json!(false);
        unmerged["state"] = json!("opened");
        write(&raw.join("change.json"), &unmerged.to_string());
        let out = script(
            &sb,
            &sb.path(),
            "normalize-change.sh",
            &[platform, "mobile", raw.to_str().unwrap()],
        );
        assert!(!out.status.success());
    }
}

#[test]
fn evidence_job_preserves_all_native_failures_without_skipping_later_checks() {
    let sb = Sandbox::new();
    let root = sb.path().join("kb");
    engine_checkout(&sb, &root);
    let out = script(&sb, &root, "check-knowledge.sh", &[]);
    assert_eq!(out.status.code(), Some(1), "{}", common::stderr(&out));
    let runs: Vec<_> = fs::read_dir(root.join(".cache/knowledge-ci"))
        .unwrap()
        .map(|r| r.unwrap().path())
        .collect();
    assert_eq!(runs.len(), 1);
    let status = fs::read_to_string(runs[0].join("status.tsv")).unwrap();
    for entry in [
        "validate\t0",
        "routing\t0",
        "anchors\t30",
        "drift\t30",
        "ledger\t30",
    ] {
        assert!(status.contains(entry), "{status}");
    }
    let ledger: Value =
        serde_json::from_slice(&fs::read(runs[0].join("ledger.json")).unwrap()).unwrap();
    assert!(
        ledger["result"]["ledger"]["accepted_counts"]["unverifiable"]
            .as_u64()
            .unwrap()
            > 0
    );
}

#[test]
fn publisher_submission_validates_real_drafts_and_stages_only_reported_paths() {
    let sb = Sandbox::new();
    let root = sb.path().join("kb");
    let host = sb.path().join("host");
    engine_checkout(&sb, &root);
    sb.init_repo(&host);
    write(
        &host.join("src/store.rs"),
        "// Synthetic source\nfn save() {}\n",
    );
    let sha = sb.commit_all(&host, "synthetic host");
    let drafts = sb.path().join("drafts");
    let text = format!(
        "+++\nschema = 2\nid = \"acme.decision.store\"\nkind = \"decision\"\ntitle = \"Synthetic storage choice\"\nstatus = \"draft\"\nowner = \"arch\"\ncontext = \"A synthetic choice.\"\ndecision = \"Use one writer for the synthetic store.\"\nreasons = [\"Avoid conflicting writers.\"]\nconsequences = [\"Serialized writes.\"]\n[scope]\nrepos = [\"mobile\"]\n[[anchors]]\nkind = \"source\"\nrepo = \"mobile\"\npath = \"src/store.rs\"\ncommit = \"{sha}\"\n+++\n"
    );
    write(&drafts.join("record.md"), &text);
    let reports = sb.path().join("reports");
    let out = script(
        &sb,
        &root,
        "submit-drafts.sh",
        &[
            drafts.to_str().unwrap(),
            host.to_str().unwrap(),
            "mobile",
            reports.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "{}\n{}",
        common::stdout(&out),
        common::stderr(&out)
    );
    let report: Value = serde_json::from_slice(&fs::read(reports.join("1.json")).unwrap()).unwrap();
    let path = report["result"]["draft"]["path"].as_str().unwrap();
    assert_eq!(sb.git(&root, &["diff", "--cached", "--name-only"]), path);
    assert!(
        fs::read_to_string(root.join(path))
            .unwrap()
            .contains("status = \"draft\"")
    );
    sb.commit_all(&root, "synthetic reviewed draft branch");
    write(
        &drafts.join("record.md"),
        &text.replace("status = \"draft\"", "status = \"accepted\""),
    );
    let rejected = sb.path().join("rejected");
    let out = script(
        &sb,
        &root,
        "submit-drafts.sh",
        &[
            drafts.to_str().unwrap(),
            host.to_str().unwrap(),
            "mobile",
            rejected.to_str().unwrap(),
        ],
    );
    assert!(!out.status.success());
    assert!(
        sb.git(&root, &["diff", "--cached", "--name-only"])
            .is_empty()
    );
}
