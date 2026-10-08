//! Schema-2 additions and preservation of the published schema-1 vocabulary.
mod common;

use std::fs;

use kb::model::{Delivery, Record, date_days};
use kb::parse::{parse_record, split_front_matter};

fn feature() -> String {
    "+++\nschema = 2\nid = \"example.feature.queue\"\nkind = \"feature\"\ntitle = \"Synthetic queue\"\nstatus = \"draft\"\nowner = \"architecture\"\nfeature = \"queue\"\nsummary = \"Synthetic fixture.\"\nintroduced = \"2024-01-01\"\nretired = \"2026-01-01\"\nverified_at = \"2024-02-29\"\nreview_by = \"2024-03-01\"\nclocks = [\"Monotonic elapsed time\"]\ndata_sources = [\"Persisted local queue\"]\n[scope]\nproduct = true\n[[behaviors]]\nid = \"persist\"\ntext = \"Persist pending items.\"\n[[states]]\nid = \"idle\"\ntext = \"No item pending.\"\n[[states]]\nid = \"pending\"\ntext = \"An item awaits confirmation.\"\n[[transitions]]\nid = \"send\"\nfrom = \"idle\"\nto = \"pending\"\nwhen = \"The item is sent.\"\n[[scenarios]]\nid = \"timeout\"\ngiven = \"The response is lost.\"\nexpect = \"Keep pending state.\"\n+++\n## Evidence\n\nSynthetic body preserved.\n".into()
}

fn schema(version: u32) -> jsonschema::Validator {
    let text = kb::schema_export::generate()
        .remove(&format!("record.v{version}.schema.json"))
        .unwrap();
    jsonschema::validator_for(&serde_json::from_str::<serde_json::Value>(&text).unwrap()).unwrap()
}

fn instance(text: &str) -> serde_json::Value {
    let (fm, _, _) = split_front_matter(text).unwrap();
    let t: toml::Table = toml::from_str(fm).unwrap();
    serde_json::to_value(t).unwrap()
}

#[test]
fn subsystem_model_roundtrips_and_conforms() {
    let input = feature();
    let parsed = parse_record("synthetic.md", input.as_bytes()).unwrap();
    assert!(schema(2).is_valid(&instance(&input)));
    assert!(!schema(1).is_valid(&instance(&input)));
    let Record::Feature(f) = &parsed.record else {
        panic!()
    };
    assert_eq!(f.scenarios[0].expect, "Keep pending state.");
    assert_eq!(f.transitions[0].to, "pending");
    assert_eq!(f.states.len(), 2);
    let roundtrip = format!("+++\n{}+++\n", toml::to_string(&parsed.record).unwrap());
    assert_eq!(
        parse_record("roundtrip.md", roundtrip.as_bytes())
            .unwrap()
            .record,
        parsed.record
    );
}

#[test]
fn schema_one_rejects_even_empty_schema_two_fields() {
    let base = "+++\nschema = 1\nid = \"example.policy.safe\"\nkind = \"policy\"\ntitle = \"Synthetic policy\"\nstatus = \"draft\"\nowner = \"architecture\"\n{extra}\n[scope]\nproduct = true\n[[rules]]\nid = \"keep\"\nlevel = \"must\"\ntext = \"Keep data.\"\n+++\n";
    for extra in [
        "delivery = \"scoped\"",
        "introduced = \"2024-01-01\"",
        "scenarios = []",
    ] {
        let text = base.replace("{extra}", extra);
        assert!(
            parse_record("v1.md", text.as_bytes())
                .unwrap_err()
                .iter()
                .any(|d| d.code == "SCHEMA_FIELD_UNAVAILABLE")
        );
        assert!(!schema(1).is_valid(&instance(&text)));
    }
    let text = base.replace("{extra}", "");
    let p = parse_record("v1.md", text.as_bytes()).unwrap();
    assert_eq!(p.record.delivery(), Delivery::Scoped);
    assert!(schema(1).is_valid(&instance(&text)));
}

#[test]
fn invalid_domain_and_temporal_values_are_rejected() {
    for (before, after, code) in [
        ("2024-02-29", "2023-02-29", "TEMPORAL_INVALID"),
        (
            "retired = \"2026-01-01\"",
            "retired = \"2023-01-01\"",
            "TEMPORAL_ORDER",
        ),
        (
            "retired = \"2026-01-01\"",
            "retired = \"abcdef01\"",
            "TEMPORAL_KIND_MISMATCH",
        ),
        (
            "review_by = \"2024-03-01\"",
            "review_by = \"2024-02-28\"",
            "FRESHNESS_ORDER",
        ),
        (
            "to = \"pending\"",
            "to = \"missing\"",
            "TRANSITION_STATE_UNKNOWN",
        ),
        (
            "expect = \"Keep pending state.\"",
            "expect = \" \"",
            "TEXT_EMPTY",
        ),
    ] {
        let text = feature().replace(before, after);
        let errors = parse_record("invalid.md", text.as_bytes()).unwrap_err();
        assert!(
            errors.iter().any(|d| d.code == code),
            "wanted {code}: {errors:?}"
        );
    }
}

#[test]
fn declarative_probes_are_strict_and_never_commands() {
    let text = "+++\nschema = 2\nid = \"example.policy.safe\"\nkind = \"policy\"\ntitle = \"Synthetic\"\nstatus = \"draft\"\nowner = \"architecture\"\ndelivery = \"always\"\n[scope]\nproduct = true\n[[rules]]\nid = \"branch\"\nlevel = \"must\"\ntext = \"Use the named branch convention.\"\n[[rules.verify]]\nkind = \"branch-name\"\npattern = \"^feature/\"\n+++\n";
    let p = parse_record("probes.md", text.as_bytes()).unwrap();
    assert_eq!(p.record.normative()[0].verify.len(), 1);
    assert!(schema(2).is_valid(&instance(text)));
    for replacement in ["pattern = \"[\"", "command = \"echo dangerous\""] {
        assert!(
            parse_record(
                "bad.md",
                text.replace("pattern = \"^feature/\"", replacement)
                    .as_bytes()
            )
            .is_err()
        );
    }
    assert!(
        parse_record(
            "bad-scope.md",
            text.replace("product = true", "repos = [\"mobile\"]")
                .as_bytes()
        )
        .unwrap_err()
        .iter()
        .any(|d| d.code == "DELIVERY_SCOPE_INVALID")
    );
}

#[test]
fn calendar_dates_respect_leap_centuries_and_epoch() {
    assert_eq!(date_days("1970-01-01"), Some(0));
    assert_eq!(date_days("1969-12-31"), Some(-1));
    assert_eq!(
        date_days("2000-03-01").unwrap() - date_days("2000-02-28").unwrap(),
        2
    );
    for invalid in [
        "1900-02-29",
        "2100-02-29",
        "2024-04-31",
        "0000-01-01",
        "2024-1-01",
        "2024-01-01Z",
        "２０２４-01-01",
    ] {
        assert_eq!(date_days(invalid), None, "{invalid}");
    }
    for year in [1, 1900, 1970, 2000, 2024, 9999] {
        for month in 1..=12 {
            let date = format!("{year:04}-{month:02}-01");
            assert_eq!(
                kb::model::date_from_days(date_days(&date).unwrap()),
                Some(date)
            );
        }
    }
}

#[test]
fn adjacent_migration_preserves_schema_one_content_comments_and_body() {
    let migration = kb::migrate::chain(1, 2).unwrap()[0];
    let base = common::repo_root().join("core/migrations/fixtures");
    for relative in [
        "knowledge/policies/logging.md",
        "knowledge/policies/token-storage.md",
        "knowledge/contracts/token-api.md",
        "knowledge/features/login.md",
        "knowledge/decisions/token-rotation.md",
    ] {
        let before = fs::read_to_string(base.join("v1-expected/project").join(relative)).unwrap();
        let after = (migration.record)(&before).unwrap();
        assert_eq!(
            after,
            fs::read_to_string(base.join("v2-expected/project").join(relative)).unwrap()
        );
        assert_eq!(
            split_front_matter(&before).unwrap().1,
            split_front_matter(&after).unwrap().1
        );
        let mut a =
            serde_json::to_value(parse_record(relative, before.as_bytes()).unwrap()).unwrap();
        let b = serde_json::to_value(parse_record(relative, after.as_bytes()).unwrap()).unwrap();
        a["record"]["schema"] = serde_json::json!(2);
        assert_eq!(a, b, "migration must not invent or discard facts");
    }
    let config = "# synthetic\nschema = 1 # keep this\n";
    assert_eq!(
        (migration.registry)(config).unwrap(),
        "# synthetic\nschema = 2 # keep this\n"
    );
}
