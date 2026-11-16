//! The agent commands (`sym`, `callers`, `impact <symbol>`, `doc`, `find`, `check`,
//! `tools`): routing through the tool registry, fallbacks, bounded answers that name their
//! source, configured checks and the knowledge lookup. A shell script stands in for
//! CodeGraph (synthetic JSON, labeled); no test needs an installed code tool or the network.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::{Sandbox, json, stdout, write};
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
        text.contains("definition  TokenRefresher::refresh  app/src/auth/TokenRefresher.kt:2-4")
    );
    assert!(text.lines().count() <= 30);
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
