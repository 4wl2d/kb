//! Synthetic Tier A routing/history evidence; no agent jobs or network access.
mod common;
use common::{Sandbox, json, write};
use serde_json::{Value, json as value};
use std::fs;
use std::path::PathBuf;

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
        sb.commit_all(&kb, "synthetic knowledge");
        sb.init_repo(&host);
        write(&host.join(".kbw.toml"), "schema = 1\nrepo = \"mobile\"\n");
        write(&host.join("app/auth/Store.rs"), "// Synthetic old state\n");
        let base = sb.commit_all(&host, "synthetic base");
        Self { sb, kb, host, base }
    }
    fn run(&self, extra: &[&str], expected: i32) -> Value {
        let mut args = vec![
            "--root",
            self.kb.to_str().unwrap(),
            "--host",
            self.host.to_str().unwrap(),
            "--snapshot",
            "working-tree",
            "--offline",
            "--json",
            "eval",
        ];
        args.extend(extra);
        let out = self.sb.kb(&self.host, &args, &[]);
        assert_eq!(
            out.status.code(),
            Some(expected),
            "{}\n{}",
            common::stdout(&out),
            common::stderr(&out)
        );
        json(&out)["result"].clone()
    }
    fn case(&self, extra: &str) {
        write(
            &self.kb.join("project/routing-tests/metrics.toml"),
            &format!(
                "schema = 1\n[[case]]\nname = \"synthetic ranking\"\nintent = \"implement\"\nrepos = [\"mobile\"]\npaths = [\"mobile:app/auth/Store.rs\"]\nexpect_mandatory = [\"acme.mobile.token-storage\"]\n{extra}"
            ),
        );
    }
    fn change(&self, message: &str) -> String {
        write(
            &self.host.join("app/auth/Store.rs"),
            &format!("// Synthetic {message}\n"),
        );
        self.sb.commit_all(&self.host, message)
    }
}

#[test]
fn routing_order_recall_and_size_are_measured_from_the_delivered_response() {
    let w = World::new();
    w.case("relevant = [\"acme.mobile.token-storage\", \"acme.contract.token-api\"]\nrecall_k = 2\nmin_recall_percent = 100\n");
    let baseline = w.run(&["routing"], 0);
    let case = &baseline["routing"]["cases"][0];
    assert_eq!(case["recall_at_k"]["fraction"], 1.0);
    assert!(case["mandatory_precision"].is_null());
    let order: Vec<String> = serde_json::from_value(case["delivery_order"].clone()).unwrap();
    assert_eq!(order.len(), 2);
    let tokens = case["tokens_est"].as_u64().unwrap();
    let mut reversed = order.clone();
    reversed.reverse();
    w.case(&format!("expect_order = {}\nmax_tokens = {}\nrelevant = [\"{}\"]\nrecall_k = 1\nmin_recall_percent = 100\n",serde_json::to_string(&reversed).unwrap(),tokens - 1,order[1]));
    let failed = w.run(&["routing"], 41);
    assert_eq!(
        failed["routing"]["cases"][0]["failures"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    w.case(&format!(
        "expect_order = {}\nmax_tokens = {tokens}\napplicability_reviewed = true\n",
        serde_json::to_string(&order).unwrap()
    ));
    let pass = w.run(&["routing"], 0);
    assert_eq!(
        pass["routing"]["cases"][0]["mandatory_precision"]["fraction"],
        1.0
    );
}

#[test]
fn invalid_metric_labels_fail_validation_instead_of_producing_a_score() {
    let w = World::new();
    for extra in [
        "min_recall_percent = 100\n",
        "recall_k = 0\n",
        "expect_order = [\"x\",\"x\"]\n",
        "not_applicable = [\"acme.mobile.token-storage\"]\n",
    ] {
        w.case(extra);
        let result = w.run(&["routing"], 40);
        assert!(
            result["validation"]["diagnostics"]
                .to_string()
                .contains("ROUTING_TEST_INVALID")
        );
    }
}

#[test]
fn routing_can_compare_renderers_without_changing_the_report_format_or_fixture_scope() {
    let w = World::new();
    w.case("");
    let compact = w.run(&["routing"], 0);
    let terse = w.run(&["routing", "--context-format", "terse"], 0);
    assert_eq!(compact["routing"]["context_format"], "compact");
    assert_eq!(terse["routing"]["context_format"], "terse");
    for key in ["mandatory", "included", "delivery_order", "status"] {
        assert_eq!(
            compact["routing"]["cases"][0][key],
            terse["routing"]["cases"][0][key]
        );
    }
    assert!(
        terse["routing"]["metrics"]["tokens_est"]["total"]
            .as_u64()
            .unwrap()
            < compact["routing"]["metrics"]["tokens_est"]["total"]
                .as_u64()
                .unwrap()
    );
}

#[test]
fn history_uses_commit_and_exported_mr_labels_and_leaves_unlabeled_precision_unknown() {
    let w = World::new();
    let first = w.change("synthetic unlabelled change");
    let second = w.change("synthetic second change");
    let labels = w.sb.path().join("labels.json");
    write(&labels,&value!({"protocol":"kb.history-labels.v1","changes":[{"commit":second,"description":"<!-- kb-impact:v1\nkb_change = \"none\"\nreason = \"Synthetic human adjudication\"\napplicable = [\"acme.contract.token-api\"]\nnot_applicable = [\"acme.mobile.token-storage\"]\napplicability_reviewed = true\n-->"}]}).to_string());
    let range = format!("{}..{}", w.base, second);
    let result = w.run(
        &[
            "history",
            "--range",
            &range,
            "--labels",
            labels.to_str().unwrap(),
            "--check",
        ],
        0,
    );
    let rows = result["history"]["changes"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["commit"], first);
    assert!(rows[0]["context"]["mandatory_precision"].is_null());
    assert_eq!(rows[1]["context"]["mandatory_precision"]["fraction"], 0.5);
    assert_eq!(rows[1]["context"]["labeled_recall"]["fraction"], 1.0);
    assert_eq!(rows[1]["drift"]["unverifiable_records"], 2);
    let repeated = w.run(
        &[
            "history",
            "--range",
            &range,
            "--labels",
            labels.to_str().unwrap(),
            "--check",
        ],
        0,
    );
    assert_eq!(result, repeated);
}

#[test]
fn historical_filter_withholds_undated_and_future_records() {
    let w = World::new();
    let record = w.kb.join("project/knowledge/policies/token-storage.md");
    let old = fs::read_to_string(&record).unwrap();
    write(
        &record,
        &old.replace("schema = 1", "schema = 2\nintroduced = \"2099-01-01\""),
    );
    let head = w.change("synthetic future-independent scope");
    let range = format!("{}..{head}", w.base);
    let result = w.run(&["history", "--range", &range, "--as-of", "--check"], 30);
    let row = &result["history"]["changes"][0];
    assert_eq!(row["context"]["included"], value!([]));
    assert_eq!(result["history"]["mode"], "parent-validity-filter");
}

#[test]
fn history_never_silently_samples_or_ignores_invalid_inputs() {
    let w = World::new();
    w.change("synthetic first");
    let head = w.change("synthetic second");
    let range = format!("{}..{head}", w.base);
    let args = [
        "--root",
        w.kb.to_str().unwrap(),
        "--host",
        w.host.to_str().unwrap(),
        "--json",
        "eval",
        "history",
        "--range",
        &range,
        "--max-changes",
        "1",
    ];
    let out = w.sb.kb(&w.host, &args, &[]);
    assert!(!out.status.success());
    assert!(json(&out).to_string().contains("range exceeds max-changes"));
    let bad = w.change("<!-- kb-impact:v1\nkb_change = \"none\"\nreason = \"\"\n-->");
    let range = format!("{head}..{bad}");
    let args = [
        "--root",
        w.kb.to_str().unwrap(),
        "--host",
        w.host.to_str().unwrap(),
        "--snapshot",
        "working-tree",
        "--offline",
        "--json",
        "eval",
        "history",
        "--range",
        &range,
    ];
    let out = w.sb.kb(&w.host, &args, &[]);
    assert!(!out.status.success());
    assert!(json(&out).to_string().contains("reason"));
}
