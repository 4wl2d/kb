//! Schema migrations (`kb migrate`): a registry of transforms between adjacent document
//! schema versions for records, the profile config and registry files.
//!
//! The synthetic pre-release 0 → 1 step exercises structural transforms; 1 → 2 adds
//! optional domain knowledge without inventing content (see `core/migrations/README.md`).
//!
//! Migration reads the working tree directly, so it also works when
//! [`crate::corpus::load_config`] rejects a legacy profile config. Every file is transformed
//! in memory and verified with the current parsers before anything is written; any failure
//! writes nothing (`MIGRATION_FAILED`). TOML is edited with `toml_edit` so comments and key
//! order survive; record bodies stay byte-identical.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value as Json, json};
use toml_edit::{ArrayOfTables, Document, DocumentMut, Item, Key, Table, Value};

use crate::corpus::{is_record_path, parse_config};
use crate::diag::Diagnostic;
use crate::error::{ErrorCode, KbError, Result};
use crate::model::registry::{
    ChangeTypesFile, ConceptsFile, FeaturesFile, ModulesFile, OwnersFile, REGISTRY_FILES,
    ReposFile, parse_registry_file,
};
use crate::model::{Profile, ProfileConfig, ProfileLocation};
use crate::parse::{parse_record, split_front_matter};
use crate::source::{SourceTree, WorkingTreeSource};
use crate::util::{atomic_write, check_rel_path, safe_join};
use crate::versions::DOCUMENT_SCHEMA;

/// Name of the synthetic, never-published legacy format migrated by the 0 → 1 step.
pub const LEGACY_V0_FORMAT: &str = "kb-legacy-synthetic-v0";

/// A pure text transform of one file; the error is a human-readable reason.
pub type Transform = fn(&str) -> std::result::Result<String, String>;

/// One registered step between two adjacent schema versions.
pub struct Migration {
    pub from: u32,
    pub to: u32,
    pub name: &'static str,
    pub description: &'static str,
    /// Transform of a record file (front matter + body).
    pub record: Transform,
    /// Transform of the profile config (`project/project.toml`).
    pub config: Transform,
    /// Transform of a registry file (`registry/*.toml`).
    pub registry: Transform,
}

impl Migration {
    fn transform(&self, kind: FileKind) -> Transform {
        match kind {
            FileKind::Config => self.config,
            FileKind::Registry => self.registry,
            FileKind::Record => self.record,
        }
    }
}

/// The migration registry, ordered by `from`.
pub static MIGRATIONS: &[Migration] = &[
    Migration {
        from: 0,
        to: 1,
        name: "v0-to-v1",
        description: "kb-legacy-synthetic-v0 to document schema 1: `type` -> `kind`, `state` -> \
                  `status`, `[applies_to]` -> `[scope]`, tags/depends_on/see_also/replaces -> \
                  selectors/links, policy `[[rule]]` -> `[[rules]]`, config `[kb]`/`[origin]` \
                  -> `[project]`/`[source]`, registry schema bump",
        record: v0::record,
        config: v0::config,
        registry: v0::registry,
    },
    Migration {
        from: 1,
        to: 2,
        name: "v1-to-v2",
        description: "document schema 2: optional domain, provenance, temporal and delivery fields; preserves content and defaults",
        record: v1_record,
        config: v1_document,
        registry: v1_document,
    },
];

/// Rewrite only the bytes of the `schema` value: every other byte, including CRLF line
/// endings, comments and layout, survives exactly.
fn v1_document(text: &str) -> std::result::Result<String, String> {
    let doc = Document::parse(text).map_err(|e| e.to_string())?;
    let item = doc.get("schema").ok_or("missing `schema` field")?;
    if item.as_integer() != Some(1) {
        return Err("expected schema 1".into());
    }
    let span = item.span().ok_or("invalid `schema` value")?;
    Ok(format!("{}2{}", &text[..span.start], &text[span.end..]))
}

fn v1_record(text: &str) -> std::result::Result<String, String> {
    // Validate before changing the declaration: mislabeled v2 fields must not gain trust.
    parse_record("migration-input", text.as_bytes()).map_err(|d| {
        d.iter()
            .map(|d| format!("{}: {}", d.code, d.message))
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    let (fm, _, _) = split_front_matter(text)?;
    let start = fm.as_ptr() as usize - text.as_ptr() as usize;
    let end = start + fm.len();
    Ok(format!(
        "{}{}{}",
        &text[..start],
        v1_document(fm)?,
        &text[end..]
    ))
}

/// Oldest schema version this engine can migrate from.
pub fn oldest_supported() -> u32 {
    MIGRATIONS
        .iter()
        .map(|m| m.from)
        .min()
        .unwrap_or(DOCUMENT_SCHEMA)
        .min(DOCUMENT_SCHEMA)
}

/// The chain of steps from `from` to `to` (`Some(empty)` when equal, `None` when no path).
pub fn chain(from: u32, to: u32) -> Option<Vec<&'static Migration>> {
    let mut steps = Vec::new();
    let mut cur = from;
    while cur < to {
        let m = MIGRATIONS.iter().find(|m| m.from == cur && m.to <= to)?;
        steps.push(m);
        cur = m.to;
    }
    (cur == to).then_some(steps)
}

// ---------------------------------------------------------------------------------------
// Scanning and planning
// ---------------------------------------------------------------------------------------

/// Which schema-versioned profile file a path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileKind {
    Config,
    Registry,
    Record,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Config => "config",
            FileKind::Registry => "registry",
            FileKind::Record => "record",
        }
    }
}

/// A profile file found by [`scan`] with its declared schema.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    /// KB-root-relative path.
    pub path: String,
    pub kind: FileKind,
    /// Declared `schema`, or why it could not be read.
    pub schema: std::result::Result<u32, String>,
    text: String,
}

/// What `apply` will do with a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileStatus {
    UpToDate,
    Migrate,
    /// The schema could not be read (not UTF-8, no front matter, TOML syntax, symlink);
    /// the file is left untouched and reported by `kb validate`.
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedFile {
    pub path: String,
    pub kind: FileKind,
    pub schema: Option<u32>,
    pub status: FileStatus,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip)]
    original: String,
}

/// Per-file migration plan for one profile.
#[derive(Debug, Clone, Serialize)]
pub struct MigrationPlan {
    #[serde(skip)]
    pub kb_root: PathBuf,
    /// Profile config path (KB-root-relative).
    pub config: String,
    /// Target schema version.
    pub target: u32,
    /// Schema served by this engine.
    pub current: u32,
    /// Oldest schema this engine migrates from.
    pub oldest_supported: u32,
    /// Files in (kind, path) order.
    pub files: Vec<PlannedFile>,
}

impl MigrationPlan {
    /// Files that will be rewritten.
    pub fn pending(&self) -> impl Iterator<Item = &PlannedFile> {
        self.files
            .iter()
            .filter(|f| f.status == FileStatus::Migrate)
    }

    fn count(&self, status: FileStatus) -> usize {
        self.files.iter().filter(|f| f.status == status).count()
    }
}

/// A verified in-memory result for one file.
#[derive(Debug, Clone, Serialize)]
pub struct FileChange {
    pub path: String,
    pub kind: FileKind,
    pub from: u32,
    pub to: u32,
    pub steps: Vec<&'static str>,
    #[serde(skip)]
    pub before: String,
    #[serde(skip)]
    pub after: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WrittenFile {
    pub path: String,
    pub from: u32,
    pub to: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

/// Outcome of [`apply`].
#[derive(Debug, Clone, Serialize)]
pub struct ApplyReport {
    pub target: u32,
    pub written: Vec<WrittenFile>,
    pub up_to_date: usize,
    pub skipped: Vec<SkippedFile>,
}

/// Scan the profile config, registry files and record files of the working tree and read
/// each declared schema. Does not judge whether a schema is supported.
pub fn scan(kb_root: &Path, loc: &ProfileLocation) -> Result<Vec<ScannedFile>> {
    let source = WorkingTreeSource::new(kb_root);
    let config_text = read_text(&source, &loc.config)?.ok_or_else(|| not_initialized(loc))?;
    let table: toml::Table = toml::from_str(&config_text).map_err(|e| {
        KbError::new(
            ErrorCode::ConfigInvalid,
            format!("`{}`: {}", loc.config, first_line(&e.to_string())),
        )
    })?;
    let config_schema = toml_schema(&table)
        .map_err(|e| KbError::new(ErrorCode::ConfigInvalid, format!("`{}`: {e}", loc.config)))?;
    let roots = knowledge_roots(&table, loc)?;
    let mut files = vec![ScannedFile {
        path: loc.config.clone(),
        kind: FileKind::Config,
        schema: Ok(config_schema),
        text: config_text,
    }];
    for name in REGISTRY_FILES {
        let path = format!("{}/{name}", loc.registry_dir());
        if let Some(bytes) = source.read_path(&path)? {
            files.push(scanned(path, FileKind::Registry, bytes, |t| {
                toml::from_str::<toml::Table>(t)
                    .map_err(|e| first_line(&e.to_string()))
                    .and_then(|t| toml_schema(&t))
            }));
        }
    }
    let (entries, issues) = source.list(&roots)?;
    for issue in issues {
        files.push(ScannedFile {
            path: issue.path,
            kind: FileKind::Record,
            schema: Err(issue.message),
            text: String::new(),
        });
    }
    let records: Vec<_> = entries
        .into_iter()
        .filter(|e| is_record_path(&e.path))
        .collect();
    let contents = source.read(&records)?;
    for (entry, bytes) in records.into_iter().zip(contents) {
        files.push(scanned(entry.path, FileKind::Record, bytes, record_schema));
    }
    files.sort_by(|a, b| (a.kind, &a.path).cmp(&(b.kind, &b.path)));
    Ok(files)
}

/// Distinct declared schema versions of the profile files (unreadable files are ignored).
pub fn project_schemas(kb_root: &Path, loc: &ProfileLocation) -> Result<BTreeSet<u32>> {
    Ok(scan(kb_root, loc)?
        .into_iter()
        .filter_map(|f| f.schema.ok())
        .collect())
}

/// Plan a migration of the profile at `loc` to `to` (default: the current schema).
/// Files with a schema newer than the engine, older than the oldest supported one, or
/// without a migration path fail with `UNSUPPORTED_SCHEMA_VERSION` listing them.
pub fn plan(kb_root: &Path, loc: &ProfileLocation, to: Option<u32>) -> Result<MigrationPlan> {
    let target = to.unwrap_or(DOCUMENT_SCHEMA);
    let oldest = oldest_supported();
    if target > DOCUMENT_SCHEMA || target < oldest {
        return Err(KbError::new(
            ErrorCode::UnsupportedSchemaVersion,
            format!(
                "this engine cannot migrate to schema {target} (supported targets: {oldest}..={DOCUMENT_SCHEMA})"
            ),
        ));
    }
    let mut files = Vec::new();
    let mut unsupported = Vec::new();
    for f in scan(kb_root, loc)? {
        let (status, steps, reason) = match &f.schema {
            Err(reason) => (FileStatus::Skipped, Vec::new(), Some(reason.clone())),
            Ok(s) if *s == target => (FileStatus::UpToDate, Vec::new(), None),
            Ok(s) => match (*s < target).then(|| chain(*s, target)).flatten() {
                Some(c) => (
                    FileStatus::Migrate,
                    c.iter().map(|m| m.name).collect(),
                    None,
                ),
                None => {
                    unsupported.push(json!({
                        "path": f.path,
                        "schema": s,
                        "reason": unsupported_reason(*s, target, oldest),
                    }));
                    continue;
                }
            },
        };
        files.push(PlannedFile {
            path: f.path,
            kind: f.kind,
            schema: f.schema.ok(),
            status,
            steps,
            reason,
            original: f.text,
        });
    }
    if !unsupported.is_empty() {
        return Err(KbError::new(
            ErrorCode::UnsupportedSchemaVersion,
            format!(
                "{} file(s) use a schema version this engine cannot migrate to {target}",
                unsupported.len()
            ),
        )
        .with_details(json!({
            "target": target,
            "supported": {"oldest": oldest, "current": DOCUMENT_SCHEMA},
            "files": unsupported,
        }))
        .with_hint(
            "files newer than the engine need an engine update (`kbw update check`); \
             nothing was written",
        ));
    }
    Ok(MigrationPlan {
        kb_root: kb_root.to_path_buf(),
        config: loc.config.clone(),
        target,
        current: DOCUMENT_SCHEMA,
        oldest_supported: oldest,
        files,
    })
}

fn unsupported_reason(schema: u32, target: u32, oldest: u32) -> String {
    if schema > DOCUMENT_SCHEMA {
        format!("schema {schema} is newer than this engine (schema {DOCUMENT_SCHEMA})")
    } else if schema > target {
        format!("downgrading from schema {schema} to {target} is not supported")
    } else if schema < oldest {
        format!("schema {schema} is older than the oldest migratable schema {oldest}")
    } else {
        format!("no migration path from schema {schema} to {target}")
    }
}

/// Transform every pending file in memory and verify each result under the current
/// engine. Returns all changes, or `MIGRATION_FAILED` listing every failing file.
pub fn preview(plan: &MigrationPlan) -> Result<Vec<FileChange>> {
    let mut changes = Vec::new();
    let mut failures = Vec::new();
    let mut diagnostics = Vec::new();
    for f in plan.pending() {
        let from = f.schema.unwrap_or(plan.target);
        let Some(steps) = chain(from, plan.target) else {
            failures.push(json!({"path": f.path, "problems": ["no migration path"]}));
            continue;
        };
        let mut text = f.original.clone();
        let mut failed = None;
        for m in &steps {
            match (m.transform(f.kind))(&text) {
                Ok(t) => text = t,
                Err(e) => {
                    failed = Some((m.name, e));
                    break;
                }
            }
        }
        if let Some((step, problem)) = failed {
            failures.push(json!({"path": f.path, "step": step, "problems": [problem]}));
            continue;
        }
        match verify(&f.path, f.kind, &text, plan.target) {
            Ok(()) => changes.push(FileChange {
                path: f.path.clone(),
                kind: f.kind,
                from,
                to: plan.target,
                steps: steps.iter().map(|m| m.name).collect(),
                before: f.original.clone(),
                after: text,
            }),
            Err((problems, diags)) => {
                failures.push(json!({"path": f.path, "step": "verify", "problems": problems}));
                diagnostics.extend(diags);
            }
        }
    }
    if !failures.is_empty() {
        crate::diag::normalize(&mut diagnostics);
        return Err(KbError::new(
            ErrorCode::MigrationFailed,
            format!(
                "migration of {} file(s) failed; nothing was written",
                failures.len()
            ),
        )
        .with_details(json!({"target": plan.target, "files": failures}))
        .with_diagnostics(diagnostics)
        .with_hint("fix the listed legacy files and re-run `kbw migrate`"));
    }
    Ok(changes)
}

/// Transform and verify everything in memory, then write each changed file atomically.
/// Nothing is written when any file fails; re-applying an up-to-date plan is a no-op.
pub fn apply(plan: &MigrationPlan) -> Result<ApplyReport> {
    let changes = preview(plan)?;
    let mut targets = Vec::with_capacity(changes.len());
    for c in &changes {
        let path = safe_join(&plan.kb_root, &c.path)?;
        let now = std::fs::read(&path).map_err(|e| KbError::io(&c.path, e))?;
        if now != c.before.as_bytes() {
            return Err(KbError::new(
                ErrorCode::MigrationFailed,
                format!("`{}` changed while migrating; nothing was written", c.path),
            )
            .with_hint("re-run `kbw migrate --apply`"));
        }
        targets.push(path);
    }
    let mut written = Vec::new();
    for (c, path) in changes.iter().zip(targets) {
        atomic_write(&path, c.after.as_bytes()).map_err(|e| {
            let done: Vec<&str> = written
                .iter()
                .map(|w: &WrittenFile| w.path.as_str())
                .collect();
            e.with_details(json!({"written": done}))
        })?;
        written.push(WrittenFile {
            path: c.path.clone(),
            from: c.from,
            to: c.to,
        });
    }
    Ok(ApplyReport {
        target: plan.target,
        written,
        up_to_date: plan.count(FileStatus::UpToDate),
        skipped: skipped(plan),
    })
}

fn skipped(plan: &MigrationPlan) -> Vec<SkippedFile> {
    plan.files
        .iter()
        .filter(|f| f.status == FileStatus::Skipped)
        .map(|f| SkippedFile {
            path: f.path.clone(),
            reason: f.reason.clone().unwrap_or_default(),
        })
        .collect()
}

/// The profile config as the current engine understands it: legacy configs are migrated
/// in memory (nothing is written). Used by commands that must read trusted settings (e.g.
/// the transport policy) before `kb migrate --apply` has run.
pub fn effective_config(kb_root: &Path, loc: &ProfileLocation) -> Result<ProfileConfig> {
    let source = WorkingTreeSource::new(kb_root);
    let text = read_text(&source, &loc.config)?.ok_or_else(|| not_initialized(loc))?;
    let schema = toml::from_str::<toml::Table>(&text)
        .map_err(|e| first_line(&e.to_string()))
        .and_then(|t| toml_schema(&t))
        .map_err(|e| KbError::new(ErrorCode::ConfigInvalid, format!("`{}`: {e}", loc.config)))?;
    if schema == DOCUMENT_SCHEMA {
        return parse_config(&loc.config, text.as_bytes());
    }
    let steps = (schema < DOCUMENT_SCHEMA)
        .then(|| chain(schema, DOCUMENT_SCHEMA))
        .flatten()
        .ok_or_else(|| {
            KbError::new(
                ErrorCode::UnsupportedSchemaVersion,
                format!(
                    "`{}` has schema {schema}; {}",
                    loc.config,
                    unsupported_reason(schema, DOCUMENT_SCHEMA, oldest_supported())
                ),
            )
        })?;
    let mut migrated = text;
    for m in steps {
        migrated = (m.config)(&migrated).map_err(|e| {
            KbError::new(
                ErrorCode::MigrationFailed,
                format!("`{}` cannot be migrated ({}): {e}", loc.config, m.name),
            )
        })?;
    }
    parse_config(&loc.config, migrated.as_bytes())
}

// ---------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------

fn not_initialized(loc: &ProfileLocation) -> KbError {
    match loc.profile {
        Profile::Project => KbError::new(
            ErrorCode::ProjectNotInitialized,
            format!(
                "project is not initialized: `{}` does not exist",
                loc.config
            ),
        )
        .with_hint(
            "run `kbw init --name <name> --namespace <ns>` (dry-run) and then with `--apply`",
        ),
        Profile::Maintainer => KbError::new(
            ErrorCode::ConfigInvalid,
            format!("maintainer profile config `{}` is missing", loc.config),
        ),
    }
}

fn read_text(source: &WorkingTreeSource, path: &str) -> Result<Option<String>> {
    match source.read_path(path)? {
        None => Ok(None),
        Some(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| KbError::new(ErrorCode::ConfigInvalid, format!("`{path}` is not UTF-8"))),
    }
}

fn scanned(
    path: String,
    kind: FileKind,
    bytes: Vec<u8>,
    schema_of: fn(&str) -> std::result::Result<u32, String>,
) -> ScannedFile {
    match String::from_utf8(bytes) {
        Ok(text) => ScannedFile {
            path,
            kind,
            schema: schema_of(&text),
            text,
        },
        Err(_) => ScannedFile {
            path,
            kind,
            schema: Err("not UTF-8".into()),
            text: String::new(),
        },
    }
}

fn record_schema(text: &str) -> std::result::Result<u32, String> {
    let (fm, _, _) = split_front_matter(text)?;
    let table: toml::Table =
        toml::from_str(fm).map_err(|e| format!("front matter: {}", first_line(&e.to_string())))?;
    toml_schema(&table)
}

fn toml_schema(table: &toml::Table) -> std::result::Result<u32, String> {
    match table.get("schema") {
        Some(toml::Value::Integer(v)) => {
            u32::try_from(*v).map_err(|_| format!("invalid schema value {v}"))
        }
        Some(_) => Err("`schema` must be an integer".into()),
        None => Err("missing `schema` field".into()),
    }
}

/// KB-root-relative knowledge roots declared by a config of any schema version.
fn knowledge_roots(table: &toml::Table, loc: &ProfileLocation) -> Result<Vec<String>> {
    let invalid =
        |m: String| KbError::new(ErrorCode::ConfigInvalid, format!("`{}`: {m}", loc.config));
    let roots: Vec<String> = match table.get("knowledge").and_then(|k| k.get("roots")) {
        None => vec!["knowledge".into()],
        Some(toml::Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| invalid("knowledge.roots must be strings".into()))
            })
            .collect::<Result<_>>()?,
        Some(_) => return Err(invalid("knowledge.roots must be an array".into())),
    };
    roots
        .iter()
        .map(|r| {
            let r = r.trim_end_matches('/');
            check_rel_path(r)
                .map(|()| format!("{}/{r}", loc.dir))
                .map_err(|e| invalid(format!("knowledge.roots: {e}")))
        })
        .collect()
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

/// Verify a migrated file. Returns problems (and parser diagnostics) on failure.
fn verify(
    path: &str,
    kind: FileKind,
    text: &str,
    target: u32,
) -> std::result::Result<(), (Vec<String>, Vec<Diagnostic>)> {
    let fail = |m: String| Err((vec![m], Vec::new()));
    if !crate::versions::supports_document_schema(target) {
        // Unreadable intermediate targets can only be checked for the declared schema.
        let schema = match kind {
            FileKind::Record => record_schema(text),
            _ => toml::from_str::<toml::Table>(text)
                .map_err(|e| first_line(&e.to_string()))
                .and_then(|t| toml_schema(&t)),
        };
        return match schema {
            Ok(s) if s == target => Ok(()),
            Ok(s) => fail(format!("result declares schema {s}, expected {target}")),
            Err(e) => fail(e),
        };
    }
    match kind {
        FileKind::Record => parse_record(path, text.as_bytes())
            .map(|_| ())
            .map_err(|diags| {
                let problems = diags
                    .iter()
                    .filter(|d| d.is_error())
                    .map(|d| format!("{}: {}", d.code, d.message))
                    .collect();
                (problems, diags)
            }),
        FileKind::Config => match parse_config(path, text.as_bytes()) {
            Ok(_) => Ok(()),
            Err(e) => fail(e.message),
        },
        FileKind::Registry => {
            let name = path.rsplit('/').next().unwrap_or(path);
            let schema = match name {
                "owners.toml" => parse_registry_file::<OwnersFile>(path, text).map(|f| f.schema),
                "repos.toml" => parse_registry_file::<ReposFile>(path, text).map(|f| f.schema),
                "modules.toml" => parse_registry_file::<ModulesFile>(path, text).map(|f| f.schema),
                "features.toml" => {
                    parse_registry_file::<FeaturesFile>(path, text).map(|f| f.schema)
                }
                "concepts.toml" => {
                    parse_registry_file::<ConceptsFile>(path, text).map(|f| f.schema)
                }
                "change-types.toml" => {
                    parse_registry_file::<ChangeTypesFile>(path, text).map(|f| f.schema)
                }
                _ => return fail(format!("`{name}` is not a registry file")),
            };
            match schema {
                Ok(s) if s == target => Ok(()),
                Ok(s) => fail(format!("result declares schema {s}, expected {target}")),
                Err(d) => Err((vec![d.message.clone()], vec![d])),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------

/// JSON result of a dry run: the plan plus a unified diff per changed file.
pub fn plan_json(plan: &MigrationPlan, changes: &[FileChange]) -> Json {
    let mut v = serde_json::to_value(plan).unwrap_or(Json::Null);
    if let Some(files) = v.get_mut("files").and_then(Json::as_array_mut) {
        for f in files {
            let path = f.get("path").and_then(Json::as_str).unwrap_or_default();
            if let Some(c) = changes.iter().find(|c| c.path == path) {
                f["diff"] = json!(unified_diff(&c.path, &c.before, &c.after));
            }
        }
    }
    v["mode"] = json!("dry-run");
    v["pending"] = json!(changes.len());
    v["migrations"] = json!(
        MIGRATIONS
            .iter()
            .map(|m| json!({"from": m.from, "to": m.to, "name": m.name, "description": m.description}))
            .collect::<Vec<_>>()
    );
    v
}

/// Text rendering of a dry run (compact, or verbose for people).
pub fn render_plan(plan: &MigrationPlan, changes: &[FileChange], human: bool) -> String {
    let mut s = format!(
        "migrate (dry-run): {} file(s) to migrate to schema {}, {} up to date, {} skipped\n",
        changes.len(),
        plan.target,
        plan.count(FileStatus::UpToDate),
        plan.count(FileStatus::Skipped)
    );
    for c in changes {
        s.push_str(&format!(
            "  migrate {} {}: {} -> {} [{}]\n",
            c.kind.as_str(),
            c.path,
            c.from,
            c.to,
            c.steps.join(", ")
        ));
    }
    for f in plan
        .files
        .iter()
        .filter(|f| f.status == FileStatus::Skipped)
    {
        s.push_str(&format!(
            "  skipped {}: {}\n",
            f.path,
            f.reason.as_deref().unwrap_or("")
        ));
    }
    if human {
        let used: BTreeSet<&str> = changes.iter().flat_map(|c| c.steps.clone()).collect();
        for m in MIGRATIONS.iter().filter(|m| used.contains(m.name)) {
            s.push_str(&format!(
                "\n{} ({} -> {}): {}\n",
                m.name, m.from, m.to, m.description
            ));
        }
    }
    for c in changes {
        s.push('\n');
        s.push_str(&unified_diff(&c.path, &c.before, &c.after));
    }
    if changes.is_empty() {
        s.push_str("nothing to migrate\n");
    } else {
        s.push_str(&format!(
            "\nrun `kbw migrate --apply` to write {} file(s)\n",
            changes.len()
        ));
    }
    s
}

/// Text rendering of an apply report.
pub fn render_apply(report: &ApplyReport, human: bool) -> String {
    let mut s = if report.written.is_empty() {
        format!(
            "migrate: nothing to migrate ({} file(s) already at schema {})\n",
            report.up_to_date, report.target
        )
    } else {
        format!(
            "migrate: wrote {} file(s) at schema {} ({} already up to date)\n",
            report.written.len(),
            report.target,
            report.up_to_date
        )
    };
    for w in &report.written {
        s.push_str(&format!("  wrote {}: {} -> {}\n", w.path, w.from, w.to));
    }
    for k in &report.skipped {
        s.push_str(&format!("  skipped {}: {}\n", k.path, k.reason));
    }
    if human && !report.written.is_empty() {
        s.push_str("next: run `kbw validate` and review the diff before committing\n");
    }
    s
}

/// Minimal unified line diff (3 lines of context) for reviewing migrations.
pub fn unified_diff(path: &str, before: &str, after: &str) -> String {
    // Lines keep their terminators, so a changed line ending (CRLF, final newline) is a
    // changed line and the preview shows exactly the bytes that would be written.
    let a: Vec<&str> = before.split_inclusive('\n').collect();
    let b: Vec<&str> = after.split_inclusive('\n').collect();
    let ops = diff_ops(&a, &b);
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    const CONTEXT: usize = 3;
    let changed: Vec<usize> = (0..ops.len())
        .filter(|&i| !matches!(ops[i], Op::Equal(..)))
        .collect();
    let mut i = 0;
    while i < changed.len() {
        let start = changed[i].saturating_sub(CONTEXT);
        let mut end = (changed[i] + CONTEXT + 1).min(ops.len());
        while i + 1 < changed.len() && changed[i + 1] <= end + CONTEXT {
            i += 1;
            end = (changed[i] + CONTEXT + 1).min(ops.len());
        }
        let hunk = &ops[start..end];
        let a_start = hunk.iter().find_map(Op::a_index).unwrap_or(0);
        let b_start = hunk.iter().find_map(Op::b_index).unwrap_or(0);
        let a_len = hunk.iter().filter(|o| o.a_index().is_some()).count();
        let b_len = hunk.iter().filter(|o| o.b_index().is_some()).count();
        out.push_str(&format!(
            "@@ -{},{a_len} +{},{b_len} @@\n",
            a_start + usize::from(a_len > 0),
            b_start + usize::from(b_len > 0)
        ));
        for op in hunk {
            let (mark, line) = match *op {
                Op::Equal(x, _) => (' ', a[x]),
                Op::Delete(x) => ('-', a[x]),
                Op::Insert(y) => ('+', b[y]),
            };
            out.push(mark);
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str("\n\\ No newline at end of file\n");
            }
        }
        i += 1;
    }
    out
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Op {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}

impl Op {
    fn a_index(&self) -> Option<usize> {
        match *self {
            Op::Equal(x, _) | Op::Delete(x) => Some(x),
            Op::Insert(_) => None,
        }
    }
    fn b_index(&self) -> Option<usize> {
        match *self {
            Op::Equal(_, y) | Op::Insert(y) => Some(y),
            Op::Delete(_) => None,
        }
    }
}

/// Longest-common-subsequence edit script after trimming the common prefix and suffix.
/// Very large middles fall back to a greedy alignment with bounded look-ahead to bound
/// memory: still a valid script that keeps runs of unchanged lines equal, not a minimal one.
pub(crate) fn diff_ops<T: PartialEq>(a: &[T], b: &[T]) -> Vec<Op> {
    const LOOKAHEAD: usize = 256;
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut ops: Vec<Op> = (0..pre).map(|i| Op::Equal(i, i)).collect();
    let (n, m) = (am.len(), bm.len());
    if n.saturating_mul(m) > 4_000_000 {
        let mut i = 0;
        for j in 0..m {
            match (i..n.min(i + LOOKAHEAD)).find(|&k| am[k] == bm[j]) {
                Some(k) => {
                    ops.extend((i..k).map(|d| Op::Delete(pre + d)));
                    ops.push(Op::Equal(pre + k, pre + j));
                    i = k + 1;
                }
                None => ops.push(Op::Insert(pre + j)),
            }
        }
        ops.extend((i..n).map(|d| Op::Delete(pre + d)));
    } else {
        // lcs[i][j] = LCS length of am[i..] and bm[j..].
        let mut lcs = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * (m + 1) + j] = if am[i] == bm[j] {
                    lcs[(i + 1) * (m + 1) + j + 1] + 1
                } else {
                    lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && am[i] == bm[j] {
                ops.push(Op::Equal(pre + i, pre + j));
                i += 1;
                j += 1;
            } else if i < n && (j == m || lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1]) {
                // Deletions first on ties, as in conventional unified diffs.
                ops.push(Op::Delete(pre + i));
                i += 1;
            } else {
                ops.push(Op::Insert(pre + j));
                j += 1;
            }
        }
    }
    ops.extend((0..suf).map(|k| Op::Equal(a.len() - suf + k, b.len() - suf + k)));
    ops
}

// ---------------------------------------------------------------------------------------
// kb-legacy-synthetic-v0 → schema 1
// ---------------------------------------------------------------------------------------

/// Transforms of the synthetic legacy format. See `core/migrations/README.md` for the
/// format definition; every unexpected shape is an error rather than silently dropped.
mod v0 {
    use super::*;

    const LEGACY_STATES: [(&str, &str); 4] = [
        ("proposed", "draft"),
        ("active", "accepted"),
        ("retired", "deprecated"),
        ("replaced", "superseded"),
    ];

    /// Record: rewrite the front matter only; bytes outside it are kept as they are.
    pub(super) fn record(text: &str) -> std::result::Result<String, String> {
        let (fm, _, _) = split_front_matter(text)?;
        // `fm` is a subslice of `text`; its offset delimits the bytes kept verbatim.
        let start = fm.as_ptr() as usize - text.as_ptr() as usize;
        let end = start + fm.len();
        let mut doc = parse_doc(fm)?;
        expect_schema(&doc, 0)?;
        let root = doc.as_table_mut();
        let mut out: Vec<(Key, Item)> = Vec::new();
        let mut selectors = Vec::new();
        let mut links = Vec::new();
        let mut scope_at = None;
        for (key, mut item) in drain(root) {
            match key.get() {
                "schema" => {
                    set_value(&mut item, Value::from(1))?;
                    out.push((key, item));
                }
                "type" => out.push((renamed(&key, "kind"), item)),
                "state" => {
                    let legacy = item.as_str().ok_or("`state` must be a string")?;
                    let status = LEGACY_STATES
                        .iter()
                        .find(|(from, _)| *from == legacy)
                        .map(|(_, to)| *to)
                        .ok_or_else(|| {
                            format!(
                                "unknown legacy state `{legacy}` (expected proposed, active, retired or replaced)"
                            )
                        })?;
                    set_value(&mut item, Value::from(status))?;
                    out.push((renamed(&key, "status"), item));
                }
                "applies_to" => {
                    scope_at = Some(out.len());
                    out.push((renamed(&key, "scope"), scope_from(item)?));
                }
                "tags" => push_list(&mut selectors, &key, "aliases", item)?,
                "depends_on" => push_list(&mut links, &key, "requires", item)?,
                "see_also" => push_list(&mut links, &key, "related", item)?,
                "replaces" => push_list(&mut links, &key, "supersedes", item)?,
                "rule" => out.push((renamed(&key, "rules"), rules_from(item)?)),
                "kind" | "status" | "scope" | "selectors" | "links" | "rules" => {
                    return Err(format!(
                        "`{}` is a schema-1 field and cannot appear in a {LEGACY_V0_FORMAT} record",
                        key.get()
                    ));
                }
                _ => out.push((key, item)),
            }
        }
        let after_scope = match scope_at {
            Some(i) => i + 1,
            None => {
                // Absent `[applies_to]` meant product-wide; place `[scope]` first among tables.
                let i = out
                    .iter()
                    .position(|(_, v)| is_standard_table(v))
                    .unwrap_or(out.len());
                out.insert(i, (Key::new("scope"), product_scope()));
                i + 1
            }
        };
        let mut tables = Vec::new();
        if !selectors.is_empty() {
            tables.push((Key::new("selectors"), Item::Table(table_of(selectors))));
        }
        if !links.is_empty() {
            tables.push((Key::new("links"), Item::Table(table_of(links))));
        }
        out.splice(after_scope..after_scope, tables);
        refill(root, out);
        Ok(format!("{}{}{}", &text[..start], doc, &text[end..]))
    }

    /// Profile config: `[kb]` → `[project]`, `[origin] remote, branch, protocols` →
    /// `[source] remote, approved_ref, allowed_protocols`.
    pub(super) fn config(text: &str) -> std::result::Result<String, String> {
        let mut doc = parse_doc(text)?;
        expect_schema(&doc, 0)?;
        let root = doc.as_table_mut();
        let mut out = Vec::new();
        let (mut saw_kb, mut saw_origin) = (false, false);
        for (key, mut item) in drain(root) {
            match key.get() {
                "schema" => {
                    set_value(&mut item, Value::from(1))?;
                    out.push((key, item));
                }
                "kb" => {
                    saw_kb = true;
                    if !item.is_table() {
                        return Err("`[kb]` must be a table".into());
                    }
                    out.push((renamed(&key, "project"), item));
                }
                "origin" => {
                    saw_origin = true;
                    out.push((renamed(&key, "source"), source_from(item)?));
                }
                "project" | "source" => {
                    return Err(format!(
                        "`[{}]` is a schema-1 table and cannot appear in a {LEGACY_V0_FORMAT} config",
                        key.get()
                    ));
                }
                _ => out.push((key, item)),
            }
        }
        if !saw_kb {
            return Err("missing `[kb]` table".into());
        }
        if !saw_origin {
            return Err("missing `[origin]` table".into());
        }
        refill(root, out);
        Ok(doc.to_string())
    }

    /// Registry file: only the schema version changes.
    pub(super) fn registry(text: &str) -> std::result::Result<String, String> {
        let mut doc = parse_doc(text)?;
        expect_schema(&doc, 0)?;
        let item = doc.get_mut("schema").ok_or("missing `schema` field")?;
        set_value(item, Value::from(1))?;
        Ok(doc.to_string())
    }

    fn source_from(mut item: Item) -> std::result::Result<Item, String> {
        let table = item.as_table_mut().ok_or("`[origin]` must be a table")?;
        let mut out = Vec::new();
        let mut saw_branch = false;
        for (key, mut value) in drain(table) {
            match key.get() {
                "branch" => {
                    saw_branch = true;
                    let branch = value
                        .as_str()
                        .filter(|b| !b.trim().is_empty())
                        .ok_or("`origin.branch` must be a non-empty string")?;
                    let approved = if branch.starts_with("refs/") {
                        branch.to_string()
                    } else {
                        format!("refs/heads/{branch}")
                    };
                    set_value(&mut value, Value::from(approved))?;
                    out.push((renamed(&key, "approved_ref"), value));
                }
                "protocols" => out.push((renamed(&key, "allowed_protocols"), value)),
                _ => out.push((key, value)),
            }
        }
        if !saw_branch {
            return Err("`origin.branch` is required".into());
        }
        refill(table, out);
        Ok(item)
    }

    /// `[applies_to]` → `[scope]`; all dimensions empty (or absent) → `product = true`.
    fn scope_from(mut item: Item) -> std::result::Result<Item, String> {
        let table = item
            .as_table_like_mut()
            .ok_or("`applies_to` must be a table")?;
        const DIMS: [&str; 3] = ["repos", "modules", "features"];
        for (k, v) in table.iter() {
            if !DIMS.contains(&k) {
                return Err(format!("unknown field `applies_to.{k}`"));
            }
            if v.as_array().is_none() {
                return Err(format!("`applies_to.{k}` must be an array"));
            }
        }
        let empty = DIMS.iter().all(|d| {
            table
                .get(d)
                .and_then(Item::as_array)
                .is_none_or(|a| a.is_empty())
        });
        if empty {
            for d in DIMS {
                table.remove(d);
            }
            table.insert("product", Item::Value(Value::from(true)));
        }
        Ok(item)
    }

    fn product_scope() -> Item {
        let mut t = Table::new();
        t.insert("product", toml_edit::value(true));
        Item::Table(t)
    }

    /// `[[rule]] must = ".." | must_not = ".."` → `[[rules]] id = "rule-<n>", level, text`.
    fn rules_from(item: Item) -> std::result::Result<Item, String> {
        let Item::ArrayOfTables(rules) = item else {
            return Err("`rule` must be an array of tables (`[[rule]]`)".into());
        };
        let mut out = ArrayOfTables::new();
        for (i, old) in rules.iter().enumerate() {
            let n = i + 1;
            let mut normative = None;
            let mut rest = Vec::new();
            for (k, v) in old.iter() {
                let key = old.key(k).cloned().unwrap_or_else(|| Key::new(k));
                match k {
                    "must" | "must_not" => {
                        if normative.is_some() {
                            return Err(format!("rule {n} sets both `must` and `must_not`"));
                        }
                        if v.as_str().is_none() {
                            return Err(format!("rule {n}: `{k}` must be a string"));
                        }
                        let level = if k == "must" { "must" } else { "must-not" };
                        normative = Some((level, key, v.clone()));
                    }
                    "id" | "level" | "text" => {
                        return Err(format!(
                            "rule {n}: `{k}` is a schema-1 field and cannot appear in a legacy rule"
                        ));
                    }
                    _ => rest.push((key, v.clone())),
                }
            }
            let (level, key, text) =
                normative.ok_or_else(|| format!("rule {n} has neither `must` nor `must_not`"))?;
            let mut rule = Table::new();
            *rule.decor_mut() = old.decor().clone();
            // Comments above the legacy statement stay at the top of the rule.
            let id = Key::new("id").with_leaf_decor(key.leaf_decor().clone());
            rule.insert_formatted(&id, toml_edit::value(format!("rule-{n}")));
            rule.insert("level", toml_edit::value(level));
            rule.insert("text", text);
            for (k, v) in rest {
                rule.insert_formatted(&k, v);
            }
            out.push(rule);
        }
        Ok(Item::ArrayOfTables(out))
    }

    fn push_list(
        dst: &mut Vec<(Key, Item)>,
        key: &Key,
        name: &str,
        item: Item,
    ) -> std::result::Result<(), String> {
        let list = item
            .as_array()
            .ok_or_else(|| format!("`{}` must be an array", key.get()))?;
        if !list.is_empty() {
            dst.push((renamed(key, name), item));
        }
        Ok(())
    }

    fn parse_doc(text: &str) -> std::result::Result<DocumentMut, String> {
        text.parse::<DocumentMut>()
            .map_err(|e| format!("TOML: {}", first_line(&e.to_string())))
    }

    fn expect_schema(doc: &DocumentMut, version: i64) -> std::result::Result<(), String> {
        match doc.get("schema").and_then(Item::as_integer) {
            Some(v) if v == version => Ok(()),
            Some(v) => Err(format!("expected schema {version}, found {v}")),
            None => Err("missing integer `schema` field".into()),
        }
    }

    /// Replace a value, keeping its surrounding whitespace and trailing comment.
    fn set_value(item: &mut Item, new: Value) -> std::result::Result<(), String> {
        let v = item.as_value_mut().ok_or("expected a value")?;
        let decor = v.decor().clone();
        *v = new;
        *v.decor_mut() = decor;
        Ok(())
    }

    fn renamed(key: &Key, name: &str) -> Key {
        Key::new(name).with_leaf_decor(key.leaf_decor().clone())
    }

    fn is_standard_table(item: &Item) -> bool {
        matches!(item, Item::Table(t) if !t.is_dotted()) || item.is_array_of_tables()
    }

    fn table_of(entries: Vec<(Key, Item)>) -> Table {
        let mut t = Table::new();
        for (k, v) in entries {
            t.insert_formatted(&k, v);
        }
        t
    }

    /// Remove all entries of `table`, keeping their formatted keys, in order.
    fn drain(table: &mut Table) -> Vec<(Key, Item)> {
        let entries = table
            .iter()
            .map(|(k, v)| {
                let key = table.key(k).cloned().unwrap_or_else(|| Key::new(k));
                (key, v.clone())
            })
            .collect();
        table.clear();
        entries
    }

    /// Insert entries in order, then renumber table positions so that the document is
    /// emitted in exactly this order.
    fn refill(table: &mut Table, entries: Vec<(Key, Item)>) {
        for (k, v) in entries {
            table.insert_formatted(&k, v);
        }
        let mut pos = 0;
        renumber(table, &mut pos);
    }

    fn renumber(table: &mut Table, pos: &mut isize) {
        if !table.is_dotted() {
            table.set_position(Some(*pos));
            *pos += 1;
        }
        for (_, item) in table.iter_mut() {
            match item {
                Item::Table(t) => renumber(t, pos),
                Item::ArrayOfTables(a) => {
                    for t in a.iter_mut() {
                        renumber(t, pos);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY_V0: &str = "+++\n\
schema = 0\n\
id = \"legacy.policy.x\"\n\
# kind of record\n\
type = \"policy\"\n\
title = \"X\"\n\
state = \"active\" # reviewed\n\
owner = \"arch\"\n\
tags = [\"token\"]\n\
depends_on = [\"legacy.contract.y\"]\n\
see_also = []\n\
\n\
[[rule]]\n\
must_not = \"Log secrets.\"\n\
\n\
[[rule]]\n\
# second\n\
must = \"Rotate keys.\"\n\
+++\n\
Intro text.\r\n\n## Notes\n\n```toml\nschema = 0\n```\n";

    #[test]
    fn record_transform_maps_fields_and_keeps_body() {
        let out = v0::record(POLICY_V0).unwrap();
        let (fm, body, _) = split_front_matter(&out).unwrap();
        assert_eq!(
            body,
            "Intro text.\r\n\n## Notes\n\n```toml\nschema = 0\n```\n"
        );
        let expected_fm = "schema = 1\n\
id = \"legacy.policy.x\"\n\
# kind of record\n\
kind = \"policy\"\n\
title = \"X\"\n\
status = \"accepted\" # reviewed\n\
owner = \"arch\"\n\
\n\
[scope]\n\
product = true\n\
\n\
[selectors]\n\
aliases = [\"token\"]\n\
\n\
[links]\n\
requires = [\"legacy.contract.y\"]\n\
\n\
[[rules]]\n\
id = \"rule-1\"\n\
level = \"must-not\"\n\
text = \"Log secrets.\"\n\
\n\
[[rules]]\n\
# second\n\
id = \"rule-2\"\n\
level = \"must\"\n\
text = \"Rotate keys.\"\n";
        assert_eq!(fm, expected_fm);
    }

    #[test]
    fn record_transform_rejects_bad_legacy_shapes() {
        let bad_state = POLICY_V0.replace("\"active\"", "\"live\"");
        assert!(
            v0::record(&bad_state)
                .unwrap_err()
                .contains("unknown legacy state")
        );
        let both = POLICY_V0.replace(
            "must_not = \"Log secrets.\"",
            "must_not = \"a\"\nmust = \"b\"",
        );
        assert!(v0::record(&both).unwrap_err().contains("both"));
        let v1_field =
            POLICY_V0.replace("owner = \"arch\"", "owner = \"arch\"\nstatus = \"draft\"");
        assert!(
            v0::record(&v1_field)
                .unwrap_err()
                .contains("schema-1 field")
        );
        let wrong_schema = POLICY_V0.replace("schema = 0", "schema = 1");
        assert!(
            v0::record(&wrong_schema)
                .unwrap_err()
                .contains("expected schema 0")
        );
    }

    #[test]
    fn applies_to_dimensions_are_kept_or_become_product() {
        let base = "+++\nschema = 0\nid = \"a.b\"\ntype = \"gap\"\n\n[applies_to]\nrepos = [\"mobile\"]\nmodules = []\n+++\n";
        let out = v0::record(base).unwrap();
        assert!(
            out.contains("[scope]\nrepos = [\"mobile\"]\nmodules = []\n"),
            "{out}"
        );
        let empty = base.replace("repos = [\"mobile\"]\n", "");
        let out = v0::record(&empty).unwrap();
        assert!(out.contains("[scope]\nproduct = true\n"), "{out}");
        assert!(!out.contains("modules"), "{out}");
    }

    #[test]
    fn config_and_registry_transforms() {
        let cfg = "# header\nschema = 0\n\n[kb]\nname = \"N\"\nnamespace = \"legacy\"\n\n[origin]\nremote = \"origin\"\nbranch = \"main\" # approved\nprotocols = [\"file\"]\n";
        let out = v0::config(cfg).unwrap();
        assert_eq!(
            out,
            "# header\nschema = 1\n\n[project]\nname = \"N\"\nnamespace = \"legacy\"\n\n[source]\nremote = \"origin\"\napproved_ref = \"refs/heads/main\" # approved\nallowed_protocols = [\"file\"]\n"
        );
        parse_config("project/project.toml", out.as_bytes()).unwrap();
        assert!(v0::config("schema = 0\n[kb]\nname = \"a\"\n").is_err());
        let reg = "schema = 0 # legacy\n\n[[owner]]\nid = \"arch\"\ntitle = \"A\"\n";
        assert_eq!(
            v0::registry(reg).unwrap(),
            "schema = 1 # legacy\n\n[[owner]]\nid = \"arch\"\ntitle = \"A\"\n"
        );
    }

    #[test]
    fn chain_and_support_bounds() {
        assert_eq!(chain(0, 1).unwrap().len(), 1);
        assert!(chain(1, 1).unwrap().is_empty());
        assert!(chain(1, 0).is_none());
        assert_eq!(chain(0, 2).unwrap().len(), 2);
        assert_eq!(chain(1, 2).unwrap().len(), 1);
        assert!(chain(0, 3).is_none());
        assert_eq!(oldest_supported(), 0);
    }

    #[test]
    fn unified_diff_marks_changes_with_context() {
        let d = unified_diff("f", "a\nb\nc\nd\n", "a\nB\nc\nd\ne\n");
        assert_eq!(
            d,
            "--- a/f\n+++ b/f\n@@ -1,4 +1,5 @@\n a\n-b\n+B\n c\n d\n+e\n"
        );
        assert_eq!(unified_diff("f", "x\n", "x\n"), "--- a/f\n+++ b/f\n");
    }

    #[test]
    fn unified_diff_shows_line_ending_changes() {
        assert_eq!(
            unified_diff("f", "a\r\nb\r\n", "a\nb\r\n"),
            "--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n-a\r\n+a\n b\r\n"
        );
        assert_eq!(
            unified_diff("f", "x", "x\n"),
            "--- a/f\n+++ b/f\n@@ -1,1 +1,1 @@\n-x\n\\ No newline at end of file\n+x\n"
        );
    }

    #[test]
    fn v1_to_v2_rewrites_only_the_schema_value_bytes() {
        let text =
            "# CRLF \r\nschema   =  1 # keep\r\n\r\n[[owner]]\r\nid = \"arch\"\r\ntitle='A'\r\n";
        assert_eq!(
            v1_document(text).unwrap(),
            text.replacen("=  1 #", "=  2 #", 1)
        );
        assert!(v1_document("schema = 2\r\n").is_err());
        assert!(v1_document("[x]\r\nschema = 1\r\n").is_err());
        let record = "+++\r\nschema = 1\r\nid = \"acme.gap.x\"\r\nkind = \"gap\"\r\ntitle = \"Synthetic gap\"\r\nstatus = \"draft\"\r\nowner = \"arch\"\r\ngap = \"missing\"\r\ndescription = \"Synthetic.\"\r\n[scope]\r\nproduct = true\r\n+++\r\nBody\r\n";
        assert_eq!(
            v1_record(record).unwrap(),
            record.replacen("schema = 1", "schema = 2", 1)
        );
    }
}
