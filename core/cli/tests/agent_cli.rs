//! The agent commands (`sym`, `callers`, `impact <symbol>`, `doc`, `find`, `check`,
//! `tools`): routing through the tool registry, fallbacks, bounded answers that name their
//! source, configured checks and the knowledge lookup. A shell script stands in for
//! CodeGraph (synthetic JSON, labeled); no test needs an installed code tool or the network.
//! The stand-ins are POSIX shell scripts, so these tests run on Unix only; Windows CI runs
//! the engine through the other suites.
#![cfg(unix)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::{Sandbox, json, stderr, stdout, write};
use serde_json::Value;

const FAKE_CODEGRAPH: &str = r#"#!/bin/sh
# SYNTHETIC stand-in for CodeGraph: prints labeled fixture JSON.
d=$(dirname "$0")
echo "$1" >> "$d/calls.log"
case "$1" in
  --version) echo "1.6.2" ;;
  status) cat "$d/status.json" ;;
  sync) echo "synced" ;;
  query) cat "$d/query.json" ;;
  callers) cat "$d/callers.json" ;;
  impact) cat "$d/impact.json" ;;
  *) exit 2 ;;
esac
"#;

const STATUS_OK: &str = r#"{"initialized": true, "pendingChanges": {"added": 0, "modified": 0, "removed": 0}, "index": {"state": "complete", "pendingRefs": 0}}"#;

const QUERY: &str = r#"[
  {"node": {"name": "refreshAll", "kind": "function", "filePath": "app/src/auth/Other.kt", "startLine": 1, "endLine": 2}},
  {"node": {"name": "refresh", "qualifiedName": "TokenRefresher::refresh", "kind": "method", "filePath": "app/src/auth/TokenRefresher.kt", "startLine": 2, "endLine": 4, "signature": "(): Token"}}
]"#;

const CALLERS: &str = r#"{
  "targets": [{"name": "refresh", "filePath": "app/src/auth/TokenRefresher.kt", "startLine": 2}],
  "ambiguous": false,
  "definitions": [{
    "callers": [{"id": "m:1", "name": "signIn", "kind": "method", "filePath": "app/src/auth/Login.kt", "startLine": 2, "relationships": ["calls"]}],
    "edges": [{"source": "m:1", "target": "m:0", "kind": "calls", "line": 3}],
    "truncated": false
  }]
}"#;

const IMPACT: &str = r#"{
  "targets": [{"name": "refresh", "filePath": "app/src/auth/TokenRefresher.kt", "startLine": 2}],
  "affected": [
    {"name": "refresh", "kind": "method", "filePath": "app/src/auth/TokenRefresher.kt", "startLine": 2},
    {"name": "signIn", "kind": "method", "filePath": "app/src/auth/Login.kt", "startLine": 2}
  ]
}"#;

/// A KB (the context fixture project) and a mobile host repository.
struct World {
    sb: Sandbox,
    kb: PathBuf,
    host: PathBuf,
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            fs::copy(e.path(), &target).unwrap();
        }
    }
}

fn executable(path: &Path, text: &str) {
    write(path, text);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

impl World {
    fn new(tools: &str) -> World {
        let sb = Sandbox::new();
        let kb = sb.path().join("kb");
        copy_dir(
            &common::repo_root().join("core/tests/fixtures/context/project"),
            &kb.join("project"),
        );
        common::copy_manifest(&kb);
        let fake = kb.join("fake");
        executable(&fake.join("codegraph"), FAKE_CODEGRAPH);
        write(&fake.join("status.json"), STATUS_OK);
        write(&fake.join("query.json"), QUERY);
        write(&fake.join("callers.json"), CALLERS);
        write(&fake.join("impact.json"), IMPACT);
        write(&kb.join("project/tools.toml"), tools);
        sb.init_repo(&kb);
        sb.commit_all(&kb, "kb");

        let host = sb.path().join("host");
        sb.init_repo(&host);
        sb.git(
            &host,
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/acme/mobile.git",
            ],
        );
        write(
            &host.join("app/src/auth/TokenRefresher.kt"),
            "class TokenRefresher {\n    fun refresh(): Token {\n        return Token()\n    }\n}\n",
        );
        write(
            &host.join("app/src/auth/Login.kt"),
            "class Login(private val refresher: TokenRefresher) {\n    fun signIn() {\n        refresher.refresh()\n    }\n}\n",
        );
        sb.commit_all(&host, "host");
        World { sb, kb, host }
    }

    fn kb(&self, args: &[&str]) -> Output {
        let mut all = vec![
            "--root",
            self.kb.to_str().unwrap(),
            "--host",
            self.host.to_str().unwrap(),
        ];
        all.extend_from_slice(args);
        self.sb.kb(&self.host, &all, &[])
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.kb.join("fake/calls.log")).unwrap_or_default()
    }
}

const TOOLS: &str = r#"schema = 1
[[tool]]
id = "codegraph"
adapter = "codegraph"
program = "{kb}/fake/codegraph"
[[tool]]
id = "git-grep"
adapter = "git-grep"
[[tool]]
id = "lint"
adapter = "command"
program = "{kb}/fake/lint"
run = ["--strict", "{files}"]
paths = ["mobile:app/**/*.kt"]
description = "synthetic lint"
[routes]
sym = ["codegraph", "git-grep"]
callers = ["codegraph", "git-grep"]
impact = ["codegraph", "git-grep"]
find = ["git-grep"]
"#;

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn items(v: &Value) -> &Vec<Value> {
    v["result"]["items"].as_array().unwrap()
}

#[test]
fn sym_is_routed_to_the_configured_tool_after_a_sync_and_names_it() {
    let w = World::new(TOOLS);
    let o = w.kb(&["--json", "sym", "refresh"]);
    assert_eq!(code(&o), 0, "{}", common::stderr(&o));
    let v = json(&o);
    assert_eq!(v["result"]["answer"], "kb.answer.v1");
    let source = &v["result"]["sources"][0];
    assert_eq!(source["id"], "codegraph");
    assert_eq!(source["version"], "codegraph 1.6.2");
    assert_eq!(source["state"], "index synced, complete");
    let items = items(&v);
    assert_eq!(items.len(), 1, "only the exact definition: {items:#?}");
    assert_eq!(items[0]["name"], "TokenRefresher::refresh");
    assert_eq!(items[0]["path"], "app/src/auth/TokenRefresher.kt");
    assert_eq!(items[0]["line"], 2);
    // The definition comes with its lines, so no read has to follow it.
    assert_eq!(v["result"]["excerpts"].as_array().unwrap().len(), 1);
    let excerpt = &v["result"]["excerpts"][0];
    assert_eq!(excerpt["path"], "app/src/auth/TokenRefresher.kt");
    assert_eq!(excerpt["first_line"], 1);
    assert_eq!(
        excerpt["lines"],
        serde_json::json!([
            "class TokenRefresher {",
            "    fun refresh(): Token {",
            "        return Token()",
            "    }"
        ])
    );
    assert!(excerpt.get("next_line").is_none(), "{excerpt:#}");
    assert_eq!(
        v["result"]["next"],
        serde_json::json!(["kb callers refresh"])
    );
    let calls = w.calls();
    let sync = calls.find("sync").expect("synced");
    assert!(
        sync < calls.find("query").unwrap(),
        "sync before query:\n{calls}"
    );

    let text = stdout(&w.kb(&["sym", "refresh"]));
    assert!(
        text.starts_with("kb sym refresh · codegraph 1.6.2 (index synced, complete)\n"),
        "{text}"
    );
    assert!(
        text.contains("definition  app/src/auth/TokenRefresher.kt:2-4  TokenRefresher::refresh")
    );
    assert!(
        text.contains("\n2:    fun refresh(): Token {\n3:        return Token()\n"),
        "{text}"
    );
    assert!(text.lines().count() <= 30);
}

#[test]
fn a_few_callers_come_with_the_lines_at_their_call_sites() {
    let w = World::new(TOOLS);
    let mut source = ["// fixture"; 60];
    source[0] = "import auth.refresh";
    source[19] = "fun signIn() {";
    source[45] = "    refresher.refresh()";
    source[55] = "}";
    write(&w.host.join("app/src/auth/Login.kt"), &source.join("\n"));
    let calls = CALLERS
        .replace(
            r#""startLine": 2, "relationships""#,
            r#""startLine": 20, "relationships""#,
        )
        .replace(r#""line": 3"#, r#""line": 46"#);
    write(&w.kb.join("fake/callers.json"), &calls);
    let v = json(&w.kb(&["--json", "callers", "refresh"]));
    assert_eq!(items(&v)[0]["line"], 46);
    // The call site is shown, so no read is suggested for it.
    let excerpts = v["result"]["excerpts"].as_array().unwrap();
    assert_eq!(excerpts.len(), 1, "{v:#}");
    assert_eq!(excerpts[0]["path"], "app/src/auth/Login.kt");
    assert_eq!(excerpts[0]["first_line"], 43);
    assert_eq!(excerpts[0]["lines"][3], "    refresher.refresh()");
    assert_eq!(excerpts[0]["lines"].as_array().unwrap().len(), 12);
    assert_eq!(
        v["result"]["next"],
        serde_json::json!(["kb impact refresh"])
    );
    let text = stdout(&w.kb(&["callers", "refresh"]));
    assert!(text.contains("\n46:    refresher.refresh()\n"), "{text}");
    assert!(!text.contains("20:fun signIn()"), "{text}");

    // Without a call line the caller's own line is the place.
    let unlocated = calls.replace(r#", "line": 46"#, "");
    write(&w.kb.join("fake/callers.json"), &unlocated);
    let v = json(&w.kb(&["--json", "callers", "refresh"]));
    assert_eq!(items(&v)[0]["line"], 20);
    assert_eq!(v["result"]["excerpts"][0]["first_line"], 17);

    // A reference that is an import line is listed, not shown among other imports.
    let imported = calls.replace(
        r#""callers": [{"#,
        r#""callers": [{"id": "m:9", "name": "auth", "kind": "file", "filePath": "app/src/auth/Login.kt", "startLine": 1, "relationships": ["name"]}, {"#,
    );
    write(&w.kb.join("fake/callers.json"), &imported);
    let v = json(&w.kb(&["--json", "callers", "refresh"]));
    assert_eq!(items(&v).len(), 2, "{v:#}");
    let excerpts = v["result"]["excerpts"].as_array().unwrap();
    assert_eq!(excerpts.len(), 1, "{v:#}");
    assert_eq!(excerpts[0]["first_line"], 43);

    // Many callers are a list to choose from: the read at the first call site is named.
    let many: Vec<String> = (0..6)
        .map(|n| {
            format!(
                r#"{{"id": "m:{n}", "name": "caller{n}", "kind": "method", "filePath": "app/src/auth/Login.kt", "startLine": {}, "relationships": ["calls"]}}"#,
                30 + n
            )
        })
        .collect();
    let edges: Vec<String> = (0..6)
        .map(|n| {
            format!(
                r#"{{"source": "m:{n}", "target": "t", "kind": "calls", "line": {}}}"#,
                30 + n
            )
        })
        .collect();
    write(
        &w.kb.join("fake/callers.json"),
        &format!(
            r#"{{"targets": [{{"name": "refresh", "filePath": "app/src/auth/TokenRefresher.kt", "startLine": 2}}], "ambiguous": false, "definitions": [{{"callers": [{}], "edges": [{}], "truncated": false}}]}}"#,
            many.join(", "),
            edges.join(", ")
        ),
    );
    let v = json(&w.kb(&["--json", "callers", "refresh"]));
    assert_eq!(items(&v).len(), 6, "{v:#}");
    assert!(v["result"].get("excerpts").is_none(), "{v:#}");
    assert_eq!(
        v["result"]["next"][0],
        "kb read app/src/auth/Login.kt 28 38"
    );
}

#[test]
fn a_missing_index_falls_back_to_the_next_tool_and_says_so() {
    let w = World::new(TOOLS);
    write(&w.kb.join("fake/status.json"), r#"{"initialized": false}"#);
    let v = json(&w.kb(&["--json", "sym", "refresh"]));
    assert_eq!(v["ok"], true, "{v:#}");
    assert_eq!(v["result"]["sources"][0]["id"], "git-grep");
    let defs = items(&v);
    assert_eq!(defs[0]["path"], "app/src/auth/TokenRefresher.kt");
    assert_eq!(defs[0]["line"], 2);
    assert_eq!(defs[0]["confidence"], "lexical");
    let notes = v["result"]["notes"].to_string();
    assert!(
        notes.contains("codegraph failed: no CodeGraph index"),
        "{notes}"
    );
    assert!(
        !w.calls().contains("query"),
        "an absent index is never queried"
    );

    let callers = json(&w.kb(&["--json", "callers", "refresh"]));
    let items = items(&callers);
    assert_eq!(items.len(), 1, "{items:#?}");
    assert_eq!(items[0]["path"], "app/src/auth/Login.kt");
    assert_eq!(items[0]["line"], 3);
}

#[test]
fn unknown_symbols_are_not_found_with_the_answer_printed() {
    let w = World::new(TOOLS);
    write(&w.kb.join("fake/query.json"), "[]");
    let o = w.kb(&["--json", "sym", "doesNotExist"]);
    assert_eq!(code(&o), 16);
    let v = json(&o);
    assert_eq!(v["error"]["code"], "NOT_FOUND");
    assert_eq!(v["result"]["total"], 0);
    assert_eq!(v["result"]["sources"][0]["id"], "codegraph");
}

#[test]
fn routes_and_forced_tools_are_checked() {
    let w = World::new(&TOOLS.replace("sym = [\"codegraph\", \"git-grep\"]", "sym = []"));
    let o = w.kb(&["--json", "sym", "refresh"]);
    assert_eq!(code(&o), 65, "{}", stdout(&o));
    assert_eq!(json(&o)["error"]["code"], "TOOL_UNAVAILABLE");
    let o = w.kb(&["callers", "refresh", "--tool", "lint"]);
    assert_eq!(code(&o), 64);
    assert!(common::stderr(&o).contains("cannot answer `callers`"));
    let o = w.kb(&["callers", "refresh", "--tool", "git-grep"]);
    assert_eq!(code(&o), 0);
    assert!(stdout(&o).starts_with("kb callers refresh · git version"));

    let w = World::new("schema = 1\n[[tool]]\nid = \"x\"\nadapter = \"ripgrep\"\n");
    let o = w.kb(&["sym", "refresh"]);
    assert_eq!(code(&o), 11, "an invalid registry is CONFIG_INVALID");
    assert!(common::stderr(&o).contains("needs `program`"));
    let o = w.kb(&[
        "--offline",
        "--snapshot",
        "working-tree",
        "validate",
        "--no-routing",
    ]);
    assert!(stdout(&o).contains("TOOLS_INVALID"), "{}", stdout(&o));
}

#[test]
fn find_is_sorted_and_bounded() {
    let w = World::new(TOOLS);
    for i in 0..40 {
        write(&w.host.join(format!("src/f{i:02}.txt")), "a needle here\n");
    }
    let o = w.kb(&["find", "needle", "--lines", "10"]);
    assert_eq!(code(&o), 0);
    let text = stdout(&o);
    assert!(text.lines().count() <= 10, "{text}");
    assert!(text.contains("(truncated; --lines widens the bound)"));
    let v = json(&w.kb(&["--json", "find", "needle", "--lines", "10"]));
    assert_eq!(v["result"]["total"], 40, "untracked files are searched");
    assert_eq!(v["result"]["truncated"], true);
    let paths: Vec<&str> = items(&v)
        .iter()
        .map(|i| i["path"].as_str().unwrap())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    assert_eq!(paths[0], "src/f00.txt");
    let o = w.kb(&["find", "no such text anywhere"]);
    assert_eq!(code(&o), 16);
}

#[test]
fn find_keeps_relevant_files_in_the_bounded_prefix() {
    let w = World::new(TOOLS);
    write(&w.host.join("src/a.kt"), &"Target usage\n".repeat(20));
    write(&w.host.join("src/b.kt"), "Target usage\n");
    write(&w.host.join("src/part/Target.kt"), "class Target {}\n");

    let result = json(&w.kb(&["--json", "find", "Target", "--lines", "6"]));
    assert_eq!(result["result"]["total"], 22);
    assert_eq!(result["result"]["truncated"], true);
    let paths: Vec<_> = items(&result).iter().map(|row| &row["path"]).collect();
    assert_eq!(paths, ["src/part/Target.kt", "src/a.kt", "src/b.kt"]);

    let result = json(&w.kb(&["--json", "find", "Target", "--glob", "Target.kt"]));
    assert_eq!(items(&result)[0]["path"], "src/part/Target.kt");
    assert_eq!(items(&result).len(), 1);

    // A glob that names a directory is searched below it; one that names nothing misses
    // and says how a directory is written.
    let result = json(&w.kb(&["--json", "find", "Target", "--glob", "*part*"]));
    assert_eq!(items(&result)[0]["path"], "src/part/Target.kt");
    assert_eq!(items(&result).len(), 1);
    let output = w.kb(&["find", "Target", "--glob", "*nowhere*"]);
    assert_eq!(code(&output), 16);
    assert!(stderr(&output).contains("--glob '**/DIR/**'"));
    let result = json(&w.kb(&["--json", "find", "Target", "--glob", "**/part/**"]));
    assert_eq!(items(&result)[0]["path"], "src/part/Target.kt");
    assert_eq!(items(&result).len(), 1);
}

#[test]
fn find_prioritizes_kotlin_and_java_body_matches_in_both_views() {
    let w = World::new(TOOLS);
    write(
        &w.host.join("src/Target.kt"),
        "package fixture.Target\nimport fixture.dep.Target\nfun first() = Target()\nfun second() = Target()\nfun third() = Target()\n",
    );
    write(&w.host.join("src/a.kt"), "import fixture.dep.Target\n");
    write(
        &w.host.join("src/b.java"),
        "package fixture.Target;\nimport fixture.dep.Target;\nclass B { Target one;\nTarget two; }\n",
    );
    let v = json(&w.kb(&["--json", "find", "Target"]));
    assert_eq!(v["result"]["total"], 10);
    let locations: Vec<_> = items(&v)
        .iter()
        .map(|item| {
            (
                item["path"].as_str().unwrap(),
                item["line"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        locations,
        [
            ("src/Target.kt", 3),
            ("src/b.java", 3),
            ("src/Target.kt", 4),
            ("src/b.java", 4),
            ("src/Target.kt", 5),
            ("src/Target.kt", 1),
            ("src/a.kt", 1),
            ("src/b.java", 1),
            ("src/Target.kt", 2),
            ("src/b.java", 2)
        ]
    );
    let text = stdout(&w.kb(&["find", "Target", "--lines", "7"]));
    let lines: Vec<_> = text
        .lines()
        .filter(|line| line.starts_with("match"))
        .collect();
    // The text groups the selected hits by file, files in rank order, each path once.
    assert!(lines[0].contains("src/Target.kt:3"), "{text}");
    assert!(lines[1].starts_with("match  :4"), "{text}");
    assert!(lines[2].contains("src/b.java:3"), "{text}");
    assert!(lines[3].starts_with("match  :4"), "{text}");

    let explicit_queries: [&[&str]; 3] = [
        &["find", "import fixture.dep.Target"],
        &["find", "package fixture.Target"],
        &["find", "^package .*Target", "--regex"],
    ];
    for query in explicit_queries {
        let text = stdout(&w.kb(query));
        assert!(
            text.lines().nth(1).unwrap().contains("src/Target.kt"),
            "{text}"
        );
        assert!(!text.contains("fun first"), "{text}");
    }

    let text = stdout(&w.kb(&["find", "Target", "--regex"]));
    let lines: Vec<_> = text
        .lines()
        .filter(|line| line.starts_with("match"))
        .collect();
    assert!(lines[0].contains("src/Target.kt:1"), "{text}");
    assert!(
        lines[1].contains(":2") && lines[1].contains("import"),
        "{text}"
    );
}

#[test]
fn oversized_line_requests_are_capped_and_invalid_values_still_fail() {
    let w = World::new(TOOLS);
    for i in 0..12 {
        write(
            &w.host.join(format!("src/f{i:02}.txt")),
            &"needle\n".repeat(20),
        );
    }
    let output = w.kb(&["find", "needle", "--lines", "500"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert_eq!(text.lines().count(), 200);
    assert!(text.contains("--lines 500 capped at 200"));
    let v = json(&w.kb(&["--json", "find", "needle", "--lines", "500"]));
    assert_eq!(v["result"]["total"], 240);
    assert_eq!(v["result"]["truncated"], true);
    assert_eq!(v["result"]["notes"][0], "--lines 500 capped at 200");

    for lines in ["4", "0", "-1", "many", "18446744073709551616"] {
        let output = w.kb(&["--json", "find", "needle", "--lines", lines]);
        assert_eq!(code(&output), 2, "{lines}: {}", stdout(&output));
        assert_eq!(json(&output)["error"]["code"], "USAGE");
    }
}

#[test]
fn check_runs_matching_commands_without_a_shell_and_plan_runs_nothing() {
    let w = World::new(TOOLS);
    executable(
        &w.kb.join("fake/lint"),
        "#!/bin/sh\ndate > \"$(dirname \"$0\")/ran\"\n[ \"$1\" = \"--strict\" ] || exit 3\nshift\nif grep -l BAD \"$@\"; then echo \"lint: BAD found\" >&2; exit 1; fi\necho ok\n",
    );
    let marker = w.kb.join("fake/ran");
    let o = w.kb(&["check", "app/src/auth/Login.kt", "--plan"]);
    assert_eq!(code(&o), 0, "{}", stdout(&o));
    assert!(stdout(&o).contains("check  lint  would run for 1 file(s): synthetic lint"));
    assert!(!marker.exists(), "--plan runs nothing");

    let o = w.kb(&["--json", "check", "app/src/auth/Login.kt"]);
    assert_eq!(code(&o), 0, "{}", stdout(&o));
    assert!(marker.exists());
    assert!(
        items(&json(&o))[0]["detail"]
            .as_str()
            .unwrap()
            .starts_with("passed")
    );

    write(&w.host.join("app/src/auth/Login.kt"), "BAD\n");
    // Default file set: the work tree's changes against HEAD.
    let o = w.kb(&["--json", "check"]);
    assert_eq!(code(&o), 46, "{}", stdout(&o));
    let v = json(&o);
    assert_eq!(v["error"]["code"], "CHECKS_FAILED");
    let details: Vec<&str> = items(&v)
        .iter()
        .filter_map(|i| i["detail"].as_str())
        .collect();
    assert!(details[0].starts_with("failed (exit 1)"), "{details:?}");
    assert!(details.contains(&"lint: BAD found"), "{details:?}");

    let o = w.kb(&["check", "README.md"]);
    assert_eq!(code(&o), 0);
    assert!(stdout(&o).contains("no command check"));
}

#[test]
fn check_validates_changed_knowledge() {
    let w = World::new(TOOLS);
    // A KB checkout used without a host: the code root is the KB and its knowledge.
    let feature = w.kb.join("project/knowledge/features/login.md");
    let o = w.sb.kb(
        &w.kb,
        &[
            "--root",
            w.kb.to_str().unwrap(),
            "check",
            "project/knowledge/features/login.md",
        ],
        &[],
    );
    assert_eq!(code(&o), 0, "{}", stdout(&o));
    assert!(
        stdout(&o).contains("check  knowledge  passed"),
        "{}",
        stdout(&o)
    );
    fs::write(&feature, "+++\nbroken\n").unwrap();
    let o = w.sb.kb(
        &w.kb,
        &[
            "--root",
            w.kb.to_str().unwrap(),
            "check",
            "project/knowledge/features/login.md",
        ],
        &[],
    );
    assert_eq!(code(&o), 46, "{}", stdout(&o));
    assert!(stdout(&o).contains("check  knowledge  failed"));
}

#[test]
fn doc_and_symbol_impact_relate_code_to_feature_knowledge() {
    let w = World::new(TOOLS);
    let o = w.kb(&[
        "--offline",
        "--snapshot",
        "working-tree",
        "--json",
        "doc",
        "app/src/auth/TokenRefresher.kt",
    ]);
    assert_eq!(code(&o), 0, "{}", common::stderr(&o));
    let v = json(&o);
    let find = |kind: &str| -> Vec<Value> {
        items(&v)
            .iter()
            .filter(|i| i["kind"] == kind)
            .cloned()
            .collect()
    };
    let feature = find("feature");
    assert_eq!(feature[0]["name"], "login", "{v:#}");
    assert!(
        find("purpose")[0]["detail"]
            .as_str()
            .unwrap()
            .starts_with("Users sign in")
    );
    assert_eq!(find("limit")[0]["name"], "no-social");
    assert!(
        !find("contract").is_empty(),
        "contracts of the module are listed: {v:#}"
    );
    assert_eq!(v["result"]["sources"][0]["id"], "knowledge");
    assert!(
        v["result"]["next"][0]
            .as_str()
            .unwrap()
            .starts_with("kb context --intent implement --path app/src/auth/TokenRefresher.kt")
    );

    // A symbol is located first, then documented.
    let v = json(&w.kb(&[
        "--offline",
        "--snapshot",
        "working-tree",
        "--json",
        "doc",
        "refresh",
    ]));
    assert_eq!(v["result"]["sources"][0]["id"], "codegraph");
    assert!(
        items(&v)
            .iter()
            .any(|i| i["kind"] == "feature" && i["name"] == "login")
    );

    let o = w.kb(&[
        "--offline",
        "--snapshot",
        "working-tree",
        "--json",
        "impact",
        "refresh",
    ]);
    assert_eq!(code(&o), 0, "{}", common::stderr(&o));
    let v = json(&o);
    let kinds: Vec<&str> = items(&v)
        .iter()
        .map(|i| i["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"feature"), "{kinds:?}");
    let affected: Vec<&Value> = items(&v)
        .iter()
        .filter(|i| i["kind"] == "affected")
        .collect();
    assert_eq!(
        affected.len(),
        1,
        "the target itself is not listed as affected"
    );
    assert_eq!(affected[0]["path"], "app/src/auth/Login.kt");
    let ids: Vec<&str> = v["result"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["codegraph", "knowledge"]);

    // Diff mode keeps its own options.
    let o = w.kb(&["impact", "refresh", "--base", "HEAD"]);
    assert_eq!(code(&o), 2);
    let o = w.kb(&["impact", "--working-tree", "--lines", "10"]);
    assert_eq!(code(&o), 64);
}

#[test]
fn compact_doc_prefers_the_source_and_specific_records_and_lists_one_copy_of_each_guide() {
    let w = World::new(TOOLS);
    let knowledge = w.kb.join("project/knowledge");
    let guide = r#"[[anchors]]
kind = "doc"
repo = "mobile"
path = "docs/sign-in.md"
"#;
    let exact = knowledge.join("policies/mobile-token-storage.md");
    let text = fs::read_to_string(&exact).unwrap().replacen(
        "\n+++\nTokens",
        &format!(
            "\n{guide}\n[[anchors]]\nkind = \"source\"\nrepo = \"mobile\"\n\
             path = \"app/src/auth/TokenRefresher.kt\"\n\n+++\nTokens"
        ),
        1,
    );
    write(&exact, &text);
    let by_path = knowledge.join("policies/mobile-auth-network.md");
    let text = fs::read_to_string(&by_path)
        .unwrap()
        .replacen(
            "\n[[overrides]]",
            "\n[selectors]\npaths = [\"app/src/auth/TokenRefresher.kt\"]\n\n[[overrides]]",
            1,
        )
        .replacen("\n+++\n", &format!("\n{guide}\n+++\n"), 1);
    write(&by_path, &text);

    let args = ["--offline", "--snapshot", "working-tree", "doc", "refresh"];
    let out = w.kb(&args);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    let expected = [
        "definition  app/src/auth/TokenRefresher.kt:2-4",
        "acme.mobile.token-storage",
        "docs/sign-in.md",
        "acme.mobile.auth-network",
        "acme.feature.login",
        "guidance  acme.product.logging",
    ];
    let positions: Vec<_> = expected
        .iter()
        .map(|part| {
            text.find(part)
                .unwrap_or_else(|| panic!("missing {part}:\n{text}"))
        })
        .collect();
    assert!(positions.windows(2).all(|p| p[0] < p[1]), "{text}");
    assert_eq!(text.matches("docs/sign-in.md").count(), 1, "{text}");
    assert!(!text.contains("Logging hygiene"), "{text}");
    assert!(!text.contains("docs/logging.md"), "{text}");
    assert!(
        text.contains("kb read app/src/auth/TokenRefresher.kt 1 10"),
        "{text}"
    );
    assert!(text.contains("knowledge is partial"), "{text}");

    let full = stdout(&w.kb(&[args.as_slice(), &["--format", "human"]].concat()));
    assert!(full.contains("Users sign in"), "{full}");
    assert!(full.contains("limit  no-social"), "{full}");
    assert!(!full.contains("guidance  acme.product.logging"), "{full}");
}

#[test]
fn compact_doc_leads_with_a_read_route_for_an_existing_document() {
    let w = World::new(TOOLS);
    write(&w.host.join("docs/sign-in.md"), "Sign-in guide\n");
    let out = w.kb(&[
        "--offline",
        "--snapshot",
        "working-tree",
        "doc",
        "mobile:docs/sign-in.md",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert_eq!(
        text.lines().nth(1),
        Some("guide  kb read docs/sign-in.md"),
        "{text}"
    );
    assert!(stdout(&w.kb(&["read", "docs/sign-in.md"])).contains("1:Sign-in guide"));

    let text = stdout(&w.kb(&[
        "--offline",
        "--snapshot",
        "working-tree",
        "doc",
        "docs/missing.md",
    ]));
    assert!(!text.contains("kb read docs/missing.md"), "{text}");
}

#[test]
fn tools_reports_routes_and_availability() {
    let w = World::new(&TOOLS.replace("{kb}/fake/lint", "{kb}/fake/missing-lint"));
    let o = w.kb(&["--json", "tools"]);
    assert_eq!(code(&o), 0, "{}", stdout(&o));
    let v = json(&o);
    let detail = |kind: &str, name: &str| -> String {
        items(&v)
            .iter()
            .find(|i| i["kind"] == kind && i["name"] == name)
            .and_then(|i| i["detail"].as_str())
            .unwrap_or_default()
            .to_string()
    };
    assert!(detail("route", "sym").starts_with("codegraph (route: codegraph → git-grep)"));
    assert!(detail("tool", "codegraph").contains("index as is, complete · available"));
    assert!(detail("tool", "lint").contains("unavailable"));
    assert!(!w.calls().contains("sync"), "kb tools never syncs an index");
}

#[test]
fn check_with_a_base_also_evaluates_declared_probes() {
    let w = World::new(TOOLS);
    let o = w.kb(&[
        "--offline",
        "--snapshot",
        "working-tree",
        "--json",
        "check",
        "--base",
        "HEAD",
    ]);
    let v = json(&o);
    let probes = items(&v)
        .iter()
        .find(|i| i["name"] == "probes")
        .unwrap_or_else(|| panic!("a probes item: {v:#}"));
    assert!(
        probes["detail"].as_str().unwrap().contains("passed,"),
        "{probes:#}"
    );
    let o = w.kb(&["check", "--base", "HEAD", "--plan"]);
    assert_eq!(code(&o), 0);
    assert!(stdout(&o).contains("would evaluate declared probes against HEAD"));
}

/// Any command-line tool, connected by configuration alone (`exec`): a project capability
/// called with `kb run`, and a tool answering a built-in request.
const EXEC_TOOLS: &str = r#"schema = 1
[[tool]]
id = "deps"
adapter = "exec"
program = "{kb}/fake/deps"
args = ["--for", "{subject}", "--root={root}"]
capabilities = ["deps"]
description = "synthetic dependency lookup"
output = "json"
items = "/modules/*/uses"
fields = { name = "module", path = "file", line = "at", detail = "why" }
env = { DEPS_MODE = "strict" }
version_args = []
[[tool]]
id = "quiet"
adapter = "exec"
program = "{kb}/fake/quiet"
args = ["{args}"]
capabilities = ["deps", "owners"]
output = "lines"
ok_exit = [0, 1]
version_args = []
[[tool]]
id = "symgrep"
adapter = "exec"
program = "{kb}/fake/symgrep"
args = ["{subject}"]
capabilities = ["sym"]
output = "grep"
confidence = "lexical"
version_args = ["-V"]
[[tool]]
id = "graph"
adapter = "exec"
program = "{kb}/fake/graph"
prepare = ["prep", "{root}"]
args = ["q", "{args}"]
capabilities = ["graph"]
output = "json"
version_args = []
[[tool]]
id = "git-grep"
adapter = "git-grep"
[routes]
sym = ["symgrep", "git-grep"]
deps = ["quiet", "deps"]
find = ["git-grep"]
"#;

fn exec_world() -> World {
    let w = World::new(EXEC_TOOLS);
    executable(
        &w.kb.join("fake/deps"),
        "#!/bin/sh\n# SYNTHETIC tool printing its own JSON shape.\n[ \"$DEPS_MODE\" = strict ] || exit 9\ncat <<JSON\n{\"modules\": [{\"uses\": [{\"module\": \"auth\", \"file\": \"app/src/auth/Login.kt\", \"at\": 3, \"why\": \"calls refresh for $2\"}]},\n {\"uses\": [{\"module\": \"net\", \"file\": \"app/src/net/Http.kt\", \"at\": \"7\", \"why\": \"transport\"}]}]}\nJSON\n",
    );
    executable(
        &w.kb.join("fake/quiet"),
        "#!/bin/sh\n# exit 1 = nothing found\nexit 1\n",
    );
    executable(
        &w.kb.join("fake/graph"),
        concat!(
            "#!/bin/sh\n# SYNTHETIC indexed tool: `prep` builds the index, `q` reads it.\n",
            "d=$(dirname \"$0\")\n",
            "if [ \"$1\" = prep ]; then touch \"$d/prepared\"; exit 0; fi\n",
            "[ -f \"$d/prepared\" ] || exit 7\n",
            "shift\n",
            "if [ \"$1\" = known ]; then printf '{\"items\": [{\"name\": \"n\", \"detail\": \"%s\"}]}\\n' \"$2\"; ",
            "else echo \"Symbol not found\"; fi\n",
        ),
    );
    executable(
        &w.kb.join("fake/symgrep"),
        "#!/bin/sh\n[ \"$1\" = -V ] && { echo symgrep 2.0; exit 0; }\necho \"app/src/auth/TokenRefresher.kt:2:5:    fun $1(): Token {\"\n",
    );
    w
}

#[test]
fn project_capabilities_run_any_tool_through_the_registry() {
    let w = exec_world();
    let o = w.kb(&["--json", "run", "deps", "TokenRefresher"]);
    assert_eq!(code(&o), 0, "{}", stdout(&o));
    let v = json(&o);
    assert_eq!(v["result"]["command"], "run");
    assert_eq!(v["result"]["subject"], "deps TokenRefresher");
    assert_eq!(
        v["result"]["sources"][0]["id"], "deps",
        "quiet found nothing and handed over"
    );
    assert_eq!(v["result"]["sources"][0]["adapter"], "exec");
    let items = items(&v);
    assert_eq!(items.len(), 2, "`*` flattens every module: {items:#?}");
    assert_eq!(items[0]["name"], "auth");
    assert_eq!(items[0]["path"], "app/src/auth/Login.kt");
    assert_eq!(items[0]["line"], 3);
    assert_eq!(items[0]["detail"], "calls refresh for TokenRefresher");
    assert_eq!(items[1]["line"], 7, "string line numbers are read");
    let notes = v["result"]["notes"].to_string();
    assert!(notes.contains("quiet did not find"), "{notes}");

    // `ok_exit` makes exit 1 an empty answer, not a failure.
    let o = w.kb(&["run", "owners", "x"]);
    assert_eq!(code(&o), 0, "{}", common::stderr(&o));
    assert!(stdout(&o).contains("note: the tool returned nothing"));

    // Misuse is rejected before any tool runs.
    let o = w.kb(&["run", "sym", "refresh"]);
    assert_eq!(code(&o), 64);
    assert!(common::stderr(&o).contains("is a kb command"));
    let o = w.kb(&["run", "lint"]);
    assert_eq!(code(&o), 64);
    assert!(common::stderr(&o).contains("declared capabilities: deps, graph, owners"));

    // `kb tools` advertises project capabilities with how to call them.
    let text = stdout(&w.kb(&["tools", "--lines", "40"]));
    assert!(text.contains("route  deps  quiet (route: quiet → deps) · call: kb run deps [args] · synthetic dependency lookup"), "{text}");
}

#[test]
fn exec_tools_can_answer_built_in_requests() {
    let w = exec_world();
    let v = json(&w.kb(&["--json", "sym", "refresh"]));
    assert_eq!(v["ok"], true, "{v:#}");
    let source = &v["result"]["sources"][0];
    assert_eq!(source["id"], "symgrep");
    assert_eq!(
        source["version"], "symgrep 2.0",
        "version_args are used for the probe"
    );
    let items = items(&v);
    assert_eq!(items[0]["kind"], "definition");
    assert_eq!(items[0]["path"], "app/src/auth/TokenRefresher.kt");
    assert_eq!(items[0]["line"], 2);
    assert_eq!(items[0]["confidence"], "lexical");
    // A tool whose argv ignores the input cannot silently drop it.
    let w = World::new(
        "schema = 1\n[[tool]]\nid = \"st\"\nadapter = \"exec\"\nprogram = \"{kb}/fake/codegraph\"\nargs = [\"status\"]\ncapabilities = [\"status\"]\nversion_args = []\n",
    );
    let o = w.kb(&["run", "status", "extra"]);
    assert_eq!(code(&o), 65, "{}", stdout(&o));
    assert!(common::stderr(&o).contains("takes no input"));
    let o = w.kb(&["run", "status"]);
    assert_eq!(code(&o), 0, "{}", common::stderr(&o));
}

#[test]
fn exec_prepare_steps_run_first_and_plain_text_is_an_empty_answer() {
    let w = exec_world();
    // kb options may follow the capability; `--` passes tool arguments that start with `-`.
    let o = w.kb(&[
        "--json", "run", "graph", "known", "--lines", "5", "--", "--flag",
    ]);
    assert_eq!(code(&o), 0, "{}", common::stderr(&o));
    let v = json(&o);
    assert!(w.kb.join("fake/prepared").exists(), "the prepare step ran");
    assert_eq!(v["result"]["sources"][0]["state"], "working tree, prepared");
    assert_eq!(
        items(&v)[0]["detail"],
        "--flag",
        "native items need no mapping"
    );
    let o = w.kb(&["run", "graph", "unknown"]);
    assert_eq!(code(&o), 0);
    assert!(
        stdout(&o).contains("note: graph printed no JSON: Symbol not found"),
        "{}",
        stdout(&o)
    );
}

const SIMILAR_ONLY: &str = r#"[
  {"node": {"name": "ExternalClockA", "kind": "class", "filePath": "app/src/a/A.kt", "startLine": 1, "endLine": 2}},
  {"node": {"name": "ExternalClockB", "kind": "class", "filePath": "app/src/a/B.kt", "startLine": 1, "endLine": 2}},
  {"node": {"name": "ExternalClockC", "kind": "class", "filePath": "app/src/a/C.kt", "startLine": 1, "endLine": 2}},
  {"node": {"name": "ExternalClockD", "kind": "class", "filePath": "app/src/a/D.kt", "startLine": 1, "endLine": 2}},
  {"node": {"name": "ExternalClockE", "kind": "class", "filePath": "app/src/a/E.kt", "startLine": 1, "endLine": 2}}
]"#;

#[test]
fn a_symbol_declared_outside_the_checkout_is_answered_with_its_uses() {
    let w = World::new(TOOLS);
    write(&w.kb.join("fake/query.json"), SIMILAR_ONLY);
    write(
        &w.host.join("app/src/auth/Session.kt"),
        "import lib.ExternalClock\n\nclass Session {\n    val started = ExternalClock.now()\n}\n",
    );
    write(
        &w.host.join("app/src/auth/Expiry.kt"),
        "fun expired(at: Long) = ExternalClock.now() > at\n",
    );

    let o = w.kb(&["--json", "sym", "ExternalClock"]);
    assert_eq!(code(&o), 16);
    let v = json(&o);
    assert_eq!(v["error"]["code"], "NOT_FOUND");
    assert_eq!(
        v["error"]["message"],
        "no declaration of `ExternalClock` was found in this checkout"
    );
    // One use per file, the code line rather than the import, and no look-alike names.
    let uses = items(&v);
    let found: Vec<_> = uses
        .iter()
        .map(|i| (i["kind"].as_str(), i["path"].as_str(), i["line"].as_u64()))
        .collect();
    assert_eq!(
        found,
        [
            (Some("reference"), Some("app/src/auth/Expiry.kt"), Some(1)),
            (Some("reference"), Some("app/src/auth/Session.kt"), Some(4)),
        ],
        "{v:#}"
    );
    let note = v["result"]["notes"][0].as_str().unwrap();
    assert!(
        note.starts_with("`ExternalClock` is used in 2 file(s) and no line of the ")
            && note.ends_with(" file(s) of this checkout reads as its declaration"),
        "{note}"
    );
    let sources: Vec<_> = v["result"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(sources, ["codegraph", "git-grep"]);
    // The answer must not send the agent searching for the declaration again.
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("cite them"), "{hint}");
    assert!(
        hint.contains("in a dependency, in generated code"),
        "{hint}"
    );
    assert!(!hint.contains("try"), "{hint}");

    // A narrowed lookup did not cover the checkout, so it draws no such conclusion.
    let v = json(&w.kb(&["--json", "sym", "ExternalClock", "--file", "Session"]));
    assert_eq!(
        v["error"]["message"],
        "no definition of `ExternalClock` was found"
    );
    assert!(
        v["error"]["hint"].as_str().unwrap().contains("--file"),
        "{v:#}"
    );

    // A name that occurs nowhere keeps a few look-alikes for a misspelling, not all of them.
    let o = w.kb(&["--json", "sym", "ExternalCloc"]);
    assert_eq!(code(&o), 16);
    let v = json(&o);
    let similar = items(&v);
    assert_eq!(similar.len(), 3, "{v:#}");
    assert!(similar.iter().all(|i| i["kind"] == "similar"));
    let note = v["result"]["notes"][0].as_str().unwrap();
    assert!(
        note.starts_with("`ExternalCloc` does not occur in the "),
        "{note}"
    );
    assert_eq!(
        v["error"]["message"],
        "`ExternalCloc` does not occur in this checkout"
    );
}

#[test]
fn a_long_definition_shows_its_start_and_the_read_that_continues_it() {
    let w = World::new(TOOLS);
    let body: String = (1..=120).map(|n| format!("    // step {n}\n")).collect();
    write(
        &w.host.join("app/src/auth/TokenRefresher.kt"),
        &format!("class TokenRefresher {{\n{body}}}\n"),
    );
    write(
        &w.kb.join("fake/query.json"),
        r#"[{"node": {"name": "refresh", "kind": "method", "filePath": "app/src/auth/TokenRefresher.kt", "startLine": 10, "endLine": 100}}]"#,
    );
    let v = json(&w.kb(&["--json", "sym", "refresh"]));
    let excerpt = &v["result"]["excerpts"][0];
    assert_eq!(excerpt["first_line"], 7, "three lines above the definition");
    // One `read` window: a definition that fits it needs no second call.
    assert_eq!(excerpt["lines"].as_array().unwrap().len(), 80);
    assert_eq!(excerpt["next_line"], 87);
    assert_eq!(
        v["result"]["next"][0],
        "kb read app/src/auth/TokenRefresher.kt 87 100"
    );
    // The excerpt is extra to the bounded lines and each line is numbered as `read` does.
    let text = stdout(&w.kb(&["sym", "refresh", "--lines", "5"]));
    assert!(text.contains("\n7:    // step 6\n"), "{text}");
    assert!(text.contains("\n86:    // step 85\n"), "{text}");
    assert!(!text.contains("\n87:"), "{text}");
}

#[test]
fn a_few_definitions_of_one_name_show_the_head_of_each() {
    let w = World::new(TOOLS);
    let body: String = (1..=40).map(|n| format!("    // step {n}\n")).collect();
    write(
        &w.host.join("app/src/auth/Other.kt"),
        &format!("class Other {{\n{body}}}\n"),
    );
    write(
        &w.kb.join("fake/query.json"),
        r#"[
  {"node": {"name": "refresh", "kind": "method", "filePath": "app/src/auth/TokenRefresher.kt", "startLine": 2, "endLine": 4}},
  {"node": {"name": "refresh", "kind": "method", "filePath": "app/src/auth/Other.kt", "startLine": 5, "endLine": 40}}
]"#,
    );
    let v = json(&w.kb(&["--json", "sym", "refresh"]));
    assert_eq!(items(&v).len(), 2, "{v:#}");
    let excerpts = v["result"]["excerpts"].as_array().unwrap();
    assert_eq!(excerpts.len(), 2, "{v:#}");
    assert_eq!(excerpts[0]["path"], "app/src/auth/TokenRefresher.kt");
    assert_eq!(excerpts[0]["lines"].as_array().unwrap().len(), 4);
    assert_eq!(excerpts[1]["path"], "app/src/auth/Other.kt");
    assert_eq!(excerpts[1]["first_line"], 2);
    // The head of a long definition, not all of it.
    assert_eq!(excerpts[1]["lines"].as_array().unwrap().len(), 28);
    assert_eq!(
        v["result"]["next"],
        serde_json::json!(["kb callers refresh"])
    );
    // Four definitions share the excerpt lines: twenty each, none cut off.
    let four: Vec<String> = (0..4)
        .map(|n| {
            write(
                &w.host.join(format!("app/src/auth/Impl{n}.kt")),
                &format!("class Impl{n} {{\n{body}}}\n"),
            );
            format!(
                r#"{{"node": {{"name": "refresh", "kind": "method", "filePath": "app/src/auth/Impl{n}.kt", "startLine": 5, "endLine": 40}}}}"#
            )
        })
        .collect();
    write(
        &w.kb.join("fake/query.json"),
        &format!("[{}]", four.join(",")),
    );
    let many = json(&w.kb(&["--json", "sym", "refresh"]));
    let lengths: Vec<usize> = many["result"]["excerpts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["lines"].as_array().unwrap().len())
        .collect();
    assert_eq!(lengths, [20, 20, 20, 20], "{many:#}");
    write(
        &w.kb.join("fake/query.json"),
        r#"[
  {"node": {"name": "refresh", "kind": "method", "filePath": "app/src/auth/TokenRefresher.kt", "startLine": 2, "endLine": 4}},
  {"node": {"name": "refresh", "kind": "method", "filePath": "app/src/auth/Other.kt", "startLine": 5, "endLine": 40}}
]"#,
    );
    // Each excerpt is headed by its path.
    let text = stdout(&w.kb(&["sym", "refresh"]));
    assert!(
        text.contains("\napp/src/auth/TokenRefresher.kt\n1:class TokenRefresher {\n"),
        "{text}"
    );
    assert!(
        text.contains("\napp/src/auth/Other.kt\n2:    // step 1\n"),
        "{text}"
    );
}

#[test]
fn a_definition_whose_file_cannot_be_shown_points_at_one_read_of_its_span() {
    let w = World::new(TOOLS);
    write(
        &w.kb.join("fake/query.json"),
        r#"[{"node": {"name": "refresh", "kind": "method", "filePath": "app/src/auth/TokenRefresher.kt", "startLine": 40, "endLine": 71}}]"#,
    );
    let text = stdout(&w.kb(&["sym", "refresh"]));
    assert!(
        text.contains("next: kb read app/src/auth/TokenRefresher.kt 38 71 · kb callers refresh"),
        "{text}"
    );
    // A span longer than one excerpt starts at the definition and fills the window.
    write(
        &w.kb.join("fake/query.json"),
        r#"[{"node": {"name": "refresh", "kind": "method", "filePath": "app/src/auth/TokenRefresher.kt", "startLine": 40, "endLine": 400}}]"#,
    );
    let text = stdout(&w.kb(&["sym", "refresh"]));
    assert!(
        text.contains("next: kb read app/src/auth/TokenRefresher.kt 38 117 "),
        "{text}"
    );
}

#[test]
fn a_find_miss_names_the_files_it_covered() {
    let w = World::new(TOOLS);
    let message = |args: &[&str]| {
        let o = w.kb(&[&["--json", "find", "absentText"], args].concat());
        assert_eq!(code(&o), 16);
        json(&o)["error"]["message"].as_str().unwrap().to_string()
    };
    let everywhere = message(&[]);
    assert!(
        everywhere.starts_with("no match for \"absentText\" in the ")
            && everywhere.ends_with(" file(s) of this checkout"),
        "{everywhere}"
    );
    assert_eq!(
        message(&["--glob", "*Login*"]),
        "no match for \"absentText\" in the 1 file(s) the glob(s) select"
    );
    assert_eq!(
        message(&["--glob", "*.swift"]),
        "no match for \"absentText\": the glob(s) select no file"
    );
}

#[test]
fn a_name_that_occurs_nowhere_is_answered_with_the_lines_that_hold_its_words() {
    let w = World::new(TOOLS);
    // No line reads `tokenReturn`, but one holds both of its words.
    let o = w.kb(&["--json", "find", "tokenReturn"]);
    assert_eq!(code(&o), 0, "{}", common::stderr(&o));
    let v = json(&o);
    assert_eq!(items(&v).len(), 1, "{v:#}");
    assert_eq!(items(&v)[0]["detail"], "return Token()");
    assert_eq!(
        v["result"]["notes"][0],
        "no line reads \"tokenReturn\"; these lines hold its words (token, return)"
    );
    // A phrase is treated the same way, with two words or three.
    let v = json(&w.kb(&["--json", "find", "private refresher"]));
    assert_eq!(items(&v)[0]["path"], "app/src/auth/Login.kt");
    assert_eq!(
        v["result"]["notes"][0],
        "no line reads \"private refresher\"; these lines hold its words (private, refresher)"
    );
    let v = json(&w.kb(&["--json", "find", "class token refresher"]));
    assert!(
        items(&v)
            .iter()
            .any(|item| item["detail"] == "class TokenRefresher {"),
        "{v:#}"
    );
    // One word, or words no line shares, stay a miss.
    assert_eq!(code(&w.kb(&["find", "absenttext"])), 16);
    assert_eq!(code(&w.kb(&["find", "absentText"])), 16);
    assert_eq!(code(&w.kb(&["find", "tokenBilling"])), 16);
    // A regular expression is searched as written.
    assert_eq!(code(&w.kb(&["find", "tokenReturn", "--regex"])), 16);
}

#[test]
fn a_miss_inside_one_file_lists_its_declarations() {
    let w = World::new(TOOLS);
    let o = w.kb(&["--json", "find", "absentText", "--glob", "*Login*"]);
    assert_eq!(code(&o), 16);
    let v = json(&o);
    let found = items(&v);
    assert_eq!(found.len(), 2, "{v:#}");
    assert!(found.iter().all(|item| item["kind"] == "declaration"));
    assert_eq!(found[0]["line"], 1);
    assert_eq!(found[1]["detail"], "fun signIn() {");
    assert_eq!(
        v["result"]["notes"][0],
        "no line of the file reads \"absentText\"; its declarations follow"
    );
    // Several files are not outlined: the miss says how many were covered.
    let v = json(&w.kb(&["--json", "find", "absentText", "--glob", "*.kt"]));
    assert!(items(&v).is_empty(), "{v:#}");
}

#[test]
fn the_outline_of_a_file_holds_declarations_not_comments_or_prose() {
    let w = World::new(TOOLS);
    write(
        &w.host.join("app/src/auth/notes.py"),
        "# helper comment\nclass Helper:\n    pass\nRecord keeping is manual\ndef keep():\n    pass\n",
    );
    let o = w.kb(&["--json", "find", "absentText", "--glob", "*notes.py"]);
    assert_eq!(code(&o), 16);
    let v = json(&o);
    let lines: Vec<_> = items(&v)
        .iter()
        .map(|i| i["detail"].as_str().unwrap())
        .collect();
    assert_eq!(lines, ["class Helper:", "def keep():"], "{v:#}");
    // In a Markdown file the outline is its headings.
    write(
        &w.host.join("docs/guide.md"),
        "# Guide\n\nclass names are listed below\n\n## Usage\n",
    );
    let v = json(&w.kb(&["--json", "find", "absentText", "--glob", "*guide.md"]));
    let lines: Vec<_> = items(&v)
        .iter()
        .map(|i| i["detail"].as_str().unwrap())
        .collect();
    assert_eq!(lines, ["# Guide", "## Usage"], "{v:#}");
}

#[test]
fn a_final_miss_says_when_submodules_were_not_scanned() {
    let w = World::new(TOOLS);
    write(&w.kb.join("fake/query.json"), SIMILAR_ONLY);
    write(
        &w.host.join(".gitmodules"),
        "[submodule \"lib\"]\n\tpath = vendor/lib\n\turl = https://example.invalid/lib.git\n",
    );
    let o = w.kb(&["--json", "sym", "NoSuchNameAnywhere"]);
    assert_eq!(code(&o), 16);
    let v = json(&o);
    assert_eq!(
        v["result"]["notes"][1], "submodules of this checkout were not scanned",
        "{v:#}"
    );
    // The hint claims what the scan did, not that no other search could find it.
    assert!(
        v["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("as a whole word, as written"),
        "{v:#}"
    );
}

#[test]
fn text_declarations_cover_extension_receivers_and_several_on_one_line() {
    let w = World::new(TOOLS);
    write(
        &w.host.join("app/src/auth/Ext.kt"),
        "fun TokenRefresher.renewLater() {}\nclass Pair(val left: Int, val rightSide: String)\nfun <T : Limit> bounded() {}\n",
    );
    // The function is declared, the receiver and the bound are not.
    let v = json(&w.kb(&["--json", "sym", "renewLater", "--tool", "git-grep"]));
    assert_eq!(items(&v)[0]["line"], 1, "{v:#}");
    let v = json(&w.kb(&["--json", "sym", "rightSide", "--tool", "git-grep"]));
    assert_eq!(items(&v)[0]["line"], 2, "{v:#}");
    let o = w.kb(&["--json", "sym", "Limit", "--tool", "git-grep"]);
    assert_eq!(code(&o), 16, "{}", stdout(&o));
    assert_eq!(
        json(&o)["error"]["message"],
        "no declaration of `Limit` was found in this checkout"
    );
}

#[test]
fn a_member_is_found_where_it_is_declared_whatever_is_known_of_its_type() {
    let w = World::new(TOOLS);
    write(&w.kb.join("fake/query.json"), SIMILAR_ONLY);
    // An extension of a type from a dependency: the line that writes the dotted name is
    // its declaration.
    write(
        &w.host.join("app/src/auth/Extensions.kt"),
        "fun ExternalClock.elapsedSince(start: Long) = now() - start\n",
    );
    let o = w.kb(&["--json", "sym", "ExternalClock.elapsedSince"]);
    assert_eq!(code(&o), 0, "{}", stdout(&o));
    assert_eq!(items(&json(&o))[0]["path"], "app/src/auth/Extensions.kt");

    // A static method of a type declared here, used with the type's name: the dotted name
    // is not called undeclared, the answer names the lookup that finds the member.
    write(
        &w.host.join("app/src/auth/Vaults.java"),
        "class Vaults {\n    static Token unseal(String id) {\n        return new Token();\n    }\n}\n",
    );
    write(
        &w.host.join("app/src/auth/UseVaults.java"),
        "class UseVaults {\n    Token run() {\n        return Vaults.unseal(\"a\");\n    }\n}\n",
    );
    let o = w.kb(&["--json", "sym", "Vaults.unseal"]);
    assert_eq!(code(&o), 16);
    let v = json(&o);
    assert_eq!(
        v["error"]["message"],
        "no declaration is written as `Vaults.unseal`"
    );
    assert_eq!(v["result"]["next"], serde_json::json!(["kb sym unseal"]));
    let o = w.kb(&["--json", "sym", "unseal"]);
    assert_eq!(code(&o), 0, "{}", stdout(&o));
    assert_eq!(items(&json(&o))[0]["line"], 2);
}

#[test]
fn a_declaration_far_into_a_long_line_is_still_a_declaration() {
    let w = World::new(TOOLS);
    write(
        &w.host.join("app/src/auth/Wide.kt"),
        &format!("/* {} */ fun farDeclared() = 1\n", "x".repeat(700)),
    );
    let o = w.kb(&["--json", "sym", "farDeclared", "--tool", "git-grep"]);
    assert_eq!(code(&o), 0, "{}", stdout(&o));
    let v = json(&o);
    assert_eq!(items(&v)[0]["path"], "app/src/auth/Wide.kt");
    // The item keeps the start of the line only.
    assert!(items(&v)[0]["detail"].as_str().unwrap().chars().count() <= 500);
    // A keyword is not the name a declaration-shaped text asks for.
    let o = w.kb(&["--json", "find", "private fun("]);
    assert_eq!(code(&o), 16);
    assert!(
        json(&o)["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("no match for \"private fun(\" in the "),
    );
}

#[test]
fn a_glob_that_names_a_directory_is_searched_below_it() {
    let w = World::new(TOOLS);
    // `*auth*` matches no file name, but the sources live below a directory of that name.
    let o = w.kb(&["--json", "find", "return Token()", "--glob", "*auth*"]);
    assert_eq!(code(&o), 0, "{}", common::stderr(&o));
    let v = json(&o);
    assert_eq!(items(&v)[0]["path"], "app/src/auth/TokenRefresher.kt");
    assert_eq!(
        v["result"]["notes"][0],
        "the glob(s) select no file as written; searched below the directories of that name"
    );
    // A glob that is right but for its case finds the file.
    let v = json(&w.kb(&[
        "--json",
        "find",
        "return Token()",
        "--glob",
        "*tokenrefresher*",
    ]));
    assert_eq!(items(&v)[0]["path"], "app/src/auth/TokenRefresher.kt");
    assert_eq!(
        v["result"]["notes"][0],
        "the glob(s) select no file as written; searched without regard to case"
    );
    // A glob that names neither a file nor a directory keeps the miss.
    let o = w.kb(&["--json", "find", "return Token()", "--glob", "*billing*"]);
    assert_eq!(code(&o), 16);
    assert_eq!(
        json(&o)["error"]["message"],
        "no match for \"return Token()\": the glob(s) select no file"
    );
}

#[test]
fn doc_lists_the_records_that_cite_a_document_outside_their_scope() {
    let w = World::new(TOOLS);
    // The policy is scoped to the auth module; the guide it was written from is under docs/.
    let record =
        w.kb.join("project/knowledge/policies/mobile-token-storage.md");
    let text = fs::read_to_string(&record).unwrap().replacen(
        "\n+++\nTokens",
        "\n[[anchors]]\nkind = \"doc\"\nrepo = \"mobile\"\npath = \"docs/token-storage.md\"\n\n+++\nTokens",
        1,
    );
    write(&record, &text);
    write(
        &w.host.join("docs/token-storage.md"),
        "Token storage guide\n",
    );
    let doc = |path: &str, more: &[&str]| {
        let out = w.kb(&[
            &["--offline", "--snapshot", "working-tree", "doc", path],
            more,
        ]
        .concat());
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        stdout(&out)
    };

    let text = doc("docs/token-storage.md", &[]);
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines[1], "guide  kb read docs/token-storage.md", "{text}");
    assert_eq!(
        lines[2], "policy  acme.mobile.token-storage  Token storage on devices",
        "{text}"
    );
    assert!(
        text.contains("next: kb show acme.mobile.token-storage"),
        "{text}"
    );
    let full = doc("docs/token-storage.md", &["--format", "human"]);
    assert!(
        full.contains("acme.mobile.token-storage  Token storage on devices (cites this path)"),
        "{full}"
    );
    // A document no record cites gains nothing.
    write(&w.host.join("docs/other.md"), "Other guide\n");
    assert!(!doc("docs/other.md", &[]).contains("acme.mobile.token-storage"));
}

#[test]
fn show_locates_anchors_in_the_checkout_and_marks_absent_files() {
    let w = World::new(TOOLS);
    write(
        &w.host.join("app/src/auth/storage/TokenStore.kt"),
        "package auth.storage\n\nimport auth.TokenStore.Companion\n\n/** Keeps tokens. */\nclass TokenStore {\n    fun save(token: Token) {}\n}\n",
    );
    let show = |more: &[&str]| {
        let args = [
            "--offline",
            "--snapshot",
            "working-tree",
            "show",
            "acme.mobile.token-storage",
        ];
        w.kb(&[more, &args].concat())
    };
    let text = stdout(&show(&[]));
    assert!(
        text.contains("- source mobile:app/src/auth/storage/TokenStore.kt:6 symbol TokenStore\n"),
        "{text}"
    );
    assert!(
        text.contains("- test mobile:app/src/test/auth/TokenStoreTest.kt (not in this checkout)\n"),
        "{text}"
    );
    let v = json(&show(&["--json"]));
    assert_eq!(
        v["result"]["anchor_targets"],
        serde_json::json!([{"state": "found", "line": 6}, {"state": "missing"}]),
        "{v:#}"
    );
    // The stored record is untouched: its anchors carry no line or marker.
    assert_eq!(v["result"]["record"]["anchors"][0].get("line"), None);
}

#[test]
fn show_says_where_an_anchored_document_speaks_of_the_record() {
    let w = World::new(TOOLS);
    let record =
        w.kb.join("project/knowledge/policies/mobile-token-storage.md");
    let text = fs::read_to_string(&record).unwrap().replacen(
        "\n+++\nTokens",
        "\n[[anchors]]\nkind = \"doc\"\nrepo = \"mobile\"\npath = \"docs/token-storage.md\"\nnote = \"§ Sign-out\"\n\n[[anchors]]\nkind = \"change\"\nchange = \"!7\"\n\n+++\nTokens",
        1,
    );
    write(&record, &text);
    write(
        &w.host.join("docs/token-storage.md"),
        "# Token storage\n\nThe store is an EncryptedTokenStore.\n\n## Sign-out\n\nTokens are deleted.\n",
    );
    let show = |more: &[&str]| {
        let args = [
            "--offline",
            "--snapshot",
            "working-tree",
            "show",
            "acme.mobile.token-storage",
        ];
        w.kb(&[more, &args].concat())
    };
    let text = stdout(&show(&[]));
    // The section the anchor names, then the code name the record's text mentions.
    assert!(
        text.contains(
            "- doc mobile:docs/token-storage.md (§ Sign-out) [7 lines; § Sign-out at :5, EncryptedTokenStore at :3]\n"
        ),
        "{text}"
    );
    let v = json(&show(&["--json"]));
    assert_eq!(
        v["result"]["anchor_targets"][2],
        serde_json::json!({"state": "found", "lines": 7, "mentions": [
            {"text": "§ Sign-out", "line": 5}, {"text": "EncryptedTokenStore", "line": 3}]}),
        "{v:#}"
    );
    // One entry per anchor, in the record's order: an anchor without a path has none.
    assert_eq!(v["result"]["anchor_targets"].as_array().unwrap().len(), 4);
    assert_eq!(v["result"]["anchor_targets"][3], serde_json::Value::Null);
}

#[test]
fn search_lists_the_code_files_named_like_the_query() {
    let w = World::new(TOOLS);
    let search = |query: &str, more: &[&str]| {
        let args = ["--offline", "--snapshot", "working-tree", "search", query];
        w.kb(&[more, &args].concat())
    };
    let v = json(&search("token refresher", &["--json"]));
    assert_eq!(
        v["result"]["files"],
        serde_json::json!([{"path": "app/src/auth/TokenRefresher.kt", "matched": ["token", "refresher"],
            "line": 1, "declaration": "class TokenRefresher {"}]),
        "{v:#}"
    );
    assert_eq!(v["result"]["files_total"], 1);
    let text = stdout(&search("token refresher", &[]));
    assert!(
        text.ends_with(
            "code: 1 of 1 file(s) named like the query; kb sym <name> or kb read <path> opens one\napp/src/auth/TokenRefresher.kt:1  class TokenRefresher {\n"
        ),
        "{text}"
    );
    // No file is named like these words: the record hits stand alone.
    let v = json(&search("plaintext storage", &["--json"]));
    assert_eq!(v["result"]["files"], serde_json::json!([]));
    assert!(!stdout(&search("plaintext storage", &[])).contains("code:"));
}

#[test]
fn a_single_find_match_comes_with_its_surroundings() {
    let w = World::new(TOOLS);
    let v = json(&w.kb(&["--json", "find", "return Token()"]));
    assert_eq!(items(&v).len(), 1, "{v:#}");
    let excerpt = &v["result"]["excerpts"][0];
    assert_eq!(excerpt["path"], "app/src/auth/TokenRefresher.kt");
    assert_eq!(excerpt["first_line"], 1);
    assert_eq!(excerpt["lines"].as_array().unwrap().len(), 5);
    // Three lines above a single match and twenty below it.
    let mut source = vec!["// fixture".to_string(); 60];
    source[9] = "val only = beaconOnce()".into();
    write(&w.host.join("app/src/auth/Once.kt"), &source.join("\n"));
    let v = json(&w.kb(&["--json", "find", "beaconOnce()"]));
    let excerpt = &v["result"]["excerpts"][0];
    assert_eq!(excerpt["first_line"], 7);
    assert_eq!(excerpt["lines"].as_array().unwrap().len(), 24);
}

#[test]
fn an_excerpt_is_headed_by_its_path_unless_every_item_is_in_that_file() {
    let w = World::new(TOOLS);
    write(&w.host.join("app/src/auth/Uses.kt"), "import lib.Beacon\n");
    write(
        &w.host.join("app/src/auth/Impl.kt"),
        "val beacon = Beacon()\nval other = 1\n",
    );
    // Two matches in two files; the import line is listed and not shown, so the one
    // excerpt must say which file it is from.
    let v = json(&w.kb(&["--json", "find", "Beacon"]));
    assert_eq!(items(&v).len(), 2, "{v:#}");
    assert_eq!(v["result"]["excerpts"].as_array().unwrap().len(), 1);
    let text = stdout(&w.kb(&["find", "Beacon"]));
    assert!(
        text.contains("\napp/src/auth/Impl.kt\n1:val beacon = Beacon()\n"),
        "{text}"
    );
    // A single match needs no header: the item above the lines names the file.
    let text = stdout(&w.kb(&["find", "val other"]));
    assert!(
        text.contains("val other = 1\n1:val beacon = Beacon()\n"),
        "{text}"
    );
}

#[test]
fn lines_too_long_or_not_text_are_listed_without_an_excerpt() {
    let w = World::new(TOOLS);
    // One line of 40 KiB, as in a minified file: the match is listed, the line is not
    // carried again as an excerpt, in JSON either.
    write(
        &w.host.join("app/src/auth/min.js"),
        &format!("var a={};var beaconMinified=1;\n", "x".repeat(40 * 1024)),
    );
    let v = json(&w.kb(&["--json", "find", "beaconMinified"]));
    assert_eq!(items(&v).len(), 1, "{v:#}");
    assert!(v["result"].get("excerpts").is_none());
    // The item keeps the start of the line, not all of it.
    assert!(items(&v)[0]["detail"].as_str().unwrap().chars().count() <= 500);
    // A control character next to the match: the same.
    write(
        &w.host.join("app/src/auth/ctl.txt"),
        "beaconControl here\nnext\u{1}line\n",
    );
    let v = json(&w.kb(&["--json", "find", "beaconControl"]));
    assert_eq!(items(&v).len(), 1, "{v:#}");
    assert!(v["result"].get("excerpts").is_none(), "{v:#}");
}

#[test]
fn a_declaration_without_a_keyword_is_not_reported_as_missing() {
    let w = World::new(TOOLS);
    write(&w.kb.join("fake/query.json"), SIMILAR_ONLY);
    write(
        &w.host.join("app/src/auth/Service.java"),
        "class Service {\n    public Token rotateToken(String id) {\n        return new Token();\n    }\n}\n",
    );
    write(
        &w.host.join("app/src/auth/Client.java"),
        "class Client {\n    Token next(Service s) {\n        return s.rotateToken(\"a\");\n    }\n}\n",
    );
    // No tool resolves it and no keyword introduces it, but the line that could declare
    // it is the answer, marked as possible, with its lines.
    let o = w.kb(&["--json", "sym", "rotateToken"]);
    assert_eq!(code(&o), 0, "{}", common::stderr(&o));
    let v = json(&o);
    let found = items(&v);
    assert_eq!(found.len(), 1, "{v:#}");
    assert_eq!(found[0]["kind"], "definition");
    assert_eq!(found[0]["confidence"], "possible");
    assert_eq!(found[0]["path"], "app/src/auth/Service.java");
    assert_eq!(found[0]["line"], 2);
    assert_eq!(
        v["result"]["notes"][0],
        "no tool resolved `rotateToken`; each line shown could be its declaration"
    );
    assert_eq!(
        v["result"]["excerpts"][0]["path"],
        "app/src/auth/Service.java"
    );
    // Its callers are ordinary callers: nothing says the name is declared elsewhere.
    let v = json(&w.kb(&["--json", "callers", "rotateToken", "--tool", "git-grep"]));
    let notes = v["result"]["notes"].as_array().unwrap();
    assert!(
        notes
            .iter()
            .all(|n| !n.as_str().unwrap().contains("declaration of")),
        "{v:#}"
    );
    // A call alone is no declaration: the name stays a final miss with its uses.
    write(
        &w.host.join("app/src/auth/Caller.java"),
        "class Caller {\n    void run() {\n        return externalRotate(1);\n    }\n}\n",
    );
    let o = w.kb(&["--json", "sym", "externalRotate"]);
    assert_eq!(code(&o), 16);
    assert_eq!(
        json(&o)["error"]["message"],
        "no declaration of `externalRotate` was found in this checkout"
    );
}

#[test]
fn lexical_callers_of_a_member_keep_the_calls_on_an_instance() {
    let w = World::new(TOOLS);
    write(
        &w.host.join("app/src/auth/Vault.kt"),
        "class Vault {\n    fun seal(value: String) {}\n}\n// See Vault.seal for details.\n",
    );
    write(
        &w.host.join("app/src/auth/Keeper.kt"),
        "fun keep(vault: Vault) {\n    vault.seal(\"a\")\n}\n",
    );
    // The type is declared here, so every use of the member counts, not only the lines
    // that write the dotted name.
    let v = json(&w.kb(&["--json", "callers", "Vault.seal", "--tool", "git-grep"]));
    let paths: Vec<_> = items(&v)
        .iter()
        .map(|i| (i["path"].as_str().unwrap(), i["line"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        paths,
        [("app/src/auth/Keeper.kt", 2), ("app/src/auth/Vault.kt", 4)],
        "{v:#}"
    );
}

#[test]
fn a_few_find_matches_come_with_the_lines_around_each() {
    let w = World::new(TOOLS);
    let mut source = vec!["// fixture".to_string(); 70];
    source[9] = "val first = marker()".into();
    source[14] = "val second = marker()".into();
    source[59] = "val third = marker()".into();
    write(&w.host.join("app/src/auth/Marks.kt"), &source.join("\n"));
    let v = json(&w.kb(&["--json", "find", "marker()"]));
    assert_eq!(items(&v).len(), 3, "{v:#}");
    // Windows that touch are one excerpt; the distant match has its own.
    let excerpts = v["result"]["excerpts"].as_array().unwrap();
    assert_eq!(excerpts.len(), 2, "{v:#}");
    assert_eq!(excerpts[0]["first_line"], 7);
    assert_eq!(excerpts[0]["lines"].as_array().unwrap().len(), 17);
    assert_eq!(excerpts[1]["first_line"], 57);
    assert_eq!(excerpts[1]["lines"][3], "val third = marker()");
    // The path heads the excerpts of one file once.
    let text = stdout(&w.kb(&["find", "marker()"]));
    assert_eq!(
        text.matches("\napp/src/auth/Marks.kt\n").count(),
        1,
        "{text}"
    );
    assert!(text.contains("\n23:// fixture\n57:// fixture\n"), "{text}");

    // Many matches are a list to choose from, not places to read.
    // Four matches still come with their lines, five are a list to choose from.
    let four: Vec<String> = (0..4).map(|n| format!("val f{n} = fourth()")).collect();
    write(&w.host.join("app/src/auth/Four.kt"), &four.join("\n"));
    let v = json(&w.kb(&["--json", "find", "fourth()"]));
    assert_eq!(items(&v).len(), 4, "{v:#}");
    assert_eq!(v["result"]["excerpts"].as_array().unwrap().len(), 1);
    let many: Vec<String> = (0..5).map(|n| format!("val v{n} = other()")).collect();
    write(&w.host.join("app/src/auth/Many.kt"), &many.join("\n"));
    let v = json(&w.kb(&["--json", "find", "other()"]));
    assert_eq!(items(&v).len(), 5, "{v:#}");
    assert!(v["result"].get("excerpts").is_none(), "{v:#}");
}

#[test]
fn a_declaration_shaped_find_miss_is_answered_for_the_name() {
    let w = World::new(TOOLS);
    write(
        &w.host.join("app/src/auth/Session.kt"),
        "import lib.ExternalClock\n\nclass Session {\n    val started: ExternalClock = ExternalClock.now()\n}\n",
    );

    // Declared, but with other keywords: the declaration is the answer.
    let o = w.kb(&["--json", "find", "sealed interface TokenRefresher"]);
    assert_eq!(code(&o), 0, "{}", common::stderr(&o));
    let v = json(&o);
    let found = items(&v);
    assert_eq!(found.len(), 1, "{v:#}");
    assert_eq!(found[0]["kind"], "definition");
    assert_eq!(found[0]["path"], "app/src/auth/TokenRefresher.kt");
    assert_eq!(found[0]["line"], 1);
    assert_eq!(
        v["result"]["notes"][0],
        "no line reads \"sealed interface TokenRefresher\"; `TokenRefresher` is declared as shown"
    );
    assert_eq!(v["result"]["excerpts"][0]["first_line"], 1);
    // The text was searched only where the globs point; the note claims no more.
    let v = json(&w.kb(&[
        "--json",
        "find",
        "sealed interface TokenRefresher",
        "--glob",
        "*Login*",
    ]));
    assert_eq!(
        v["result"]["notes"][0],
        "no line of the files the glob(s) select reads \"sealed interface TokenRefresher\"; `TokenRefresher` is declared as shown"
    );
    // A modifier before a plain word is prose, not a declaration: it misses as a text.
    let o = w.kb(&["--json", "find", "open billing"]);
    assert_eq!(code(&o), 16);
    assert!(
        json(&o)["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("no match for \"open billing\" in the "),
    );

    // Used but declared nowhere, a typed property included: the uses, and a final miss.
    let o = w.kb(&["--json", "find", "object ExternalClock"]);
    assert_eq!(code(&o), 16);
    let v = json(&o);
    assert_eq!(
        v["error"]["message"],
        "no declaration of `ExternalClock` was found in this checkout"
    );
    let uses = items(&v);
    assert_eq!(uses.len(), 1, "{v:#}");
    assert_eq!(uses[0]["kind"], "reference");
    assert_eq!(uses[0]["line"], 4);

    // Nowhere at all: no keyword variant will find it either.
    let o = w.kb(&["--json", "find", "data class NoSuchThing("]);
    assert_eq!(code(&o), 16);
    assert_eq!(
        json(&o)["error"]["message"],
        "`NoSuchThing` does not occur in this checkout"
    );

    // Other text keeps the plain miss, and a regular expression is never reinterpreted.
    let o = w.kb(&["--json", "find", "returns ExternalClock"]);
    assert!(
        json(&o)["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("no match for \"returns ExternalClock\""),
    );
    let o = w.kb(&["--json", "find", "--regex", "class NoSuchThing"]);
    assert!(
        json(&o)["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("no match for"),
    );
}

#[test]
fn callers_and_sym_of_a_member_of_an_undeclared_type_do_not_send_the_agent_on() {
    let w = World::new(TOOLS);
    write(&w.kb.join("fake/query.json"), "[]");
    write(
        &w.kb.join("fake/callers.json"),
        r#"{"targets": [], "callers": []}"#,
    );
    write(
        &w.kb.join("fake/impact.json"),
        r#"{"targets": [], "affected": []}"#,
    );
    write(
        &w.host.join("app/src/auth/Handler.kt"),
        "fun reduce(action: Any) = when (action) {\n    is RemoteCommand.Begin -> 1\n    else -> 0\n}\n",
    );
    // Another type has a member of the same name; it is not the one asked for.
    write(
        &w.host.join("app/src/auth/LocalCommand.kt"),
        "sealed interface LocalCommand {\n    data class Begin(val at: Long) : LocalCommand\n    object Halt : LocalCommand\n}\nfun begin() = LocalCommand.Begin(0)\nfun end() = LocalCommand.Halt\n",
    );

    let o = w.kb(&["--json", "callers", "RemoteCommand.Begin"]);
    assert_eq!(code(&o), 0, "{}", common::stderr(&o));
    let v = json(&o);
    let lines: Vec<_> = items(&v)
        .iter()
        .map(|i| (i["path"].as_str().unwrap(), i["line"].as_u64().unwrap()))
        .collect();
    assert_eq!(lines, [("app/src/auth/Handler.kt", 2)], "{v:#}");
    assert_eq!(
        v["result"]["notes"][0],
        "no line of this checkout reads as a declaration of `RemoteCommand`; these lines are textual uses of `RemoteCommand.Begin` in it"
    );
    assert_eq!(v["result"]["next"], serde_json::json!([]), "{v:#}");

    let o = w.kb(&["--json", "sym", "RemoteCommand.Begin"]);
    assert_eq!(code(&o), 16, "the other type's member is not the answer");
    let v = json(&o);
    assert_eq!(
        v["error"]["message"],
        "no declaration of `RemoteCommand.Begin` was found in this checkout"
    );
    assert_eq!(items(&v)[0]["path"], "app/src/auth/Handler.kt");

    // A member that is written only for the other type is not a use of this one.
    // Nothing final is said: the member may be called on an instance, so the answer
    // points at the lookup by its own name.
    let o = w.kb(&["--json", "callers", "RemoteCommand.Halt"]);
    assert_eq!(code(&o), 16, "{}", stdout(&o));
    let v = json(&o);
    assert!(items(&v).is_empty(), "{v:#}");
    assert_eq!(v["error"]["message"], "no line writes `RemoteCommand.Halt`");
    assert_eq!(
        v["result"]["notes"][0],
        "no line of this checkout writes `RemoteCommand.Halt`; `Halt` occurs in it"
    );
    assert_eq!(
        v["result"]["next"],
        serde_json::json!(["kb sym Halt", "kb callers Halt"])
    );
    // A dotted name whose member occurs nowhere either is absent.
    let o = w.kb(&["--json", "callers", "RemoteCommand.Vanish"]);
    assert_eq!(code(&o), 16);
    assert_eq!(
        json(&o)["error"]["message"],
        "`RemoteCommand.Vanish` does not occur in this checkout"
    );

    // A type that is declared here keeps the ordinary lexical answer.
    let o = w.kb(&["--json", "sym", "LocalCommand.Begin"]);
    assert_eq!(code(&o), 0, "{}", common::stderr(&o));
    assert_eq!(items(&json(&o))[0]["path"], "app/src/auth/LocalCommand.kt");

    // A symbol that occurs nowhere is a final miss for callers and impact as well.
    for command in ["callers", "impact"] {
        let o = w.kb(&["--json", command, "NoSuchSymbol"]);
        assert_eq!(code(&o), 16);
        assert_eq!(
            json(&o)["error"]["message"],
            "`NoSuchSymbol` does not occur in this checkout"
        );
    }
}

#[test]
fn a_long_path_does_not_cut_the_matched_line() {
    let w = World::new(TOOLS);
    let dir = "app/src/main/kotlin/com/example/product/feature/billing/presentation/summary/panels/overview/components";
    let call = "val renewedLease = leaseRenewer.refreshWithBackoff(attempts = 3, initialDelayMillis = 250)";
    write(
        &w.host.join(format!("{dir}/BillingSummaryPanel.kt")),
        &format!("fun panel() {{\n    {call}\n}}\nfun other() = leaseRenewer\n"),
    );
    let text = stdout(&w.kb(&["find", "leaseRenewer"]));
    // The item line holds the whole matched line, not only the excerpt below the items.
    let item = text
        .lines()
        .find(|line| line.starts_with("match") && line.contains("SummaryPanel.kt:2"))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(item.contains(call), "{text}");
}
