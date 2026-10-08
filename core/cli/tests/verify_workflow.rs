//! Declarative probes through the CLI, using only synthetic local Git history.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{Sandbox, json, write};
use serde_json::{Value, json as value};

const PROBES: &str = "project/knowledge/policies/probes.md";

fn policy_text(scope: &str, rules: &str) -> String {
    format!(
        "+++\nschema = 2\nid = \"acme.policy.probes\"\nkind = \"policy\"\ntitle = \"Synthetic verification\"\nstatus = \"accepted\"\nowner = \"arch\"\n[scope]\n{scope}\n{rules}\n+++\n"
    )
}

/// Point `core.hooksPath` at the shipped template directory: Git itself then decides
/// whether the template is executable and which environment it receives.
fn install_hooks_path(sb: &Sandbox, host: &Path) {
    let hooks = common::repo_root().join("core/templates/ci/hooks");
    sb.git(host, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
}

/// Run Git in `dir` with an isolated configuration plus `envs` (inherited by hooks).
fn git_env(sb: &Sandbox, dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(args);
    common::isolated_env(&mut cmd, &sb.home());
    cmd.envs(envs.iter().copied());
    cmd.output().unwrap()
}

fn assert_rejected(out: &Output, code: &str) {
    assert!(!out.status.success(), "{}", common::stderr(out));
    assert!(
        common::stderr(out).contains(code),
        "expected {code}:\n{}",
        common::stderr(out)
    );
}

fn assert_committed(out: &Output) {
    assert!(
        out.status.success(),
        "{}\n{}",
        common::stdout(out),
        common::stderr(out)
    );
}

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
        self.scoped_policy("product = true", rules);
    }

    fn scoped_policy(&self, scope: &str, rules: &str) {
        write(&self.kb.join(PROBES), &policy_text(scope, rules));
    }

    /// Install the shipped hook the way Git runs it (`core.hooksPath`, no `sh` wrapper), with
    /// the engine at the hook's KB entry path.
    fn install_hook(&self) {
        fs::create_dir_all(self.host.join(".kb")).unwrap();
        // The production launcher has separate e2e/launcher tests.
        fs::hard_link(common::kb_bin(), self.host.join(".kb/kbw")).unwrap();
        write(&self.host.join(".gitignore"), ".kb/\n");
        self.sb.git(&self.host, &["add", ".gitignore"]);
        self.sb
            .git(&self.host, &["commit", "-q", "-m", "feat: ignore the KB"]);
        install_hooks_path(&self.sb, &self.host);
    }

    /// Git with the hook's environment; the KB working tree holds the probes.
    fn hooked(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut all = vec![
            ("KB_ROOT", self.kb.to_str().unwrap()),
            ("KB_SNAPSHOT", "working-tree"),
        ];
        all.extend_from_slice(envs);
        git_env(&self.sb, &self.host, args, &all)
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
fn diffs_with_more_paths_than_the_path_option_limit_are_still_verified() {
    let w = World::new(true);
    w.scoped_policy(
        "modules = [\"mobile.auth\"]",
        &BANNED.replace("app/**/*.kt", "app/auth/Client.kt"),
    );
    for n in 0..520 {
        write(
            &w.host.join(format!("app/auth/generated/G{n}.kt")),
            "// synthetic generated file\n",
        );
    }
    write(
        &w.host.join("app/auth/Client.kt"),
        "// Synthetic fixture\nlegacyCall()\nlegacyCall()\n",
    );
    // Diff paths are bounded by the diff, not by the 512 `--path` values of context.
    let failed = w.run(&[], 40);
    let probe = &failed["result"]["verification"]["probes"][0];
    assert_eq!(probe["state"], "failed", "{failed}");
    assert_eq!(probe["evidence"][0]["line"], 3);
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
    // A typo in the repo qualifier, including its case, must not disarm the must-not guard.
    for repo in ["mobil", "Mobile"] {
        w.policy(&BANNED.replace("\"app/**/*.kt\"", &format!("\"{repo}:app/**/*.kt\"")));
        let failed = w.run(&[], 40);
        assert_eq!(failed["error"]["code"], "VALIDATION_FAILED", "{failed}");
        let message = format!("verify: `{repo}:app/**/*.kt` names unknown repo `{repo}`");
        assert!(
            failed["error"]["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["code"] == "UNKNOWN_REPO" && d["message"] == message.as_str()),
            "{failed}"
        );
    }
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

#[cfg(unix)]
#[test]
fn shipped_commit_msg_hook_is_executable() {
    use std::os::unix::fs::PermissionsExt;
    let hook = common::repo_root().join("core/templates/ci/hooks/commit-msg");
    // Git silently ignores a hook without the executable bit.
    assert_ne!(fs::metadata(&hook).unwrap().permissions().mode() & 0o111, 0);
}

const MESSAGE_PREFIX: &str = r#"
[[rules]]
id = "message"
level = "must"
text = "Use the synthetic prefix."
[[rules.verify]]
kind = "commit-message"
pattern = '^feat:'
"#;

/// An editor that writes `$KB_TEST_SUBJECT` above the text Git prepared, minus `drop`
/// (a sed script).
fn editor(w: &World, name: &str, drop: &str) -> String {
    let path = w.sb.path().join(name);
    write(
        &path,
        &format!(
            "#!/bin/sh\n{{ printf '%s\\n' \"$KB_TEST_SUBJECT\"; sed '{drop}' \"$1\"; }} >\"$1.edit\" && mv \"$1.edit\" \"$1\"\n"
        ),
    );
    format!("sh '{}'", path.display())
}

#[cfg(unix)]
#[test]
fn shipped_hook_checks_the_index_git_commits_for_all_and_pathspec_commits() {
    let w = World::new(true);
    w.scoped_policy("modules = [\"mobile.auth\"]", MESSAGE_PREFIX);
    w.install_hook();
    let head = w.sb.git(&w.host, &["rev-parse", "HEAD"]);
    write(
        &w.host.join("app/auth/Client.kt"),
        "// Synthetic auth edit\n",
    );
    // Nothing is staged in the default index: Git commits an index that only the hook's
    // GIT_INDEX_FILE names, and the module-scoped probe applies to it.
    for args in [
        &["commit", "-q", "-a", "-m", "bad all message"][..],
        &[
            "commit",
            "-q",
            "-m",
            "bad pathspec message",
            "--",
            "app/auth/Client.kt",
        ],
    ] {
        assert_rejected(&w.hooked(args, &[]), "VALIDATION_FAILED");
        assert_eq!(w.sb.git(&w.host, &["rev-parse", "HEAD"]), head);
    }
    assert_committed(&w.hooked(&["commit", "-q", "-a", "-m", "feat: synthetic auth"], &[]));
    let missing = w.run(&["--staged", "--index-file", "missing-index"], 64);
    assert_eq!(missing["error"]["code"], "INVALID_INPUT");
}

#[cfg(unix)]
#[test]
fn shipped_hook_checks_the_message_git_commits_after_cleanup() {
    let w = World::new(true);
    w.policy(
        r#"
[[rules]]
id = "message"
level = "must"
text = "Use a single synthetic subject line."
[[rules.verify]]
kind = "commit-message"
pattern = '^feat: [a-z ]+$'
"#,
    );
    w.install_hook();
    let edit = editor(&w, "editor.sh", "");
    let commit = |args: &[&str], subject: &str| {
        write(
            &w.host.join("app/auth/Client.kt"),
            &format!("// {subject}\n"),
        );
        w.sb.git(&w.host, &["add", "app/auth/Client.kt"]);
        w.hooked(args, &[("GIT_EDITOR", &edit), ("KB_TEST_SUBJECT", subject)])
    };
    // The editor keeps Git's comment template; `-v` adds a scissors line and the diff.
    assert_committed(&commit(&["commit", "-q"], "feat: synthetic edited message"));
    assert_eq!(
        w.sb.git(&w.host, &["log", "-1", "--format=%B"]),
        "feat: synthetic edited message"
    );
    // CI reads the committed message and agrees with the hook.
    w.run(
        &[
            "--diff",
            "HEAD~1",
            "--head",
            "HEAD",
            "--only",
            "commit-message",
        ],
        0,
    );
    assert_committed(&commit(
        &["-c", "core.commentChar=;", "commit", "-q", "-v"],
        "feat: synthetic verbose message",
    ));
    assert_rejected(
        &commit(&["commit", "-q"], "bad subject"),
        "VALIDATION_FAILED",
    );
    // A user's GIT_EDITOR=: skips the editor, yet Git still strips its comment template.
    assert_committed(&w.hooked(&["commit", "-q", "--amend"], &[("GIT_EDITOR", ":")]));
    assert_eq!(
        w.sb.git(&w.host, &["log", "-1", "--format=%B"]),
        "feat: synthetic verbose message"
    );
    // Without an editor Git keeps `#` lines. The hook cannot tell that case from the one
    // above, so it rejects only a message that fails either way; CI rejects this one.
    let message = |subject| ["commit", "-q", "-m", subject, "-m", "#1 body"];
    let rejected = commit(&message("bad subject"), "unused");
    assert_rejected(&rejected, "VALIDATION_FAILED");
    assert_eq!(
        common::stderr(&rejected)
            .matches("VALIDATION_FAILED")
            .count(),
        1,
        "{}",
        common::stderr(&rejected)
    );
    assert_committed(&commit(&message("feat: synthetic subject"), "unused"));
    w.run(
        &[
            "--diff",
            "HEAD~1",
            "--head",
            "HEAD",
            "--only",
            "commit-message",
        ],
        40,
    );
}

#[cfg(unix)]
#[test]
fn shipped_hook_checks_the_rebased_branch_while_head_is_detached() {
    let w = World::new(true);
    w.policy(&format!(
        "{MESSAGE_PREFIX}{}",
        r#"
[[rules]]
id = "branch"
level = "must"
text = "Use the synthetic feature branch."
[[rules.verify]]
kind = "branch-name"
pattern = '^feature/'
"#
    ));
    w.install_hook();
    w.sb.git(&w.host, &["checkout", "-qb", "feature/synthetic"]);
    write(
        &w.host.join("app/auth/Client.kt"),
        "// Synthetic feature edit\n",
    );
    w.sb.git(&w.host, &["add", "app/auth/Client.kt"]);
    assert_committed(&w.hooked(&["commit", "-q", "-m", "feat: synthetic feature"], &[]));
    let todo = w.sb.path().join("todo.sh");
    write(
        &todo,
        "#!/bin/sh\nsed 's/^pick /reword /' \"$1\" >\"$1.edit\" && mv \"$1.edit\" \"$1\"\n",
    );
    let todo = format!("sh '{}'", todo.display());
    let reword = editor(&w, "reword.sh", "1d");
    let rebase = |subject: &str| {
        w.hooked(
            &["rebase", "-q", "-i", "HEAD~1"],
            &[
                ("GIT_SEQUENCE_EDITOR", &todo),
                ("GIT_EDITOR", &reword),
                ("KB_TEST_SUBJECT", subject),
            ],
        )
    };
    // The hook runs for the reword while HEAD is detached.
    assert_rejected(&rebase("bad reword"), "VALIDATION_FAILED");
    w.sb.git(&w.host, &["rebase", "--abort"]);
    assert_committed(&rebase("feat: synthetic reword"));
    assert_eq!(
        w.sb.git(&w.host, &["log", "-1", "--format=%s"]),
        "feat: synthetic reword"
    );
    assert_eq!(
        w.sb.git(&w.host, &["symbolic-ref", "--short", "HEAD"]),
        "feature/synthetic"
    );
}

/// A bare KB remote whose first approved revision asks for the `feat` prefix and whose
/// approved tip asks for `chore`; returns the remote and the first revision.
fn prefix_remote(sb: &Sandbox) -> (String, String) {
    let origin = sb.path().join("kb.git");
    sb.init_bare(&origin);
    let origin = origin.to_str().unwrap().to_string();
    let seed = sb.path().join("seed");
    sb.init_repo(&seed);
    common::write_min_project(&seed);
    write(&seed.join(".gitignore"), ".cache/\n");
    write(
        &seed.join(PROBES),
        &policy_text("product = true", MESSAGE_PREFIX),
    );
    let first = sb.commit_all(&seed, "synthetic KB");
    write(
        &seed.join(PROBES),
        &policy_text("product = true", &MESSAGE_PREFIX.replace("feat", "chore")),
    );
    sb.commit_all(&seed, "synthetic prefix change");
    sb.git(&seed, &["push", "-q", &origin, "main"]);
    (origin, first)
}

/// A host repository without a KB, with the shipped hook installed.
fn prefix_host(sb: &Sandbox) -> PathBuf {
    let host = sb.path().join("host");
    sb.init_repo(&host);
    write(&host.join(".kbw.toml"), "schema = 1\nrepo = \"mobile\"\n");
    write(&host.join("app/auth/Client.kt"), "// Synthetic fixture\n");
    sb.commit_all(&host, "feat: synthetic baseline");
    install_hooks_path(sb, &host);
    host
}

/// Commit a synthetic edit through the hook, which runs the KB checkout at `.kb`.
fn hook_commit(sb: &Sandbox, host: &Path, message: &str, envs: &[(&str, &str)]) -> Output {
    write(&host.join("app/auth/Client.kt"), &format!("// {message}\n"));
    sb.git(host, &["add", "app/auth/Client.kt"]);
    let kb = host.join(".kb");
    let all = [&[("KB_ROOT", kb.to_str().unwrap())][..], envs].concat();
    git_env(sb, host, &["commit", "-q", "-m", message], &all)
}

#[cfg(unix)]
#[test]
fn shipped_hook_reads_the_host_pin_unless_the_binding_selects_and_skips_freshness_on_request() {
    let sb = Sandbox::new();
    let (origin, first) = prefix_remote(&sb);
    let host = prefix_host(&sb);
    let kb = host.join(".kb");
    // The commit that first adds the KB gitlink has no pin in HEAD: the approved tip applies.
    sb.git(&host, &["submodule", "add", "-q", &origin, ".kb"]);
    sb.git(&kb, &["checkout", "-q", "--detach", &first]);
    sb.git(&host, &["add", ".kb"]);
    fs::hard_link(common::kb_bin(), kb.join("kbw")).unwrap();
    let commit = |message: &str, envs: &[(&str, &str)]| hook_commit(&sb, &host, message, envs);
    assert_rejected(&commit("feat: mount the KB", &[]), "VALIDATION_FAILED");
    assert_committed(&commit("chore: mount the KB", &[]));
    // No KB_SNAPSHOT: the pinned KB applies, not UPDATE_REQUIRED or the approved tip.
    assert_committed(&commit("feat: synthetic change", &[]));
    assert_rejected(&commit("chore: not yet pinned", &[]), "VALIDATION_FAILED");
    // A selection that `.kbw.toml` declares is the engine's to apply.
    let binding = host.join(".kbw.toml");
    write(
        &binding,
        "schema = 1\nrepo = \"mobile\"\nselection = \"latest\"\n",
    );
    assert_committed(&commit("chore: selected latest", &[]));
    write(&binding, "schema = 1\nrepo = \"mobile\"\n");
    // The pin-bump commit is checked against the pin it replaces.
    sb.git(&kb, &["checkout", "-q", "--detach", "origin/main"]);
    sb.git(&host, &["add", ".kb"]);
    assert_committed(&commit("feat: bump the KB pin", &[]));
    let missing = format!("{origin}-missing");
    sb.git(&kb, &["remote", "set-url", "origin", &missing]);
    assert_rejected(&commit("chore: offline", &[]), "FRESHNESS_UNVERIFIED");
    assert_committed(&commit("chore: offline", &[("KB_OFFLINE", "1")]));
}

#[cfg(unix)]
#[test]
fn shipped_hook_reads_the_approved_tip_for_a_separate_checkout_without_a_pin() {
    let sb = Sandbox::new();
    let (origin, first) = prefix_remote(&sb);
    let host = prefix_host(&sb);
    let kb = host.join(".kb");
    // A KB clone that the host neither tracks nor pins.
    sb.git(&host, &["clone", "-q", &origin, ".kb"]);
    write(&host.join(".git/info/exclude"), ".kb/\n");
    fs::hard_link(common::kb_bin(), kb.join("kbw")).unwrap();
    let commit = |message: &str| hook_commit(&sb, &host, message, &[]);
    assert_committed(&commit("chore: synthetic change"));
    assert_rejected(&commit("feat: synthetic change"), "VALIDATION_FAILED");
    // A `.kbw.toml` pin is a host pin too.
    write(
        &host.join(".kbw.toml"),
        &format!("schema = 1\nrepo = \"mobile\"\npin = \"{first}\"\n"),
    );
    assert_committed(&commit("feat: synthetic pinned change"));
    assert_rejected(&commit("chore: not pinned"), "VALIDATION_FAILED");
}
