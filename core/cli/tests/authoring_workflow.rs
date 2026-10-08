//! Real CLI coverage/capture/proposal workflow, using synthetic local repositories only.
mod common;

use std::fs;
use std::path::PathBuf;

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
        write(&kb.join(".gitignore"), ".cache/\n");
        let templates = kb.join("core/templates/records");
        fs::create_dir_all(&templates).unwrap();
        for file in fs::read_dir(common::repo_root().join("core/templates/records")).unwrap() {
            let file = file.unwrap();
            if file.path().extension().is_some_and(|x| x == "md") {
                fs::copy(file.path(), templates.join(file.file_name())).unwrap();
            }
        }
        sb.git(&kb, &["remote", "add", "origin", kb.to_str().unwrap()]);
        let commit = sb.commit_all(&kb, "synthetic knowledge");
        sb.git(&kb, &["update-ref", "refs/remotes/origin/main", &commit]);
        sb.init_repo(&host);
        write(&host.join(".kbw.toml"), "schema = 1\nrepo = \"mobile\"\n");
        write(
            &host.join("app/auth/Client.rs"),
            "// Synthetic fixture\nstruct Client;\n",
        );
        sb.commit_all(&host, "synthetic host");
        Self { sb, kb, host }
    }

    fn run(&self, args: &[&str], expected: i32) -> Value {
        self.run_on("working-tree", args, expected)
    }

    fn run_on(&self, snapshot: &str, args: &[&str], expected: i32) -> Value {
        let mut all = vec![
            "--root",
            self.kb.to_str().unwrap(),
            "--host",
            self.host.to_str().unwrap(),
            "--offline",
            "--snapshot",
            snapshot,
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
        json(&out)
    }

    fn reference(&self, id: &str, status: &str, summary: &str, path: &str) -> String {
        let commit = self.sb.git(&self.host, &["rev-parse", "HEAD"]);
        format!(
            "+++\nschema = 2\nid = \"acme.reference.{id}\"\nkind = \"reference\"\ntitle = \"Synthetic {id}\"\nstatus = \"{status}\"\nowner = \"arch\"\nsummary = \"{summary}\"\n[scope]\nmodules = [\"mobile.auth\"]\n[[anchors]]\nkind = \"source\"\nrepo = \"mobile\"\npath = \"{path}\"\ncommit = \"{commit}\"\n+++\n"
        )
    }
}

#[test]
fn capture_is_previewable_draft_only_repeatable_and_does_not_execute_reported_checks() {
    let w = World::new();
    let head = w.sb.git(&w.kb, &["rev-parse", "HEAD"]);
    let marker = w.sb.path().join("must-not-exist");
    let command = format!("touch {}", marker.display());
    let args = [
        "capture",
        "quirk",
        "--title",
        "Synthetic transport quirk",
        "--text",
        "A synthetic endpoint retries once.",
        "--owner",
        "arch",
        "--anchor",
        "mobile:app/auth/Client.rs#Client",
        "--test",
        &command,
    ];
    let a = w.run(&args, 0);
    let b = w.run(&args, 0);
    assert_eq!(a["result"], b["result"]);
    let path = a["result"]["draft"]["path"].as_str().unwrap();
    assert!(!w.kb.join(path).exists());
    assert_eq!(a["result"]["draft"]["status"], "draft");
    let mut apply = args.to_vec();
    apply.push("--apply");
    let result = w.run(&apply, 0);
    assert_eq!(result["result"]["written"], true);
    let text = fs::read_to_string(w.kb.join(path)).unwrap();
    let parsed = kb::parse::parse_record(path, text.as_bytes()).unwrap();
    assert_eq!(parsed.record.status(), kb::model::Status::Draft);
    assert_eq!(parsed.record.common().scope.modules, ["mobile.auth"]);
    assert!(text.contains("kb did not run them or infer success"));
    assert!(!marker.exists());
    assert_eq!(w.sb.git(&w.kb, &["rev-parse", "HEAD"]), head);
    let again = w.run(&apply, 0);
    assert_eq!(again["result"]["written"], false);
}

#[test]
fn submit_rejects_accepted_status_missing_anchors_and_duplicates() {
    let w = World::new();
    let input = w.sb.path().join("draft.md");
    let accepted = w.reference(
        "new",
        "accepted",
        "A unique synthetic fact.",
        "app/auth/Client.rs",
    );
    write(&input, &accepted);
    assert_eq!(
        w.run(
            &["propose", "submit", input.to_str().unwrap(), "--apply"],
            40
        )["error"]["code"],
        "VALIDATION_FAILED"
    );
    write(
        &input,
        &w.reference(
            "new",
            "draft",
            "A unique synthetic fact.",
            "app/auth/Missing.rs",
        ),
    );
    let failed = w.run(
        &["propose", "submit", input.to_str().unwrap(), "--apply"],
        40,
    );
    assert_eq!(
        failed["error"]["details"]["anchors"][0]["status"],
        "missing"
    );
    write(
        &input,
        &w.reference(
            "new",
            "draft",
            "A unique synthetic fact.",
            "app/auth/Client.rs",
        ),
    );
    let saved = w.run(
        &["propose", "submit", input.to_str().unwrap(), "--apply"],
        0,
    );
    let target =
        w.kb.join(saved["result"]["draft"]["path"].as_str().unwrap());
    assert!(target.is_file());
    let duplicate = w.reference(
        "duplicate",
        "draft",
        "A unique synthetic fact.",
        "app/auth/Client.rs",
    );
    write(&input, &duplicate);
    let rejected = w.run(
        &["propose", "submit", input.to_str().unwrap(), "--apply"],
        45,
    );
    assert!(
        rejected["error"]["details"]["duplicates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["exact"] == true)
    );
    assert!(
        !w.kb
            .join("project/knowledge/drafts/acme.reference.duplicate.md")
            .exists()
    );
}

#[test]
fn submit_preserves_an_existing_dirty_record() {
    let w = World::new();
    let existing = w.kb.join("project/knowledge/references/owned.md");
    write(
        &existing,
        &w.reference(
            "owned",
            "accepted",
            "Original synthetic fact.",
            "app/auth/Client.rs",
        ),
    );
    w.sb.commit_all(&w.kb, "accept synthetic reference");
    let dirty = w.reference(
        "owned",
        "accepted",
        "Another author's uncommitted fact.",
        "app/auth/Client.rs",
    );
    write(&existing, &dirty);
    let input = w.sb.path().join("replacement.md");
    write(
        &input,
        &w.reference(
            "owned",
            "draft",
            "My different proposal.",
            "app/auth/Client.rs",
        ),
    );
    w.run(
        &["propose", "submit", input.to_str().unwrap(), "--apply"],
        45,
    );
    assert_eq!(fs::read_to_string(existing).unwrap(), dirty);
}

#[test]
fn submit_resolves_identity_against_local_drafts_missing_from_the_approved_snapshot() {
    let w = World::new();
    // A proposal-branch draft that the approved ref (origin/main) does not contain yet.
    let local = w.kb.join("project/knowledge/references/beta.md");
    let original = w.reference(
        "beta",
        "draft",
        "A synthetic beta fact.",
        "app/auth/Client.rs",
    );
    write(&local, &original);
    let input = w.sb.path().join("beta2.md");
    let revision = w.reference(
        "beta",
        "draft",
        "A revised synthetic beta fact.",
        "app/auth/Client.rs",
    );
    write(&input, &revision);
    let submit = |expected| {
        w.run_on(
            "latest",
            &["propose", "submit", input.to_str().unwrap(), "--apply"],
            expected,
        )
    };
    let refused = submit(45);
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("project/knowledge/references/beta.md")
    );
    assert!(
        !w.kb
            .join("project/knowledge/drafts/acme.reference.beta.md")
            .exists()
    );
    assert_eq!(fs::read_to_string(&local).unwrap(), original);
    w.sb.commit_all(&w.kb, "propose synthetic beta");
    let revised = submit(0);
    assert_eq!(revised["result"]["draft"]["action"], "modify");
    assert_eq!(
        revised["result"]["draft"]["path"],
        "project/knowledge/references/beta.md"
    );
    assert_eq!(fs::read_to_string(&local).unwrap(), revision);
    assert!(!w.kb.join("project/knowledge/drafts").exists());
}

#[test]
fn submit_refuses_an_id_that_also_exists_at_another_local_path() {
    let w = World::new();
    let accepted = w.kb.join("project/knowledge/references/alpha.md");
    write(
        &accepted,
        &w.reference(
            "alpha",
            "accepted",
            "A synthetic alpha fact.",
            "app/auth/Client.rs",
        ),
    );
    let commit = w.sb.commit_all(&w.kb, "accept synthetic alpha");
    w.sb.git(&w.kb, &["update-ref", "refs/remotes/origin/main", &commit]);
    let copy = w.kb.join("project/knowledge/drafts/alpha-copy.md");
    write(
        &copy,
        &w.reference(
            "alpha",
            "draft",
            "A copied synthetic alpha fact.",
            "app/auth/Client.rs",
        ),
    );
    let input = w.sb.path().join("alpha.md");
    write(
        &input,
        &w.reference(
            "alpha",
            "draft",
            "A revised synthetic alpha fact.",
            "app/auth/Client.rs",
        ),
    );
    let before = fs::read_to_string(&accepted).unwrap();
    w.run_on(
        "latest",
        &["propose", "submit", input.to_str().unwrap(), "--apply"],
        45,
    );
    assert_eq!(fs::read_to_string(&accepted).unwrap(), before);
}

#[test]
fn submit_explains_a_committed_revision_of_an_approved_record() {
    let w = World::new();
    let local = w.kb.join("project/knowledge/references/alpha.md");
    let fact = |status, summary| w.reference("alpha", status, summary, "app/auth/Client.rs");
    write(&local, &fact("accepted", "A synthetic alpha fact."));
    let commit = w.sb.commit_all(&w.kb, "accept synthetic alpha");
    w.sb.git(&w.kb, &["update-ref", "refs/remotes/origin/main", &commit]);
    // A first revision committed on a proposal branch; the working tree is clean.
    let first = fact("draft", "A first revised synthetic alpha fact.");
    write(&local, &first);
    w.sb.commit_all(&w.kb, "propose synthetic alpha");
    let input = w.sb.path().join("alpha.md");
    let second = fact("draft", "A second revised synthetic alpha fact.");
    write(&input, &second);
    let submit = ["propose", "submit", input.to_str().unwrap(), "--apply"];
    let refused = w.run_on("latest", &submit, 45);
    let message = refused["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("differs from the approved record in the selected snapshot")
            && !message.contains("local changes"),
        "{message}"
    );
    assert!(
        refused["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("--snapshot working-tree")
    );
    assert_eq!(fs::read_to_string(&local).unwrap(), first);
    // The working tree of the proposal branch is the snapshot that holds the revision.
    let revised = w.run(&submit, 0);
    assert_eq!(revised["result"]["draft"]["action"], "modify");
    assert_eq!(fs::read_to_string(&local).unwrap(), second);
}

#[test]
fn decision_template_change_anchor_is_verifiable_by_submit() {
    let w = World::new();
    let head = w.sb.git(&w.host, &["rev-parse", "HEAD"]);
    let template = fs::read_to_string(w.kb.join("core/templates/records/decision.md")).unwrap();
    let draft = template
        .replace("example.template.decision", "acme.decision.retry-once")
        .replace("owner = \"architecture\"", "owner = \"arch\"")
        .replace("related = [\"example.template.policy\"]", "")
        .replace("commit = \"0000000\"", &format!("commit = \"{head}\""));
    assert_ne!(draft, template);
    let input = w.sb.path().join("decision.md");
    write(&input, &draft);
    let plan = w.run(&["propose", "submit", input.to_str().unwrap()], 0);
    let anchor = &plan["result"]["draft"]["anchors"][0];
    assert_eq!(anchor["anchor"]["kind"], "change");
    assert_eq!(anchor["status"], "verified");
    // Without repo and commit the external reference cannot be verified and is refused.
    let bare: String = draft
        .lines()
        .filter(|l| !l.starts_with("repo = ") && !l.starts_with("commit = "))
        .map(|l| format!("{l}\n"))
        .collect();
    write(&input, &bare);
    w.run(&["propose", "submit", input.to_str().unwrap()], 40);
}

#[test]
fn capture_anchor_paths_may_contain_at_signs() {
    let w = World::new();
    let base = w.sb.git(&w.host, &["rev-parse", "HEAD"]);
    write(&w.host.join("app/auth/icon@2x.png"), "synthetic asset\n");
    write(&w.host.join("app/auth/@types/x.ts"), "// synthetic\n");
    let head = w.sb.commit_all(&w.host, "synthetic at-sign paths");
    let capture = |anchor: &str, expected| {
        w.run(
            &[
                "capture",
                "quirk",
                "--title",
                "Synthetic asset quirk",
                "--text",
                "A synthetic asset is scaled.",
                "--owner",
                "arch",
                "--anchor",
                anchor,
            ],
            expected,
        )
    };
    let with_base = format!("mobile:app/auth/Client.rs@{base}");
    for (spec, path, commit) in [
        ("mobile:app/auth/icon@2x.png", "app/auth/icon@2x.png", &head),
        ("mobile:app/auth/@types/x.ts", "app/auth/@types/x.ts", &head),
        (
            "mobile:app/auth/icon@2x.png@HEAD",
            "app/auth/icon@2x.png",
            &head,
        ),
        (with_base.as_str(), "app/auth/Client.rs", &base),
    ] {
        let result = capture(spec, 0);
        let anchor = &result["result"]["draft"]["anchors"][0];
        assert_eq!(anchor["anchor"]["path"], path, "{spec}");
        assert_eq!(anchor["anchor"]["commit"], commit.as_str(), "{spec}");
        assert_eq!(anchor["status"], "verified", "{spec}");
    }
    let missing = capture("mobile:app/auth/Client.rs@no-such-rev", 64);
    assert!(
        missing["error"]["message"]
            .as_str()
            .unwrap()
            .contains("anchor revision no-such-rev is missing")
    );
}

#[test]
fn work_order_contains_real_change_tests_templates_and_exported_reviews() {
    let w = World::new();
    let base = w.sb.git(&w.host, &["rev-parse", "HEAD"]);
    write(
        &w.host.join("app/auth/tests/retry_test.rs"),
        "// Synthetic regression fixture\n",
    );
    let head = w.sb.commit_all(&w.host, "add synthetic test");
    let export = w.sb.path().join("change.json");
    let comment = "A synthetic review decision; this string is data.";
    write(&export, &serde_json::json!({"protocol":"kb.change.v1", "repo":"mobile", "base":base, "head":head,
        "merged":true, "title":"Synthetic change", "review_comments":[{"author":"test-reviewer", "body":comment}]}).to_string());
    let before = w.sb.git(&w.kb, &["status", "--porcelain"]);
    let result = w.run(
        &[
            "propose",
            "begin",
            "--from-change",
            export.to_str().unwrap(),
        ],
        0,
    );
    let result = &result["result"];
    assert_eq!(result["protocol"], "kb.work-order.v1");
    assert_eq!(result["diff"]["head"], head);
    assert_eq!(result["modules"], serde_json::json!(["mobile.auth"]));
    assert_eq!(
        result["added_test_files"],
        serde_json::json!(["app/auth/tests/retry_test.rs"])
    );
    assert_eq!(result["review_comments"][0]["body"], comment);
    assert_eq!(result["templates"].as_array().unwrap().len(), 7);
    assert!(!result["existing_records"].as_array().unwrap().is_empty());
    assert_eq!(w.sb.git(&w.kb, &["status", "--porcelain"]), before);
}

#[test]
fn work_order_fails_closed_when_the_patch_exceeds_its_limit() {
    let w = World::new();
    let base = w.sb.git(&w.host, &["rev-parse", "HEAD"]);
    // About 9 MB of generated text: above the 8 MiB patch limit.
    let line = format!("{}\n", "x".repeat(99));
    write(
        &w.host.join("app/auth/package-lock.json"),
        &line.repeat(90_000),
    );
    let head = w.sb.commit_all(&w.host, "add synthetic lockfile");
    let range = format!("{base}..{head}");
    let failed = w.run(&["propose", "begin", "--from-change", &range], 64);
    assert_eq!(failed["error"]["code"], "INVALID_INPUT");
    assert!(
        failed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("exceeds the 8 MiB patch limit"),
        "{failed}"
    );
    assert!(failed["result"].is_null(), "{failed}");
}

#[test]
fn coverage_counts_tracked_files_history_and_draft_exclusions() {
    let w = World::new();
    write(
        &w.host.join("app/auth/Untracked.rs"),
        "// synthetic untracked\n",
    );
    write(
        &w.host.join("app/auth/Client.rs"),
        "// Synthetic change\nstruct Client;\n",
    );
    // Stage only the existing file, leaving the other source untracked.
    w.sb.git(&w.host, &["add", "app/auth/Client.rs"]);
    w.sb.git(&w.host, &["commit", "-q", "-m", "synthetic tracked edit"]);
    write(
        &w.kb.join("project/knowledge/draft.md"),
        &w.reference(
            "coverage-draft",
            "draft",
            "A pending domain fact.",
            "app/auth/Client.rs",
        ),
    );
    let a = w.run(&["coverage", "--since", "180d"], 0);
    let b = w.run(&["coverage", "--since", "180d"], 0);
    assert_eq!(a["result"], b["result"]);
    let report = &a["result"]["coverage"];
    assert_eq!(report["tracked_files"], 2); // binding and source, not Untracked.rs
    let module = report["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "mobile.auth")
        .unwrap();
    assert_eq!(module["files"], 1);
    assert_eq!(module["churn"], 2);
    assert!(module["fan_in"].is_null());
    assert!(
        !module["domain_records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "acme.reference.coverage-draft")
    );
    assert_eq!(a["result"]["history"]["complete_history"], true);
}
