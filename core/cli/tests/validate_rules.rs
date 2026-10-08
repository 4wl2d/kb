//! Cross-record validation rules (docs/architecture.md §3–§4), one test per rule family,
//! on temporary corpora loaded through the real working-tree source.
mod common;

use std::fs;
use std::path::{Path, PathBuf};

use kb::corpus::{Corpus, load_corpus};
use kb::diag::{Diagnostic, Severity};
use kb::model::{Profile, ProfileLocation};
use kb::output::Format;
use kb::source::WorkingTreeSource;
use kb::validate::{
    meta_inputs, render_report, validate_against_base, validate_corpus, validate_metas,
    validate_templates,
};

/// A temporary KB with the minimal project (namespace `acme`, repos mobile + backend).
struct Kb {
    _sandbox: common::Sandbox,
    root: PathBuf,
}

impl Kb {
    fn new() -> Kb {
        let sandbox = common::Sandbox::new();
        let root = sandbox.path().join("kb");
        common::write_min_project(&root);
        Kb {
            _sandbox: sandbox,
            root,
        }
    }

    /// Write a record under `project/knowledge/`.
    fn add(&self, rel: &str, text: &str) -> &Kb {
        common::write(&self.root.join("project/knowledge").join(rel), text);
        self
    }

    fn registry(&self, file: &str, text: &str) -> &Kb {
        common::write(&self.root.join("project/registry").join(file), text);
        self
    }

    fn corpus(&self) -> Corpus {
        let c = load_corpus(
            &WorkingTreeSource::new(&self.root),
            &ProfileLocation::for_profile(Profile::Project),
        )
        .unwrap();
        assert!(
            c.diagnostics.is_empty(),
            "fixture records must parse: {:#?}",
            c.diagnostics
        );
        c
    }

    /// Cross-record diagnostics of the whole corpus.
    fn check(&self) -> Vec<Diagnostic> {
        let c = self.corpus();
        validate_metas(&c.config, &c.registry, &meta_inputs(&c))
    }
}

/// Record text with the common header; `top` holds extra top-level keys, `tables` the
/// kind-specific tables (and `[links]` etc.).
fn doc(
    id: &str,
    kind: &str,
    status: &str,
    owner: &str,
    top: &str,
    scope: &str,
    tables: &str,
) -> String {
    format!(
        "+++\nschema = 1\nid = \"{id}\"\nkind = \"{kind}\"\ntitle = \"Record {id}\"\nstatus = \"{status}\"\nowner = \"{owner}\"\n{top}\n[scope]\n{scope}\n{tables}\n+++\n"
    )
}

const STATEMENT: &str = "[[statements]]\nid = \"s\"\nlevel = \"must\"\ntext = \"Hold.\"\n";

/// An invariant owned by `arch` with the given scope and extra tables (e.g. `[links]`).
fn inv(id: &str, status: &str, scope: &str, tables: &str) -> String {
    doc(
        id,
        "invariant",
        status,
        "arch",
        "",
        scope,
        &format!("{tables}\n{STATEMENT}"),
    )
}

const PRODUCT: &str = "product = true";
const MOBILE: &str = "repos = [\"mobile\"]";

fn with_code<'a>(d: &'a [Diagnostic], code: &str) -> Vec<&'a Diagnostic> {
    d.iter().filter(|x| x.code == code).collect()
}

fn codes(d: &[Diagnostic]) -> Vec<(String, String)> {
    d.iter()
        .map(|x| (x.code.clone(), x.record.clone().unwrap_or_default()))
        .collect()
}

/// Exactly the given (code, record) pairs, in any order.
fn assert_exactly(d: &[Diagnostic], want: &[(&str, &str)]) {
    let mut got = codes(d);
    got.sort();
    let mut want: Vec<(String, String)> = want
        .iter()
        .map(|(c, r)| (c.to_string(), r.to_string()))
        .collect();
    want.sort();
    assert_eq!(got, want, "diagnostics: {d:#?}");
}

#[test]
fn minimal_project_and_valid_fixture_set_are_clean() {
    let kb = Kb::new();
    let report = validate_corpus(&kb.corpus());
    assert!(report.is_ok(true), "{report:#?}");
    assert_eq!((report.files, report.records), (2, 2));

    // The valid record fixtures form a consistent corpus against the minimal registry.
    let kb = Kb::new();
    let knowledge = kb.root.join("project/knowledge");
    fs::remove_file(knowledge.join("policies/token-storage.md")).unwrap();
    fs::remove_file(knowledge.join("contracts/token-api.md")).unwrap();
    let fixtures = common::repo_root().join("core/tests/fixtures/validate/records/valid");
    for e in fs::read_dir(fixtures).unwrap() {
        let p = e.unwrap().path();
        fs::copy(&p, knowledge.join(p.file_name().unwrap())).unwrap();
    }
    let report = validate_corpus(&kb.corpus());
    assert!(report.is_ok(true), "{:#?}", report.diagnostics);
    assert_eq!(report.records, 10);
}

#[test]
fn duplicate_ids_name_every_defining_file() {
    let kb = Kb::new();
    kb.add(
        "b/dup.md",
        &inv("acme.mobile.token-storage", "accepted", MOBILE, ""),
    );
    let d = kb.check();
    let dups = with_code(&d, "DUPLICATE_ID");
    assert_eq!(dups.len(), 2, "{d:#?}");
    let paths: Vec<&str> = dups.iter().map(|x| x.path.as_deref().unwrap()).collect();
    assert_eq!(
        paths,
        [
            "project/knowledge/b/dup.md",
            "project/knowledge/policies/token-storage.md"
        ]
    );
    for x in dups {
        assert!(x.message.contains("project/knowledge/b/dup.md"));
        assert!(
            x.message
                .contains("project/knowledge/policies/token-storage.md")
        );
        assert_eq!(x.record.as_deref(), Some("acme.mobile.token-storage"));
    }
}

#[test]
fn namespace_must_match_the_profile() {
    let kb = Kb::new();
    kb.add("x.md", &inv("other.thing", "accepted", PRODUCT, ""));
    assert_exactly(&kb.check(), &[("NAMESPACE_MISMATCH", "other.thing")]);
}

#[test]
fn owners_must_exist_and_be_authorized() {
    let kb = Kb::new();
    let owned = |id: &str, owner: &str, scope: &str| {
        doc(id, "invariant", "accepted", owner, "", scope, STATEMENT)
    };
    kb.add("a.md", &owned("acme.o.unknown", "nobody", MOBILE))
        // team-mobile is restricted to the mobile repo.
        .add(
            "b.md",
            &owned("acme.o.backend", "team-mobile", "repos = [\"backend\"]"),
        )
        .add("c.md", &owned("acme.o.product", "team-mobile", PRODUCT))
        // `login` is implemented in mobile and backend.
        .add(
            "d.md",
            &owned("acme.o.feature", "team-mobile", "features = [\"login\"]"),
        )
        // Implied repos from a module stay within the restriction.
        .add(
            "e.md",
            &owned(
                "acme.o.module",
                "team-mobile",
                "modules = [\"mobile.auth\"]",
            ),
        )
        // Unrestricted product owner.
        .add(
            "f.md",
            &owned("acme.o.arch", "arch", "repos = [\"backend\"]"),
        );
    let d = kb.check();
    assert_exactly(
        &d,
        &[
            ("OWNER_UNKNOWN", "acme.o.unknown"),
            ("OWNER_NOT_AUTHORIZED", "acme.o.backend"),
            ("OWNER_NOT_AUTHORIZED", "acme.o.product"),
            ("OWNER_NOT_AUTHORIZED", "acme.o.feature"),
        ],
    );
    let product = with_code(&d, "OWNER_NOT_AUTHORIZED")
        .into_iter()
        .find(|x| x.record.as_deref() == Some("acme.o.product"))
        .unwrap();
    assert!(product.message.contains("product = true"));
}

#[test]
fn registry_references_must_exist() {
    let kb = Kb::new();
    kb.add(
        "refs.md",
        &doc(
            "acme.refs.all",
            "policy",
            "accepted",
            "arch",
            "",
            "repos = [\"mobile\", \"web\"]\nmodules = [\"mobile.auth\", \"mobile.ghost\"]\nfeatures = [\"login\", \"checkout\"]",
            r#"[selectors]
concepts = ["auth-token", "no-such-concept"]
paths = ["web:src/**", "mobile:app/**", "app/**"]

[applicability]
versions = { mobile = ">=1.0.0", desktop = "^2" }

[[anchors]]
kind = "source"
repo = "tv"
path = "src/main.rs"

[[settings]]
name = "limit"
type = "integer"
value = 3
override = "any"
override_owners = ["arch", "ghost-team"]
"#,
        ),
    )
    .add(
        "feature.md",
        &doc(
            "acme.feature.search",
            "feature",
            "accepted",
            "arch",
            "feature = \"search\"\nsummary = \"Search.\"",
            MOBILE,
            "[[behaviors]]\nid = \"find\"\ntext = \"Finds.\"",
        ),
    );
    let d = kb.check();
    let messages: Vec<String> = d
        .iter()
        .map(|x| format!("{} {}", x.code, x.message))
        .collect();
    for want in [
        "UNKNOWN_REPO scope.repos: unknown repo `web`",
        "UNKNOWN_MODULE scope.modules: unknown module `mobile.ghost`",
        "UNKNOWN_FEATURE scope.features: unknown feature `checkout`",
        "UNKNOWN_CONCEPT selectors.concepts: unknown concept `no-such-concept`",
        "UNKNOWN_REPO selectors.paths: `web:src/**` names unknown repo `web`",
        "UNKNOWN_REPO applicability.versions: unknown repo `desktop`",
        "UNKNOWN_REPO anchors[0].repo: unknown repo `tv`",
        "OWNER_UNKNOWN settings.limit.override_owners: unknown owner `ghost-team`",
        "UNKNOWN_FEATURE feature: `search` is not in registry/features.toml",
    ] {
        assert!(
            messages.iter().any(|m| m == want),
            "missing `{want}` in {messages:#?}"
        );
    }
    assert_eq!(d.len(), 9, "{messages:#?}");
}

#[test]
fn verify_probe_globs_must_name_registered_repos() {
    let kb = Kb::new();
    // A qualifier is checked as written: `Mobile` is not id-shaped, so the glob would
    // otherwise be read as a literal pattern that never matches. A `:` inside a class or
    // brace group is pattern text, not a qualifier.
    let rules = r#"[selectors]
paths = ["Mobile:app/**", "docs/a:b.md", "[a:b]/x.md", "{a:b,c}/x.md"]

[[rules]]
id = "no-print"
level = "must-not"
text = "Print to standard output."

[[rules.verify]]
kind = "banned-api"
paths = ["mobil:app/**", "mobile:app/**", "app/**", "Mobile:app/**", "app/x:y/**", "x[:]y.md", "{mobile,backend}:app/**"]
pattern = "println"

[[rules.verify]]
kind = "naming"
paths = ["backend:src/**"]
pattern = "^[a-z]"

[[rules.verify]]
kind = "forbidden-import"
from = ["mobile:app/ui/**"]
to = ["bakend:src/db/**"]
"#;
    let text = doc(
        "acme.p.probes",
        "policy",
        "accepted",
        "arch",
        "",
        PRODUCT,
        rules,
    )
    .replacen("schema = 1", "schema = 2", 1);
    kb.add("probes.md", &text);
    let messages: Vec<String> = kb
        .check()
        .iter()
        .map(|x| format!("{} {}", x.code, x.message))
        .collect();
    assert_eq!(
        messages,
        [
            "UNKNOWN_REPO selectors.paths: `Mobile:app/**` names unknown repo `Mobile`",
            "UNKNOWN_REPO verify: `Mobile:app/**` names unknown repo `Mobile`",
            "UNKNOWN_REPO verify: `bakend:src/db/**` names unknown repo `bakend`",
            "UNKNOWN_REPO verify: `mobil:app/**` names unknown repo `mobil`",
            "UNKNOWN_REPO verify: `{mobile,backend}:app/**` names unknown repo `{mobile,backend}`",
        ]
    );
}

#[test]
fn unsatisfiable_scopes_are_errors() {
    let kb = Kb::new();
    kb.registry(
        "features.toml",
        r#"schema = 1
[[feature]]
id = "login"
title = "Login"
repos = ["mobile", "backend"]
[[feature]]
id = "billing"
title = "Billing"
repos = ["backend"]
[[feature]]
id = "anywhere"
title = "Not tied to repos"
"#,
    );
    kb.add(
        "a.md",
        &inv(
            "acme.s.module-outside",
            "accepted",
            "repos = [\"backend\"]\nmodules = [\"mobile.auth\"]",
            "",
        ),
    )
    .add(
        "b.md",
        &inv(
            "acme.s.feature-outside",
            "accepted",
            "repos = [\"mobile\"]\nfeatures = [\"billing\"]",
            "",
        ),
    )
    .add(
        "c.md",
        &inv(
            "acme.s.disjoint-dims",
            "accepted",
            "modules = [\"mobile.auth\"]\nfeatures = [\"billing\"]",
            "",
        ),
    )
    .add(
        "d.md",
        &inv(
            "acme.s.ok",
            "accepted",
            "repos = [\"mobile\"]\nmodules = [\"mobile.auth\"]\nfeatures = [\"login\", \"anywhere\"]",
            "",
        ),
    );
    let d = kb.check();
    assert_exactly(
        &d,
        &[
            ("SCOPE_UNSATISFIABLE", "acme.s.module-outside"),
            ("SCOPE_UNSATISFIABLE", "acme.s.feature-outside"),
            ("SCOPE_UNSATISFIABLE", "acme.s.disjoint-dims"),
        ],
    );
    assert!(d.iter().any(|x| {
        x.message
            .contains("module `mobile.auth` belongs to repo `mobile`")
    }));
}

#[test]
fn contract_parties_must_match_the_registry_and_scope() {
    let kb = Kb::new();
    let contract = |id: &str, scope: &str, parties: &str| {
        doc(
            id,
            "contract",
            "accepted",
            "arch",
            "",
            scope,
            &format!(
                "{parties}\n[[obligations]]\nid = \"o\"\nparty = \"a\"\nlevel = \"must\"\ntext = \"Call.\"\n"
            ),
        )
    };
    let party = |id: &str, repo: &str, modules: &str| {
        format!(
            "[[parties]]\nid = \"{id}\"\nrepo = \"{repo}\"\nmodules = [{modules}]\nrole = \"Role\"\n"
        )
    };
    kb.add(
        "k1.md",
        &contract(
            "acme.k.unknown",
            "repos = [\"mobile\", \"backend\"]",
            &(party("a", "mobile", "\"mobile.nope\"") + &party("b", "desktop", "")),
        ),
    )
    .add(
        "k2.md",
        &contract(
            "acme.k.mismatch",
            "repos = [\"mobile\", \"backend\"]",
            &(party("a", "mobile", "\"backend.api\"") + &party("b", "backend", "\"backend.api\"")),
        ),
    )
    .add(
        "k3.md",
        &contract(
            "acme.k.out-of-scope",
            MOBILE,
            &(party("a", "mobile", "") + &party("b", "backend", "")),
        ),
    )
    .add(
        "k4.md",
        &contract(
            "acme.k.product",
            PRODUCT,
            &(party("a", "mobile", "\"mobile.auth\"") + &party("b", "backend", "")),
        ),
    );
    let d = kb.check();
    assert_exactly(
        &d,
        &[
            ("UNKNOWN_MODULE", "acme.k.unknown"),
            ("UNKNOWN_REPO", "acme.k.unknown"),
            ("CONTRACT_PARTY_MODULE_MISMATCH", "acme.k.mismatch"),
            ("CONTRACT_PARTY_OUT_OF_SCOPE", "acme.k.out-of-scope"),
        ],
    );
}

#[test]
fn every_link_target_must_exist() {
    let kb = Kb::new();
    kb.add(
        "links.md",
        &inv(
            "acme.l.links",
            "draft",
            PRODUCT,
            r#"[links]
requires = ["acme.contract.token-api", "acme.l.missing-a"]
rationale = ["acme.l.missing-b"]
related = ["acme.l.missing-c"]
supersedes = ["acme.l.missing-d"]"#,
        ),
    )
    .add(
        "gap.md",
        &doc(
            "acme.l.gap",
            "gap",
            "accepted",
            "arch",
            "gap = \"missing\"\ndescription = \"Unknown.\"\naffects = [\"acme.mobile.token-storage\", \"acme.l.missing-e\"]",
            PRODUCT,
            "",
        ),
    );
    let d = kb.check();
    let dangling = with_code(&d, "DANGLING_LINK");
    let messages: Vec<&str> = dangling.iter().map(|x| x.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "affects: target `acme.l.missing-e` does not exist",
            "links.rationale: target `acme.l.missing-b` does not exist",
            "links.related: target `acme.l.missing-c` does not exist",
            "links.requires: target `acme.l.missing-a` does not exist",
            "links.supersedes: target `acme.l.missing-d` does not exist",
        ]
    );
    assert_eq!(d.len(), 5, "{d:#?}");
}

#[test]
fn requires_cycles_are_reported_once_and_deterministically() {
    let kb = Kb::new();
    let req = |targets: &str| format!("[links]\nrequires = [{targets}]");
    kb.add(
        "c1.md",
        &inv("acme.c.b", "accepted", PRODUCT, &req("\"acme.c.c\"")),
    )
    .add(
        "c2.md",
        &inv("acme.c.c", "accepted", PRODUCT, &req("\"acme.c.a\"")),
    )
    .add(
        "c3.md",
        &inv(
            "acme.c.a",
            "accepted",
            PRODUCT,
            &req("\"acme.c.b\", \"acme.c.x\""),
        ),
    )
    // Not on the cycle, but reaches it.
    .add(
        "c4.md",
        &inv("acme.c.d", "accepted", PRODUCT, &req("\"acme.c.a\"")),
    )
    // A second, independent two-node cycle.
    .add(
        "c5.md",
        &inv("acme.c.x", "accepted", PRODUCT, &req("\"acme.c.y\"")),
    )
    .add(
        "c6.md",
        &inv("acme.c.y", "accepted", PRODUCT, &req("\"acme.c.x\"")),
    );
    let c = kb.corpus();
    let metas = meta_inputs(&c);
    let d = validate_metas(&c.config, &c.registry, &metas);
    let cycles = with_code(&d, "REQUIRES_CYCLE");
    let messages: Vec<&str> = cycles.iter().map(|x| x.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "requires cycle: acme.c.a -> acme.c.b -> acme.c.c -> acme.c.a",
            "requires cycle: acme.c.x -> acme.c.y -> acme.c.x",
        ]
    );
    assert_eq!(cycles[0].path.as_deref(), Some("project/knowledge/c3.md"));
    assert_eq!(d.len(), 2, "{d:#?}");
    // Input order does not change the result.
    let mut reversed = metas.clone();
    reversed.reverse();
    assert_eq!(validate_metas(&c.config, &c.registry, &reversed), d);
}

#[test]
fn supersedes_cycles_are_reported() {
    let kb = Kb::new();
    let sup = |t: &str| format!("[links]\nsupersedes = [\"{t}\"]");
    kb.add(
        "s1.md",
        &inv("acme.v.one", "superseded", PRODUCT, &sup("acme.v.two")),
    )
    .add(
        "s2.md",
        &inv("acme.v.two", "superseded", PRODUCT, &sup("acme.v.one")),
    );
    let d = kb.check();
    assert_exactly(&d, &[("SUPERSEDES_CYCLE", "acme.v.one")]);
    assert_eq!(
        d[0].message,
        "supersedes cycle: acme.v.one -> acme.v.two -> acme.v.one"
    );
}

#[test]
fn lifecycle_rules() {
    let kb = Kb::new();
    let links = |rel: &str, targets: &str| format!("[links]\n{rel} = [{targets}]");
    kb.add("l1.md", &inv("acme.lc.draft", "draft", PRODUCT, ""))
        .add(
            "l2.md",
            &inv("acme.lc.deprecated", "deprecated", PRODUCT, ""),
        )
        .add("l3.md", &inv("acme.lc.old", "superseded", PRODUCT, ""))
        .add(
            "l4.md",
            &inv(
                "acme.lc.new",
                "accepted",
                PRODUCT,
                &links("supersedes", "\"acme.lc.old\", \"acme.lc.still-accepted\""),
            ),
        )
        .add(
            "l5.md",
            &inv("acme.lc.still-accepted", "accepted", PRODUCT, ""),
        )
        .add("l6.md", &inv("acme.lc.orphan", "superseded", PRODUCT, ""))
        .add(
            "l7.md",
            &inv(
                "acme.lc.user",
                "accepted",
                PRODUCT,
                &links(
                    "requires",
                    "\"acme.lc.draft\", \"acme.lc.old\", \"acme.lc.deprecated\"",
                ),
            ),
        )
        // Drafts may depend on drafts.
        .add(
            "l8.md",
            &inv(
                "acme.lc.draft-user",
                "draft",
                PRODUCT,
                &links("requires", "\"acme.lc.draft\""),
            ),
        );
    let d = kb.check();
    assert_exactly(
        &d,
        &[
            ("SUPERSEDED_STATUS", "acme.lc.still-accepted"),
            ("SUPERSEDED_ORPHAN", "acme.lc.orphan"),
            ("REQUIRES_NOT_ACCEPTED", "acme.lc.user"),
            ("REQUIRES_NOT_ACCEPTED", "acme.lc.user"),
            ("REQUIRES_DEPRECATED", "acme.lc.user"),
        ],
    );
    let deprecated = with_code(&d, "REQUIRES_DEPRECATED")[0];
    assert_eq!(deprecated.severity, Severity::Warning);
    assert!(
        with_code(&d, "SUPERSEDED_STATUS")[0]
            .message
            .contains("acme.lc.new")
    );
}

/// A product-wide base policy with one setting per stricter direction plus `any` and
/// `forbidden` settings, owned by `arch`.
fn base_policy() -> String {
    let setting = |name: &str, ty: &str, value: &str, mode: &str, stricter: Option<&str>| {
        let dir = stricter
            .map(|s| format!("stricter = \"{s}\"\n"))
            .unwrap_or_default();
        format!(
            "[[settings]]\nname = \"{name}\"\ntype = \"{ty}\"\nvalue = {value}\noverride = \"{mode}\"\n{dir}"
        )
    };
    let settings = [
        setting("max-age", "integer", "30", "stricter", Some("lower")),
        setting("min-tls", "integer", "12", "stricter", Some("higher")),
        setting("pinning", "boolean", "false", "stricter", Some("true")),
        setting("strict-mode", "boolean", "true", "stricter", Some("true")),
        setting("debug-menu", "boolean", "true", "stricter", Some("false")),
        setting("telemetry", "boolean", "false", "stricter", Some("false")),
        setting(
            "audit",
            "string-set",
            "[\"login\"]",
            "stricter",
            Some("superset"),
        ),
        setting(
            "ciphers",
            "string-set",
            "[\"aes\", \"chacha\"]",
            "stricter",
            Some("subset"),
        ),
        setting("log-level", "string", "\"warn\"", "any", None),
        "[[settings]]\nname = \"frozen\"\ntype = \"integer\"\nvalue = 1\n".to_string(),
    ];
    doc(
        "acme.base.limits",
        "policy",
        "accepted",
        "arch",
        "",
        PRODUCT,
        &settings.join("\n"),
    )
}

/// A mobile policy overriding one setting of `acme.base.limits`.
fn override_policy(id: &str, setting: &str, value: &str) -> String {
    doc(
        id,
        "policy",
        "accepted",
        "arch",
        "",
        MOBILE,
        &format!(
            "[[overrides]]\ntarget = \"acme.base.limits#{setting}\"\nvalue = {value}\nreason = \"Test.\"\n"
        ),
    )
}

#[test]
fn allowed_overrides_are_clean() {
    let kb = Kb::new();
    kb.add("base.md", &base_policy())
        .add(
            "o1.md",
            &override_policy("acme.ok.max-age", "max-age", "20"),
        )
        .add(
            "o2.md",
            &override_policy("acme.ok.log-level", "log-level", "\"debug\""),
        )
        .add(
            "o3.md",
            &override_policy("acme.ok.ciphers", "ciphers", "[\"aes\"]"),
        );
    assert_exactly(&kb.check(), &[]);
}

#[test]
fn override_targets_must_be_overridable_settings_of_the_right_type() {
    let kb = Kb::new();
    let ov = |id: &str, target: &str, value: &str| {
        doc(
            id,
            "policy",
            "accepted",
            "arch",
            "",
            MOBILE,
            &format!("[[overrides]]\ntarget = \"{target}\"\nvalue = {value}\nreason = \"Test.\"\n"),
        )
    };
    kb.add("base.md", &base_policy())
        .add(
            "t1.md",
            &ov("acme.t.no-record", "acme.base.nothing#max-age", "1"),
        )
        .add(
            "t2.md",
            &ov("acme.t.not-policy", "acme.contract.token-api#max-age", "1"),
        )
        .add(
            "t3.md",
            &ov("acme.t.no-setting", "acme.base.limits#nope", "1"),
        )
        .add(
            "t4.md",
            &ov("acme.t.forbidden", "acme.base.limits#frozen", "2"),
        )
        .add(
            "t5.md",
            &ov("acme.t.type", "acme.base.limits#max-age", "\"ten\""),
        );
    let d = kb.check();
    assert_exactly(
        &d,
        &[
            ("OVERRIDE_TARGET_UNKNOWN", "acme.t.no-record"),
            ("OVERRIDE_TARGET_UNKNOWN", "acme.t.not-policy"),
            ("OVERRIDE_TARGET_UNKNOWN", "acme.t.no-setting"),
            ("OVERRIDE_FORBIDDEN", "acme.t.forbidden"),
            ("OVERRIDE_TYPE_MISMATCH", "acme.t.type"),
        ],
    );
}

#[test]
fn stricter_overrides_may_not_weaken_in_any_direction() {
    let cases: [(&str, &str, bool); 16] = [
        ("max-age", "20", false),
        ("max-age", "30", false),
        ("max-age", "31", true),
        ("min-tls", "13", false),
        ("min-tls", "11", true),
        ("pinning", "true", false),
        ("pinning", "false", false),
        ("strict-mode", "false", true),
        ("debug-menu", "false", false),
        ("debug-menu", "true", false),
        ("telemetry", "true", true),
        ("audit", "[\"login\", \"logout\"]", false),
        ("audit", "[\"logout\"]", true),
        ("ciphers", "[\"aes\"]", false),
        ("ciphers", "[\"chacha\", \"aes\"]", false),
        ("ciphers", "[\"aes\", \"rc4\"]", true),
    ];
    // One KB per case: several valid overrides of one setting with the same scope and
    // different values would be OVERRIDE_AMBIGUOUS.
    for (i, (setting, value, weak)) in cases.iter().enumerate() {
        let kb = Kb::new();
        let id = format!("acme.w.case{i}");
        kb.add("base.md", &base_policy())
            .add("w.md", &override_policy(&id, setting, value));
        let want: &[(&str, &str)] = if *weak {
            &[("OVERRIDE_WEAKENS", id.as_str())]
        } else {
            &[]
        };
        assert_exactly(&kb.check(), want);
    }
}

#[test]
fn overrides_respect_target_scope_and_override_owners() {
    let kb = Kb::new();
    kb.add(
        "base.md",
        &doc(
            "acme.base.mobile",
            "policy",
            "accepted",
            "team-mobile",
            "",
            MOBILE,
            r#"[[settings]]
name = "retries"
type = "integer"
value = 3
override = "any"
override_owners = ["team-mobile"]

[[settings]]
name = "timeout"
type = "integer"
value = 30
override = "any"
"#,
        ),
    );
    let ov = |id: &str, owner: &str, scope: &str, setting: &str| {
        doc(
            id,
            "policy",
            "accepted",
            owner,
            "",
            scope,
            &format!(
                "[[overrides]]\ntarget = \"acme.base.mobile#{setting}\"\nvalue = 1\nreason = \"Test.\"\n"
            ),
        )
    };
    kb.add(
        "a.md",
        &ov(
            "acme.sa.module",
            "team-mobile",
            "modules = [\"mobile.auth\"]",
            "retries",
        ),
    )
    .add(
        "b.md",
        &ov(
            "acme.sa.backend",
            "team-backend",
            "repos = [\"backend\"]",
            "timeout",
        ),
    )
    .add("c.md", &ov("acme.sa.product", "arch", PRODUCT, "timeout"))
    .add("d.md", &ov("acme.sa.owner", "arch", MOBILE, "retries"));
    let d = kb.check();
    assert_exactly(
        &d,
        &[
            ("OVERRIDE_SCOPE_EXCEEDS", "acme.sa.backend"),
            ("OVERRIDE_SCOPE_EXCEEDS", "acme.sa.product"),
            ("OVERRIDE_NOT_AUTHORIZED", "acme.sa.owner"),
        ],
    );
}

#[test]
fn conflicting_overrides_with_overlapping_scopes_are_ambiguous() {
    let kb = Kb::new();
    let ov = |id: &str, status: &str, scope: &str, value: &str| {
        doc(
            id,
            "policy",
            status,
            "arch",
            "",
            scope,
            &format!(
                "[[overrides]]\ntarget = \"acme.base.limits#log-level\"\nvalue = \"{value}\"\nreason = \"Test.\"\n"
            ),
        )
    };
    kb.add("base.md", &base_policy())
        .add("a1.md", &ov("acme.amb.mobile", "accepted", MOBILE, "debug"))
        .add(
            "a2.md",
            &ov(
                "acme.amb.login",
                "accepted",
                "features = [\"login\"]",
                "trace",
            ),
        )
        // More specific than `mobile`: not ambiguous with it; same value as `login`.
        .add(
            "a3.md",
            &ov(
                "acme.amb.auth",
                "accepted",
                "modules = [\"mobile.auth\"]",
                "trace",
            ),
        )
        // Disjoint from `mobile`; same value as `login`.
        .add(
            "a4.md",
            &ov(
                "acme.amb.backend",
                "accepted",
                "repos = [\"backend\"]",
                "trace",
            ),
        )
        // Drafts are not effective and never ambiguous.
        .add(
            "a5.md",
            &ov("acme.amb.draft", "draft", "features = [\"login\"]", "error"),
        );
    let d = kb.check();
    assert_exactly(&d, &[("OVERRIDE_AMBIGUOUS", "acme.amb.mobile")]);
    assert!(d[0].message.contains("acme.amb.login"));
    assert!(d[0].message.contains("project/knowledge/a2.md"));
}

#[test]
fn conflicting_overrides_with_equivalent_scopes_are_ambiguous() {
    let kb = Kb::new();
    let ov = |id: &str, scope: &str, setting: &str, value: &str| {
        doc(
            id,
            "policy",
            "accepted",
            "arch",
            "",
            scope,
            &format!(
                "[[overrides]]\ntarget = \"acme.base.limits#{setting}\"\nvalue = {value}\nreason = \"Test.\"\n"
            ),
        )
    };
    let auth = "modules = [\"mobile.auth\"]";
    kb.add("base.md", &base_policy())
        // Identical scopes, different values: context could never pick one.
        .add(
            "e1.md",
            &ov("acme.eq.first", auth, "log-level", "\"debug\""),
        )
        .add(
            "e2.md",
            &ov("acme.eq.second", auth, "log-level", "\"trace\""),
        )
        // Textually different but mutually subsuming scopes (mobile.auth implies mobile).
        .add("f1.md", &ov("acme.eq.module", auth, "max-age", "20"))
        .add(
            "f2.md",
            &ov(
                "acme.eq.repo-module",
                "repos = [\"mobile\"]\nmodules = [\"mobile.auth\"]",
                "max-age",
                "10",
            ),
        )
        // Identical scopes with the same value agree and stay valid.
        .add("g1.md", &ov("acme.eq.same-a", auth, "min-tls", "13"))
        .add("g2.md", &ov("acme.eq.same-b", auth, "min-tls", "13"));
    let d = kb.check();
    assert_exactly(
        &d,
        &[
            ("OVERRIDE_AMBIGUOUS", "acme.eq.first"),
            ("OVERRIDE_AMBIGUOUS", "acme.eq.module"),
        ],
    );
    let first = d
        .iter()
        .find(|x| x.record.as_deref() == Some("acme.eq.first"))
        .unwrap();
    assert!(first.message.contains("acme.eq.second"), "{first:?}");
    assert!(
        first.message.contains("neither is strictly more specific"),
        "{first:?}"
    );
}

#[test]
fn base_comparison_keeps_historical_ids() {
    let base = Kb::new();
    base.add("gone.md", &inv("acme.h.gone", "accepted", PRODUCT, ""))
        .add("kind.md", &inv("acme.h.kind", "accepted", PRODUCT, ""))
        .add("kept.md", &inv("acme.h.kept", "deprecated", PRODUCT, ""));
    let current = Kb::new();
    current
        .add("kept.md", &inv("acme.h.kept", "deprecated", PRODUCT, ""))
        .add(
            "kind.md",
            &doc(
                "acme.h.kind",
                "reference",
                "accepted",
                "arch",
                "summary = \"Now a reference.\"",
                PRODUCT,
                "",
            ),
        );
    let d = validate_against_base(
        &meta_inputs(&current.corpus()),
        &meta_inputs(&base.corpus()),
    );
    assert_exactly(
        &d,
        &[
            ("ID_REMOVED", "acme.h.gone"),
            ("ID_KIND_CHANGED", "acme.h.kind"),
        ],
    );
    let removed = with_code(&d, "ID_REMOVED")[0];
    assert_eq!(removed.path.as_deref(), Some("project/knowledge/gone.md"));
    assert!(removed.message.contains("deprecated"));
    // Identical snapshots are clean.
    let c = current.corpus();
    assert!(validate_against_base(&meta_inputs(&c), &meta_inputs(&c)).is_empty());
}

#[test]
fn report_combines_parse_rules_and_lints_and_renders_every_format() {
    let kb = Kb::new();
    kb.add(
        "lint.md",
        &(inv(
            "acme.r.lint",
            "accepted",
            PRODUCT,
            "[links]\nrelated = [\"acme.r.missing\"]",
        ) + "## Notes\nClients MUST retry.\n"),
    )
    .add("broken.md", "+++\nschema = 1\n+++\n");
    let corpus = load_corpus(
        &WorkingTreeSource::new(&kb.root),
        &ProfileLocation::for_profile(Profile::Project),
    )
    .unwrap();
    let report = validate_corpus(&corpus);
    assert_eq!((report.files, report.records), (4, 3));
    assert_eq!((report.errors, report.warnings), (2, 1), "{report:#?}");
    assert!(!report.is_ok(false));
    let lint = with_code(&report.diagnostics, "NORMATIVE_LANGUAGE_IN_BODY")[0];
    assert_eq!(lint.path.as_deref(), Some("project/knowledge/lint.md"));
    assert_eq!(lint.record.as_deref(), Some("acme.r.lint"));
    // Errors sort before warnings.
    assert_eq!(report.diagnostics[2].severity, Severity::Warning);

    let compact = render_report(&report, Format::Compact);
    assert!(
        compact.starts_with("validate project: records: 3, files: 4, errors: 2, warnings: 1\n")
    );
    assert!(compact.contains(
        "error DANGLING_LINK project/knowledge/lint.md [acme.r.lint]: links.related: target `acme.r.missing` does not exist\n"
    ));
    assert!(compact.contains("error KIND_MISSING project/knowledge/broken.md"));
    let human = render_report(&report, Format::Human);
    assert!(human.contains("  --> project/knowledge/lint.md\n"));
    assert!(human.ends_with("Result: INVALID\n"));
    let json: serde_json::Value =
        serde_json::from_str(&render_report(&report, Format::Json)).unwrap();
    assert_eq!(json["errors"], 2);
    assert_eq!(json["diagnostics"].as_array().unwrap().len(), 3);

    let mut strict = validate_corpus(&Kb::new().corpus());
    assert!(strict.is_ok(true));
    strict.extend([Diagnostic::warning("X_WARNING", "extra")]);
    assert!(strict.is_ok(false) && !strict.is_ok(true));
    assert_eq!(strict.warnings, 1);
}

#[test]
fn front_matter_toml_errors_point_at_file_lines() {
    let kb = Kb::new();
    // Line 1 is the opening `+++`: the duplicate key is on file line 3, the unknown
    // field on file line 8.
    kb.add("dup.md", "+++\nschema = 1\nschema = 1\n+++\n").add(
        "unknown.md",
        &inv("acme.r.unknown", "accepted", PRODUCT, "").replacen(
            "owner = \"arch\"\n",
            "owner = \"arch\"\nsurprise = 1\n",
            1,
        ),
    );
    let corpus = load_corpus(
        &WorkingTreeSource::new(&kb.root),
        &ProfileLocation::for_profile(Profile::Project),
    )
    .unwrap();
    let report = validate_corpus(&corpus);
    let syntax = with_code(&report.diagnostics, "FRONT_MATTER_SYNTAX");
    let invalid = with_code(&report.diagnostics, "FRONT_MATTER_INVALID");
    assert_eq!((syntax[0].line, invalid[0].line), (Some(3), Some(8)));
    let compact = render_report(&report, Format::Compact);
    assert!(
        compact.contains(
            "error FRONT_MATTER_SYNTAX project/knowledge/dup.md:3: TOML parse error at line 3, column 1: "
        ),
        "{compact}"
    );
    assert!(
        compact.contains(
            "error FRONT_MATTER_INVALID project/knowledge/unknown.md:8: TOML parse error at line 8, column 1: unknown field `surprise`"
        ),
        "{compact}"
    );
}

// ---------------------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------------------

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &dst);
        } else {
            fs::copy(e.path(), dst).unwrap();
        }
    }
}

/// A KB root whose `core/templates` is the synthetic fixture tree plus the valid record
/// fixtures as record templates.
fn template_root(sb: &common::Sandbox) -> PathBuf {
    let root = sb.path().join("upstream");
    let fixtures = common::repo_root().join("core/tests/fixtures/validate");
    copy_tree(&fixtures.join("templates"), &root.join("core/templates"));
    copy_tree(
        &fixtures.join("records/valid"),
        &root.join("core/templates/records"),
    );
    root
}

#[test]
#[cfg(unix)]
fn templates_validate_records_skeleton_and_examples() {
    let sb = common::Sandbox::new();
    let root = template_root(&sb);
    let d = validate_templates(&root).unwrap();
    assert!(d.is_empty(), "{d:#?}");

    let t = root.join("core/templates");
    // A broken record template and a now-missing kind.
    common::write(
        &t.join("records/gap.md"),
        "+++\nschema = 1\nkind = \"gap\"\n+++\n",
    );
    // A strictly parsed registry template with an unknown field.
    common::write(
        &t.join("project/registry/owners.toml"),
        "schema = 1\n[[owner]]\nid = \"a\"\ntitle = \"A\"\nteam = \"x\"\n",
    );
    // A `.tmpl` without placeholders is parsed as its destination type.
    common::write(
        &t.join("project/upstream.toml.tmpl"),
        "schema = 1\nurl = 5\n",
    );
    // An example record with a dangling link.
    common::write(
        &t.join("examples/demo/project/knowledge/extra.md"),
        &doc(
            "demo.app.extra",
            "invariant",
            "accepted",
            "arch",
            "",
            "repos = [\"app\"]",
            &format!("[links]\nrequires = [\"demo.nothing\"]\n{STATEMENT}"),
        ),
    );
    // Example profile files outside the corpus are parsed strictly too.
    common::write(
        &t.join("examples/demo/project/routing-tests/bad.toml"),
        "schema = 1\n[[case]]\nname = \"x\"\nintent = \"deploy\"\n",
    );
    // An example directory without a profile, and a symlinked example.
    fs::create_dir_all(t.join("examples/empty")).unwrap();
    std::os::unix::fs::symlink(t.join("examples/demo"), t.join("examples/linked")).unwrap();
    let d = validate_templates(&root).unwrap();
    let got: Vec<(String, String)> = d
        .iter()
        .map(|x| (x.code.clone(), x.path.clone().unwrap_or_default()))
        .collect();
    let want: Vec<(String, String)> = [
        (
            "DANGLING_LINK",
            "core/templates/examples/demo/project/knowledge/extra.md",
        ),
        ("FRONT_MATTER_INVALID", "core/templates/records/gap.md"),
        (
            "REGISTRY_PARSE",
            "core/templates/project/registry/owners.toml",
        ),
        (
            "TEMPLATE_INVALID",
            "core/templates/project/upstream.toml.tmpl",
        ),
        ("TEMPLATE_KIND_MISSING", "core/templates/records"),
        ("TEMPLATE_EXAMPLE_EMPTY", "core/templates/examples/empty"),
        (
            "TEMPLATE_INVALID",
            "core/templates/examples/demo/project/routing-tests/bad.toml",
        ),
        ("SYMLINK_NOT_ALLOWED", "core/templates/examples/linked"),
    ]
    .iter()
    .map(|(c, p)| (c.to_string(), p.to_string()))
    .collect();
    let mut got_sorted = got.clone();
    got_sorted.sort();
    let mut want_sorted = want;
    want_sorted.sort();
    assert_eq!(got_sorted, want_sorted, "{d:#?}");
    assert_eq!(d.last().unwrap().severity, Severity::Warning);

    // A broken example config is reported at the example's config path.
    fs::remove_file(t.join("examples/demo/project/project.toml")).unwrap();
    let d = validate_templates(&root).unwrap();
    assert!(d.iter().any(|x| x.code == "PROJECT_NOT_INITIALIZED"
        && x.path.as_deref() == Some("core/templates/examples/demo/project/project.toml")));
}

#[test]
fn change_types_template_is_parsed_strictly_and_must_declare_schema_two() {
    let sb = common::Sandbox::new();
    let root = template_root(&sb);
    let rel = "core/templates/project/registry/change-types.toml";
    let shipped = fs::read_to_string(common::repo_root().join(rel)).unwrap();
    common::write(&root.join(rel), &shipped);
    let d = validate_templates(&root).unwrap();
    assert!(d.is_empty(), "{d:#?}");
    let entry = |id: &str, extra: &str| {
        format!("[[change_type]]\nid = \"{id}\"\ntitle = \"Synthetic\"\n{extra}\n")
    };
    for (text, codes) in [
        (
            "schema = 2\n[[change_type]]\nid = \"retry\"\ntitle = \"Retries\"\nalias = [\"retry\"]\n"
                .to_string(),
            &["REGISTRY_PARSE"][..],
        ),
        ("schema = 7\n".to_string(), &["UNSUPPORTED_SCHEMA_VERSION"]),
        ("schema = 1\n".to_string(), &["UNSUPPORTED_SCHEMA_VERSION"]),
        // The change-type checks a downstream `kb validate` applies to the copied file.
        (
            format!("schema = 2\n{}", entry("Bad Id", "paths = [\"../../etc/**\"]")),
            &[
                "CHANGE_TYPE_INVALID",
                "REGISTRY_GLOB_INVALID",
                "REGISTRY_ID_INVALID",
            ],
        ),
        (
            format!(
                "schema = 2\n{}{}",
                entry("retry", "aliases = [\"!!!\"]"),
                entry("retry", "symbols = [\"Retry\"]")
            ),
            &["REGISTRY_ALIAS_INVALID", "REGISTRY_DUPLICATE_ID"],
        ),
        // Id-shaped repo qualifiers name the downstream's repos and are checked there.
        (
            format!(
                "schema = 2\n{}",
                entry("retry", "paths = [\"mobile:app/**\", \"[a:b]/x.md\"]")
            ),
            &[],
        ),
        // A qualifier that is not a registry id names no repo in any downstream.
        (
            format!(
                "schema = 2\n{}",
                entry(
                    "retry",
                    "paths = [\"mobile:app/**\", \"Mobile:app/**\", \"-web:src/**\"]"
                )
            ),
            &["REGISTRY_UNKNOWN_REPO", "REGISTRY_UNKNOWN_REPO"],
        ),
    ] {
        common::write(&root.join(rel), &text);
        let d = validate_templates(&root).unwrap();
        let mut got: Vec<_> = d
            .iter()
            .map(|x| (x.code.as_str(), x.path.as_deref().unwrap_or_default()))
            .collect();
        got.sort();
        let want: Vec<_> = codes.iter().map(|c| (*c, rel)).collect();
        assert_eq!(got, want, "{text}: {d:#?}");
    }
}

#[test]
fn registry_templates_must_declare_a_supported_schema() {
    let sb = common::Sandbox::new();
    let root = template_root(&sb);
    let dir = "core/templates/project/registry";
    for file in [
        "owners.toml",
        "repos.toml.tmpl",
        "modules.toml",
        "features.toml",
        "concepts.toml",
    ] {
        let rel = format!("{dir}/{file}");
        common::write(&root.join(&rel), "schema = 2\n");
        assert!(validate_templates(&root).unwrap().is_empty(), "{rel}");
        // A loaded corpus refuses this file (`UNSUPPORTED_SCHEMA_VERSION`), so must the
        // template check.
        common::write(&root.join(&rel), "schema = 7\n");
        let d = validate_templates(&root).unwrap();
        let got: Vec<_> = d
            .iter()
            .map(|x| (x.code.as_str(), x.path.as_deref().unwrap_or_default()))
            .collect();
        assert_eq!(
            got,
            [("UNSUPPORTED_SCHEMA_VERSION", rel.as_str())],
            "{d:#?}"
        );
        common::write(&root.join(&rel), "schema = 1\n");
    }
}

#[test]
fn missing_templates_are_reported() {
    let sb = common::Sandbox::new();
    let d = validate_templates(&sb.path()).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code, "TEMPLATES_MISSING");
}
