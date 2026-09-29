//! Deterministic synthetic corpus generator.
//!
//! Produces a complete KB checkout layout (engine manifest + `project/`) with registries and
//! `records` knowledge records of all kinds. The same (records, seed) always yields
//! byte-identical files. All content is synthetic.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

/// SplitMix64: tiny, deterministic PRNG (no external crates).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

pub const REPOS: [&str; 4] = ["mobile", "backend", "web", "shared"];

/// Only records with a lower index may be repo-wide (about 2% of them).
const REPO_WIDE_LIMIT: usize = 5000;

const WORDS: [&str; 32] = [
    "token",
    "session",
    "cache",
    "payment",
    "invoice",
    "profile",
    "search",
    "upload",
    "retry",
    "timeout",
    "schema",
    "migration",
    "render",
    "layout",
    "queue",
    "worker",
    "audit",
    "consent",
    "locale",
    "currency",
    "export",
    "import",
    "sync",
    "offline",
    "notification",
    "feature-flag",
    "analytics",
    "billing",
    "catalog",
    "cart",
    "checkout",
    "review",
];

const RU_WORDS: [&str; 8] = [
    "токен",
    "сессия",
    "кэш",
    "платёж",
    "профиль",
    "поиск",
    "очередь",
    "композиция",
];

/// Generation parameters.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub records: usize,
    pub seed: u64,
}

/// Number of modules per repo for a corpus size (grows slowly with the corpus).
pub fn modules_per_repo(records: usize) -> usize {
    (records / 200).clamp(4, 1024)
}

fn kind_for(i: usize) -> &'static str {
    // 20% policy, 10% invariant, 5% contract, 15% feature, 15% decision,
    // 10% procedure, 20% reference, 5% gap (by index, deterministic).
    match i % 20 {
        0..=3 => "policy",
        4..=5 => "invariant",
        6 => "contract",
        7..=9 => "feature",
        10..=12 => "decision",
        13..=14 => "procedure",
        15..=18 => "reference",
        _ => "gap",
    }
}

pub fn record_id(i: usize) -> String {
    format!("bench.{}.r{i:06}", kind_for(i))
}

fn module_id(repo: &str, m: usize) -> String {
    format!("{repo}.m{m:02}")
}

fn feature_id(f: usize) -> String {
    format!("f{f:03}")
}

fn concept_id(c: usize) -> String {
    format!("c-{}", WORDS[c % WORDS.len()])
}

fn write(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    fs::write(path, text)
}

/// Generate a corpus into `out` (which must be empty or absent). `manifest` is the text of
/// the engine's `core/release.toml` so that snapshots are engine-compatible.
pub fn generate(out: &Path, params: Params, manifest: &str) -> std::io::Result<()> {
    let mut rng = Rng::new(params.seed);
    let mpr = modules_per_repo(params.records);
    let features = (params.records / 100).clamp(8, 200);
    write(&out.join("core/release.toml"), manifest)?;
    write(
        &out.join("project/project.toml"),
        "schema = 1\n\n[project]\nname = \"Synthetic benchmark corpus\"\nnamespace = \"bench\"\n\n[source]\nremote = \"origin\"\napproved_ref = \"refs/heads/main\"\nallowed_protocols = [\"file\"]\n",
    )?;
    let mut owners = String::from(
        "schema = 1\n\n[[owner]]\nid = \"arch\"\ntitle = \"Architecture\"\nproduct = true\n",
    );
    for r in REPOS {
        let _ = write!(
            owners,
            "\n[[owner]]\nid = \"team-{r}\"\ntitle = \"Team {r}\"\nrepos = [\"{r}\"]\n"
        );
    }
    write(&out.join("project/registry/owners.toml"), &owners)?;
    let mut repos = String::from("schema = 1\n");
    for r in REPOS {
        let _ = write!(
            repos,
            "\n[[repo]]\nid = \"{r}\"\ntitle = \"Synthetic {r}\"\nremotes = [\"bench.invalid/synthetic/{r}\"]\n"
        );
    }
    write(&out.join("project/registry/repos.toml"), &repos)?;
    let mut modules = String::from("schema = 1\n");
    for r in REPOS {
        for m in 0..mpr {
            let f = (m * 7 + r.len()) % features;
            let _ = write!(
                modules,
                "\n[[module]]\nid = \"{}\"\nrepo = \"{r}\"\ntitle = \"Module {m} of {r}\"\npaths = [\"src/m{m:02}/**\"]\nfeatures = [\"{}\"]\n",
                module_id(r, m),
                feature_id(f)
            );
        }
    }
    write(&out.join("project/registry/modules.toml"), &modules)?;
    let mut feats = String::from("schema = 1\n");
    for f in 0..features {
        let _ = write!(
            feats,
            "\n[[feature]]\nid = \"{}\"\ntitle = \"Feature {f}\"\nrepos = [\"mobile\", \"backend\", \"web\", \"shared\"]\n",
            feature_id(f)
        );
    }
    write(&out.join("project/registry/features.toml"), &feats)?;
    let mut concepts = String::from("schema = 1\n");
    for (c, w) in WORDS.iter().enumerate() {
        let ru = RU_WORDS[c % RU_WORDS.len()];
        let _ = write!(
            concepts,
            "\n[[concept]]\nid = \"{}\"\ntitle = \"Concept {w}\"\naliases = [\"{w}\", \"{ru}\"]\n",
            concept_id(c)
        );
    }
    write(&out.join("project/registry/concepts.toml"), &concepts)?;

    for i in 0..params.records {
        let kind = kind_for(i);
        let repo = REPOS[rng.below(REPOS.len() as u64) as usize];
        let m = rng.below(mpr as u64) as usize;
        let c1 = rng.below(WORDS.len() as u64) as usize;
        let c2 = (c1 + 1 + rng.below(5) as usize) % WORDS.len();
        // Scope: a handful of product-wide records, a bounded number of repo-wide records
        // (real knowledge bases do not grow repo-wide obligations linearly with size), and
        // module-scoped records otherwise.
        let repo_wide = rng.chance(2) && i < REPO_WIDE_LIMIT;
        let (scope, owner) = if i < 10 && kind == "policy" {
            ("product = true".to_string(), "arch".to_string())
        } else if repo_wide {
            (format!("repos = [\"{repo}\"]"), format!("team-{repo}"))
        } else {
            (
                format!("modules = [\"{}\"]", module_id(repo, m)),
                format!("team-{repo}"),
            )
        };
        let mut fm = String::new();
        let _ = write!(
            fm,
            "schema = 1\nid = \"{}\"\nkind = \"{kind}\"\ntitle = \"{} {} rule {i}\"\nstatus = \"accepted\"\nowner = \"{owner}\"\n",
            record_id(i),
            WORDS[c1],
            WORDS[c2]
        );
        let body_kind = kind_fields(kind, i, repo, &mut rng, c1);
        fm.push_str(&body_kind.top);
        let _ = write!(fm, "\n[scope]\n{scope}\n");
        let _ = write!(
            fm,
            "\n[selectors]\npaths = [\"src/m{m:02}/**\"]\nconcepts = [\"{}\"]\naliases = [\"{} {}\"]\n",
            concept_id(c1),
            WORDS[c1],
            WORDS[c2]
        );
        // Links only point to lower indices, so `requires` stays acyclic. Targets must be
        // accepted and valid; requires only point at decisions/references (never superseded).
        let mut requires = Vec::new();
        let mut rationale = Vec::new();
        let mut related = Vec::new();
        if i > 40 && rng.chance(25) {
            let j = find_lower(i, &mut rng, |k| matches!(kind_for(k), "reference"));
            if let Some(j) = j {
                requires.push(record_id(j));
            }
        }
        if i > 40
            && rng.chance(40)
            && let Some(j) = find_lower(i, &mut rng, |k| kind_for(k) == "decision")
        {
            rationale.push(record_id(j));
        }
        if i > 40
            && rng.chance(30)
            && let Some(j) = find_lower(i, &mut rng, |_| true)
        {
            related.push(record_id(j));
        }
        if !(requires.is_empty() && rationale.is_empty() && related.is_empty()) {
            let list = |v: &[String]| {
                v.iter()
                    .map(|s| format!("\"{s}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let _ = write!(
                fm,
                "\n[links]\nrequires = [{}]\nrationale = [{}]\nrelated = [{}]\n",
                list(&requires),
                list(&rationale),
                list(&related)
            );
        }
        fm.push_str(&body_kind.tables);
        let body = format!(
            "## Background\n\nSynthetic explanation for record {i} about {} and {} in {repo}.\nIt exists only to exercise indexing and retrieval at scale.\n",
            WORDS[c1], WORDS[c2]
        );
        let text = format!("+++\n{fm}+++\n{body}");
        let dir = format!("project/knowledge/{repo}/{kind}");
        write(&out.join(format!("{dir}/r{i:06}.md")), &text)?;
    }
    Ok(())
}

fn find_lower(i: usize, rng: &mut Rng, ok: impl Fn(usize) -> bool) -> Option<usize> {
    for _ in 0..8 {
        let j = 20 + rng.below((i - 20) as u64) as usize;
        if ok(j) {
            return Some(j);
        }
    }
    None
}

struct KindFields {
    /// Top-level keys (must precede tables).
    top: String,
    /// Arrays of tables appended after other tables.
    tables: String,
}

fn kind_fields(kind: &str, i: usize, repo: &str, rng: &mut Rng, c: usize) -> KindFields {
    let w = WORDS[c];
    let mut top = String::new();
    let mut tables = String::new();
    match kind {
        "policy" => {
            let _ = write!(
                tables,
                "\n[[rules]]\nid = \"r1\"\nlevel = \"must\"\ntext = \"Handle {w} failures explicitly in {repo} (rule {i}).\"\n\n[[rules.exceptions]]\nid = \"e1\"\ntext = \"Generated test doubles may skip {w} handling.\"\n"
            );
        }
        "invariant" => {
            let _ = write!(
                tables,
                "\n[[statements]]\nid = \"s1\"\nlevel = \"must\"\ntext = \"The {w} state remains consistent after every change (invariant {i}).\"\n"
            );
        }
        "contract" => {
            let _ = write!(
                tables,
                "\n[[parties]]\nid = \"provider\"\nrepo = \"{repo}\"\nrole = \"Provides {w}\"\n\n[[parties]]\nid = \"consumer\"\nrepo = \"{repo}\"\nrole = \"Consumes {w}\"\n\n[[obligations]]\nid = \"o1\"\nparty = \"provider\"\nlevel = \"must\"\ntext = \"Keep the {w} interface backward compatible (contract {i}).\"\n"
            );
        }
        "feature" => {
            let _ = write!(
                top,
                "feature = \"f{:03}\"\nsummary = \"Synthetic feature summary for {w}.\"\n",
                rng.below(8)
            );
            let _ = write!(
                tables,
                "\n[[behaviors]]\nid = \"b1\"\ntext = \"Users can manage {w} (feature record {i}).\"\n"
            );
        }
        "decision" => {
            let _ = write!(
                top,
                "context = \"Synthetic context for {w}.\"\ndecision = \"Use approach {i} for {w}.\"\nreasons = [\"It is deterministic.\"]\n"
            );
        }
        "procedure" => {
            let _ = writeln!(top, "expected = [\"The {w} procedure completes.\"]");
            let _ = write!(
                tables,
                "\n[[steps]]\nid = \"s1\"\ntext = \"Inspect the {w} logs (procedure {i}).\"\n"
            );
        }
        "reference" => {
            let _ = writeln!(
                top,
                "summary = \"Reference material about {w} (reference {i}).\""
            );
        }
        _ => {
            let _ = write!(
                top,
                "gap = \"missing\"\ndescription = \"Unknown {w} behavior under load (gap {i}).\"\n"
            );
        }
    }
    KindFields { top, tables }
}
