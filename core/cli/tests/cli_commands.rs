//! End-to-end tests of the knowledge-reading and checking commands through the real `kb`
//! executable: `context`, `search`, `show`, `index`, `validate`, `impact` and `doctor`.
//!
//! Each test builds its own world in a temporary directory: a KB repository pushed to a
//! local bare `origin` (file transport only), and a host repository that mounts the KB as a
//! submodule at `.kb` and whose remote URL matches the registry repo `mobile`. Nothing is
//! ever fetched from the host remote, and no test touches the network.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::{Sandbox, json, repo_root, stderr, stdout, write, write_min_project};
use serde_json::Value;

const SESSION_REFRESH: &str = "project/knowledge/invariants/session-refresh.md";
/// No trailing newline on purpose: `show --raw` must print the file bytes unchanged.
const SESSION_REFRESH_TEXT: &str = r#"+++
schema = 1
id = "acme.mobile.session-refresh"
kind = "invariant"
title = "Session refresh keeps one refresh in flight"
status = "accepted"
owner = "team-mobile"

[scope]
modules = ["mobile.auth"]

[links]
requires = ["acme.contract.session-api"]

[[statements]]
id = "single-flight"
level = "must"
text = "Serialize refresh calls so that at most one refresh request is in flight."
+++
Why this matters for the mobile client.

## Rationale

Parallel refresh calls race on the rotated refresh token and log the user out."#;

const SESSION_API: &str = r#"+++
schema = 1
id = "acme.contract.session-api"
kind = "contract"
title = "Session API between backend and mobile"
status = "accepted"
owner = "arch"

[scope]
repos = ["mobile", "backend"]
modules = ["backend.api"]

[[parties]]
id = "provider"
repo = "backend"
modules = ["backend.api"]
role = "Serves the session endpoints"

[[parties]]
id = "consumer"
repo = "mobile"
role = "Calls the session endpoints"

[[obligations]]
id = "idempotent-refresh"
party = "provider"
level = "must"
text = "Answer a repeated refresh with the same token pair for 30 seconds."
+++
"#;

const GLOSSARY: &str = r#"+++
schema = 1
id = "acme.reference.token-glossary"
kind = "reference"
title = "Token glossary"
status = "accepted"
owner = "arch"
summary = "Terms used for authentication tokens."

[scope]
product = true

[[sources]]
title = "Internal wiki"
+++
"#;

const ROUTING: &str = r#"schema = 1

[[case]]
name = "mobile auth path pulls the cross-repo contract through requires"
intent = "implement"
repos = ["mobile"]
paths = ["app/auth/Token.kt"]
expect_mandatory = [
  "acme.mobile.token-storage",
  "acme.contract.token-api",
  "acme.mobile.session-refresh",
  "acme.contract.session-api",
]
expect_status = "complete"
"#;

const MOBILE_PATH: &str = "app/auth/Token.kt";

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The shared scenario of every test.
struct World {
    sb: Sandbox,
    origin: PathBuf,
    host: PathBuf,
    /// The KB checkout mounted in the host (`<host>/.kb`), canonical.
    kb: PathBuf,
    /// The approved commit the host pins.
    c1: String,
}

impl World {
    fn new() -> World {
        let sb = Sandbox::new();
        let origin = sb.path().join("origin.git");
        sb.init_bare(&origin);
        let seed = sb.path().join("seed");
        sb.init_repo(&seed);
        write_min_project(&seed);
        write(&seed.join(".gitignore"), ".cache/\n");
        write(&seed.join(SESSION_REFRESH), SESSION_REFRESH_TEXT);
        write(
            &seed.join("project/knowledge/contracts/session-api.md"),
            SESSION_API,
        );
        write(
            &seed.join("project/knowledge/references/token-glossary.md"),
            GLOSSARY,
        );
        write(&seed.join("project/routing-tests/mobile.toml"), ROUTING);
        let c1 = sb.commit_all(&seed, "initial knowledge");
        sb.git(&seed, &["push", "-q", s(&origin), "main"]);

        let host = sb.path().join("host");
        sb.init_repo(&host);
        write(&host.join(MOBILE_PATH), "class Token\n");
        write(&host.join("docs/notes.txt"), "notes\n");
        sb.git(
            &host,
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/acme/mobile.git",
            ],
        );
        sb.commit_all(&host, "host code");
        sb.git(&host, &["submodule", "add", "-q", s(&origin), ".kb"]);
        sb.commit_all(&host, "add kb submodule");
        let kb = host.join(".kb").canonicalize().unwrap();
        World {
            sb,
            origin,
            host,
            kb,
            c1,
        }
    }

    /// Run kb from the host root with `--root <host>/.kb`.
    fn kb(&self, args: &[&str]) -> Output {
        self.kb_in(&self.host, args, &[])
    }

    fn kb_in(&self, cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut all = vec!["--root", s(&self.kb)];
        all.extend_from_slice(args);
        self.sb.kb(cwd, &all, envs)
    }

    /// Another developer pushes `files` to the approved branch; returns the new tip.
    fn push_approved(&self, name: &str, files: &[(&str, &str)]) -> String {
        let dev = self.sb.path().join(format!("dev-{name}"));
        self.sb
            .git(&self.sb.path(), &["clone", "-q", s(&self.origin), s(&dev)]);
        for (rel, text) in files {
            write(&dev.join(rel), text);
        }
        let c = self.sb.commit_all(&dev, name);
        self.sb.git(&dev, &["push", "-q", "origin", "main"]);
        c
    }

    /// Everything a reading command must leave untouched in a checkout.
    fn checkout_state(&self, dir: &Path) -> String {
        let mut out = String::new();
        let queries: [&[&str]; 8] = [
            &["rev-parse", "HEAD"],
            &["symbolic-ref", "-q", "HEAD"],
            &["for-each-ref", "--format=%(refname) %(objectname)"],
            &["ls-files", "--stage"],
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ],
            &["diff", "HEAD", "--no-ext-diff"],
            &["stash", "list"],
            &["worktree", "list", "--porcelain"],
        ];
        for args in queries {
            let o = self.sb.git_output(dir, args);
            out.push_str(&format!(
                "$ git {args:?} -> {:?}\n{}\n",
                o.status.code(),
                String::from_utf8_lossy(&o.stdout)
            ));
        }
        let untracked = self
            .sb
            .git(dir, &["ls-files", "--others", "--exclude-standard"]);
        for f in untracked.lines().filter(|l| !l.is_empty()) {
            let bytes = fs::read(dir.join(f)).unwrap_or_default();
            out.push_str(&format!(
                "untracked {f}: {}\n",
                String::from_utf8_lossy(&bytes)
            ));
        }
        out
    }
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn assert_exit(o: &Output, expected: i32) {
    assert_eq!(
        code(o),
        expected,
        "unexpected exit code\nstdout:\n{}\nstderr:\n{}",
        stdout(o),
        stderr(o)
    );
}

/// Record ids of the units of a context result in the given tiers.
fn unit_ids(result: &Value, tiers: &[&str]) -> Vec<String> {
    result["units"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|u| tiers.contains(&u["tier"].as_str().unwrap()))
        .map(|u| u["record"].as_str().unwrap().to_string())
        .collect()
}

/// The result part of a context envelope (for comparisons across runs).
fn context_json(w: &World, cwd: &Path, args: &[&str]) -> (Output, Value) {
    let mut all = vec!["--json", "context", "--intent", "implement"];
    all.extend_from_slice(args);
    let o = w.kb_in(cwd, &all, &[]);
    let v = json(&o);
    (o, v)
}

const STATEMENT_NONE: &str = "Refactoring of the token class.\n\n<!-- kb-impact:v1\n\
kb_change = \"none\"\nreason = \"Behavior is unchanged; the session invariant still holds.\"\n-->\n";

#[test]
fn context_envelope_complete_and_partial() {
    let w = World::new();
    let (o, v) = context_json(&w, &w.host, &["--path", MOBILE_PATH, "--explain"]);
    assert_exit(&o, 0);
    // Machine mode: stdout is exactly one protocol document (json() rejects trailing data);
    // progress is on stderr only.
    assert!(!stdout(&o).contains("kb: "), "{}", stdout(&o));
    assert!(stderr(&o).contains("kb: snapshot "), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("kb: index: built snapshot"),
        "{}",
        stderr(&o)
    );
    assert_eq!(v["protocol"], "kb.cli.v1");
    assert_eq!(v["command"], "context");
    assert_eq!(v["ok"], true);
    assert_eq!(v["error"], Value::Null);
    assert_eq!(v["meta"]["index"]["reused"], false);
    let r = &v["result"];
    assert_eq!(r["completeness"], "complete", "{r:#}");
    let snap = &r["snapshot"];
    assert_eq!(snap["freshness"], "verified");
    assert_eq!(snap["selection"], "latest");
    assert_eq!(snap["revision"], w.c1.as_str());
    assert_eq!(snap["latest_approved"], w.c1.as_str());
    assert_eq!(snap["approved"], true);
    assert_eq!(snap["pin"]["revision"], w.c1.as_str());
    assert_eq!(r["host"]["repo"], "mobile");
    assert_eq!(r["host"]["repo_source"], "remote");
    assert_eq!(
        unit_ids(r, &["mandatory"]),
        [
            "acme.mobile.token-storage",
            "acme.mobile.session-refresh",
            "acme.contract.token-api"
        ]
    );
    // The backend-scoped contract is outside the task scope but reachable through
    // `requires`: it stays mandatory (dependency tier).
    assert_eq!(unit_ids(r, &["dependency"]), ["acme.contract.session-api"]);
    let receipt = r["receipt"]["id"].as_str().unwrap();
    assert!(receipt.starts_with("sha256:"), "{receipt}");
    assert_eq!(kb::context::receipt_id_of(r), receipt);

    // Same inputs, same snapshot: identical result; the index is reused.
    let (o2, v2) = context_json(&w, &w.host, &["--path", MOBILE_PATH, "--explain"]);
    assert_exit(&o2, 0);
    assert_eq!(v2["result"], v["result"]);
    assert_eq!(v2["meta"]["index"]["reused"], true);
    assert!(stderr(&o2).contains("kb: index: reused snapshot"));

    // A cwd-relative path from a host subdirectory is converted to a host-relative one.
    let (o3, v3) = context_json(&w, &w.host.join("app"), &["--path", "auth/Token.kt"]);
    assert_exit(&o3, 0);
    assert_eq!(v3["result"]["request"]["paths"][0], MOBILE_PATH);
    // Paths escaping the host root are rejected.
    let (o4, v4) = context_json(&w, &w.host.join("app"), &["--path", "../../outside.kt"]);
    assert_exit(&o4, 63);
    assert_eq!(v4["error"]["code"], "UNSAFE_PATH");

    // Without --path the module-scoped invariant is undetermined: partial, exit 30, and the
    // full result is still printed.
    let (o5, v5) = context_json(&w, &w.host, &[]);
    assert_exit(&o5, 30);
    assert_eq!(v5["ok"], false);
    assert_eq!(v5["error"]["code"], "CONTEXT_INCOMPLETE");
    assert_eq!(v5["error"]["exit_code"], 30);
    assert_eq!(v5["result"]["completeness"], "partial");
    let undetermined: Vec<&str> = v5["result"]["undetermined"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["id"].as_str().unwrap())
        .collect();
    assert_eq!(undetermined, ["acme.mobile.session-refresh"]);

    // Text mode prints the context on stdout and the error on stderr.
    let o6 = w.kb(&["context", "--intent", "implement"]);
    assert_exit(&o6, 30);
    assert!(stdout(&o6).contains("status: PARTIAL"), "{}", stdout(&o6));
    assert!(stderr(&o6).contains("error[CONTEXT_INCOMPLETE]"));
}

#[test]
fn offline_and_remote_failure() {
    let w = World::new();
    let (o, v) = context_json(&w, &w.host, &["--offline", "--path", MOBILE_PATH]);
    // Offline: the last known approved revision (remote-tracking ref), freshness unverified,
    // which makes the context at best partial.
    assert_exit(&o, 30);
    let r = &v["result"];
    assert_eq!(r["snapshot"]["freshness"], "unverified");
    assert_eq!(r["snapshot"]["revision"], w.c1.as_str());
    assert_eq!(r["completeness"], "partial");
    assert!(!stderr(&o).contains("checking refs/heads/main"));

    // The approved remote is unreachable: no fallback to offline.
    let moved = w.sb.path().join("origin-moved.git");
    fs::rename(&w.origin, &moved).unwrap();
    for args in [
        vec![
            "--json",
            "context",
            "--intent",
            "implement",
            "--path",
            MOBILE_PATH,
        ],
        vec!["--json", "search", "token"],
        vec!["--json", "show", "acme.mobile.token-storage"],
    ] {
        let o = w.kb(&args);
        assert_exit(&o, 20);
        let v = json(&o);
        assert_eq!(v["error"]["code"], "FRESHNESS_UNVERIFIED");
        assert_eq!(v["result"], Value::Null);
    }
    // Explicit offline still works from the last fetched state.
    let o = w.kb(&["--offline", "show", "acme.mobile.token-storage"]);
    assert_exit(&o, 0);
    fs::rename(&moved, &w.origin).unwrap();
}

#[test]
fn pinned_host_behind_approved_requires_explicit_selection() {
    let w = World::new();
    let before_host = w.checkout_state(&w.host);
    let before_kb = w.checkout_state(&w.kb);
    let updated = SESSION_REFRESH_TEXT.replace(
        "at most one refresh request is in flight.",
        "at most one refresh request is in flight per account.",
    );
    let c2 = w.push_approved("v2", &[(SESSION_REFRESH, &updated)]);

    let (o, v) = context_json(&w, &w.host, &["--path", MOBILE_PATH]);
    assert_exit(&o, 21);
    assert_eq!(v["error"]["code"], "UPDATE_REQUIRED");
    assert_eq!(v["error"]["details"]["pin"]["revision"], w.c1.as_str());
    assert_eq!(v["error"]["details"]["latest_approved"], c2.as_str());

    let statement = |v: &Value| -> String {
        v["result"]["units"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["record"] == "acme.mobile.session-refresh")
            .map(|u| {
                u["content"]["statements"][0]["text"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .unwrap()
    };
    // Pinned: the host's revision, still reporting the approved tip (never called latest).
    let (o, v) = context_json(
        &w,
        &w.host,
        &["--snapshot", "pinned", "--path", MOBILE_PATH],
    );
    assert_exit(&o, 0);
    let snap = &v["result"]["snapshot"];
    assert_eq!(snap["selection"], "pinned");
    assert_eq!(snap["revision"], w.c1.as_str());
    assert_eq!(snap["latest_approved"], c2.as_str());
    assert_eq!(snap["freshness"], "verified");
    assert!(!statement(&v).contains("per account"));

    // Latest: the new approved tip, read from the isolated mirror.
    let (o, v) = context_json(
        &w,
        &w.host,
        &["--snapshot", "latest", "--path", MOBILE_PATH],
    );
    assert_exit(&o, 0);
    assert_eq!(v["result"]["snapshot"]["selection"], "latest");
    assert_eq!(v["result"]["snapshot"]["revision"], c2.as_str());
    assert!(statement(&v).contains("per account"));

    // A host binding may make `pinned` the default for `auto`.
    write(
        &w.host.join(".kbw.toml"),
        "schema = 1\nselection = \"pinned\"\n",
    );
    let (o, v) = context_json(&w, &w.host, &["--path", MOBILE_PATH]);
    assert_exit(&o, 0);
    assert_eq!(v["result"]["snapshot"]["selection"], "pinned");
    fs::remove_file(w.host.join(".kbw.toml")).unwrap();

    // doctor reports the stale pin as a warning, not a failure.
    let o = w.kb(&["--json", "doctor", "--online"]);
    assert_exit(&o, 0);
    let checks = json(&o)["result"]["checks"].clone();
    let pin = checks
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "host-pin")
        .unwrap()
        .clone();
    assert_eq!(pin["status"], "warn", "{pin:#}");
    assert!(pin["message"].as_str().unwrap().contains("behind"));

    // Reading never moved the host pin or the KB checkout.
    assert_eq!(w.checkout_state(&w.host), before_host);
    assert_eq!(w.checkout_state(&w.kb), before_kb);
}

#[test]
fn include_proposals_labels_local_edits_without_overriding() {
    let w = World::new();
    let storage = w.kb.join("project/knowledge/policies/token-storage.md");
    let original = fs::read_to_string(&storage).unwrap();
    fs::write(
        &storage,
        original.replace("in plaintext storage.", "anywhere (DRAFT)."),
    )
    .unwrap();
    write(
        &w.kb.join("project/knowledge/invariants/new-rule.md"),
        &SESSION_REFRESH_TEXT
            .replace("acme.mobile.session-refresh", "acme.mobile.new-rule")
            .replace("Session refresh keeps", "Proposed: session refresh keeps"),
    );

    // Registry changes cannot be applied as proposals; that is reported, not hidden.
    let concepts = w.kb.join("project/registry/concepts.toml");
    let text = fs::read_to_string(&concepts).unwrap();
    fs::write(
        &concepts,
        text.replace("\"token\", ", "\"token\", \"jwt\", "),
    )
    .unwrap();

    let rule_text = |v: &Value| -> String {
        v["result"]["units"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["tier"] == "mandatory" && u["record"] == "acme.mobile.token-storage")
            .map(|u| {
                u["content"]["rules"][0]["text"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .unwrap()
    };

    let (_, plain) = context_json(&w, &w.host, &["--path", MOBILE_PATH]);
    assert!(unit_ids(&plain["result"], &["proposal"]).is_empty());
    assert_eq!(plain["result"]["snapshot"]["overlay"], Value::Null);

    let (o, v) = context_json(&w, &w.host, &["--include-proposals", "--path", MOBILE_PATH]);
    assert!(matches!(code(&o), 0 | 30), "{}", stderr(&o));
    let r = &v["result"];
    assert_eq!(r["snapshot"]["overlay"]["files"], 3, "{:#}", r["snapshot"]);
    let notes: Vec<&str> = v["meta"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert!(notes.contains(&"PROPOSAL_NOT_APPLIED"), "{:#}", v["meta"]);
    assert!(stderr(&o).contains("PROPOSAL_NOT_APPLIED"));
    let mut proposals = unit_ids(r, &["proposal"]);
    proposals.sort();
    assert_eq!(
        proposals,
        ["acme.mobile.new-rule", "acme.mobile.token-storage"]
    );
    for u in r["units"].as_array().unwrap() {
        if u["tier"] == "proposal" {
            assert_eq!(u["origin"], "proposal");
            assert!(!u["labels"].as_array().unwrap().is_empty(), "{u:#}");
        }
    }
    // The accepted rule stays authoritative; proposals are never mandatory.
    assert_eq!(rule_text(&v), "Store refresh tokens in plaintext storage.");
    assert!(
        !unit_ids(r, &["mandatory", "dependency"]).contains(&"acme.mobile.new-rule".to_string())
    );

    // show can list the local proposal of an accepted record without applying it.
    let o = w.kb(&[
        "--json",
        "show",
        "acme.mobile.token-storage",
        "--include-proposals",
    ]);
    assert_exit(&o, 0);
    let shown = json(&o);
    assert_eq!(shown["result"]["origin"], "accepted");
    assert!(shown.to_string().contains("modifies"), "{shown:#}");
}

#[test]
fn search_and_show() {
    let w = World::new();
    let o = w.kb(&["--json", "search", "refresh token"]);
    assert_exit(&o, 0);
    let v = json(&o);
    assert_eq!(v["result"]["context_assembly"], false);
    let ids: Vec<&str> = v["result"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"acme.mobile.token-storage"), "{ids:?}");
    assert_eq!(v["result"]["snapshot"]["freshness"], "verified");

    let o = w.kb(&["--json", "search", "refresh", "--kind", "contract"]);
    assert_exit(&o, 0);
    for h in json(&o)["result"]["hits"].as_array().unwrap() {
        assert_eq!(h["kind"], "contract");
    }
    let o = w.kb(&["--json", "search", "refresh", "--kind", "bogus"]);
    assert_exit(&o, 64);
    assert_eq!(json(&o)["error"]["code"], "INVALID_INPUT");

    let o = w.kb(&["show", "acme.mobile.session-refresh"]);
    assert_exit(&o, 0);
    assert!(stdout(&o).contains("MUST [single-flight] Serialize refresh calls"));

    let o = w.kb(&[
        "show",
        "acme.mobile.session-refresh",
        "--section",
        "rationale",
    ]);
    assert_exit(&o, 0);
    assert!(stdout(&o).contains("Parallel refresh calls race"));
    assert!(!stdout(&o).contains("single-flight"));

    // --raw: the authoritative bytes of the approved revision, unchanged (the file has no
    // trailing newline).
    let o = w.kb(&["show", "acme.mobile.session-refresh", "--raw"]);
    assert_exit(&o, 0);
    assert_eq!(o.stdout, SESSION_REFRESH_TEXT.as_bytes());

    let o = w.kb(&["--json", "show", "acme.mobile.missing"]);
    assert_exit(&o, 16);
    assert_eq!(json(&o)["error"]["code"], "NOT_FOUND");
    let o = w.kb(&["show", "acme.mobile.session-refresh#nope"]);
    assert_exit(&o, 16);
}

#[test]
fn index_reuses_warm_snapshots_and_collects_old_ones() {
    let w = World::new();
    let index = |args: &[&str]| {
        let mut all = vec!["--json", "index"];
        all.extend_from_slice(args);
        let o = w.kb(&all);
        assert_exit(&o, 0);
        json(&o)["result"].clone()
    };
    let first = index(&[]);
    assert_eq!(first["build"]["reused"], false);
    // The result is deterministic: the machine-specific cache path and the file size
    // (which varies with SQLite page reuse) are metadata.
    assert_eq!(first["index"].get("path"), None, "{first:#}");
    assert_eq!(first["index"].get("bytes"), None, "{first:#}");
    let o = w.kb(&["--json", "index"]);
    assert_exit(&o, 0);
    let v = json(&o);
    assert!(
        v["meta"]["cache"]["path"]
            .as_str()
            .unwrap()
            .ends_with("project.sqlite"),
        "{:#}",
        v["meta"]
    );
    assert!(v["meta"]["cache"]["bytes"].as_u64().unwrap() > 0);
    assert_eq!(index(&[]), v["result"], "warm runs give identical results");
    assert_eq!(first["build"]["files"], 5);
    assert_eq!(first["freshness_verified"], false);
    assert_eq!(first["snapshot"]["freshness"], "unverified");
    assert!(
        first["note"]
            .as_str()
            .unwrap()
            .contains("freshness is not verified")
    );
    let second = index(&[]);
    assert_eq!(second["build"]["reused"], true);
    assert_eq!(second["build"]["parsed"], 0);
    assert_eq!(
        second["build"]["snapshot_key"],
        first["build"]["snapshot_key"]
    );

    // Working-tree snapshots: content ids are file digests (not Git blob ids), so the
    // first one parses every file; a second edit re-parses only the changed file.
    let glossary = w.kb.join("project/knowledge/references/token-glossary.md");
    fs::write(
        &glossary,
        GLOSSARY.replace("Token glossary", "Token glossary v2"),
    )
    .unwrap();
    let wt1 = index(&["--snapshot", "working-tree"]);
    assert_eq!(wt1["snapshot"]["selection"], "working-tree");
    assert_eq!(wt1["build"]["reused"], false);
    fs::write(
        &glossary,
        GLOSSARY.replace("Token glossary", "Token glossary v3"),
    )
    .unwrap();
    let wt2 = index(&["--snapshot", "working-tree"]);
    assert_eq!(wt2["build"]["reused"], false);
    assert_eq!(wt2["build"]["parsed"], 1);
    assert_eq!(wt2["build"]["reused_docs"], 4);
    assert_eq!(wt2["index"]["snapshots"].as_array().unwrap().len(), 3);

    // --gc keeps the selected (newest) snapshot and removes the older ones.
    let gc = index(&["--snapshot", "working-tree", "--gc"]);
    assert_eq!(gc["build"]["reused"], true);
    assert_eq!(gc["gc"]["removed"], 2);
    assert_eq!(gc["index"]["snapshots"].as_array().unwrap().len(), 1);
    assert_eq!(
        gc["index"]["snapshots"][0]["key"],
        wt2["build"]["snapshot_key"]
    );
    let again = index(&[]);
    assert_eq!(
        again["build"]["reused"], false,
        "collected snapshot is rebuilt"
    );

    let rebuilt = w.kb(&["--json", "index", "--rebuild"]);
    assert_exit(&rebuilt, 0);
    let v = json(&rebuilt);
    assert_eq!(v["result"]["build"]["reused"], false);
    // Recovery (here: the requested rebuild) is never silent.
    assert!(
        stderr(&rebuilt).contains("INDEX_REBUILT"),
        "{}",
        stderr(&rebuilt)
    );
    assert_eq!(v["meta"]["diagnostics"][0]["code"], "INDEX_REBUILT");

    // A damaged cache file is moved aside and rebuilt, and the reading command says so.
    let db = w.kb.join(".cache/index/project.sqlite");
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(db.with_file_name(format!("project.sqlite{suffix}")));
    }
    fs::write(
        &db,
        b"this is not a database, just garbage bytes of some length..",
    )
    .unwrap();
    let o = w.kb(&["--json", "--quiet", "search", "token"]);
    assert_exit(&o, 0);
    let v = json(&o);
    assert_eq!(
        v["meta"]["diagnostics"][0]["code"], "INDEX_RECOVERED",
        "{:#}",
        v["meta"]
    );
    assert!(
        stderr(&o).contains("warning[INDEX_RECOVERED]"),
        "shown even with --quiet: {}",
        stderr(&o)
    );
    assert!(!v["result"]["hits"].as_array().unwrap().is_empty());
}

#[test]
fn validate_reports_rules_routing_and_base() {
    let w = World::new();
    let work = w.sb.path().join("work");
    w.sb.git(&w.sb.path(), &["clone", "-q", s(&w.origin), s(&work)]);
    let validate = |args: &[&str]| {
        let mut all = vec!["--root", s(&work), "--json", "validate"];
        all.extend_from_slice(args);
        let o = w.sb.kb(&work, &all, &[]);
        let v = json(&o);
        (o, v)
    };
    let (o, v) = validate(&[]);
    assert_exit(&o, 0);
    assert_eq!(v["result"]["source"]["selection"], "working-tree");
    assert_eq!(v["result"]["validation"]["records"], 5);
    assert_eq!(v["result"]["validation"]["errors"], 0);
    assert_eq!(v["result"]["routing"]["passed"], 1);
    assert_eq!(v["result"]["routing"]["failed"], 0);

    // A routing case that forbids a mandatory record fails: ROUTING_TESTS_FAILED.
    let routing = work.join("project/routing-tests/mobile.toml");
    fs::write(
        &routing,
        ROUTING.replace(
            "expect_status = \"complete\"",
            "expect_status = \"complete\"\nforbid = [\"acme.contract.token-api\"]",
        ),
    )
    .unwrap();
    let (o, v) = validate(&[]);
    assert_exit(&o, 41);
    assert_eq!(v["error"]["code"], "ROUTING_TESTS_FAILED");
    assert_eq!(v["result"]["routing"]["failed"], 1);
    let (o, _) = validate(&["--no-routing"]);
    assert_exit(&o, 0);

    // A dangling link is a validation error, which takes precedence over routing failures.
    write(
        &work.join("project/knowledge/invariants/dangling.md"),
        &SESSION_REFRESH_TEXT
            .replace("acme.mobile.session-refresh", "acme.mobile.dangling")
            .replace("acme.contract.session-api", "acme.contract.missing"),
    );
    let (o, v) = validate(&[]);
    assert_exit(&o, 40);
    assert_eq!(v["error"]["code"], "VALIDATION_FAILED");
    let codes: Vec<&str> = v["result"]["validation"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"DANGLING_LINK"), "{codes:?}");
    fs::remove_file(work.join("project/knowledge/invariants/dangling.md")).unwrap();
    fs::write(&routing, ROUTING).unwrap();

    // Warnings fail only under --strict.
    write(
        &work.join("project/knowledge/invariants/loud.md"),
        &SESSION_REFRESH_TEXT
            .replace("acme.mobile.session-refresh", "acme.mobile.loud")
            .replace(
                "log the user out.",
                "log the user out, so clients MUST retry.",
            ),
    );
    let (o, v) = validate(&[]);
    assert_exit(&o, 0);
    assert!(v["result"]["validation"]["warnings"].as_u64().unwrap() > 0);
    let (o, _) = validate(&["--strict"]);
    assert_exit(&o, 40);
    fs::remove_file(work.join("project/knowledge/invariants/loud.md")).unwrap();

    // Historical ids stay addressable: deleting a record is ID_REMOVED against the base.
    fs::remove_file(work.join("project/knowledge/references/token-glossary.md")).unwrap();
    let (o, _) = validate(&[]);
    assert_exit(&o, 0);
    let (o, v) = validate(&["--base", "HEAD"]);
    assert_exit(&o, 40);
    let removed: Vec<&Value> = v["result"]["validation"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "ID_REMOVED")
        .collect();
    assert_eq!(removed.len(), 1, "{:#}", v["result"]);
    assert_eq!(removed[0]["record"], "acme.reference.token-glossary");
    let (o, v) = validate(&["--base", "no-such-rev"]);
    assert_exit(&o, 64);
    assert_eq!(v["error"]["code"], "INVALID_INPUT");

    // An explicit snapshot validates that revision (the approved tip), not the work tree.
    let (o, v) = validate(&["--snapshot", "latest"]);
    assert_exit(&o, 0);
    assert_eq!(v["result"]["source"]["selection"], "latest");
    assert_eq!(v["result"]["source"]["freshness"], "verified");
    assert_eq!(v["result"]["validation"]["records"], 5);
}

#[test]
fn clean_upstream_templates_and_maintainer_profile() {
    let sb = Sandbox::new();
    let root = repo_root();
    let cache = sb.path().join("cache");
    let envs = [("KB_CACHE_DIR", s(&cache))];
    let run = |args: &[&str]| {
        let mut all = vec!["--root", s(&root), "--json"];
        all.extend_from_slice(args);
        let o = sb.kb(&sb.path(), &all, &envs);
        let v = json(&o);
        (o, v)
    };
    // A clean upstream passes the template checks; the missing project is reported as info.
    let (o, v) = run(&["validate", "--templates"]);
    assert_exit(&o, 0);
    let diags = v["result"]["validation"]["diagnostics"].as_array().unwrap();
    assert!(
        diags
            .iter()
            .any(|d| d["code"] == "PROJECT_NOT_INITIALIZED" && d["severity"] == "info"),
        "{diags:#?}"
    );
    assert_eq!(v["result"]["validation"]["errors"], 0);
    assert_eq!(v["result"]["templates"], true);
    // Text output says that the templates were checked, too.
    let text = |format: &str| {
        let o = sb.kb(
            &sb.path(),
            &[
                "--root",
                s(&root),
                "--format",
                format,
                "validate",
                "--templates",
            ],
            &envs,
        );
        assert_exit(&o, 0);
        stdout(&o)
    };
    assert!(
        text("compact").contains(
            "templates: checked (shipped templates and examples): 0 error(s), 0 warning(s)"
        ),
        "{}",
        text("compact")
    );
    assert!(
        text("human").contains("Shipped templates and examples were validated (--templates)"),
        "{}",
        text("human")
    );

    // The maintainer profile validates with its own routing tests.
    let (o, v) = run(&["--profile", "maintainer", "validate"]);
    assert_exit(&o, 0);
    assert_eq!(v["result"]["validation"]["profile"], "maintainer");
    assert!(v["result"]["routing"]["passed"].as_u64().unwrap() > 0);
    assert_eq!(v["result"]["routing"]["failed"], 0);

    // Project knowledge is not available before initialization.
    let (o, v) = run(&["validate"]);
    assert_exit(&o, 10);
    assert_eq!(v["error"]["code"], "PROJECT_NOT_INITIALIZED");
    let (o, v) = run(&["context", "--intent", "explain"]);
    assert_exit(&o, 10);
    assert_eq!(v["error"]["code"], "PROJECT_NOT_INITIALIZED");
    assert!(
        !cache.exists(),
        "nothing is cached for an uninitialized project"
    );
}

#[test]
fn impact_links_changes_and_checks_the_statement() {
    let w = World::new();
    let dir = w.sb.path();
    let o = w.kb(&["--json", "impact"]);
    assert_exit(&o, 2);
    assert_eq!(json(&o)["error"]["code"], "USAGE");

    write(
        &w.host.join(MOBILE_PATH),
        "class Token { fun refresh() {} }\n",
    );
    write(&w.host.join("docs/notes.txt"), "changed notes\n");

    let o = w.kb(&["--json", "impact", "--working-tree"]);
    assert_exit(&o, 0);
    let v = json(&o);
    let r = &v["result"];
    assert_eq!(r["repo"], "mobile");
    assert_eq!(r["snapshot"]["freshness"], "verified");
    let affected: Vec<&str> = r["affected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert!(affected.contains(&"acme.mobile.session-refresh"), "{r:#}");
    let unknown: Vec<&str> = r["unknown_coverage"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["path"].as_str().unwrap())
        .collect();
    assert_eq!(unknown, ["docs/notes.txt"]);
    assert_eq!(r["check"], Value::Null);

    // --check needs an acknowledgement.
    let o = w.kb(&["--json", "impact", "--working-tree", "--check"]);
    assert_exit(&o, 44);
    let v = json(&o);
    assert_eq!(v["error"]["code"], "IMPACT_UNACKNOWLEDGED");
    assert_eq!(v["result"]["check"]["ok"], false);

    let good = dir.join("mr-good.md");
    write(&good, STATEMENT_NONE);
    let o = w.kb(&[
        "--json",
        "impact",
        "--working-tree",
        "--check",
        "--statement",
        s(&good),
    ]);
    assert_exit(&o, 0);
    let v = json(&o);
    assert_eq!(v["result"]["check"]["ok"], true);
    assert_eq!(v["result"]["check"]["reasoning_evaluated"], false);

    // `included` contradicts an unchanged KB pointer.
    let included = dir.join("mr-included.md");
    write(
        &included,
        &STATEMENT_NONE.replace("kb_change = \"none\"", "kb_change = \"included\""),
    );
    let o = w.kb(&[
        "impact",
        "--working-tree",
        "--check",
        "--statement",
        s(&included),
    ]);
    assert_exit(&o, 44);

    // A malformed block fails the gate under --check and is invalid input otherwise.
    let bad = dir.join("mr-bad.md");
    write(
        &bad,
        &STATEMENT_NONE.replace("kb_change = \"none\"", "kb_change = \"maybe\""),
    );
    let o = w.kb(&[
        "--json",
        "impact",
        "--working-tree",
        "--check",
        "--statement",
        s(&bad),
    ]);
    assert_exit(&o, 44);
    assert_eq!(json(&o)["error"]["code"], "IMPACT_UNACKNOWLEDGED");
    let o = w.kb(&["--json", "impact", "--working-tree", "--statement", s(&bad)]);
    assert_exit(&o, 64);

    // Committed changes against a base revision.
    let base = w.sb.git(&w.host, &["rev-parse", "HEAD"]);
    w.sb.commit_all(&w.host, "change token");
    let o = w.kb(&["--json", "impact", "--base", &base]);
    assert_exit(&o, 0);
    let v = json(&o);
    assert_eq!(v["result"]["diff"]["base"], base.as_str());
    assert_eq!(v["result"]["summary"]["files"], 2);
}

#[test]
fn doctor_reports_healthy_and_uninitialized_setups() {
    let w = World::new();
    let fp = "ab".repeat(32);
    write(
        &w.kb.join(format!(".cache/runtime/{fp}/BUILD-INFO")),
        &format!(
            "engine_version={}\nfingerprint={fp}\ntarget=test-target\nsource=source-build\n",
            kb::versions::ENGINE_VERSION
        ),
    );
    let o = w.kb_in(&w.host, &["--json", "doctor"], &[("KBW_FINGERPRINT", &fp)]);
    assert_exit(&o, 0);
    let v = json(&o);
    let checks = v["result"]["checks"].as_array().unwrap();
    let ids: Vec<&str> = checks.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        [
            "git",
            "kb-root",
            "runtime",
            "profile",
            "knowledge",
            "source",
            "freshness",
            "host",
            "host-repo",
            "host-pin",
            "snapshot-engine",
            "index",
            "skill",
            "bundle",
            "engine"
        ]
    );
    let status = |id: &str| {
        checks
            .iter()
            .find(|c| c["id"] == id)
            .map(|c| c["status"].as_str().unwrap().to_string())
            .unwrap()
    };
    assert_eq!(v["result"]["summary"]["fail"], 0, "{:#}", v["result"]);
    for id in [
        "git",
        "runtime",
        "profile",
        "knowledge",
        "source",
        "host",
        "host-repo",
        "host-pin",
        "snapshot-engine",
    ] {
        assert_eq!(status(id), "ok", "{id}: {:#}", v["result"]);
    }
    assert_eq!(status("freshness"), "skip");
    let runtime = checks.iter().find(|c| c["id"] == "runtime").unwrap();
    assert_eq!(runtime["details"]["source"], "source-build");

    let o = w.kb(&["--json", "doctor", "--online"]);
    assert_exit(&o, 0);
    let v = json(&o);
    let fresh = v["result"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "freshness")
        .unwrap()
        .clone();
    assert_eq!(fresh["status"], "ok", "{fresh:#}");

    // Integration and engine checks once they are configured: an outdated installed skill
    // is a warning that tells agents to reload their instructions.
    write(
        &w.kb.join("project/skill-config/skill.toml"),
        "schema = 1\nharnesses = [\"claude\"]\nkb_path = \".kb\"\n",
    );
    write(
        &w.host.join(".kbw/integration.lock"),
        "schema = 1\nskill_protocol = 0\nengine_version = \"0.0.1\"\nharnesses = [\"claude\"]\n",
    );
    write(
        &w.kb.join("project/upstream.toml"),
        &format!("schema = 1\nrevision = \"{}\"\n", w.c1),
    );
    let o = w.kb(&["--json", "doctor"]);
    assert_exit(&o, 0);
    let v = json(&o);
    let check = |id: &str| {
        v["result"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap()
            .clone()
    };
    let skill = check("skill");
    assert_eq!(skill["status"], "warn", "{skill:#}");
    assert!(
        skill["message"]
            .as_str()
            .unwrap()
            .contains("re-read the skill"),
        "{skill:#}"
    );
    assert_eq!(skill["details"]["state"], "outdated");
    // This test KB has no canonical skill sources, so the bundle cannot be rendered.
    assert_eq!(check("bundle")["status"], "warn");
    let engine = check("engine");
    assert_eq!(engine["status"], "ok", "{engine:#}");
    assert_eq!(engine["details"]["revision"], w.c1.as_str());

    // An uninitialized KB fails the profile check (exit 40).
    let bare = w.sb.path().join("uninitialized");
    w.sb.init_repo(&bare);
    common::copy_manifest(&bare);
    let o =
        w.sb.kb(&bare, &["--root", s(&bare), "--json", "doctor"], &[]);
    assert_exit(&o, 40);
    let v = json(&o);
    assert_eq!(v["error"]["code"], "VALIDATION_FAILED");
    let profile = v["result"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "profile")
        .unwrap()
        .clone();
    assert_eq!(profile["status"], "fail");
    assert!(
        profile["message"]
            .as_str()
            .unwrap()
            .contains("PROJECT_NOT_INITIALIZED")
    );
}

#[test]
fn host_binding_with_an_unknown_repo_is_reported() {
    let w = World::new();
    write(&w.host.join(".kbw.toml"), "schema = 1\nrepo = \"nope\"\n");
    let (o, v) = context_json(&w, &w.host, &["--path", MOBILE_PATH]);
    assert_exit(&o, 0);
    // The remote still identifies the host; the bad binding is a warning on stderr/meta.
    assert_eq!(v["result"]["host"]["repo"], "mobile");
    assert_eq!(
        v["meta"]["diagnostics"][0]["code"],
        "HOST_BINDING_REPO_UNKNOWN"
    );
    assert!(stderr(&o).contains("HOST_BINDING_REPO_UNKNOWN"));
    let o = w.kb(&["--json", "doctor"]);
    assert_exit(&o, 0);
    let v = json(&o);
    let check = v["result"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "host-repo")
        .unwrap()
        .clone();
    assert_eq!(check["status"], "warn");

    // Without a matching remote the engine reports the unknown repo in the result itself.
    w.sb.git(&w.host, &["remote", "remove", "origin"]);
    let (o, v) = context_json(&w, &w.host, &["--path", MOBILE_PATH]);
    assert_exit(&o, 30);
    let codes: Vec<&str> = v["result"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"HOST_REPO_UNKNOWN"), "{:#}", v["result"]);
}

#[test]
fn usage_and_protocol_errors() {
    let w = World::new();
    let o = w.kb(&["--no-such-flag", "version"]);
    assert_exit(&o, 2);
    let o = w.kb(&["context", "--intent", "implement", "--bogus"]);
    assert_exit(&o, 2);
    let o = w.kb(&["context"]);
    assert_exit(&o, 2);
    let o = w.kb(&[
        "--json",
        "--skill-protocol",
        "99",
        "context",
        "--intent",
        "implement",
    ]);
    assert_exit(&o, 23);
    let v = json(&o);
    assert_eq!(v["error"]["code"], "SKILL_OUTDATED");
    assert_eq!(v["ok"], false);
    let current = kb::versions::SKILL_PROTOCOL.to_string();
    let o = w.kb(&["--json", "--skill-protocol", &current, "search", "token"]);
    assert_exit(&o, 0);
    let o = w.kb(&["--json", "--skill-protocol", "1", "search", "token"]);
    assert_exit(&o, 23);
}

#[test]
fn read_commands_leave_checkouts_untouched() {
    let w = World::new();
    // Local, uncommitted knowledge work must survive every read.
    let storage = w.kb.join("project/knowledge/policies/token-storage.md");
    let text = fs::read_to_string(&storage).unwrap();
    fs::write(&storage, text.replace("plaintext", "plain-text")).unwrap();
    write(
        &w.kb.join("project/knowledge/invariants/draft.md"),
        &SESSION_REFRESH_TEXT.replace("acme.mobile.session-refresh", "acme.mobile.draft"),
    );
    write(&w.host.join(MOBILE_PATH), "class Token // local edit\n");
    let statement = w.sb.path().join("mr.md");
    write(&statement, STATEMENT_NONE);
    let before_host = w.checkout_state(&w.host);
    let before_kb = w.checkout_state(&w.kb);
    let gitlink = w.sb.git(&w.host, &["ls-tree", "HEAD", ".kb"]);

    let runs: Vec<Vec<&str>> = vec![
        vec!["context", "--intent", "implement", "--path", MOBILE_PATH],
        vec![
            "context",
            "--intent",
            "review",
            "--include-proposals",
            "--explain",
        ],
        vec![
            "--offline",
            "context",
            "--intent",
            "debug",
            "--task",
            "token refresh",
        ],
        vec![
            "--snapshot",
            "working-tree",
            "--offline",
            "context",
            "--intent",
            "explain",
        ],
        vec!["search", "token", "--include-proposals"],
        vec!["show", "acme.mobile.token-storage", "--raw"],
        vec!["index"],
        vec!["index", "--gc"],
        vec!["validate", "--base", "HEAD"],
        vec!["impact", "--working-tree", "--statement", s(&statement)],
        vec!["doctor", "--online"],
        vec!["sync"],
    ];
    for args in runs {
        let o = w.kb(&args);
        assert!(
            matches!(code(&o), 0 | 30),
            "{args:?} exited {:?}: {}",
            o.status.code(),
            stderr(&o)
        );
        assert_eq!(
            w.checkout_state(&w.host),
            before_host,
            "host changed by {args:?}"
        );
        assert_eq!(w.checkout_state(&w.kb), before_kb, "KB changed by {args:?}");
    }
    assert_eq!(w.sb.git(&w.host, &["ls-tree", "HEAD", ".kb"]), gitlink);
}

/// A revision as text output abbreviates it (12 characters).
fn short(rev: &str) -> &str {
    &rev[..12]
}

#[test]
fn text_output_states_the_snapshot() {
    let w = World::new();
    let updated = SESSION_REFRESH_TEXT.replace(
        "at most one refresh request is in flight.",
        "at most one refresh request is in flight per account.",
    );
    let c2 = w.push_approved("v2", &[(SESSION_REFRESH, &updated)]);
    let id = "acme.mobile.session-refresh";

    // Pinned (verified): the pinned revision and the newer approved tip, on stdout even
    // with --quiet; the knowledge is the pinned one.
    let o = w.kb(&["-q", "--snapshot", "pinned", "show", id]);
    assert_exit(&o, 0);
    let out = stdout(&o);
    assert_eq!(
        out.lines().next().unwrap(),
        format!(
            "snapshot: {} (pinned, freshness=verified); approved=yes; latest={}; pin={}",
            short(&w.c1),
            short(&c2),
            short(&w.c1)
        )
    );
    assert!(out.contains("MUST [single-flight]"), "{out}");
    assert!(!out.contains("per account"), "{out}");

    // Offline: the revision and the unverified freshness, in both text formats.
    let offline = format!("snapshot: {} (pinned, freshness=unverified)", short(&w.c1));
    for format in ["compact", "human"] {
        for args in [
            vec!["show", id],
            vec!["search", "token"],
            vec!["impact", "--working-tree"],
        ] {
            let mut all = vec![
                "-q",
                "--offline",
                "--snapshot",
                "pinned",
                "--format",
                format,
            ];
            all.extend_from_slice(&args);
            let o = w.kb(&all);
            assert_exit(&o, 0);
            assert!(stdout(&o).starts_with(&offline), "{all:?}:\n{}", stdout(&o));
        }
    }

    // Working-tree content is never presented as approved.
    let o = w.kb(&["-q", "--offline", "--snapshot", "working-tree", "show", id]);
    assert_exit(&o, 0);
    assert!(
        stdout(&o).starts_with(
            "snapshot: working-tree (working-tree, freshness=unverified); approved=no"
        ),
        "{}",
        stdout(&o)
    );

    // --raw stays byte-exact; unverified content is flagged on stderr, even with --quiet.
    let o = w.kb(&[
        "-q",
        "--offline",
        "--snapshot",
        "pinned",
        "show",
        id,
        "--raw",
    ]);
    assert_exit(&o, 0);
    assert_eq!(o.stdout, SESSION_REFRESH_TEXT.as_bytes());
    assert!(
        stderr(&o).contains(&format!("kb: {offline}")),
        "{}",
        stderr(&o)
    );
    // Verified, approved content needs no such note.
    let o = w.kb(&["-q", "--snapshot", "latest", "show", id, "--raw"]);
    assert_exit(&o, 0);
    assert_eq!(o.stdout, updated.as_bytes());
    assert!(!stderr(&o).contains("snapshot"), "{}", stderr(&o));

    // JSON keeps the provenance in the result only.
    let o = w.kb(&["--json", "--offline", "--snapshot", "pinned", "show", id]);
    assert_exit(&o, 0);
    let v = json(&o);
    assert_eq!(v["result"]["snapshot"]["freshness"], "unverified");
    assert_eq!(v["result"]["snapshot"]["latest_approved"], c2.as_str());
}

#[cfg(unix)]
#[test]
fn absolute_paths_through_a_symlinked_host_directory() {
    let w = World::new();
    let link = w.sb.path().join("host-link");
    std::os::unix::fs::symlink(&w.host, &link).unwrap();
    // Run from the symlinked spelling with absolute paths through it (the file itself
    // need not exist yet).
    for file in [MOBILE_PATH, "app/auth/NotYetWritten.kt"] {
        let abs = link.join(file);
        let (o, v) = context_json(&w, &link, &["--path", s(&abs)]);
        assert_exit(&o, 0);
        assert_eq!(v["result"]["request"]["paths"][0], file, "{v:#}");
    }
    // A symlinked directory that leaves the host is still outside.
    let elsewhere = w.sb.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let away = w.sb.path().join("away-link");
    std::os::unix::fs::symlink(&elsewhere, &away).unwrap();
    let (o, v) = context_json(&w, &w.host, &["--path", s(&away.join("x.kt"))]);
    assert_exit(&o, 63);
    assert_eq!(v["error"]["code"], "UNSAFE_PATH");
}

#[test]
fn doctor_checks_snapshot_engines_source_and_pin_selection() {
    let w = World::new();
    // The approved ref moves to a revision that needs another engine.
    let manifest = fs::read_to_string(w.kb.join("core/release.toml")).unwrap();
    let from = format!("engine_version = \"{}\"", kb::versions::ENGINE_VERSION);
    assert!(manifest.contains(&from));
    let c2 = w.push_approved(
        "engine-bump",
        &[(
            "core/release.toml",
            &manifest.replace(&from, "engine_version = \"99.0.0\""),
        )],
    );
    let check = |v: &Value, id: &str| -> Value {
        v["result"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("no check {id}: {v:#}"))
            .clone()
    };
    let text = |c: &Value, key: &str| c[key].as_str().unwrap_or("").to_string();

    // --offline and --online contradict each other.
    let o = w.kb(&["--json", "--offline", "doctor", "--online"]);
    assert_exit(&o, 64);
    assert_eq!(json(&o)["error"]["code"], "INVALID_INPUT");
    assert!(
        !stderr(&o).contains("checking refs/heads/main"),
        "{}",
        stderr(&o)
    );

    // Online: the source check uses the fetched tip, like the freshness check.
    let o = w.kb(&["--json", "doctor", "--online"]);
    assert_exit(&o, 0);
    let v = json(&o);
    let source = check(&v, "source");
    assert_eq!(
        source["details"]["latest_approved"],
        c2.as_str(),
        "{source:#}"
    );
    assert_eq!(source["details"]["local"]["behind"], 1, "{source:#}");
    assert!(text(&source, "message").contains(&format!("approved tip {}", short(&c2))));
    // No `.kbw.toml selection`: auto fails on the stale pin; the incompatible tip is a
    // warning, and the pin hint does not recommend reading it.
    let engine = check(&v, "snapshot-engine");
    assert_eq!(engine["status"], "warn", "{engine:#}");
    assert!(
        text(&engine, "message").contains("needs another engine"),
        "{engine:#}"
    );
    assert!(text(&engine, "message").contains("99.0.0"), "{engine:#}");
    let pin = check(&v, "host-pin");
    assert!(
        text(&pin, "message").contains("returns UPDATE_REQUIRED"),
        "{pin:#}"
    );
    assert!(!text(&pin, "hint").contains("--snapshot latest"), "{pin:#}");

    // `selection = "latest"`: auto reads the incompatible tip, so every read fails.
    write(
        &w.host.join(".kbw.toml"),
        "schema = 1\nselection = \"latest\"\n",
    );
    let (o, v) = context_json(&w, &w.host, &["--path", MOBILE_PATH]);
    assert_exit(&o, 21);
    assert_eq!(v["error"]["code"], "UPDATE_REQUIRED");
    let o = w.kb(&["--json", "doctor"]);
    assert_exit(&o, 40);
    let v = json(&o);
    let engine = check(&v, "snapshot-engine");
    assert_eq!(engine["status"], "fail", "{engine:#}");
    assert!(text(&engine, "message").contains(&short(&c2).to_string()));
    let pin = check(&v, "host-pin");
    assert_eq!(pin["status"], "warn");
    assert!(
        !text(&pin, "message").contains("UPDATE_REQUIRED"),
        "{pin:#}"
    );
    assert!(
        text(&pin, "message").contains("selection = \"latest\""),
        "{pin:#}"
    );

    // `selection = "pinned"`: auto reads the compatible pin and succeeds.
    write(
        &w.host.join(".kbw.toml"),
        "schema = 1\nselection = \"pinned\"\n",
    );
    let (o, _) = context_json(&w, &w.host, &["--path", MOBILE_PATH]);
    assert_exit(&o, 0);
    let o = w.kb(&["--json", "doctor"]);
    assert_exit(&o, 0);
    let v = json(&o);
    assert_eq!(check(&v, "snapshot-engine")["status"], "warn");
    let pin = check(&v, "host-pin");
    assert!(
        !text(&pin, "message").contains("UPDATE_REQUIRED"),
        "{pin:#}"
    );
    assert!(text(&pin, "message").contains("reads the pin"), "{pin:#}");
}
