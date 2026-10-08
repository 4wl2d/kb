//! `kb init` and `kb integrate` through the real executable (temporary KB roots and host
//! directories), plus consistency checks of the shipped templates, skills and the synthetic
//! example.
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use common::{Sandbox, json, repo_root, write};
use kb::corpus::load_corpus;
use kb::integrate::template::{Vars, render_dir};
use kb::model::routing::{RoutingCase, RoutingTestFile};
use kb::model::{Kind, Profile, ProfileLocation, Registry, Scope, Status};
use kb::source::WorkingTreeSource;
use kb::validate::{MetaInput, validate_metas};

const EXAMPLE: &str = "core/templates/examples/synthetic-multirepo/project";

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for e in fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let ft = e.file_type().unwrap();
        let to = dst.join(e.file_name());
        if ft.is_dir() {
            copy_tree(&e.path(), &to);
        } else if ft.is_file() {
            fs::copy(e.path(), &to).unwrap();
        }
    }
}

/// Every regular file under `dir` (relative path → bytes).
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd {
            let e = e.unwrap();
            let p = e.path();
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                walk(root, &p, out);
            } else if ft.is_file() {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                out.insert(rel, fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// A temporary upstream-like KB root: engine manifest, templates, skills and the
/// uninitialized `project/README.md`.
struct Kb {
    sb: Sandbox,
    root: PathBuf,
}

impl Kb {
    fn new() -> Kb {
        let sb = Sandbox::new();
        let root = sb.path().join("kb");
        common::copy_manifest(&root);
        copy_tree(
            &repo_root().join("core/templates"),
            &root.join("core/templates"),
        );
        copy_tree(&repo_root().join("core/skills"), &root.join("core/skills"));
        fs::create_dir_all(root.join("project")).unwrap();
        fs::copy(
            repo_root().join("project/README.md"),
            root.join("project/README.md"),
        )
        .unwrap();
        Kb { sb, root }
    }

    fn run_in(&self, cwd: &Path, args: &[&str]) -> Output {
        let mut all = vec!["--root", self.root.to_str().unwrap(), "--json"];
        all.extend_from_slice(args);
        self.sb.kb(cwd, &all, &[])
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_in(&self.sb.path(), args)
    }

    fn init(&self) {
        let o = self.run(&[
            "init",
            "--name",
            "Acme Mobile",
            "--namespace",
            "acme",
            "--apply",
        ]);
        assert_ok(&o);
    }

    fn host(&self, name: &str) -> PathBuf {
        let h = self.sb.path().join(name);
        fs::create_dir_all(&h).unwrap();
        h
    }

    fn integrate(&self, host: &Path, extra: &[&str]) -> Output {
        let mut args = vec!["integrate", "--host", host.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    fn skill_toml(&self) -> PathBuf {
        self.root.join("project/skill-config/skill.toml")
    }

    fn set_skill(&self, harnesses: &str, notes: Option<&str>) {
        let mut s = format!("schema = 1\nharnesses = {harnesses}\nkb_path = \".kb\"\n");
        if let Some(n) = notes {
            s.push_str(&format!("notes = \"{n}\"\n"));
        }
        write(&self.skill_toml(), &s);
    }

    fn regenerate(&self) {
        assert_ok(&self.run(&["integrate", "--generate", "--apply"]));
    }
}

fn assert_ok(o: &Output) {
    assert!(
        o.status.success(),
        "exit {:?}\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        common::stdout(o),
        common::stderr(o)
    );
}

fn assert_code(o: &Output, code: &str, exit: i32) {
    let v = json(o);
    assert_eq!(v["error"]["code"], code, "{v:#}");
    assert_eq!(o.status.code(), Some(exit), "{v:#}");
}

/// `path[#block]` → action of an `integrate` result.
fn actions(v: &serde_json::Value) -> BTreeMap<String, String> {
    change_actions(&v["result"]["changes"])
}

/// `path` → action of an `init` result.
fn plan_actions(v: &serde_json::Value) -> BTreeMap<String, String> {
    change_actions(&v["result"]["plan"]["changes"])
}

fn change_actions(changes: &serde_json::Value) -> BTreeMap<String, String> {
    changes
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let mut k = c["path"].as_str().unwrap().to_string();
            if let Some(b) = c["block"].as_str() {
                k.push_str(&format!("#{b}"));
            }
            (k, c["action"].as_str().unwrap().to_string())
        })
        .collect()
}

fn project_location() -> ProfileLocation {
    ProfileLocation::for_profile(Profile::Project)
}

// ---------------------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------------------

#[test]
fn init_dry_run_writes_nothing() {
    let kb = Kb::new();
    let before = snapshot(&kb.root);
    let o = kb.run(&["init", "--name", "Acme Mobile", "--namespace", "acme"]);
    assert_ok(&o);
    let v = json(&o);
    assert_eq!(v["result"]["mode"], "dry-run");
    let a = plan_actions(&v);
    assert_eq!(a["project/project.toml"], "create");
    assert_eq!(a["project/README.md"], "replace");
    assert_eq!(a[".github/workflows/kb-knowledge.yml"], "create");
    assert_eq!(
        a["project/skill-config/generated/skills/kb/SKILL.md"],
        "create"
    );
    assert_eq!(snapshot(&kb.root), before, "dry-run must not write");
}

#[test]
fn init_apply_creates_a_loadable_project_and_refuses_a_second_init() {
    let kb = Kb::new();
    let o = kb.run(&[
        "init",
        "--name",
        "Acme \"Mobile\"",
        "--namespace",
        "acme",
        "--harness",
        "codex",
        "--harness",
        "claude",
        "--upstream-url",
        "https://git.example.invalid/kb/upstream.git",
        "--apply",
    ]);
    assert_ok(&o);
    let v = json(&o);
    assert_eq!(v["result"]["mode"], "apply");
    assert_eq!(
        v["result"]["written"].as_u64().unwrap(),
        v["result"]["plan"]["changes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| matches!(c["action"].as_str(), Some("create" | "replace" | "update")))
            .count() as u64
    );
    let files = snapshot(&kb.root);
    for p in [
        "project/project.toml",
        "project/upstream.toml",
        "project/registry/owners.toml",
        "project/registry/concepts.toml",
        "project/knowledge/gaps/README.md",
        "project/knowledge/contracts/README.md",
        "project/routing-tests/README.md",
        "project/skill-config/skill.toml",
        "project/skill-config/generated/manifest.toml",
        "project/skill-config/generated/blocks/AGENTS.md.block",
        "project/skill-config/generated/blocks/CLAUDE.md.block",
        ".github/workflows/kb-knowledge.yml",
    ] {
        assert!(files.contains_key(p), "missing {p}");
    }
    let readme = String::from_utf8(files["project/README.md"].clone()).unwrap();
    assert!(readme.starts_with("# Acme \"Mobile\": engineering knowledge base"));
    assert_eq!(
        files[".github/workflows/kb-knowledge.yml"],
        fs::read(repo_root().join("core/templates/ci/github/kb-knowledge.yml")).unwrap()
    );
    let upstream = String::from_utf8(files["project/upstream.toml"].clone()).unwrap();
    let up: kb::model::UpstreamConfig = toml::from_str(&upstream).unwrap();
    assert_eq!(
        up.url.as_deref(),
        Some("https://git.example.invalid/kb/upstream.git")
    );
    assert_eq!(
        up.revision, None,
        "the temporary KB is not a Git repository"
    );
    let skill: kb::model::SkillConfig =
        toml::from_str(std::str::from_utf8(&files["project/skill-config/skill.toml"]).unwrap())
            .unwrap();
    assert_eq!(
        skill.harnesses,
        vec![kb::model::Harness::Claude, kb::model::Harness::Codex]
    );

    let corpus = load_corpus(&WorkingTreeSource::new(&kb.root), &project_location()).unwrap();
    assert!(corpus.diagnostics.is_empty(), "{:#?}", corpus.diagnostics);
    assert_eq!(corpus.config.project.name, "Acme \"Mobile\"");
    assert_eq!(corpus.config.project.namespace, "acme");
    assert_eq!(corpus.records().count(), 0);
    for path in [
        ".github/pull_request_template.md",
        ".gitlab/merge_request_templates/Knowledge.md",
        ".gitlab/ci/kb-knowledge.yml",
        "project/ci/hosts.tsv",
        "project/knowledge/subsystems/README.md",
        "project/knowledge/checklists/README.md",
        "project/knowledge/glossary/README.md",
    ] {
        assert!(
            files.contains_key(path),
            "missing downstream scaffold {path}"
        );
    }

    assert_ok(&kb.run(&["integrate", "--generate", "--check"]));

    let again = kb.run(&["init", "--name", "Other", "--namespace", "other", "--apply"]);
    assert_code(&again, "ALREADY_INITIALIZED", 14);
    assert_eq!(snapshot(&kb.root), files);
}

#[test]
fn init_records_the_kb_head_as_upstream_revision() {
    let kb = Kb::new();
    kb.sb.init_repo(&kb.root);
    let head = kb.sb.commit_all(&kb.root, "upstream engine");
    kb.init();
    let up: kb::model::UpstreamConfig =
        toml::from_str(&fs::read_to_string(kb.root.join("project/upstream.toml")).unwrap())
            .unwrap();
    assert_eq!(up.revision.as_deref(), Some(head.as_str()));
    assert_eq!(up.url, None);
}

#[test]
fn init_rejects_invalid_parameters_without_writing() {
    let kb = Kb::new();
    let before = snapshot(&kb.root);
    let cases: [(&[&str], &str, i32); 8] = [
        (
            &["init", "--name", "A", "--namespace", "Acme"],
            "INVALID_INPUT",
            64,
        ),
        (&["init", "--name", "A"], "USAGE", 2),
        (&["init", "--namespace", "acme"], "USAGE", 2),
        (
            &[
                "init",
                "--name",
                "A",
                "--namespace",
                "acme",
                "--approved-ref",
                "main",
            ],
            "INVALID_INPUT",
            64,
        ),
        (
            &[
                "init",
                "--name",
                "A",
                "--namespace",
                "acme",
                "--kb-path",
                "../kb",
            ],
            "INVALID_INPUT",
            64,
        ),
        (
            &[
                "init",
                "--name",
                "A",
                "--namespace",
                "acme",
                "--upstream-url",
                "https://user:secret@git.example.invalid/kb.git",
            ],
            "INVALID_INPUT",
            64,
        ),
        (
            &[
                "init",
                "--example",
                "synthetic-multirepo",
                "--namespace",
                "acme",
            ],
            "USAGE",
            2,
        ),
        (&["init", "--example", "no-such-example"], "NOT_FOUND", 16),
    ];
    for (args, code, exit) in cases {
        let mut a = args.to_vec();
        a.push("--apply");
        let o = kb.run(&a);
        assert_code(&o, code, exit);
        let text = common::stdout(&o);
        assert!(!text.contains("secret"), "credentials leaked: {text}");
    }
    assert_eq!(snapshot(&kb.root), before);
}

#[test]
fn init_never_overwrites_existing_files() {
    let kb = Kb::new();
    let owners = "schema = 1\n# hand-written before init\n";
    write(&kb.root.join("project/registry/owners.toml"), owners);
    write(
        &kb.root.join(".github/pull_request_template.md"),
        "Human review process.\n",
    );
    let o = kb.run(&["init", "--name", "Acme", "--namespace", "acme", "--apply"]);
    assert_ok(&o);
    let a = plan_actions(&json(&o));
    assert_eq!(a["project/registry/owners.toml"], "keep");
    assert_eq!(a[".github/pull_request_template.md"], "keep");
    assert_eq!(
        fs::read_to_string(kb.root.join(".github/pull_request_template.md")).unwrap(),
        "Human review process.\n"
    );
    assert_eq!(
        fs::read_to_string(kb.root.join("project/registry/owners.toml")).unwrap(),
        owners
    );
}

#[test]
fn example_initializes_and_validates_without_errors() {
    let kb = Kb::new();
    let o = kb.run(&["init", "--example", "synthetic-multirepo", "--apply"]);
    assert_ok(&o);
    assert_eq!(json(&o)["result"]["plan"]["namespace"], "example");
    let corpus = load_corpus(&WorkingTreeSource::new(&kb.root), &project_location()).unwrap();
    assert!(corpus.diagnostics.is_empty(), "{:#?}", corpus.diagnostics);
    assert!(corpus.records().count() >= 15);
    let kinds: BTreeSet<Kind> = corpus.records().map(|(_, r)| r.record.kind()).collect();
    assert_eq!(kinds.len(), Kind::ALL.len(), "every kind is demonstrated");
    let metas: Vec<MetaInput> = corpus
        .records()
        .map(|(e, r)| MetaInput {
            path: e.path.clone(),
            meta: Arc::new(r.meta()),
        })
        .collect();
    let diags = validate_metas(&corpus.config, &corpus.registry, &metas);
    assert!(
        diags.iter().all(|d| !d.is_error()),
        "example must validate: {diags:#?}"
    );
    for (_, r) in corpus.records() {
        assert!(kb::parse::lint_record(r).is_empty(), "{}", r.record.id());
    }
    assert!(
        fs::read_to_string(kb.root.join("project/README.md"))
            .unwrap()
            .contains("SYNTHETIC")
    );
    assert_ok(&kb.run(&["integrate", "--generate", "--check"]));
}

// ---------------------------------------------------------------------------------------
// Templates, skills and the example as shipped
// ---------------------------------------------------------------------------------------

#[test]
fn record_templates_are_valid_strict_records() {
    let dir = repo_root().join("core/templates/records");
    let mut kinds = BTreeSet::new();
    let sb = Sandbox::new();
    let root = sb.path().join("kb");
    // The templates use the synthetic example registry.
    let loc = project_location();
    let example = repo_root().join(EXAMPLE);
    copy_tree(&example.join("registry"), &root.join("project/registry"));
    fs::copy(
        example.join("project.toml"),
        root.join("project/project.toml"),
    )
    .unwrap();
    for e in fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = fs::read(&p).unwrap();
        let parsed =
            kb::parse::parse_record(&name, &bytes).unwrap_or_else(|d| panic!("{name}: {d:#?}"));
        assert_eq!(name, format!("{}.md", parsed.record.kind().as_str()));
        assert!(parsed.record.id().starts_with("example.template."));
        assert_eq!(parsed.record.status(), Status::Draft);
        kinds.insert(parsed.record.kind());
        fs::create_dir_all(root.join("project/knowledge")).unwrap();
        fs::write(root.join("project/knowledge").join(&name), &bytes).unwrap();
    }
    assert_eq!(kinds.len(), Kind::ALL.len());
    let corpus = load_corpus(&WorkingTreeSource::new(&root), &loc).unwrap();
    assert!(corpus.diagnostics.is_empty(), "{:#?}", corpus.diagnostics);
    let metas: Vec<MetaInput> = corpus
        .records()
        .map(|(e, r)| MetaInput {
            path: e.path.clone(),
            meta: Arc::new(r.meta()),
        })
        .collect();
    let diags = validate_metas(&corpus.config, &corpus.registry, &metas);
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:#?}");
}

/// Every `.tmpl` renders with the documented placeholders only, and files copied verbatim
/// into projects or skill bundles contain no placeholder syntax.
#[test]
fn templates_use_only_documented_placeholders() {
    let mut v = Vars::new();
    for k in [
        "name",
        "namespace",
        "remote",
        "approved_ref",
        "kb_path",
        "harnesses",
        "upstream_url_line",
        "upstream_revision_line",
        "engine_owners",
        "knowledge_owners",
        "project_name",
        "security_contact",
        "skill_protocol",
        "engine_version",
        "default_intent",
        "notes",
        "snapshot_arg",
        "agents_skill_path",
        "claude_skill_path",
        "core_source",
        "core_lines",
        "core_arg",
        "managed_instructions",
    ] {
        v.text(k, "x");
    }
    let root = repo_root();
    for dir in [
        "core/templates/project",
        "core/templates/ownership",
        "core/templates/security",
        "core/skills/kb",
        "core/skills/blocks",
    ] {
        for f in render_dir(&root, dir, &v).unwrap() {
            let text = String::from_utf8_lossy(&f.bytes);
            assert!(!text.contains("{{"), "{dir}/{} keeps a placeholder", f.path);
        }
    }
}

#[test]
fn generated_skill_front_matter_is_valid_for_every_harness() {
    let kb = Kb::new();
    kb.init();
    let skill_dir = kb.root.join("project/skill-config/generated/skills/kb");
    let text = fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("---"));
    let mut fields = BTreeMap::new();
    for l in lines.by_ref() {
        if l == "---" {
            break;
        }
        let (k, val) = l.split_once(": ").expect("single-line `key: value` fields");
        fields.insert(k.to_string(), val.to_string());
    }
    assert_eq!(
        fields.keys().cloned().collect::<Vec<_>>(),
        vec!["description", "name"]
    );
    let folder = skill_dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(fields["name"], "kb");
    assert_eq!(fields["name"], folder, "Cursor requires name == folder");
    let d = &fields["description"];
    assert!(!d.trim().is_empty() && d.chars().count() <= 1024);
    assert!(
        !d.contains(": ") && !d.contains(" #") && !d.starts_with(['"', '\'', '[', '{', '&', '*']),
        "description must be a plain YAML scalar: {d}"
    );
    let total = text.lines().count();
    assert!(total <= 160, "SKILL.md has {total} lines");
    for r in ["recovery", "protocol", "proposals", "harnesses"] {
        assert!(text.contains(&format!("references/{r}.md")));
        assert!(skill_dir.join(format!("references/{r}.md")).is_file());
    }
    assert!(text.contains(".kb/kbw context --intent implement"));
    assert!(text.contains(&format!(
        "--skill-protocol {}",
        kb::versions::SKILL_PROTOCOL
    )));
}

// ---------------------------------------------------------------------------------------
// integrate --generate
// ---------------------------------------------------------------------------------------

#[test]
fn generate_check_apply_is_idempotent_and_removes_stale_files() {
    let kb = Kb::new();
    kb.init();
    kb.set_skill(
        "[\"claude\", \"codex\", \"cursor\"]",
        Some("Mobile lives in `mobile`."),
    );
    let check = kb.run(&["integrate", "--generate", "--check"]);
    assert_code(&check, "DRIFT_DETECTED", 42);
    let gen_dir = kb.root.join("project/skill-config/generated");
    let before = snapshot(&gen_dir);
    let dry = kb.run(&["integrate", "--generate"]);
    assert_ok(&dry);
    assert_eq!(
        actions(&json(&dry))["project/skill-config/generated/skills/kb/SKILL.md"],
        "update"
    );
    assert_eq!(snapshot(&gen_dir), before, "dry-run must not write");

    write(&gen_dir.join("skills/kb/references/stale.md"), "old\n");
    kb.regenerate();
    let skill = fs::read_to_string(gen_dir.join("skills/kb/SKILL.md")).unwrap();
    assert!(skill.contains("Mobile lives in `mobile`."));
    assert!(!gen_dir.join("skills/kb/references/stale.md").exists());
    assert_ok(&kb.run(&["integrate", "--generate", "--check"]));

    let after = snapshot(&gen_dir);
    let again = kb.run(&["integrate", "--generate", "--apply"]);
    assert_ok(&again);
    let v = json(&again);
    assert_eq!(v["result"]["clean"], true);
    assert!(actions(&v).values().all(|a| a == "unchanged"));
    assert_eq!(snapshot(&gen_dir), after);

    let force = kb.run(&["integrate", "--generate", "--apply", "--force"]);
    assert_code(&force, "USAGE", 2);
}

#[test]
fn generate_requires_an_initialized_project() {
    let kb = Kb::new();
    assert_code(
        &kb.run(&["integrate", "--generate", "--check"]),
        "PROJECT_NOT_INITIALIZED",
        10,
    );
}

// ---------------------------------------------------------------------------------------
// Host installation
// ---------------------------------------------------------------------------------------

const AGENTS_USER: &[u8] = b"# Team rules\r\n\r\nKeep answers short.\n\xff raw byte kept\n";
const CLAUDE_USER: &[u8] = b"Project notes without a trailing newline";

#[test]
fn host_install_preserves_user_content_and_second_apply_is_a_no_op() {
    let kb = Kb::new();
    kb.init();
    let host = kb.host("host");
    fs::write(host.join("AGENTS.md"), AGENTS_USER).unwrap();
    fs::write(host.join("CLAUDE.md"), CLAUDE_USER).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&kb.root, host.join(".kb")).unwrap();

    let before = snapshot(&host);
    let dry = kb.integrate(&host, &[]);
    assert_ok(&dry);
    assert_eq!(snapshot(&host), before, "dry-run must not write");

    let o = kb.integrate(&host, &["--apply"]);
    assert_ok(&o);
    let v = json(&o);
    #[cfg(unix)]
    assert_eq!(v["result"]["warnings"], serde_json::json!([]), "{v:#}");
    let a = actions(&v);
    assert_eq!(a[".agents/skills/kb/SKILL.md"], "create");
    assert_eq!(a[".agents/skills/kb/references/recovery.md"], "create");
    assert_eq!(a["AGENTS.md#kb-instructions"], "create");
    assert_eq!(v["result"]["lock"]["action"], "create");

    let bundle = kb.root.join("project/skill-config/generated");
    let skill = fs::read(bundle.join("skills/kb/SKILL.md")).unwrap();
    assert!(!host.join(".claude/skills/kb/SKILL.md").exists());
    assert_eq!(
        fs::read(host.join(".agents/skills/kb/SKILL.md")).unwrap(),
        skill
    );
    for (file, user, block) in [
        ("AGENTS.md", AGENTS_USER, "AGENTS.md.block"),
        ("CLAUDE.md", CLAUDE_USER, "CLAUDE.md.block"),
    ] {
        let now = fs::read(host.join(file)).unwrap();
        assert!(now.starts_with(user), "{file}: unmanaged bytes changed");
        let text = String::from_utf8_lossy(&now);
        assert_eq!(text.matches("<!-- kb:begin kb-instructions -->").count(), 1);
        assert_eq!(text.matches("<!-- kb:end kb-instructions -->").count(), 1);
        let content =
            String::from_utf8(fs::read(bundle.join("blocks").join(block)).unwrap()).unwrap();
        assert!(text.contains(&content));
    }
    assert!(host.join(".kbw/integration.lock").is_file());

    let installed = snapshot(&host);
    let again = kb.integrate(&host, &["--apply"]);
    assert_ok(&again);
    let v = json(&again);
    assert_eq!(v["result"]["clean"], true);
    assert_eq!(v["result"]["lock"]["action"], "unchanged");
    assert!(actions(&v).values().all(|a| a == "unchanged"));
    assert_eq!(snapshot(&host), installed);
    assert_ok(&kb.integrate(&host, &["--check"]));
}

#[test]
fn edited_generated_file_or_block_is_a_conflict_until_forced() {
    let kb = Kb::new();
    kb.init();
    let host = kb.host("host");
    fs::write(host.join("AGENTS.md"), AGENTS_USER).unwrap();
    assert_ok(&kb.integrate(&host, &["--apply"]));
    let pristine = snapshot(&host);

    // A hand-edited generated SKILL.md.
    let skill = host.join(".agents/skills/kb/SKILL.md");
    let mut edited = fs::read(&skill).unwrap();
    edited.extend_from_slice(b"\nmy local tweak\n");
    fs::write(&skill, &edited).unwrap();
    let o = kb.integrate(&host, &["--apply"]);
    assert_code(&o, "CONFLICT", 45);
    assert_eq!(actions(&json(&o))[".agents/skills/kb/SKILL.md"], "conflict");
    assert_eq!(
        fs::read(&skill).unwrap(),
        edited,
        "conflicting file untouched"
    );
    assert_code(&kb.integrate(&host, &["--check"]), "DRIFT_DETECTED", 42);
    assert_code(&kb.integrate(&host, &[]), "CONFLICT", 45);
    assert_ok(&kb.integrate(&host, &["--apply", "--force"]));
    assert_eq!(snapshot(&host), pristine);

    // A hand-edited managed block.
    let agents = host.join("AGENTS.md");
    let text = fs::read(&agents).unwrap();
    let marker = b"## Engineering knowledge base (kb)";
    let pos = text
        .windows(marker.len())
        .position(|w| w == marker)
        .unwrap();
    let mut edited = text[..pos].to_vec();
    edited.extend_from_slice(b"## My own kb notes");
    edited.extend_from_slice(&text[pos + marker.len()..]);
    fs::write(&agents, &edited).unwrap();
    let o = kb.integrate(&host, &["--apply"]);
    assert_code(&o, "CONFLICT", 45);
    assert_eq!(actions(&json(&o))["AGENTS.md#kb-instructions"], "conflict");
    assert_eq!(fs::read(&agents).unwrap(), edited);
    assert_ok(&kb.integrate(&host, &["--apply", "--force"]));
    let restored = fs::read(&agents).unwrap();
    assert_eq!(restored, pristine["AGENTS.md"]);
    assert!(restored.starts_with(AGENTS_USER));
}

#[test]
fn crafted_lock_cannot_make_kb_remove_host_files() {
    let kb = Kb::new();
    kb.init();
    let host = kb.host("host");
    let source = b"fn main() {}\n";
    write(&host.join("src/main.rs"), "fn main() {}\n");
    let lock = format!(
        "schema = 1\nskill_protocol = 1\nengine_version = \"0.1.0\"\nharnesses = [\"claude\"]\n\n[[file]]\npath = \"src/main.rs\"\nsha256 = \"{}\"\n",
        kb::util::sha256_hex(source)
    );
    write(&host.join(".kbw/integration.lock"), &lock);
    let o = kb.integrate(&host, &["--apply", "--force"]);
    assert_code(&o, "CONFIG_INVALID", 11);
    assert_eq!(fs::read(host.join("src/main.rs")).unwrap(), source);
}

#[cfg(unix)]
#[test]
fn symlinked_instruction_file_is_refused() {
    let kb = Kb::new();
    kb.init();
    let host = kb.host("host");
    fs::write(host.join("AGENTS.md"), AGENTS_USER).unwrap();
    std::os::unix::fs::symlink("AGENTS.md", host.join("CLAUDE.md")).unwrap();
    let before = snapshot(&host);
    let o = kb.integrate(&host, &["--apply"]);
    assert_code(&o, "UNSAFE_PATH", 63);
    assert!(
        json(&o)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("CLAUDE.md links to AGENTS.md")
    );
    assert_eq!(snapshot(&host), before);
}

#[test]
fn unmanaged_existing_skill_file_is_a_conflict() {
    let kb = Kb::new();
    kb.init();
    let host = kb.host("host");
    write(
        &host.join(".agents/skills/kb/SKILL.md"),
        "someone else's skill\n",
    );
    let o = kb.integrate(&host, &["--apply"]);
    assert_code(&o, "CONFLICT", 45);
    assert!(!host.join(".kbw/integration.lock").exists());
    assert!(
        !host.join(".claude").exists(),
        "nothing is written on conflict"
    );
}

#[test]
fn check_detects_drift_after_the_bundle_changes() {
    let kb = Kb::new();
    kb.init();
    let host = kb.host("host");
    assert_ok(&kb.integrate(&host, &["--apply"]));
    assert_ok(&kb.integrate(&host, &["--check"]));
    kb.set_skill("[\"claude\", \"codex\", \"cursor\"]", Some("New note."));
    kb.regenerate();
    let o = kb.integrate(&host, &["--check"]);
    assert_code(&o, "DRIFT_DETECTED", 42);
    assert_eq!(actions(&json(&o))[".agents/skills/kb/SKILL.md"], "update");
    assert_ok(&kb.integrate(&host, &["--apply"]));
    assert_ok(&kb.integrate(&host, &["--check"]));
    assert!(
        fs::read_to_string(host.join(".agents/skills/kb/SKILL.md"))
            .unwrap()
            .contains("New note.")
    );
}

#[test]
fn malformed_markers_are_an_error_and_nothing_is_written() {
    let kb = Kb::new();
    kb.init();
    for (i, bad) in [
        "<!-- kb:begin kb-instructions -->\na\n<!-- kb:end kb-instructions -->\n<!-- kb:begin kb-instructions -->\nb\n<!-- kb:end kb-instructions -->\n",
        "intro\n<!-- kb:begin kb-instructions -->\nunterminated\n",
        "<!-- kb:end kb-instructions -->\n",
        "<!-- kb:begin kb-instructions\n",
    ]
    .iter()
    .enumerate()
    {
        let host = kb.host(&format!("host{i}"));
        fs::write(host.join("AGENTS.md"), bad).unwrap();
        let before = snapshot(&host);
        let o = kb.integrate(&host, &["--apply"]);
        assert_code(&o, "INVALID_INPUT", 64);
        assert_eq!(snapshot(&host), before);
    }
}

#[test]
fn harness_selection_controls_targets_and_stale_targets_are_removed() {
    let kb = Kb::new();
    kb.init();
    kb.set_skill("[\"claude\"]", None);
    kb.regenerate();
    let host = kb.host("host");
    fs::write(host.join("CLAUDE.md"), CLAUDE_USER).unwrap();
    assert_ok(&kb.integrate(&host, &["--apply"]));
    assert!(host.join(".claude/skills/kb/SKILL.md").is_file());
    assert!(
        !host.join(".agents").exists(),
        "one canonical skill is installed"
    );
    assert!(!host.join("AGENTS.md").exists());
    assert!(
        fs::read_to_string(host.join("CLAUDE.md"))
            .unwrap()
            .contains("kb:begin")
    );

    kb.set_skill("[\"codex\"]", None);
    kb.regenerate();
    let o = kb.integrate(&host, &["--apply"]);
    assert_ok(&o);
    let a = actions(&json(&o));
    assert_eq!(a[".claude/skills/kb/SKILL.md"], "remove");
    assert_eq!(a["CLAUDE.md#kb-instructions"], "remove");
    assert_eq!(a[".agents/skills/kb/SKILL.md"], "create");
    assert!(
        !host.join(".claude").exists(),
        "emptied directories are pruned"
    );
    assert!(host.join(".agents/skills/kb/SKILL.md").is_file());
    let claude = fs::read(host.join("CLAUDE.md")).unwrap();
    assert!(claude.starts_with(CLAUDE_USER));
    assert!(!String::from_utf8_lossy(&claude).contains("kb:begin"));
    let agents = fs::read_to_string(host.join("AGENTS.md")).unwrap();
    assert!(agents.contains(".agents/skills/kb/SKILL.md"));
    assert_ok(&kb.integrate(&host, &["--check"]));
}

#[test]
fn host_root_is_the_superproject_or_the_current_repository() {
    let kb = Kb::new();
    kb.init();
    kb.sb.init_repo(&kb.root);
    kb.sb.commit_all(&kb.root, "downstream KB");
    let host = kb.host("host");
    kb.sb.init_repo(&host);
    kb.sb.commit_all(&host, "host");
    kb.sb.git(
        &host,
        &["submodule", "add", "-q", kb.root.to_str().unwrap(), ".kb"],
    );
    let mounted = host.join(".kb");
    let run = |cwd: &Path| {
        let o = kb.sb.kb(
            cwd,
            &["--root", mounted.to_str().unwrap(), "--json", "integrate"],
            &[],
        );
        assert_ok(&o);
        json(&o)
    };
    let from_kb = run(&mounted);
    assert_eq!(from_kb["result"]["host"], host.to_str().unwrap());
    assert_eq!(from_kb["result"]["warnings"], serde_json::json!([]));
    fs::create_dir_all(host.join("src")).unwrap();
    assert_eq!(
        run(&host.join("src"))["result"]["host"],
        host.to_str().unwrap()
    );

    // Neither a host repository nor a submodule: an explicit --host is required.
    let o = kb.run(&["integrate"]);
    assert_code(&o, "INVALID_INPUT", 64);
}

#[test]
fn skill_status_reports_installed_protocol() {
    use kb::integrate::{SkillState, skill_status};
    let kb = Kb::new();
    kb.init();
    let host = kb.host("host");
    let s = skill_status(&kb.root, &host).unwrap();
    assert_eq!(s.state, SkillState::NotInstalled);
    assert_eq!(s.bundle_skill_protocol, Some(kb::versions::SKILL_PROTOCOL));
    assert_ok(&kb.integrate(&host, &["--apply"]));
    let s = skill_status(&kb.root, &host).unwrap();
    assert_eq!(s.state, SkillState::Current);
    assert_eq!(
        s.installed_skill_protocol,
        Some(kb::versions::SKILL_PROTOCOL)
    );

    let lock = host.join(".kbw/integration.lock");
    let text = fs::read_to_string(&lock).unwrap();
    fs::write(
        &lock,
        text.replace(
            &format!("skill_protocol = {}", kb::versions::SKILL_PROTOCOL),
            "skill_protocol = 0",
        ),
    )
    .unwrap();
    let s = skill_status(&kb.root, &host).unwrap();
    assert_eq!(s.state, SkillState::Outdated);
    assert!(s.message.contains("new agent session"));
    fs::write(&lock, "not = [valid").unwrap();
    assert_eq!(
        skill_status(&kb.root, &host).unwrap().state,
        SkillState::LockInvalid
    );
}

/// Every snapshot-reading `kbw` command (`context`, `impact`, `show`, `search`) shown in a
/// generated text, up to its closing backtick or the end of the line.
fn snapshot_reading_commands(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        for (i, _) in line.match_indices("/kbw ") {
            let cmd = &line[i + 1..];
            let cmd = &cmd[..cmd.find('`').unwrap_or(cmd.len())];
            let sub = cmd["kbw ".len()..].split(' ').next().unwrap_or("");
            if ["context", "impact", "show", "search"].contains(&sub) {
                out.push(cmd.to_string());
            }
        }
    }
    out
}

#[test]
fn skill_snapshot_setting_applies_to_every_generated_command() {
    let kb = Kb::new();
    kb.init();
    let gen_dir = kb.root.join("project/skill-config/generated");
    let commands = || -> BTreeMap<String, Vec<String>> {
        snapshot(&gen_dir)
            .into_iter()
            .map(|(p, b)| (p, snapshot_reading_commands(&String::from_utf8(b).unwrap())))
            .filter(|(_, c)| !c.is_empty())
            .collect()
    };

    // `auto` defers to the host binding: no command selects a snapshot.
    let auto = commands();
    for f in ["skills/kb/SKILL.md", "blocks/AGENTS.md.block"] {
        assert!(
            auto.get(f).is_some_and(|c| c.len() >= 2),
            "{f} shows context and impact: {auto:#?}"
        );
    }
    assert!(
        fs::read_to_string(gen_dir.join("blocks/CLAUDE.md.block"))
            .unwrap()
            .contains("Read `AGENTS.md`")
    );
    assert!(
        auto.values().flatten().all(|c| !c.contains("--snapshot")),
        "{auto:#?}"
    );

    // An explicit setting reaches every command, including the always-loaded blocks.
    write(
        &kb.skill_toml(),
        "schema = 1\nharnesses = [\"claude\", \"codex\", \"cursor\"]\nkb_path = \".kb\"\nsnapshot = \"pinned\"\n",
    );
    kb.regenerate();
    let pinned = commands();
    assert_eq!(
        pinned.values().map(Vec::len).collect::<Vec<_>>(),
        auto.values().map(Vec::len).collect::<Vec<_>>()
    );
    for (f, cmds) in &pinned {
        for c in cmds {
            assert!(c.contains(" --snapshot pinned"), "{f}: `{c}`");
        }
    }
}

// ---------------------------------------------------------------------------------------
// Shipped merge request and CI templates for host repositories
// ---------------------------------------------------------------------------------------

const MR_TEMPLATES: [&str; 2] = [
    "core/templates/mr/github_pull_request_template.md",
    "core/templates/mr/gitlab_merge_request_template.md",
];

/// Fill the `kb-impact` block of a shipped MR template as its instructions say: set
/// `kb_change` and `reason`, and uncomment and set the optional keys the mode uses.
fn fill_mr_template(template: &str, mode: &str, reason: &str, optional: &[(&str, &str)]) -> String {
    let mut filled = Vec::new();
    let mut set = Vec::new();
    for line in template.lines() {
        let opt = optional
            .iter()
            .find(|(k, _)| line.starts_with(&format!("# {k} = ")));
        filled.push(if line.starts_with("kb_change = ") {
            set.push("kb_change");
            format!("kb_change = \"{mode}\"")
        } else if line.starts_with("reason = ") {
            set.push("reason");
            format!("reason = \"{reason}\"")
        } else if let Some((k, v)) = opt {
            set.push(*k);
            format!("{k} = \"{v}\"")
        } else {
            line.to_string()
        });
    }
    let mut want: Vec<&str> = ["kb_change", "reason"]
        .into_iter()
        .chain(optional.iter().map(|(k, _)| *k))
        .collect();
    want.sort_unstable();
    set.sort_unstable();
    assert_eq!(
        set, want,
        "each key the template asks for appears exactly once"
    );
    filled.join("\n") + "\n"
}

#[test]
fn mr_templates_filled_as_instructed_pass_the_statement_check() {
    use kb::impact::parse_statement;
    let rev = "0123456789abcdef";
    let cases: [(&str, &[(&str, &str)]); 5] = [
        ("none", &[]),
        ("linked", &[("change_id", "PROJ-123")]),
        ("linked", &[("kb_revision", rev)]),
        ("linked", &[("kb_revision", rev), ("change_id", "PROJ-123")]),
        ("included", &[]),
    ];
    for path in MR_TEMPLATES {
        let template = fs::read_to_string(repo_root().join(path)).unwrap();
        // Unfilled (empty reason): rejected, so CI makes the author fill it in.
        let err = parse_statement(&template).expect_err(path);
        assert!(err.contains("reason"), "{path}: {err}");
        for (mode, optional) in cases {
            let filled = fill_mr_template(&template, mode, "Pure refactoring.", optional);
            let s = parse_statement(&filled)
                .unwrap_or_else(|e| panic!("{path}, {mode} {optional:?}: {e}"))
                .expect("one kb-impact block");
            let opt = |key: &str| optional.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
            assert_eq!(s.kb_change.as_str(), mode);
            assert_eq!(s.reason, "Pure refactoring.");
            assert_eq!(s.kb_revision.as_deref(), opt("kb_revision"), "{path}");
            assert_eq!(s.change_id.as_deref(), opt("change_id"), "{path}");
        }
    }
}

/// The shell command of the single step or script line of a CI template containing
/// `needle` (a `run:` value or a GitLab `script` item; YAML single quotes removed).
fn ci_command(yaml: &str, needle: &str) -> String {
    let lines: Vec<&str> = yaml.lines().filter(|l| l.contains(needle)).collect();
    assert_eq!(lines.len(), 1, "`{needle}`: {lines:?}");
    let l = lines[0].trim_start();
    let l = l
        .strip_prefix("run: ")
        .or_else(|| l.strip_prefix("- "))
        .unwrap_or_else(|| panic!("not a command line: {l}"));
    match l.strip_prefix('\'').and_then(|q| q.strip_suffix('\'')) {
        Some(quoted) => quoted.replace("''", "'"),
        None => l.to_string(),
    }
}

/// The host CI templates run as shipped (description step, then the impact step) in a host
/// whose approved KB tip is ahead of its pin, the normal state while a linked knowledge
/// change waits for the host pin update.
#[cfg(unix)]
#[test]
fn host_ci_impact_step_passes_for_a_filled_template_while_the_pin_lags() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    let sb = Sandbox::new();
    let p = |p: &Path| p.to_str().unwrap().to_string();
    let origin = sb.path().join("origin.git");
    sb.init_bare(&origin);
    let seed = sb.path().join("seed");
    sb.init_repo(&seed);
    common::write_min_project(&seed);
    write(&seed.join(".gitignore"), ".cache/\n");
    sb.commit_all(&seed, "approved knowledge");
    sb.git(&seed, &["push", "-q", &p(&origin), "main"]);

    let host = sb.path().join("host");
    sb.init_repo(&host);
    let remote = "https://example.invalid/acme/mobile.git";
    sb.git(&host, &["remote", "add", "origin", remote]);
    write(&host.join("app/auth/Token.kt"), "class Token\n");
    sb.commit_all(&host, "host code");
    sb.git(&host, &["submodule", "add", "-q", &p(&origin), ".kb"]);
    let base = sb.commit_all(&host, "mount the KB");
    sb.git(&host, &["update-ref", "refs/remotes/origin/main", &base]);
    write(
        &host.join("app/auth/Token.kt"),
        "class Token { fun refresh() {} }\n",
    );
    sb.commit_all(&host, "code change under review");
    // A knowledge change is merged into the approved ref; the host pin is not updated yet.
    sb.commit_all(&seed, "merged knowledge change");
    sb.git(&seed, &["push", "-q", &p(&origin), "main"]);

    let kb_root = host.join(".kb").canonicalize().unwrap();
    let auto = sb.kb(
        &host,
        &[
            "--root",
            &p(&kb_root),
            "--json",
            "impact",
            "--base",
            "origin/main",
        ],
        &[],
    );
    assert_code(&auto, "UPDATE_REQUIRED", 21);

    // `$KB_PATH/kbw` stands in for the bootstrapped launcher of the pinned KB checkout.
    let shim = sb.path().join("shim");
    write(
        &shim.join("kbw"),
        &format!(
            "#!/bin/sh\nexec '{}' --root '{}' \"$@\"\n",
            common::kb_bin().display(),
            kb_root.display()
        ),
    );
    fs::set_permissions(shim.join("kbw"), fs::Permissions::from_mode(0o755)).unwrap();
    let ci = [
        (
            "core/templates/ci/github/host-kb-impact.yml",
            MR_TEMPLATES[0],
            "PR_BODY",
        ),
        (
            "core/templates/ci/gitlab/host-kb-impact.gitlab-ci.yml",
            MR_TEMPLATES[1],
            "CI_MERGE_REQUEST_DESCRIPTION",
        ),
    ];
    for (workflow, mr, body_var) in ci {
        let yaml = fs::read_to_string(repo_root().join(workflow)).unwrap();
        let mr = fs::read_to_string(repo_root().join(mr)).unwrap();
        let description = fill_mr_template(&mr, "none", "Refactoring only.", &[]);
        let tmp = sb.path().join("ci-tmp");
        fs::create_dir_all(&tmp).unwrap();
        for step in [
            ci_command(&yaml, "printf "),
            ci_command(&yaml, "kbw\" impact"),
        ] {
            let mut c = Command::new("sh");
            c.current_dir(&host).arg("-c").arg(&step);
            common::isolated_env(&mut c, &sb.home());
            let o = c
                .env("KB_PATH", &shim)
                .env(body_var, &description)
                .env("RUNNER_TEMP", &tmp)
                .env("TMPDIR", &tmp)
                .env("BASE_REF", "main")
                .env("CI_MERGE_REQUEST_TARGET_BRANCH_NAME", "main")
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{workflow}: `{step}` exited {:?}\nstdout:\n{}\nstderr:\n{}",
                o.status.code(),
                common::stdout(&o),
                common::stderr(&o)
            );
        }
        fs::remove_file(tmp.join("mr-description.md")).unwrap();
    }
}

#[test]
fn ci_templates_follow_platform_rules() {
    // GitHub: the checked statement is the PR description, so editing it re-runs the check.
    let yaml = fs::read_to_string(repo_root().join("core/templates/ci/github/host-kb-impact.yml"))
        .unwrap();
    let on = yaml
        .split_once("\non:\n")
        .expect("`on:` block")
        .1
        .split("\n\n")
        .next()
        .unwrap();
    let types = on
        .lines()
        .find_map(|l| l.trim().strip_prefix("types: "))
        .expect("explicit pull_request activity types");
    for t in ["opened", "synchronize", "reopened", "edited"] {
        assert!(types.contains(t), "pull_request types {types} lack `{t}`");
    }

    // GitLab: `cache:key:files` takes one or two existing files.
    let mut checked = 0;
    for f in ["kb-knowledge.gitlab-ci.yml", "host-kb-impact.gitlab-ci.yml"] {
        let yaml =
            fs::read_to_string(repo_root().join("core/templates/ci/gitlab").join(f)).unwrap();
        let lines: Vec<&str> = yaml
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect();
        for w in lines.windows(2) {
            if w[0].trim() != "key:" {
                continue;
            }
            let Some(list) = w[1].trim().strip_prefix("files: ") else {
                continue;
            };
            let files: Vec<&str> = list
                .trim_matches(['[', ']'])
                .split(',')
                .map(str::trim)
                .collect();
            assert!(
                (1..=2).contains(&files.len()),
                "{f}: cache:key:files {list}"
            );
            for file in files {
                assert!(repo_root().join(file).is_file(), "{f}: missing {file}");
            }
            checked += 1;
        }
    }
    assert!(checked >= 1, "no GitLab cache:key:files found");
}

// ---------------------------------------------------------------------------------------
// Example routing fixtures against the scope rules (docs/architecture.md §3.3, §5)
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum App {
    Applies,
    Undetermined,
    NotApplicable,
}

/// Task scope per dimension: `None` = unknown.
struct TaskScope {
    repos: Option<BTreeSet<String>>,
    modules: Option<BTreeSet<String>>,
    features: Option<BTreeSet<String>>,
}

fn task_scope(reg: &Registry, case: &RoutingCase) -> TaskScope {
    let mut repos: BTreeSet<String> = case.repos.iter().cloned().collect();
    let mut modules: BTreeSet<String> = case.modules.iter().cloned().collect();
    let mut features: BTreeSet<String> = case.features.iter().cloned().collect();
    for m in &case.modules {
        features.extend(reg.module(m).unwrap().features.iter().cloned());
    }
    for p in &case.paths {
        let (repo, path) = p.split_once(':').expect("fixtures use `repo:path`");
        repos.insert(repo.to_string());
        for m in reg.modules_for_path(repo, path) {
            modules.insert(m.id.clone());
            features.extend(m.features.iter().cloned());
        }
        features.extend(
            reg.features_for_path(repo, path)
                .iter()
                .map(|f| f.id.clone()),
        );
    }
    let has_paths = !case.paths.is_empty();
    TaskScope {
        repos: (!repos.is_empty()).then_some(repos),
        modules: (has_paths || !case.modules.is_empty()).then_some(modules),
        features: (has_paths || !case.modules.is_empty() || !case.features.is_empty())
            .then_some(features),
    }
}

fn applicability(scope: &Scope, task: &TaskScope) -> App {
    if scope.product {
        return App::Applies;
    }
    let mut result = App::Applies;
    for (dim, known) in [
        (&scope.repos, &task.repos),
        (&scope.modules, &task.modules),
        (&scope.features, &task.features),
    ] {
        if dim.is_empty() {
            continue;
        }
        match known {
            Some(set) if dim.iter().any(|d| set.contains(d)) => {}
            Some(_) => return App::NotApplicable,
            None => result = App::Undetermined,
        }
    }
    result
}

#[test]
fn example_routing_fixtures_follow_the_scope_rules() {
    let root = repo_root();
    let loc = ProfileLocation {
        profile: Profile::Project,
        dir: EXAMPLE.into(),
        config: format!("{EXAMPLE}/project.toml"),
    };
    let corpus = load_corpus(&WorkingTreeSource::new(&root), &loc).unwrap();
    assert!(corpus.diagnostics.is_empty(), "{:#?}", corpus.diagnostics);
    let metas: BTreeMap<String, kb::model::RecordMeta> = corpus
        .records()
        .map(|(_, r)| (r.record.id().to_string(), r.meta()))
        .collect();
    let dir = root.join(EXAMPLE).join("routing-tests");
    let mut cases = 0;
    for e in fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let file: RoutingTestFile = toml::from_str(&fs::read_to_string(&p).unwrap())
            .unwrap_or_else(|err| panic!("{}: {err}", p.display()));
        assert_eq!(file.schema, 1);
        for case in &file.case {
            cases += 1;
            let task = task_scope(&corpus.registry, case);
            let app = |id: &str| applicability(&metas[id].scope, &task);
            let accepted = |id: &str| metas[id].status == Status::Accepted;
            let direct: BTreeSet<&str> = metas
                .values()
                .filter(|m| m.kind.is_mandatory() && m.status == Status::Accepted)
                .filter(|m| app(&m.id) == App::Applies)
                .map(|m| m.id.as_str())
                .collect();
            let mut closure: BTreeSet<&str> = direct.clone();
            let mut queue: Vec<&str> = direct.iter().copied().collect();
            while let Some(id) = queue.pop() {
                for r in &metas[id].links.requires {
                    if closure.insert(r.as_str()) {
                        queue.push(r.as_str());
                    }
                }
            }
            let rationale: BTreeSet<&str> = direct
                .iter()
                .flat_map(|id| metas[*id].links.rationale.iter().map(String::as_str))
                .collect();
            let expected: BTreeSet<&str> =
                case.expect_mandatory.iter().map(String::as_str).collect();
            assert_eq!(expected, direct, "{}: expect_mandatory", case.name);
            for id in &case.expect_included {
                assert!(
                    closure.contains(id.as_str()) || rationale.contains(id.as_str()),
                    "{}: `{id}` is neither a required dependency nor a rationale",
                    case.name
                );
            }
            for id in &case.forbid {
                assert!(metas.contains_key(id), "{}: unknown id `{id}`", case.name);
                assert!(
                    !closure.contains(id.as_str()) && !rationale.contains(id.as_str()),
                    "{}: forbidden `{id}` is reachable",
                    case.name
                );
                assert!(
                    !accepted(id) || app(id) == App::NotApplicable,
                    "{}: forbidden `{id}` could be supplementary",
                    case.name
                );
            }
            let undetermined = metas.values().any(|m| {
                m.kind.is_mandatory()
                    && m.status == Status::Accepted
                    && app(&m.id) == App::Undetermined
            });
            match case.expect_status.as_deref() {
                Some("complete") => {
                    assert!(!undetermined, "{}", case.name);
                    assert!(closure.iter().all(|id| accepted(id)), "{}", case.name);
                }
                Some("partial") => assert!(undetermined, "{}", case.name),
                None => {}
                Some(other) => panic!("{}: unexpected status {other}", case.name),
            }
        }
    }
    assert!(cases >= 6, "example has {cases} routing cases");
}
