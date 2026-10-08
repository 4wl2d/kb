//! The generated JSON Schemas agree with the strict runtime parser, match the committed
//! files in core/schemas, and describe real CLI output and shipped configuration.
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use kb::error::ErrorCode;
use kb::model::ids;
use kb::parse::{parse_record, split_front_matter};
use kb::schema_export;
use proptest::prelude::*;
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    common::repo_root().join("core/tests/fixtures/validate/records")
}

fn schema(file: &str) -> Value {
    let text = schema_export::generate()
        .remove(file)
        .unwrap_or_else(|| panic!("no generated schema {file}"));
    serde_json::from_str(&text).unwrap()
}

fn validator(file: &str) -> jsonschema::Validator {
    jsonschema::validator_for(&schema(file)).unwrap()
}

/// TOML → JSON. TOML floats and datetimes have no JSON equivalent that JSON Schema can tell
/// apart from integers/strings, and no model field accepts them, so they become tagged
/// objects that no schema accepts.
fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => json!(s),
        toml::Value::Integer(i) => json!(i),
        toml::Value::Float(f) => json!({ "$toml-float": f.to_string() }),
        toml::Value::Boolean(b) => json!(b),
        toml::Value::Datetime(d) => json!({ "$toml-datetime": d.to_string() }),
        toml::Value::Array(a) => Value::Array(a.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => table_to_json(t),
    }
}

fn table_to_json(t: &toml::Table) -> Value {
    Value::Object(
        t.iter()
            .map(|(k, v)| (k.clone(), toml_to_json(v)))
            .collect(),
    )
}

/// JSON form of a record's front matter, `None` when there is none (no `+++`, bad TOML).
fn front_matter_json(bytes: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(bytes).ok()?;
    let (fm, _, _) = split_front_matter(text).ok()?;
    let table: toml::Table = toml::from_str(fm).ok()?;
    Some(table_to_json(&table))
}

fn schema_errors(v: &jsonschema::Validator, instance: &Value) -> Vec<String> {
    v.iter_errors(instance)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect()
}

fn md_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    v.sort();
    v
}

#[test]
fn generated_schemas_are_valid_draft_2020_12_with_kb_ids() {
    let all = schema_export::generate();
    let expected: BTreeSet<&str> = [
        "record",
        "project",
        "registry-owners",
        "registry-repos",
        "registry-modules",
        "registry-features",
        "registry-concepts",
        "routing-test",
        "skill-config",
        "host-binding",
        "upstream",
        "release-manifest",
        "cli-envelope",
        "code-request",
        "code-response",
    ]
    .into_iter()
    .collect();
    let names: BTreeSet<String> = all.keys().cloned().collect();
    let mut want: BTreeSet<String> = expected
        .iter()
        .map(|n| format!("{n}.v1.schema.json"))
        .collect();
    want.extend(
        expected
            .iter()
            .filter(|n| **n == "record" || **n == "project" || n.starts_with("registry-"))
            .map(|n| format!("{n}.v2.schema.json")),
    );
    want.insert("registry-change-types.v2.schema.json".into());
    assert_eq!(names, want);
    for (file, text) in &all {
        assert!(text.ends_with("}\n"), "{file} must end with a newline");
        let v: Value = serde_json::from_str(text).unwrap();
        jsonschema::meta::validate(&v).unwrap_or_else(|e| panic!("{file}: {e}"));
        jsonschema::validator_for(&v).unwrap_or_else(|e| panic!("{file}: {e}"));
        let (name, version) = file
            .trim_end_matches(".schema.json")
            .rsplit_once('.')
            .unwrap();
        assert_eq!(v["$id"], json!(format!("kb:schema/{name}/{version}")));
        assert_eq!(
            v["$schema"],
            json!("https://json-schema.org/draft/2020-12/schema")
        );
        assert!(
            !text.contains("http://") && !text.contains("https://example"),
            "{file} must not invent web URLs"
        );
    }
    // Deterministic output.
    assert_eq!(all, schema_export::generate());
}

#[test]
fn committed_schemas_match_the_model() {
    let drift = schema_export::check(&common::repo_root()).unwrap();
    assert!(
        drift.is_empty(),
        "core/schemas is out of date (run `kbw schema --write`): {drift:#?}"
    );
}

#[test]
fn valid_fixtures_pass_parser_and_schema() {
    let v = validator("record.v1.schema.json");
    let files = md_files(&fixtures().join("valid"));
    assert!(files.len() >= 8);
    let mut kinds = BTreeSet::new();
    for f in files {
        let bytes = fs::read(&f).unwrap();
        let parsed = parse_record("fixture.md", &bytes)
            .unwrap_or_else(|e| panic!("{}: parser rejected: {e:#?}", f.display()));
        kinds.insert(parsed.record.kind());
        let instance = front_matter_json(&bytes).unwrap();
        let errors = schema_errors(&v, &instance);
        assert!(
            errors.is_empty(),
            "{}: schema rejected: {errors:#?}",
            f.display()
        );
    }
    assert_eq!(kinds.len(), 8, "every kind needs a valid fixture");
}

#[derive(serde::Deserialize)]
struct Cases {
    cases: BTreeMap<String, Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    code: String,
    schema: String,
}

#[test]
fn invalid_fixtures_have_the_expected_runtime_and_schema_verdicts() {
    let cases: Cases =
        toml::from_str(&fs::read_to_string(fixtures().join("cases.toml")).unwrap()).unwrap();
    let files: BTreeSet<String> = md_files(&fixtures().join("invalid"))
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let listed: BTreeSet<String> = cases.cases.keys().cloned().collect();
    assert_eq!(files, listed, "cases.toml must list every invalid fixture");

    let v = validator("record.v1.schema.json");
    let mut expressible = 0;
    for (file, case) in &cases.cases {
        let bytes = fs::read(fixtures().join("invalid").join(file)).unwrap();
        let diags = match parse_record(file, &bytes) {
            Ok(_) => panic!("{file}: parser accepted an invalid record"),
            Err(d) => d,
        };
        assert!(
            diags.iter().any(|d| d.code == case.code),
            "{file}: expected {}, got {:#?}",
            case.code,
            diags
        );
        let instance = front_matter_json(&bytes);
        match case.schema.as_str() {
            "reject" => {
                expressible += 1;
                let instance = instance.unwrap_or_else(|| panic!("{file}: no JSON form"));
                assert!(
                    !v.is_valid(&instance),
                    "{file}: schema accepted a record the parser rejects ({})",
                    case.code
                );
            }
            "accept" => {
                let instance = instance.unwrap_or_else(|| panic!("{file}: no JSON form"));
                let errors = schema_errors(&v, &instance);
                assert!(
                    errors.is_empty(),
                    "{file}: runtime-only defect unexpectedly rejected by the schema: {errors:#?}"
                );
            }
            "no-instance" => assert!(instance.is_none(), "{file}: expected no JSON form"),
            other => panic!("{file}: unknown schema verdict `{other}`"),
        }
    }
    assert!(expressible >= 40);
}

#[test]
fn shipped_manifest_and_minimal_project_conform_to_their_schemas() {
    let check = |schema_file: &str, path: &Path| {
        let table: toml::Table = toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let errors = schema_errors(&validator(schema_file), &table_to_json(&table));
        assert!(errors.is_empty(), "{}: {errors:#?}", path.display());
    };
    check(
        "release-manifest.v1.schema.json",
        &common::repo_root().join("core/release.toml"),
    );
    let sb = common::Sandbox::new();
    let root = sb.path().join("kb");
    common::write_min_project(&root);
    let p = root.join("project");
    check("project.v1.schema.json", &p.join("project.toml"));
    for name in ["owners", "repos", "modules", "features", "concepts"] {
        check(
            &format!("registry-{name}.v1.schema.json"),
            &p.join(format!("registry/{name}.toml")),
        );
    }
    // Schema-expressible config rules are enforced by the schema as well.
    let bad = json!({
        "schema": 1,
        "project": {"name": "X", "namespace": "Bad_NS"},
        "source": {"approved_ref": "refs/heads/../main"}
    });
    assert!(!validator("project.v1.schema.json").is_valid(&bad));
    let bad_registry = json!({"schema": 1, "repo": [{"id": "-mobile", "title": "M"}]});
    assert!(!validator("registry-repos.v1.schema.json").is_valid(&bad_registry));
}

#[test]
fn cli_envelope_schema_describes_real_output() {
    let v = validator("cli-envelope.v1.schema.json");
    let sb = common::Sandbox::new();
    let ok = sb.kb(&sb.path(), &["version", "--json"], &[]);
    assert!(ok.status.success(), "{}", common::stderr(&ok));
    let doc = common::json(&ok);
    assert!(
        schema_errors(&v, &doc).is_empty(),
        "{:#?}",
        schema_errors(&v, &doc)
    );

    // A failing command: schema drift in a scratch KB root (exit 42, DRIFT_DETECTED).
    let root = sb.path().join("kb");
    common::copy_manifest(&root);
    let out = sb.kb(
        &sb.path(),
        &[
            "--root",
            root.to_str().unwrap(),
            "schema",
            "--check",
            "--json",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(42), "{}", common::stderr(&out));
    let doc = common::json(&out);
    assert_eq!(doc["error"]["code"], "DRIFT_DETECTED");
    assert!(
        schema_errors(&v, &doc).is_empty(),
        "{:#?}",
        schema_errors(&v, &doc)
    );
    // A wrong exit code for the error code is rejected.
    let mut tampered = doc.clone();
    tampered["error"]["exit_code"] = json!(41);
    assert!(!v.is_valid(&tampered));
    // `ok` and `error` must agree.
    let mut inconsistent = doc;
    inconsistent["ok"] = json!(true);
    assert!(!v.is_valid(&inconsistent));
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Every protocol error code (and so every code the envelope schema admits) is produced by
/// some engine code path and has a row with its exit code in the skill's recovery table,
/// which lists no other codes.
#[test]
fn every_protocol_error_code_is_emitted_and_documented() {
    let src = common::repo_root().join("core/cli/src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    let engine: String = files
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n != "error.rs"))
        .map(|p| fs::read_to_string(p).unwrap())
        .collect();
    // Codes also built through `KbError` helpers defined next to the enum.
    let helpers = [
        (ErrorCode::Internal, "KbError::internal("),
        (ErrorCode::IoError, "KbError::io("),
        (ErrorCode::InvalidInput, "KbError::invalid_input("),
        (ErrorCode::UnsafePath, "KbError::unsafe_path("),
    ];
    let never_emitted: Vec<&str> = ErrorCode::ALL
        .iter()
        .filter(|c| {
            let variant = format!("ErrorCode::{c:?}");
            let direct = engine.match_indices(&variant).any(|(i, _)| {
                !engine[i + variant.len()..].starts_with(|ch: char| ch.is_alphanumeric())
            });
            let helper = helpers
                .iter()
                .any(|(h, call)| h == *c && engine.contains(call));
            !direct && !helper
        })
        .map(|c| c.as_str())
        .collect();
    assert!(
        never_emitted.is_empty(),
        "declared but never emitted: {never_emitted:?}"
    );

    let recovery =
        fs::read_to_string(common::repo_root().join("core/skills/kb/references/recovery.md.tmpl"))
            .unwrap();
    let documented: BTreeMap<String, i32> = recovery
        .lines()
        .filter_map(|l| {
            let mut cells = l.strip_prefix("| `")?.splitn(3, '|');
            let code = cells.next()?.trim().strip_suffix('`')?;
            let exit = cells.next()?.trim().parse().ok()?;
            Some((code.to_string(), exit))
        })
        .collect();
    let declared: BTreeMap<String, i32> = ErrorCode::ALL
        .iter()
        .map(|c| (c.as_str().to_string(), c.exit_code()))
        .collect();
    assert_eq!(documented, declared);
}

#[test]
fn json_mode_argument_errors_still_produce_one_protocol_envelope() {
    let v = validator("cli-envelope.v1.schema.json");
    let sb = common::Sandbox::new();
    // A typo in a value of a known command: the command is identified.
    let out = sb.kb(
        &sb.path(),
        &["--json", "context", "--intent", "implment"],
        &[],
    );
    assert_eq!(out.status.code(), Some(2));
    let doc = common::json(&out);
    assert_eq!(doc["ok"], json!(false));
    assert_eq!(doc["command"], json!("context"));
    assert_eq!(doc["error"]["code"], json!("USAGE"));
    assert!(
        schema_errors(&v, &doc).is_empty(),
        "{:#?}",
        schema_errors(&v, &doc)
    );
    assert!(
        common::stderr(&out).contains("implment"),
        "clap text goes to stderr"
    );

    // An unknown flag before any command: `command` is null.
    let out = sb.kb(&sb.path(), &["--format", "json", "--bogus"], &[]);
    assert_eq!(out.status.code(), Some(2));
    let doc = common::json(&out);
    assert_eq!(doc["command"], json!(null));
    assert!(
        schema_errors(&v, &doc).is_empty(),
        "{:#?}",
        schema_errors(&v, &doc)
    );

    // Help in JSON mode is an ok envelope carrying the help text.
    let out = sb.kb(&sb.path(), &["--json", "search", "--help"], &[]);
    assert_eq!(out.status.code(), Some(0));
    let doc = common::json(&out);
    assert_eq!(doc["ok"], json!(true));
    assert!(doc["result"]["help"].as_str().unwrap().contains("Usage"));
    assert!(
        schema_errors(&v, &doc).is_empty(),
        "{:#?}",
        schema_errors(&v, &doc)
    );

    // Without --json the usage error stays plain text on stderr and stdout is empty.
    let out = sb.kb(&sb.path(), &["context", "--bogus"], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

#[test]
fn schema_command_writes_then_checks_clean() {
    let sb = common::Sandbox::new();
    let root = sb.path().join("kb");
    common::copy_manifest(&root);
    let r = root.to_str().unwrap();
    let stale = root.join("core/schemas/old.v1.schema.json");
    common::write(&stale, "{}\n");
    common::write(&root.join("core/schemas/README.md"), "not a schema\n");

    let status = sb.kb(&sb.path(), &["--root", r, "schema"], &[]);
    assert_eq!(status.status.code(), Some(0), "status never fails");
    assert!(common::stdout(&status).contains("core/schemas/old.v1.schema.json: extra"));

    let write = sb.kb(
        &sb.path(),
        &["--root", r, "schema", "--write", "--json"],
        &[],
    );
    assert!(write.status.success(), "{}", common::stderr(&write));
    let changes = common::json(&write)["result"]["changes"].clone();
    assert!(
        changes
            .as_array()
            .unwrap()
            .contains(&json!("core/schemas/old.v1.schema.json: removed"))
    );
    assert!(!stale.exists());
    assert!(
        root.join("core/schemas/README.md").exists(),
        "non-schema files are kept"
    );

    let check = sb.kb(&sb.path(), &["--root", r, "schema", "--check"], &[]);
    assert_eq!(check.status.code(), Some(0), "{}", common::stderr(&check));
    let again = sb.kb(
        &sb.path(),
        &["--root", r, "schema", "--write", "--json"],
        &[],
    );
    assert_eq!(common::json(&again)["result"]["changes"], json!([]));

    // Hand edits are drift.
    let record = root.join("core/schemas/record.v1.schema.json");
    let text = fs::read_to_string(&record).unwrap();
    fs::write(&record, text.replace("\"const\": 1", "\"const\": 2")).unwrap();
    let check = sb.kb(&sb.path(), &["--root", r, "schema", "--check"], &[]);
    assert_eq!(check.status.code(), Some(42));
    assert!(common::stdout(&check).contains("core/schemas/record.v1.schema.json: drifted"));
}

// ---------------------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------------------

fn pattern_validator(pattern: &str, max_len: usize) -> jsonschema::Validator {
    jsonschema::validator_for(&json!({"type": "string", "pattern": pattern, "maxLength": max_len}))
        .unwrap()
}

/// Valid fixtures as (file name, text).
fn valid_fixture_texts() -> &'static [(String, String)] {
    static TEXTS: OnceLock<Vec<(String, String)>> = OnceLock::new();
    TEXTS.get_or_init(|| {
        md_files(&fixtures().join("valid"))
            .into_iter()
            .map(|p| {
                (
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    fs::read_to_string(&p).unwrap(),
                )
            })
            .collect()
    })
}

/// The record schema validator, compiled once.
fn record_validator() -> &'static jsonschema::Validator {
    static V: OnceLock<jsonschema::Validator> = OnceLock::new();
    V.get_or_init(|| validator("record.v1.schema.json"))
}

/// Every property name that appears anywhere in the record schema.
fn known_keys() -> &'static BTreeSet<String> {
    static KEYS: OnceLock<BTreeSet<String>> = OnceLock::new();
    KEYS.get_or_init(|| {
        fn walk(v: &Value, out: &mut BTreeSet<String>) {
            match v {
                Value::Object(m) => {
                    if let Some(Value::Object(props)) = m.get("properties") {
                        out.extend(props.keys().cloned());
                    }
                    m.values().for_each(|x| walk(x, out));
                }
                Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
                _ => {}
            }
        }
        let mut out = BTreeSet::new();
        walk(&schema("record.v1.schema.json"), &mut out);
        out
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn parse_record_never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = parse_record("fuzz.md", &bytes);
    }

    #[test]
    fn parse_record_never_panics_inside_front_matter(body in "[ -~\\n\\[\\]=\"#.]{0,400}") {
        let text = format!("+++\n{body}\n+++\n## x\n{body}\n");
        let _ = parse_record("fuzz.md", text.as_bytes());
        let text = format!("+++\nschema = 1\nkind = \"policy\"\n{body}\n+++\n");
        let _ = parse_record("fuzz.md", text.as_bytes());
    }

    #[test]
    fn unknown_keys_are_rejected_by_parser_and_schema(
        pick in 0usize..64,
        key in "[a-z][a-z0-9_]{0,10}",
        nested in any::<bool>(),
    ) {
        let fixtures = valid_fixture_texts();
        let (name, text) = &fixtures[pick % fixtures.len()];
        prop_assume!(!known_keys().contains(&key));
        // Top level (right after the opening `+++`) or inside the `[scope]` table.
        let mutated = if nested {
            text.replacen("[scope]\n", &format!("[scope]\n{key} = 1\n"), 1)
        } else {
            text.replacen("+++\n", &format!("+++\n{key} = \"x\"\n"), 1)
        };
        let diags = parse_record(name, mutated.as_bytes()).expect_err("unknown key accepted");
        prop_assert!(diags.iter().any(|d| d.code == "FRONT_MATTER_INVALID"), "{:?}", diags);
        let instance = front_matter_json(mutated.as_bytes()).unwrap();
        prop_assert!(!record_validator().is_valid(&instance));
    }

    #[test]
    fn duplicated_keys_are_rejected(pick in 0usize..64, line_pick in 0usize..16) {
        let fixtures = valid_fixture_texts();
        let (name, text) = &fixtures[pick % fixtures.len()];
        // Indexes of top-level `key = value` lines (before the first table header).
        let lines: Vec<&str> = text.lines().collect();
        let top: Vec<usize> = (1..lines.len())
            .take_while(|&i| !lines[i].starts_with('[') && lines[i] != "+++")
            .filter(|&i| lines[i].contains(" = "))
            .collect();
        let dup = top[line_pick % top.len()];
        let mut mutated_lines = lines.clone();
        mutated_lines.insert(dup + 1, lines[dup]);
        let mutated = mutated_lines.join("\n") + "\n";
        let diags = parse_record(name, mutated.as_bytes()).expect_err("duplicate key accepted");
        prop_assert_eq!(&diags[0].code, "FRONT_MATTER_SYNTAX");
        prop_assert!(front_matter_json(mutated.as_bytes()).is_none());
    }

    #[test]
    fn id_patterns_agree_with_runtime_checks(
        s in "[a-zA-Z0-9._-]{0,24}|[a-z][a-z0-9-]{0,6}(\\.[a-z0-9-]{0,6}){0,3}"
    ) {
        let record = pattern_validator(ids::RECORD_ID_PATTERN, ids::MAX_RECORD_ID);
        prop_assert_eq!(record.is_valid(&json!(s)), ids::check_record_id(&s).is_ok(), "record id {:?}", s);
        let registry = pattern_validator(ids::REGISTRY_ID_PATTERN, ids::MAX_REGISTRY_ID);
        prop_assert_eq!(registry.is_valid(&json!(s)), ids::check_registry_id(&s).is_ok(), "registry id {:?}", s);
        let local = pattern_validator(ids::LOCAL_ID_PATTERN, ids::MAX_LOCAL_ID);
        prop_assert_eq!(local.is_valid(&json!(s)), ids::check_local_id(&s).is_ok(), "local id {:?}", s);
        let ns = pattern_validator(ids::NAMESPACE_PATTERN, ids::MAX_NAMESPACE);
        prop_assert_eq!(ns.is_valid(&json!(s)), ids::check_namespace(&s).is_ok(), "namespace {:?}", s);
    }
}

#[test]
fn id_patterns_agree_on_length_limits() {
    let long_record = format!("acme.{}", "a".repeat(ids::MAX_RECORD_ID));
    let record = pattern_validator(ids::RECORD_ID_PATTERN, ids::MAX_RECORD_ID);
    assert!(ids::check_record_id(&long_record).is_err());
    assert!(!record.is_valid(&json!(long_record)));
    let edge = format!("acme.{}", "a".repeat(ids::MAX_RECORD_ID - 5));
    assert!(ids::check_record_id(&edge).is_ok());
    assert!(record.is_valid(&json!(edge)));
}
