//! Declarative probes through the CLI, using only synthetic local Git history.
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
    fn new(committed: bool) -> Self {
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
            &host.join("app/auth/Client.kt"),
            "// Synthetic fixture\nlegacyCall()\n",
        );
        write(
            &host.join("app/private/Secret.kt"),
            "// Synthetic private declaration\n",
        );
        let base = if committed {
            sb.commit_all(&host, "feat: synthetic baseline")
        } else {
            "empty-tree".into()
        };
        Self { sb, kb, host, base }
    }

    fn policy(&self, rules: &str) {
        write(
            &self.kb.join("project/knowledge/policies/probes.md"),
            &format!(
                "+++\nschema = 2\nid = \"acme.policy.probes\"\nkind = \"policy\"\ntitle = \"Synthetic verification\"\nstatus = \"accepted\"\nowner = \"arch\"\n[scope]\nproduct = true\n{rules}\n+++\n"
            ),
        );
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
            "verify",
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

    fn graph(&self, confidence: Option<&str>, complete: bool) -> PathBuf {
        let head = self.sb.git(&self.host, &["rev-parse", "HEAD"]);
        let symbols:Vec<_>=[("client","app/auth/Client.kt"),("secret","app/private/Secret.kt")].iter().map(|(id,path)|{
            let bytes=self.sb.git_output(&self.host,&["show",&format!("{head}:{path}")]).stdout;
            value!({"id":id,"name":id,"kind":"file","path":path,"start_line":1,"end_line":bytes.split_inclusive(|b|*b==b'\n').count().max(1),"extent":"file","sha256":kb::util::sha256_hex(&bytes)})
        }).collect();
        let refs:Vec<_>=confidence.map(|confidence|value!({"from":"client","to":"secret","line":2,"kind":"import","confidence":confidence})).into_iter().collect();
        let path = self.sb.path().join("provider.json");
        fs::write(&path,serde_json::to_vec(&value!({"protocol":"kb.code.v1","repo":"mobile","commit":head,"tool":{"name":"synthetic","version":"1"},"capabilities":["refs"],"complete":complete,"symbols":symbols,"refs":refs})).unwrap()).unwrap();
        path
    }
}

const BANNED: &str = r#"
[[rules]]
id = "api"
level = "must-not"
text = "Introduce the synthetic legacy call."
[[rules.verify]]
kind = "banned-api"
paths = ["app/**/*.kt"]
pattern = 'legacyCall\('
"#;

const IMPORT: &str = r#"
[[rules]]
id = "boundary"
level = "must-not"
text = "Import the synthetic private module from auth."
[[rules.verify]]
kind = "forbidden-import"
from = ["app/auth/**"]
to = ["app/private/**"]
"#;

#[test]
fn banned_api_checks_added_lines_and_staged_bytes_not_unstaged_edits() {
    let w = World::new(true);
    w.policy(BANNED);
    let file = w.host.join("app/auth/Client.kt");
    write(&file, "// Synthetic fixture\nlegacyCall()\nsafeCall()\n");
    let pass = w.run(&[], 0);
    assert_eq!(pass["result"]["verification"]["counts"]["passed"], 1);
    write(&file, "// Synthetic fixture\nlegacyCall()\nlegacyCall()\n");
    w.sb.git(&w.host, &["add", "app/auth/Client.kt"]);
    write(&file, "// Synthetic fixture\nlegacyCall()\nsafeCall()\n");
    w.run(&[], 0);
    let failed = w.run(&["--staged"], 40);
    let probe = &failed["result"]["verification"]["probes"][0];
    assert_eq!(probe["state"], "failed");
    assert_eq!(probe["evidence"][0]["line"], 3);
    assert_eq!(failed["result"]["verification"]["mode"], "staged");
}

#[test]
fn naming_and_branch_probes_are_positive_constraints() {
    let w = World::new(true);
    w.policy(
        r#"
[[rules]]
id = "branch"
level = "must"
text = "Use a synthetic feature branch."
[[rules.verify]]
kind = "branch-name"
pattern = '^feature/'
[[rules]]
id = "naming"
level = "should"
text = "Use synthetic capitalized Kotlin filenames."
[[rules.verify]]
kind = "naming"
paths = ["app/**"]
pattern = '(^|/)[A-Z][A-Za-z0-9]*\.kt$'
"#,
    );
    write(
        &w.host.join("app/auth/bad_name.kt"),
        "// Synthetic fixture\n",
    );
    let failed = w.run(&[], 40);
    assert_eq!(failed["result"]["verification"]["blocking_failures"], 1);
    w.sb.git(&w.host, &["checkout", "-qb", "feature/synthetic"]);
    let advisory = w.run(&[], 0);
    assert_eq!(advisory["result"]["verification"]["counts"]["failed"], 1);
    w.run(&["--strict"], 40);
}

#[test]
fn commit_message_text_is_never_executed_and_first_commit_is_supported() {
    let w = World::new(false);
    w.policy(
        r#"
[[rules]]
id = "message"
level = "must"
text = "Use the synthetic commit prefix."
[[rules.verify]]
kind = "commit-message"
pattern = '^feat:'
"#,
    );
    w.sb.git(&w.host, &["add", "--all"]);
    let message = w.sb.path().join("message.txt");
    let marker = w.sb.path().join("must-not-exist");
    write(&message, &format!("feat: $(touch {})\n", marker.display()));
    let pass = w.run(
        &[
            "--staged",
            "--only",
            "commit-message",
            "--commit-message",
            message.to_str().unwrap(),
        ],
        0,
    );
    assert_eq!(pass["result"]["verification"]["diff"]["base"], "empty-tree");
    assert_eq!(pass["result"]["verification"]["messages_checked"], 1);
    assert!(!marker.exists());
    write(&message, "wrong prefix\n");
    w.run(
        &[
            "--staged",
            "--only",
            "commit-message",
            "--commit-message",
            message.to_str().unwrap(),
        ],
        40,
    );
}

#[test]
fn committed_message_range_and_detached_target_have_explicit_evidence() {
    let w = World::new(true);
    w.policy(
        r#"
[[rules]]
id = "message"
level = "must"
text = "Use the synthetic commit prefix."
[[rules.verify]]
kind = "commit-message"
pattern = '^feat:'
[[rules]]
id = "branch"
level = "must"
text = "Use the synthetic feature branch."
[[rules.verify]]
kind = "branch-name"
pattern = '^feature/'
"#,
    );
    write(
        &w.host.join("app/auth/Client.kt"),
        "// Synthetic changed fixture\n",
    );
    let head = w.sb.commit_all(&w.host, "bad message");
    w.sb.git(&w.host, &["checkout", "--detach", &head]);
    let failed = w.run(&["--diff", &w.base, "--head", &head], 40);
    assert_eq!(failed["result"]["verification"]["blocking_failures"], 1);
    assert_eq!(failed["result"]["verification"]["blocking_unverifiable"], 1);
    assert!(
        failed["result"]["verification"]["probes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["commit"] == head))
    );
}

#[test]
fn incomplete_or_possible_import_graphs_never_prove_compliance() {
    let w = World::new(true);
    w.policy(IMPORT);
    write(
        &w.host.join("app/auth/Client.kt"),
        "// Synthetic fixture\nimport private.Secret\n",
    );
    w.sb.commit_all(&w.host, "feat: synthetic import");
    w.run(&["--diff", &w.base, "--head", "HEAD"], 30);
    let path = w.graph(Some("possible"), true);
    w.run(
        &[
            "--diff",
            &w.base,
            "--head",
            "HEAD",
            "--provider-file",
            path.to_str().unwrap(),
        ],
        30,
    );
    w.graph(Some("resolved"), false);
    let violation = w.run(
        &[
            "--diff",
            &w.base,
            "--head",
            "HEAD",
            "--provider-file",
            path.to_str().unwrap(),
        ],
        40,
    );
    assert_eq!(
        violation["result"]["verification"]["probes"][0]["evidence"][0]["confidence"],
        "resolved"
    );
    w.graph(None, false);
    w.run(
        &[
            "--diff",
            &w.base,
            "--head",
            "HEAD",
            "--provider-file",
            path.to_str().unwrap(),
        ],
        30,
    );
    write(
        &w.host.join("app/auth/Client.kt"),
        "// Synthetic dependency removed\nclass Client {}\n",
    );
    w.sb.commit_all(&w.host, "feat: remove synthetic import");
    w.graph(None, true);
    w.run(
        &[
            "--diff",
            &w.base,
            "--head",
            "HEAD",
            "--provider-file",
            path.to_str().unwrap(),
        ],
        0,
    );
}

#[test]
fn bounded_import_report_retains_a_proven_violation_after_many_possible_edges() {
    let w = World::new(true);
    w.policy(IMPORT);
    write(
        &w.host.join("app/auth/Client.kt"),
        &format!(
            "// Synthetic stress fixture\n{}",
            "import private.Secret\n".repeat(101)
        ),
    );
    w.sb.commit_all(&w.host, "feat: synthetic import evidence limit");
    let path = w.graph(None, false);
    let mut graph: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    graph["refs"]=Value::Array((0..101).map(|i|value!({"from":"client","to":"secret","line":i+2,"kind":"import","confidence":if i==100{"resolved"}else{"possible"}})).collect());
    fs::write(&path, serde_json::to_vec(&graph).unwrap()).unwrap();
    let result = w.run(
        &[
            "--diff",
            &w.base,
            "--head",
            "HEAD",
            "--provider-file",
            path.to_str().unwrap(),
        ],
        40,
    );
    let probe = &result["result"]["verification"]["probes"][0];
    assert_eq!(probe["matches"], 101);
    assert_eq!(probe["evidence_truncated"], true);
    assert_eq!(probe["evidence"].as_array().unwrap().len(), 100);
    assert!(
        probe["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["confidence"] == "resolved" && e["line"] == 102)
    );
}

#[test]
fn conditions_and_exceptions_need_explicit_applicability_before_enforcement() {
    let w = World::new(true);
    w.policy(&BANNED.replace("[[rules.verify]]","conditions = [\"When the synthetic feature is enabled.\"]\nexceptions = [{id=\"fixture\",text=\"Only disposable test data.\"}]\n[[rules.verify]]"));
    write(&w.host.join("app/auth/New.kt"), "legacyCall()\n");
    w.run(&[], 30);
    let failed = w.run(&["--applicable", "acme.policy.probes#api"], 40);
    assert_eq!(
        failed["result"]["verification"]["explicit_applicability"],
        value!(["acme.policy.probes#api"])
    );
}

#[test]
fn invalid_knowledge_cannot_turn_into_a_successful_empty_probe_set() {
    let w = World::new(true);
    w.policy(&BANNED.replace("pattern = 'legacyCall\\('", "command = 'touch dangerous'"));
    let failed = w.run(&[], 40);
    assert_eq!(failed["error"]["code"], "VALIDATION_FAILED");
}

#[test]
fn probe_glob_of_an_unregistered_repo_fails_validation_instead_of_skipping() {
    let w = World::new(true);
    write(&w.host.join("app/auth/New.kt"), "legacyCall()\n");
    w.policy(&BANNED.replace("\"app/**/*.kt\"", "\"mobile:app/**/*.kt\""));
    w.run(&[], 40);
    // A one-character typo in the repo qualifier must not disarm the must-not guard.
    w.policy(&BANNED.replace("\"app/**/*.kt\"", "\"mobil:app/**/*.kt\""));
    let failed = w.run(&[], 40);
    assert_eq!(failed["error"]["code"], "VALIDATION_FAILED", "{failed}");
    assert!(
        failed["error"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "UNKNOWN_REPO"
                && d["message"] == "verify: `mobil:app/**/*.kt` names unknown repo `mobil`"),
        "{failed}"
    );
}

#[cfg(unix)]
#[test]
fn shipped_commit_msg_hook_preserves_native_verification_exit_codes() {
    let w = World::new(false);
    w.policy(
        r#"
[[rules]]
id = "message"
level = "must"
text = "Use the synthetic prefix."
[[rules.verify]]
kind = "commit-message"
pattern = '^feat:'
"#,
    );
    fs::create_dir_all(w.host.join(".kb")).unwrap();
    // Exercise the shipped hook's argv/exit behavior with the actual engine binary at
    // its entry path. The production launcher has separate e2e/launcher tests.
    fs::hard_link(common::kb_bin(), w.host.join(".kb/kbw")).unwrap();
    write(&w.host.join(".gitignore"), ".kb/\n");
    w.sb.git(&w.host, &["add", "--all"]);
    let message = w.sb.path().join("pending message.txt");
    let invoke = || {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg(common::repo_root().join("core/templates/ci/hooks/commit-msg"))
            .arg(&message)
            .current_dir(&w.host);
        common::isolated_env(&mut cmd, &w.sb.home());
        cmd.env("KB_ROOT", &w.kb).env("KB_SNAPSHOT", "working-tree");
        cmd.output().unwrap()
    };
    write(&message, "feat: synthetic hook check\n");
    let ok = invoke();
    assert_eq!(
        ok.status.code(),
        Some(0),
        "{}\n{}",
        common::stdout(&ok),
        common::stderr(&ok)
    );
    write(&message, "invalid synthetic prefix\n");
    assert_eq!(invoke().status.code(), Some(40));
}
