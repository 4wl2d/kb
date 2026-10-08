//! Context assembly (docs/architecture.md §5): task scope resolution, applicability,
//! mandatory selection independent of full-text ranking, requires closure, effective
//! settings, supplementary ranking, packing whole units into a budget, rendering and
//! receipts.
//!
//! Pure logic over [`KnowledgeView`]: no Git, filesystem or clock access. Identical logical
//! inputs and snapshot produce byte-identical output; the result never contains timings.
//!
//! Submodules: `task` (scope resolution), `applicability`, `rank` (candidates, points and the
//! kind-prior table), `settings` (overrides), `pack` (budget measurement), `present`
//! (compact/human/JSON rendering), plus the public [`memory`], [`routing`], [`search`] and
//! [`show`] modules.

mod applicability;
pub mod code;
pub mod delivery;
pub mod discovery;
pub(crate) mod lexical;
pub mod memory;
pub mod outline;
mod pack;
mod present;
mod rank;
pub mod routing;
pub mod search;
mod settings;
pub mod show;
mod task;
pub mod temporal;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::diag::{Diagnostic, Severity};
use crate::error::{ErrorCode, KbError, Result};
use crate::knowledge::{
    Freshness, KnowledgeView, MetaEntry, Origin, ProposalChange, RecordEntry, Selection,
    SnapshotInfo,
};
use crate::model::{BudgetUnit, Intent, Kind, ParsedRecord, RecordMeta, Section, Status};
use crate::output::Format;
use crate::util::sha256_hex;

pub use applicability::evaluate as evaluate_applicability;
pub use applicability::evaluate_saved_scope;
pub use applicability::{Applicability, Dim, Verdict};
pub use pack::{estimate_tokens, measure};
pub use rank::{MIN_SCORE, Signal, SignalKind, kind_prior, mentioned_ids};
pub use settings::{EffectiveSetting, SettingSource};
pub use task::resolve as resolve_scope;
pub use task::{
    Ambiguity, DimScope, MAX_PATHS, MAX_SCOPE_IDS, MAX_TASK_BYTES, ResolvedPath, TaskScope,
};

/// Upper bound accepted for `max_supplementary`.
pub const MAX_SUPPLEMENTARY_LIMIT: usize = 200;

/// Which optional Markdown sections to add as separate units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SectionsMode {
    #[default]
    None,
    /// Sections of mandatory-tier records.
    Mandatory,
    /// Sections of every included record.
    All,
}

impl SectionsMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SectionsMode::None => "none",
            SectionsMode::Mandatory => "mandatory",
            SectionsMode::All => "all",
        }
    }
}

/// A context request (CLI options of `kb context`).
#[derive(Debug, Clone)]
pub struct ContextRequest {
    pub stale: Option<u32>,
    pub intent: Intent,
    pub task: Option<String>,
    pub repos: Vec<String>,
    /// Host-relative paths or `repo:path`.
    pub paths: Vec<String>,
    /// Host-relative paths of a host diff (new and old names). Bounded by the diff, not by
    /// the `--path` limit; reported with `paths` in the result's `request.paths`.
    pub changed_paths: Vec<String>,
    pub modules: Vec<String>,
    pub features: Vec<String>,
    pub concepts: Vec<String>,
    /// Explicit, exhaustive change categories. Empty means unknown, not no change.
    pub change_types: Vec<String>,
    pub as_of: Option<temporal::AsOf>,
    pub change: Option<ChangeScope>,
    /// Budget (default: profile `context.default_budget`).
    pub budget: Option<u64>,
    /// Budget unit (default: profile `context.default_budget_unit`).
    pub budget_unit: Option<BudgetUnit>,
    pub include_proposals: bool,
    pub sections: SectionsMode,
    /// Cap on ranked supplementary records (default: profile `context.max_supplementary`).
    pub max_supplementary: Option<usize>,
    /// Explicit host versions; they override versions detected from `version_file`.
    pub host_versions: Vec<(String, semver::Version)>,
}

impl ContextRequest {
    pub fn new(intent: Intent) -> ContextRequest {
        ContextRequest {
            stale: None,
            intent,
            task: None,
            repos: Vec::new(),
            paths: Vec::new(),
            changed_paths: Vec::new(),
            modules: Vec::new(),
            features: Vec::new(),
            concepts: Vec::new(),
            change_types: Vec::new(),
            as_of: None,
            change: None,
            budget: None,
            budget_unit: None,
            include_proposals: false,
            sections: SectionsMode::None,
            max_supplementary: None,
            host_versions: Vec::new(),
        }
    }

    /// The result's `request.paths`: `paths`, or the sorted union of `paths` and
    /// `changed_paths` when a diff supplied paths.
    pub fn all_paths(&self) -> Vec<String> {
        if self.changed_paths.is_empty() {
            return self.paths.clone();
        }
        let all: BTreeSet<&String> = self.paths.iter().chain(&self.changed_paths).collect();
        all.into_iter().cloned().collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangeScope {
    pub base: String,
    pub merge_base: String,
    pub head: Option<String>,
    pub working_tree: bool,
}

/// Facts about the invocation environment gathered by adapters (host detection, snapshot
/// selection). Context assembly never probes the environment itself.
#[derive(Debug, Clone)]
pub struct TaskEnv {
    pub reference_date: Option<crate::freshness::ReferenceDate>,
    /// Registry repo id of the host repository, when identified.
    pub host_repo: Option<String>,
    /// How the host repo was identified (`argument`, `binding-file`, `remote`).
    pub host_repo_source: Option<String>,
    /// Host `HEAD` commit.
    pub host_head: Option<String>,
    /// Host code versions read from registry `version_file`s.
    pub host_versions: BTreeMap<String, semver::Version>,
    pub snapshot: SnapshotInfo,
    /// Tracked file candidates supplied by a host/provider adapter, not filesystem reads.
    pub inferred_paths: Vec<String>,
    pub inferred_features: Vec<String>,
    /// Paths proved to be files by a Git diff/listing, including extensionless files.
    pub known_files: BTreeSet<String>,
    /// A complete diff was supplied, even if it contained no ordinary files.
    pub changed_scope: bool,
    /// Zero-context patch from the selected host diff; used only as lexical evidence.
    pub changed_text: String,
    /// Adapter findings about the supplied facts (skipped names, dropped lexical evidence),
    /// reported in the result's `issues`.
    pub notes: Vec<Diagnostic>,
    pub code_info: Option<code::CodeInfo>,
    pub code_units: Vec<code::CodeEvidence>,
    pub delivery: delivery::DeliveryState,
}

impl TaskEnv {
    /// An environment without host information.
    pub fn new(snapshot: SnapshotInfo) -> TaskEnv {
        TaskEnv {
            reference_date: None,
            host_repo: None,
            host_repo_source: None,
            host_head: None,
            host_versions: BTreeMap::new(),
            snapshot,
            inferred_paths: Vec::new(),
            inferred_features: Vec::new(),
            known_files: BTreeSet::new(),
            changed_scope: false,
            changed_text: String::new(),
            notes: Vec::new(),
            code_info: None,
            code_units: Vec::new(),
            delivery: delivery::DeliveryState::default(),
        }
    }

    /// Use the patch text of the selected host diff as lexical evidence. `None` (the patch
    /// exceeded the adapter's size limit) skips the changed-text hints with a note: they only
    /// add change-type candidates, so their absence never prunes an obligation.
    pub fn set_changed_text(&mut self, text: Option<String>) {
        match text {
            Some(text) => self.changed_text = text,
            None => self.notes.push(Diagnostic::info(
                "CHANGED_TEXT_SKIPPED",
                "the host patch exceeds the lexical-analysis limit; changed-text identifier \
                 hints were skipped (pass --change-type for explicit categories)",
            )),
        }
    }
}

/// Completeness of the mandatory part, from best to worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Completeness {
    Complete,
    Partial,
    Conflict,
    Incomplete,
}

impl Completeness {
    pub fn as_str(self) -> &'static str {
        match self {
            Completeness::Complete => "complete",
            Completeness::Partial => "partial",
            Completeness::Conflict => "conflict",
            Completeness::Incomplete => "incomplete",
        }
    }

    pub fn parse(s: &str) -> Option<Completeness> {
        [
            Completeness::Complete,
            Completeness::Partial,
            Completeness::Conflict,
            Completeness::Incomplete,
        ]
        .into_iter()
        .find(|c| c.as_str() == s)
    }
}

/// Why the result is not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReason {
    pub status: Completeness,
    /// Stable code, e.g. `REQUIRES_MISSING`, `FRESHNESS_UNVERIFIED`.
    pub code: String,
    pub message: String,
    /// True for snapshot provenance reasons (freshness, approval, working tree).
    pub provenance: bool,
}

/// Packing tier of a unit, in output order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// Accepted obligation (policy, invariant, contract, gap) whose scope applies.
    Mandatory,
    /// Record reached through `requires` from the mandatory tier (any kind, any scope;
    /// accepted, or deprecated with the label `deprecated`).
    Dependency,
    /// Local proposal (only with `include_proposals`); never mandatory.
    Proposal,
    /// Ranked optional context.
    Supplementary,
    /// Optional Markdown section of an included record.
    Section,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Mandatory => "mandatory",
            Tier::Dependency => "dependency",
            Tier::Proposal => "proposal",
            Tier::Supplementary => "supplementary",
            Tier::Section => "section",
        }
    }

    /// Mandatory and dependency units must all fit into the budget.
    pub fn is_required(self) -> bool {
        matches!(self, Tier::Mandatory | Tier::Dependency)
    }
}

/// Content of a unit.
#[derive(Debug, Clone)]
pub enum UnitBody {
    /// Static provider evidence, never an accepted knowledge record or obligation.
    Code(Box<code::CodeEvidence>),
    /// A record core: all typed normative content.
    Record(Arc<ParsedRecord>),
    /// One optional Markdown section, verbatim.
    Section(Section),
    /// A proposal that removes an accepted record.
    Removal,
}

/// A whole unit of output; units are never truncated.
#[derive(Debug, Clone)]
pub struct Unit {
    pub reuse: Option<delivery::Reuse>,
    pub content_digest: Option<String>,
    pub tier: Tier,
    /// Unique unit key: the record id; `proposal:<id>` for proposal units; `<unit id>#<section>`
    /// for section units.
    pub id: String,
    pub record_id: String,
    pub kind: Kind,
    pub title: String,
    pub status: Status,
    pub origin: Origin,
    /// KB-root-relative source path (provenance).
    pub path: String,
    /// Short inclusion reason (rendered, counted in the budget).
    pub why: String,
    pub labels: Vec<String>,
    /// Ranking score (supplementary units only).
    pub score: Option<i64>,
    /// Scoring breakdown (explain only).
    pub signals: Vec<Signal>,
    pub body: UnitBody,
}

impl Unit {
    fn code(evidence: code::CodeEvidence) -> Self {
        let key = format!(
            "{}:{}:{}",
            evidence.repo, evidence.commit, evidence.symbol.id
        );
        let id = format!("code:{}", sha256_hex(key.as_bytes()));
        Self {
            reuse: None,
            content_digest: None,
            tier: Tier::Supplementary,
            id: id.clone(),
            record_id: id,
            kind: Kind::Reference,
            title: evidence.symbol.name.clone(),
            status: Status::Draft,
            origin: Origin::Proposal,
            path: evidence.symbol.path.clone(),
            why: evidence.reason.clone(),
            labels: vec!["non-normative-code-evidence".into()],
            score: None,
            signals: Vec::new(),
            body: UnitBody::Code(Box::new(evidence)),
        }
    }
    fn record(
        tier: Tier,
        entry: &MetaEntry,
        parsed: Arc<ParsedRecord>,
        why: String,
        labels: Vec<String>,
    ) -> Unit {
        Unit {
            reuse: None,
            content_digest: None,
            tier,
            id: entry.meta.id.clone(),
            record_id: entry.meta.id.clone(),
            kind: entry.meta.kind,
            title: entry.meta.title.clone(),
            status: entry.meta.status,
            origin: entry.origin,
            path: entry.path.clone(),
            why,
            labels,
            score: None,
            signals: Vec::new(),
            body: UnitBody::Record(parsed),
        }
    }

    fn sections(&self) -> Vec<Unit> {
        let UnitBody::Record(parsed) = &self.body else {
            return Vec::new();
        };
        parsed
            .sections
            .iter()
            .map(|s| Unit {
                reuse: None,
                content_digest: None,
                tier: Tier::Section,
                id: format!("{}#{}", self.id, s.id),
                record_id: self.record_id.clone(),
                kind: self.kind,
                title: self.title.clone(),
                status: self.status,
                origin: self.origin,
                path: self.path.clone(),
                why: format!("section of {}", self.record_id),
                labels: self
                    .labels
                    .iter()
                    .filter(|l| l.starts_with("proposal"))
                    .cloned()
                    .collect(),
                score: None,
                signals: Vec::new(),
                body: UnitBody::Section(s.clone()),
            })
            .collect()
    }
}

/// An obligation whose applicability could not be determined (listed, not included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeterminedEntry {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    /// E.g. `undetermined: modules unknown`.
    pub detail: String,
}

/// Why a candidate is not in the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExcludedReason {
    Temporal,
    /// Did not fit into the remaining budget.
    Budget,
    /// Beyond the `max_supplementary` cap.
    MaxSupplementary,
    BelowMinScore,
    NotApplicable,
    NotAccepted,
    /// Proposal unrelated to the task.
    NotRelevant,
    /// Proposal that does not parse.
    Invalid,
}

impl ExcludedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ExcludedReason::Temporal => "temporal",
            ExcludedReason::Budget => "budget",
            ExcludedReason::MaxSupplementary => "max-supplementary",
            ExcludedReason::BelowMinScore => "below-min-score",
            ExcludedReason::NotApplicable => "not-applicable",
            ExcludedReason::NotAccepted => "not-accepted",
            ExcludedReason::NotRelevant => "not-relevant",
            ExcludedReason::Invalid => "invalid",
        }
    }
}

/// An excluded candidate (receipt / explain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excluded {
    /// Record id, `id#section`, or a proposal path.
    pub id: String,
    pub reason: ExcludedReason,
    pub score: Option<i64>,
    pub detail: String,
}

impl Excluded {
    pub(crate) fn new(
        id: &str,
        reason: ExcludedReason,
        score: Option<i64>,
        detail: impl Into<String>,
    ) -> Excluded {
        Excluded {
            id: id.to_string(),
            reason,
            score,
            detail: detail.into(),
        }
    }
}

/// Number of included units per tier.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TierCounts {
    pub mandatory: usize,
    pub dependencies: usize,
    pub proposals: usize,
    pub supplementary: usize,
    pub sections: usize,
}

impl TierCounts {
    fn of<'a>(units: impl IntoIterator<Item = &'a Unit>) -> TierCounts {
        let mut c = TierCounts::default();
        for u in units {
            match u.tier {
                Tier::Mandatory => c.mandatory += 1,
                Tier::Dependency => c.dependencies += 1,
                Tier::Proposal => c.proposals += 1,
                Tier::Supplementary => c.supplementary += 1,
                Tier::Section => c.sections += 1,
            }
        }
        c
    }
}

/// Budget accounting of the rendered payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetReport {
    pub limit: u64,
    pub unit: BudgetUnit,
    /// Measured size of header + units + receipt summary.
    pub used: u64,
    /// Format the payload was measured in.
    pub format: Format,
}

/// Receipt summary and budget (the last counted piece of the payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footer {
    pub snapshot_digest: Option<String>,
    /// `sha256:<hex>` of the canonical deterministic result (`to_json(result, false)`)
    /// without this field; see [`receipt_id_of`].
    pub receipt_id: String,
    pub counts: TierCounts,
    /// Optional units that did not fit, in packing order.
    pub excluded_budget: Vec<String>,
    /// Number of candidates excluded for other reasons (see explain).
    pub excluded_other: usize,
    pub budget: BudgetReport,
}

/// Everything rendered before the units.
#[derive(Debug, Clone)]
pub struct Header {
    pub reference_date: Option<crate::freshness::ReferenceDate>,
    pub pruned_change_types: Vec<String>,
    pub delivery: delivery::DeliveryState,
    pub code_info: Option<code::CodeInfo>,
    pub request: ContextRequest,
    /// Effective cap on supplementary records.
    pub max_supplementary: usize,
    pub snapshot: SnapshotInfo,
    pub scope: TaskScope,
    pub completeness: Completeness,
    /// Sorted by severity (worst first), then code.
    pub reasons: Vec<StatusReason>,
    pub settings: Vec<EffectiveSetting>,
    pub undetermined: Vec<UndeterminedEntry>,
    /// Context-level findings (dependency problems, ignored overrides, invalid proposals).
    pub issues: Vec<Diagnostic>,
    /// Snapshot diagnostics (parse + validation).
    pub diagnostics: Vec<Diagnostic>,
    pub notes: Vec<String>,
}

/// An assembled context.
#[derive(Debug, Clone)]
pub struct ContextResult {
    pub header: Header,
    /// Included units in output order (mandatory, dependencies, proposals, supplementary,
    /// sections).
    pub units: Vec<Unit>,
    pub footer: Footer,
    /// `requires` edges followed from the mandatory tier.
    pub requires: Vec<(String, String)>,
    /// Every excluded candidate with its reason.
    pub excluded: Vec<Excluded>,
}

impl ContextResult {
    pub fn status(&self) -> Completeness {
        self.header.completeness
    }

    /// Completeness ignoring snapshot provenance reasons (freshness, approval, working tree).
    /// Routing fixtures compare expectations against this status.
    pub fn knowledge_status(&self) -> Completeness {
        self.header
            .reasons
            .iter()
            .filter(|r| !r.provenance)
            .map(|r| r.status)
            .max()
            .unwrap_or(Completeness::Complete)
    }

    /// `CONTEXT_INCOMPLETE` unless the result is complete. The full result is still output.
    pub fn failure(&self) -> Option<KbError> {
        let status = self.status();
        if status == Completeness::Complete {
            return None;
        }
        let codes: Vec<&str> = self
            .header
            .reasons
            .iter()
            .map(|r| r.code.as_str())
            .collect();
        let reasons: Vec<Value> = self
            .header
            .reasons
            .iter()
            .map(|r| json!({"status": r.status.as_str(), "code": r.code, "message": r.message}))
            .collect();
        Some(
            KbError::new(
                ErrorCode::ContextIncomplete,
                format!("context is {}: {}", status.as_str(), codes.join(", ")),
            )
            .with_details(json!({
                "completeness": status.as_str(),
                "reasons": reasons,
                "receipt": self.footer.receipt_id,
            }))
            .with_hint(
                "the result above lists what is missing; do not treat it as complete \
                 (pass --repo/--path/--host-version, sync, or fix validation errors)",
            ),
        )
    }

    /// Record ids of included mandatory-tier units (mandatory + dependencies).
    pub fn mandatory_ids(&self) -> Vec<&str> {
        self.units
            .iter()
            .filter(|u| u.tier.is_required())
            .map(|u| u.record_id.as_str())
            .collect()
    }

    /// Ids of all included units (record ids and `id#section`).
    pub fn included_ids(&self) -> Vec<&str> {
        self.units.iter().map(|u| u.id.as_str()).collect()
    }

    pub fn receipt_id(&self) -> &str {
        &self.footer.receipt_id
    }
}

/// Render compact or human text (JSON: [`to_json`] pretty-printed as a standalone document).
/// The explain part is appended after the counted payload and marked as not counted.
///
/// In JSON mode the budget measures the `result` member exactly as `kb context --json`
/// prints it: `to_json(result, false)` inside the `kb.cli.v1` envelope pretty-printed by
/// `output::emit` (from its `{` to its `}`). The envelope framing, `error`, `meta` and the
/// `explain` member are not counted.
pub fn render(result: &ContextResult, format: Format, explain: bool) -> String {
    if format == Format::Json {
        let mut s = serde_json::to_string_pretty(&to_json(result, explain)).unwrap_or_default();
        s.push('\n');
        return s;
    }
    let mut s = present::header(&result.header, format);
    for u in &result.units {
        s.push_str(&present::unit(u, format));
    }
    s.push_str(&present::footer(&result.footer, format));
    if explain {
        s.push_str(&present::explain_text(result));
    }
    s
}

/// Deterministic JSON result (the `result` of the CLI envelope). Contains no timings.
pub fn to_json(result: &ContextResult, explain: bool) -> Value {
    let mut m = present::header_json(&result.header);
    m.insert(
        "units".into(),
        Value::Array(result.units.iter().map(present::unit_json).collect()),
    );
    m.extend(present::footer_json(&result.footer));
    if explain {
        m.insert("explain".into(), present::explain_json(result));
    }
    Value::Object(m)
}

/// Parse a `--host-version` value `repo=x.y.z` (semver). Errors: `INVALID_INPUT`.
pub fn parse_host_version(s: &str) -> Result<(String, semver::Version)> {
    let (repo, v) = s.split_once('=').ok_or_else(|| {
        KbError::invalid_input(format!("host version `{s}` must be `repo=x.y.z`"))
    })?;
    let version = semver::Version::parse(v.trim())
        .map_err(|e| KbError::invalid_input(format!("host version `{s}`: {e}")))?;
    Ok((repo.trim().to_string(), version))
}

/// Receipt id of a JSON result produced by [`to_json`]: `sha256:` + hex of the canonical JSON
/// (sorted keys, compact) of the default result, i.e. without `receipt.id` and without the
/// uncounted `explain` member. The id is therefore verifiable from the default output and
/// from `--explain` output alike.
pub fn receipt_id_of(result_json: &Value) -> String {
    let mut v = result_json.clone();
    if let Some(m) = v.as_object_mut() {
        m.remove("explain");
    }
    if let Some(r) = v.get_mut("receipt").and_then(Value::as_object_mut) {
        r.remove("id");
    }
    format!("sha256:{}", sha256_hex(canonical_json(&v).as_bytes()))
}

/// Canonical JSON: object keys sorted, no insignificant whitespace.
pub fn canonical_json(v: &Value) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_canonical(&m[k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn receipt_placeholder() -> String {
    format!("sha256:{}", "0".repeat(64))
}

// ---------------------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------------------

/// Status reasons and context-level issues collected during assembly.
#[derive(Default)]
struct Findings {
    reasons: BTreeMap<String, StatusReason>,
    issues: Vec<Diagnostic>,
}

impl Findings {
    fn reason(&mut self, status: Completeness, code: &str, provenance: bool, detail: &str) {
        let r = self
            .reasons
            .entry(code.to_string())
            .or_insert_with(|| StatusReason {
                status,
                code: code.to_string(),
                message: String::new(),
                provenance,
            });
        if !r.message.is_empty() {
            r.message.push_str("; ");
        }
        r.message.push_str(detail);
        r.status = r.status.max(status);
    }

    fn issue(&mut self, severity: Severity, code: &str, record: Option<&str>, message: String) {
        let mut d = Diagnostic::new(severity, code, message);
        d.record = record.map(str::to_string);
        self.issues.push(d);
    }

    fn status(&self) -> Completeness {
        self.reasons
            .values()
            .map(|r| r.status)
            .max()
            .unwrap_or(Completeness::Complete)
    }

    fn sorted_reasons(&self) -> Vec<StatusReason> {
        let mut v: Vec<StatusReason> = self.reasons.values().cloned().collect();
        v.sort_by(|a, b| b.status.cmp(&a.status).then_with(|| a.code.cmp(&b.code)));
        v
    }
}

/// A record in the mandatory tier.
struct Chosen {
    entry: MetaEntry,
    tier: Tier,
    why: String,
    labels: Vec<String>,
    required_by: BTreeSet<String>,
}

/// Mandatory-tier order: policy, invariant, contract, gap, then the other kinds.
fn tier_kind_rank(kind: Kind) -> u8 {
    match kind {
        Kind::Policy => 0,
        Kind::Invariant => 1,
        Kind::Contract => 2,
        Kind::Gap => 3,
        Kind::Feature => 4,
        Kind::Decision => 5,
        Kind::Procedure => 6,
        Kind::Reference => 7,
    }
}

/// Assemble task context. Errors: `CONTEXT_BUDGET_EXCEEDED` (details `required`, `limit`,
/// `unit`) when the header and mandatory units do not fit, `UNKNOWN_SCOPE` for explicit
/// unknown registry ids, `INVALID_INPUT` for limits and unsafe paths, and view errors.
/// Incomplete results are `Ok`; see [`ContextResult::failure`].
pub fn assemble(
    req: &ContextRequest,
    env: &TaskEnv,
    view: &dyn KnowledgeView,
    format: Format,
) -> Result<ContextResult> {
    if env.snapshot.content_digest.is_none()
        && (env.delivery.since_receipt.is_some() || env.delivery.core_receipt.is_some())
    {
        return Err(KbError::invalid_input(
            "delivery reuse requires a snapshot content digest",
        ));
    }
    if req.as_of.is_some() && req.include_proposals {
        return Err(KbError::invalid_input(
            "historical context cannot include proposal overlays",
        ));
    }
    let temporal = req
        .as_of
        .as_ref()
        .map(|point| temporal::TemporalView::new(view, point))
        .transpose()?;
    let view: &dyn KnowledgeView = temporal.as_ref().map_or(view, |v| v);
    let cfg = view.config().context.clone();
    let limit = req.budget.unwrap_or(cfg.default_budget);
    let unit = req.budget_unit.unwrap_or(cfg.default_budget_unit);
    let max_supplementary = req.max_supplementary.unwrap_or(cfg.max_supplementary);
    if limit == 0 {
        return Err(KbError::invalid_input("the budget must be positive"));
    }
    if max_supplementary > MAX_SUPPLEMENTARY_LIMIT {
        return Err(KbError::invalid_input(format!(
            "max_supplementary is {max_supplementary}; the limit is {MAX_SUPPLEMENTARY_LIMIT}"
        )));
    }
    let registry = view.registry();
    let mut scoped_env = env.clone();
    if req.intent == Intent::Diagnose {
        let tokens = crate::normalize::tokens(req.task.as_deref().unwrap_or_default());
        for candidate in view.term_candidates(&req.concepts, &tokens)? {
            if candidate.meta.status != Status::Accepted {
                continue;
            }
            if let Some(id) = &candidate.meta.feature {
                scoped_env.inferred_features.push(id.clone());
            }
            if matches!(candidate.meta.kind, Kind::Feature | Kind::Gap) {
                scoped_env
                    .inferred_features
                    .extend(candidate.meta.scope.features.clone());
            }
        }
        scoped_env.inferred_features.sort();
        scoped_env.inferred_features.dedup();
    }
    let mut task = task::resolve(req, &scoped_env, registry)?;
    let mut f = Findings::default();
    if let Some(slice) = &temporal
        && slice.undated_accepted > 0
    {
        f.reason(
            Completeness::Partial,
            "AS_OF_UNDATED",
            false,
            &format!(
                "withheld {} accepted record(s) without introduced evidence; see --explain",
                slice.undated_accepted
            ),
        );
    }
    if let Some(slice) = &temporal
        && slice.unresolved_accepted > 0
    {
        f.reason(
            Completeness::Partial,
            "AS_OF_BOUND_UNRESOLVED",
            false,
            &format!(
                "withheld {} accepted record(s) scoped to other repositories whose commit \
                 bounds cannot be resolved in this host; see --explain",
                slice.unresolved_accepted
            ),
        );
    }
    if req.intent == Intent::Diagnose
        && req.paths.is_empty()
        && req.modules.is_empty()
        && !env.changed_scope
    {
        f.reason(Completeness::Partial, "DIAGNOSE_SCOPE_PROVISIONAL", false,
            "path-free diagnosis supplies candidate subsystem knowledge; confirm paths before editing");
    }
    f.issues.extend(task.notes.iter().cloned());

    // Mandatory selection: accepted obligations whose applicability is Applies.
    let mut chosen: BTreeMap<String, Chosen> = BTreeMap::new();
    let mut undetermined: BTreeMap<String, (MetaEntry, Applicability)> = BTreeMap::new();
    let mut pruned_changes = Vec::new();
    for e in view.metas_by_kind(&Kind::MANDATORY, Origin::Accepted)? {
        if e.meta.status != Status::Accepted || chosen.contains_key(&e.meta.id) {
            continue;
        }
        let app = applicability::evaluate(&e.meta, &task);
        match app.verdict() {
            Verdict::Applies => {
                chosen.insert(
                    e.meta.id.clone(),
                    Chosen {
                        tier: Tier::Mandatory,
                        why: app.describe(),
                        labels: Vec::new(),
                        required_by: BTreeSet::new(),
                        entry: e,
                    },
                );
            }
            Verdict::Undetermined => {
                undetermined.insert(e.meta.id.clone(), (e, app));
            }
            Verdict::NotApplicable => {
                if app.not_applicable.contains(&Dim::ChangeTypes) {
                    pruned_changes.push(Excluded {
                        id: e.meta.id.clone(),
                        reason: ExcludedReason::NotApplicable,
                        score: None,
                        detail: app.describe(),
                    });
                }
            }
        }
    }
    let applying_policies: Vec<Arc<RecordMeta>> = chosen
        .values()
        .filter(|c| c.entry.meta.kind == Kind::Policy)
        .map(|c| c.entry.meta.clone())
        .collect();

    // Overrides from policies of undetermined applicability are reported as possible, even
    // when such a policy is later pulled in as a required dependency.
    let undetermined_policies: Vec<Arc<RecordMeta>> = undetermined
        .values()
        .filter(|(e, _)| e.meta.kind == Kind::Policy)
        .map(|(e, _)| e.meta.clone())
        .collect();
    let requires = requires_closure(view, &task, &mut chosen, &mut f)?;
    undetermined.retain(|id, _| !chosen.contains_key(id));

    // Effective settings of mandatory-tier policies.
    let tier_policies: Vec<Arc<RecordMeta>> = chosen
        .values()
        .filter(|c| c.entry.meta.kind == Kind::Policy)
        .map(|c| c.entry.meta.clone())
        .collect();
    let (effective, setting_issues) = settings::effective(
        &tier_policies,
        &applying_policies,
        &undetermined_policies,
        registry,
    );
    f.issues.extend(setting_issues);
    for s in effective.iter().filter(|s| s.is_conflict()) {
        let detail = format!("`{}` has conflicting applicable overrides", s.target());
        f.reason(Completeness::Conflict, "SETTING_CONFLICT", false, &detail);
        f.issue(Severity::Error, "SETTING_CONFLICT", Some(&s.policy), detail);
    }

    // Supplementary candidates and ranking.
    let mandatory_metas: BTreeMap<String, Arc<RecordMeta>> = chosen
        .iter()
        .map(|(k, c)| (k.clone(), c.entry.meta.clone()))
        .collect();
    let input = rank::RankInput {
        intent: req.intent,
        task_text: req.task.as_deref(),
        task: &task,
        mandatory: &mandatory_metas,
        max_supplementary,
    };
    let mut ambiguities = task.ambiguities.clone();
    let supp = rank::supplementary(view, &input, &mut ambiguities)?;
    task.ambiguities = ambiguities;
    let mut excluded = supp.excluded;
    let pruned_change_types: Vec<_> = pruned_changes
        .iter()
        .filter(|p| !chosen.contains_key(&p.id))
        .map(|p| p.id.clone())
        .collect();
    for pruned in pruned_changes
        .into_iter()
        .filter(|p| !chosen.contains_key(&p.id))
    {
        if !excluded
            .iter()
            .any(|e| e.id == pruned.id && e.reason == pruned.reason)
        {
            excluded.push(pruned);
        }
    }
    if let Some(slice) = &temporal {
        excluded.extend(slice.excluded.clone());
    }

    // Content of mandatory-tier and supplementary records.
    let mut ids: Vec<String> = chosen.keys().cloned().collect();
    ids.extend(supp.selected.iter().map(|r| r.entry.meta.id.clone()));
    let content: BTreeMap<String, RecordEntry> = view
        .records(&ids, Origin::Accepted)?
        .into_iter()
        .map(|r| (r.parsed.record.id().to_string(), r))
        .collect();
    let fetch = |id: &str| -> Result<Arc<ParsedRecord>> {
        content.get(id).map(|r| r.parsed.clone()).ok_or_else(|| {
            KbError::new(
                ErrorCode::IndexError,
                format!("content of `{id}` is missing from the snapshot view"),
            )
        })
    };

    let mut order: Vec<&Chosen> = chosen.values().collect();
    order.sort_by(|a, b| {
        (a.tier, tier_kind_rank(a.entry.meta.kind), &a.entry.meta.id).cmp(&(
            b.tier,
            tier_kind_rank(b.entry.meta.kind),
            &b.entry.meta.id,
        ))
    });
    let mut mandatory_units = Vec::with_capacity(order.len());
    for c in order {
        let mut why = c.why.clone();
        if c.tier == Tier::Mandatory && !c.required_by.is_empty() {
            why.push_str(&format!("; also required by {}", join(&c.required_by)));
        }
        mandatory_units.push(Unit::record(
            c.tier,
            &c.entry,
            fetch(&c.entry.meta.id)?,
            why,
            c.labels.clone(),
        ));
    }
    let mut optional_units = Vec::new();
    if req.include_proposals {
        let mut included: BTreeMap<String, Arc<RecordMeta>> = mandatory_metas.clone();
        for r in &supp.selected {
            included.insert(r.entry.meta.id.clone(), r.entry.meta.clone());
        }
        let (units, dropped) = proposal_units(view, &task, &included, &mut f)?;
        optional_units.extend(units);
        excluded.extend(dropped);
    }
    for r in &supp.selected {
        let mut u = Unit::record(
            Tier::Supplementary,
            &r.entry,
            fetch(&r.entry.meta.id)?,
            short_why(r),
            r.labels.clone(),
        );
        u.score = Some(r.score);
        u.signals = r.signals.clone();
        optional_units.push(u);
    }
    optional_units.extend(env.code_units.iter().cloned().map(Unit::code));
    for unit in mandatory_units.iter_mut().chain(optional_units.iter_mut()) {
        delivery::annotate(unit, &env.delivery, env.snapshot.content_digest.is_some());
    }

    // Completeness.
    status_reasons(&env.snapshot, view.diagnostics(), &task, &mut f);
    let undetermined: Vec<UndeterminedEntry> = undetermined
        .values()
        .map(|(e, app)| UndeterminedEntry {
            id: e.meta.id.clone(),
            kind: e.meta.kind,
            title: e.meta.title.clone(),
            detail: app.describe(),
        })
        .collect();
    if !undetermined.is_empty() {
        let ids: Vec<&str> = undetermined.iter().map(|u| u.id.as_str()).collect();
        f.reason(
            Completeness::Partial,
            "UNDETERMINED_OBLIGATIONS",
            false,
            &format!("{} obligation(s) may apply: {}", ids.len(), ids.join(", ")),
        );
    }
    let mut notes = Vec::new();
    if mandatory_units.is_empty() && optional_units.is_empty() {
        notes.push(
            "no applicable knowledge was found; completeness is relative to the declared, \
             validated knowledge base and proves nothing about the project"
                .to_string(),
        );
    }
    let mut issues = std::mem::take(&mut f.issues);
    let dated = req.stale.is_some()
        || mandatory_units.iter().chain(optional_units.iter()).any(
            |u| matches!(&u.body, UnitBody::Record(p) if crate::freshness::has_dates(&p.record)),
        );
    for unit in mandatory_units.iter().chain(optional_units.iter()) {
        if let UnitBody::Record(parsed) = &unit.body {
            issues.extend(crate::freshness::warnings(
                &parsed.record,
                env.reference_date.as_ref(),
                req.stale,
            ));
        }
    }
    crate::diag::normalize(&mut issues);
    let mut diagnostics = view.diagnostics().to_vec();
    crate::diag::normalize(&mut diagnostics);
    let header = Header {
        reference_date: dated.then(|| env.reference_date.clone()).flatten(),
        pruned_change_types,
        code_info: env.code_info.clone(),
        delivery: env.delivery.clone(),
        request: req.clone(),
        max_supplementary,
        snapshot: env.snapshot.clone(),
        scope: task,
        completeness: f.status(),
        reasons: f.sorted_reasons(),
        settings: effective,
        undetermined,
        issues,
        diagnostics,
        notes,
    };

    let excluded_other = excluded.len();
    let packed = pack_units(
        &header,
        mandatory_units,
        optional_units,
        req.sections,
        excluded_other,
        (limit, unit, format),
    )?;
    excluded.extend(packed.dropped);
    excluded.sort_by(|a, b| (a.reason, &a.id).cmp(&(b.reason, &b.id)));

    let mut result = ContextResult {
        header,
        units: packed.units,
        footer: packed.footer,
        requires,
        excluded,
    };
    result.footer.receipt_id = receipt_id_of(&to_json(&result, false));
    Ok(result)
}

fn join(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// Short reason line of a supplementary unit: strongest source signals.
fn short_why(r: &rank::Ranked) -> String {
    let sources: Vec<String> = r
        .signals
        .iter()
        .filter(|s| {
            !matches!(
                s.kind,
                SignalKind::KindPrior | SignalKind::ScopeApplies | SignalKind::IntentSelector
            )
        })
        .map(|s| s.detail.clone())
        .collect();
    format!("score {}: {}", r.score, sources.join(", "))
}

/// Breadth-first closure over `requires` from the mandatory tier, ignoring scope filters but
/// checking every version constraint of a reached record (unknown version → undetermined).
/// A mandatory-tier record that is also reached through `requires` is re-checked once the
/// same way (mandatory selection skips unknown versions of repos outside the task). A missing,
/// draft or superseded target makes the result incomplete; a deprecated target is included
/// with the label `deprecated` and a `REQUIRES_DEPRECATED` warning, as validation only warns.
fn requires_closure(
    view: &dyn KnowledgeView,
    task: &TaskScope,
    chosen: &mut BTreeMap<String, Chosen>,
    f: &mut Findings,
) -> Result<Vec<(String, String)>> {
    let mut edges: BTreeSet<(String, String)> = BTreeSet::new();
    let mut frontier: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (id, c) in chosen.iter() {
        for t in &c.entry.meta.links.requires {
            edges.insert((id.clone(), t.clone()));
            frontier.entry(t.clone()).or_default().insert(id.clone());
        }
    }
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut rechecked: BTreeSet<String> = BTreeSet::new();
    while !frontier.is_empty() {
        let batch = std::mem::take(&mut frontier);
        let need: Vec<String> = batch
            .keys()
            .filter(|t| !chosen.contains_key(*t) && !visited.contains(*t))
            .cloned()
            .collect();
        visited.extend(need.iter().cloned());
        let found: BTreeMap<String, MetaEntry> = if need.is_empty() {
            BTreeMap::new()
        } else {
            view.metas_by_ids(&need, Origin::Accepted)?
                .into_iter()
                .map(|e| (e.meta.id.clone(), e))
                .collect()
        };
        for (target, sources) in batch {
            let src = join(&sources);
            let first_src = sources.iter().next().map(String::as_str);
            if let Some(c) = chosen.get_mut(&target) {
                if c.tier == Tier::Mandatory && rechecked.insert(target.clone()) {
                    let app = applicability::evaluate_required(&c.entry.meta, task);
                    dependency_versions(&app, &src, &target, first_src, f, &mut c.labels);
                }
                c.required_by.extend(sources);
                continue;
            }
            if !need.contains(&target) {
                continue;
            }
            let Some(e) = found.get(&target) else {
                let msg = format!("{src} requires `{target}`, which does not exist");
                f.reason(Completeness::Incomplete, "REQUIRES_MISSING", false, &msg);
                f.issue(Severity::Error, "REQUIRES_MISSING", first_src, msg);
                continue;
            };
            let mut labels = Vec::new();
            match e.meta.status {
                Status::Accepted => {}
                Status::Deprecated => {
                    let msg = format!("{src} requires `{target}`, which is deprecated");
                    f.issue(Severity::Warning, "REQUIRES_DEPRECATED", first_src, msg);
                    labels.push("deprecated".to_string());
                }
                Status::Draft | Status::Superseded => {
                    let msg = format!(
                        "{src} requires `{target}`, which is {}",
                        e.meta.status.as_str()
                    );
                    f.reason(
                        Completeness::Incomplete,
                        "REQUIRES_NOT_ACCEPTED",
                        false,
                        &msg,
                    );
                    f.issue(Severity::Error, "REQUIRES_NOT_ACCEPTED", first_src, msg);
                    continue;
                }
            }
            let app = applicability::evaluate_required(&e.meta, task);
            dependency_versions(&app, &src, &target, first_src, f, &mut labels);
            if app.verdict() == Verdict::NotApplicable
                && app.not_applicable.iter().any(|d| *d != Dim::Version)
            {
                labels.push("outside-task-scope".to_string());
            }
            for t in &e.meta.links.requires {
                edges.insert((target.clone(), t.clone()));
                frontier
                    .entry(t.clone())
                    .or_default()
                    .insert(target.clone());
            }
            chosen.insert(
                target.clone(),
                Chosen {
                    entry: e.clone(),
                    tier: Tier::Dependency,
                    why: format!("required by {src}"),
                    labels,
                    required_by: sources,
                },
            );
        }
    }
    Ok(edges.into_iter().collect())
}

/// Version applicability of a record reached through `requires` (`app` from
/// [`applicability::evaluate_required`]): a mismatch is `INCOMPATIBLE_DEPENDENCY` (conflict),
/// an unknown version `DEPENDENCY_VERSION_UNDETERMINED` (partial); both label the unit.
fn dependency_versions(
    app: &Applicability,
    src: &str,
    target: &str,
    first_src: Option<&str>,
    f: &mut Findings,
    labels: &mut Vec<String>,
) {
    let (status, code, severity, label) = match app.version_verdict() {
        Verdict::Applies => return,
        Verdict::NotApplicable => (
            Completeness::Conflict,
            "INCOMPATIBLE_DEPENDENCY",
            Severity::Error,
            "version-incompatible",
        ),
        Verdict::Undetermined => (
            Completeness::Partial,
            "DEPENDENCY_VERSION_UNDETERMINED",
            Severity::Warning,
            "version-undetermined",
        ),
    };
    let msg = format!(
        "{src} requires `{target}`: {}",
        app.version_notes.join("; ")
    );
    f.reason(status, code, false, &msg);
    f.issue(severity, code, first_src, msg);
    labels.push(label.to_string());
}

/// Proposal units relevant to the task: applicable (or undetermined) new/modified records and
/// changes to included records. Proposals never enter the mandatory tier.
fn proposal_units(
    view: &dyn KnowledgeView,
    task: &TaskScope,
    included: &BTreeMap<String, Arc<RecordMeta>>,
    f: &mut Findings,
) -> Result<(Vec<Unit>, Vec<Excluded>)> {
    let mut units = Vec::new();
    let mut dropped = Vec::new();
    let mut wanted = Vec::new();
    for p in view.proposals()? {
        let stale = p.stale.then(|| "stale".to_string());
        match &p.change {
            ProposalChange::Invalid => {
                f.issue(
                    Severity::Warning,
                    "PROPOSAL_INVALID",
                    None,
                    format!(
                        "local change `{}` does not parse; run `kb validate`",
                        p.path
                    ),
                );
                dropped.push(Excluded::new(
                    &p.path,
                    ExcludedReason::Invalid,
                    None,
                    "proposal does not parse",
                ));
            }
            ProposalChange::Removes { id } => match included.get(id) {
                Some(meta) => units.push(Unit {
                    reuse: None,
                    content_digest: None,
                    tier: Tier::Proposal,
                    id: format!("proposal:{id}"),
                    record_id: id.clone(),
                    kind: meta.kind,
                    title: meta.title.clone(),
                    status: meta.status,
                    origin: Origin::Proposal,
                    path: p.path.clone(),
                    why: "local proposal removes this record; the accepted record stays \
                          authoritative until the change is reviewed and merged"
                        .to_string(),
                    labels: std::iter::once("proposal:removes".to_string())
                        .chain(stale)
                        .collect(),
                    score: None,
                    signals: Vec::new(),
                    body: UnitBody::Removal,
                }),
                None => dropped.push(Excluded::new(
                    id,
                    ExcludedReason::NotRelevant,
                    None,
                    format!("removal proposal `{}`", p.path),
                )),
            },
            ProposalChange::New { id } | ProposalChange::Modifies { id } => {
                let relevant = included.contains_key(id)
                    || p.meta.as_ref().is_some_and(|m| {
                        applicability::evaluate(m, task).verdict() != Verdict::NotApplicable
                    });
                if relevant {
                    let modifies = matches!(p.change, ProposalChange::Modifies { .. });
                    wanted.push((p.clone(), id.clone(), modifies, stale));
                } else {
                    dropped.push(Excluded::new(
                        id,
                        ExcludedReason::NotRelevant,
                        None,
                        format!("proposal `{}` does not apply to the task", p.path),
                    ));
                }
            }
        }
    }
    let ids: Vec<String> = wanted
        .iter()
        .map(|(_, id, _, _)| id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let content: BTreeMap<String, RecordEntry> = if ids.is_empty() {
        BTreeMap::new()
    } else {
        view.records(&ids, Origin::Proposal)?
            .into_iter()
            .map(|r| (r.parsed.record.id().to_string(), r))
            .collect()
    };
    for (p, id, modifies, stale) in wanted {
        let Some(r) = content.get(&id) else {
            dropped.push(Excluded::new(
                &id,
                ExcludedReason::Invalid,
                None,
                format!("content of proposal `{}` is unavailable", p.path),
            ));
            continue;
        };
        let (label, why) = if modifies {
            (
                "proposal:modifies",
                format!(
                    "local proposal modifying accepted `{id}` (not reviewed; the accepted \
                     version stays authoritative and is not overridden)"
                ),
            )
        } else {
            (
                "proposal:new",
                "local proposal for a new record (not reviewed; never mandatory)".to_string(),
            )
        };
        let entry = MetaEntry {
            path: r.path.clone(),
            origin: Origin::Proposal,
            meta: Arc::new(r.parsed.meta()),
        };
        let labels = std::iter::once(label.to_string()).chain(stale).collect();
        let mut unit = Unit::record(Tier::Proposal, &entry, r.parsed.clone(), why, labels);
        unit.id = format!("proposal:{id}");
        units.push(unit);
    }
    units.sort_by(|a, b| (&a.id, &a.path).cmp(&(&b.id, &b.path)));
    Ok((units, dropped))
}

/// Snapshot provenance, validity and scope reasons.
fn status_reasons(
    snap: &SnapshotInfo,
    diagnostics: &[Diagnostic],
    task: &TaskScope,
    f: &mut Findings,
) {
    if snap.freshness == Freshness::Unverified {
        f.reason(
            Completeness::Partial,
            "FRESHNESS_UNVERIFIED",
            true,
            "the approved ref was not checked against the remote in this call",
        );
    }
    if snap.selection == Selection::WorkingTree {
        f.reason(
            Completeness::Partial,
            "WORKING_TREE",
            true,
            "knowledge comes from the local working tree, not an approved revision",
        );
    } else {
        match snap.approved {
            Some(true) => {}
            Some(false) => f.reason(
                Completeness::Partial,
                "NOT_APPROVED",
                true,
                &format!(
                    "revision {} is not reachable from the approved tip",
                    snap.revision.as_deref().unwrap_or("?")
                ),
            ),
            None => f.reason(
                Completeness::Partial,
                "APPROVAL_UNKNOWN",
                true,
                "whether the selected revision is approved is unknown",
            ),
        }
    }
    let errors = diagnostics.iter().filter(|d| d.is_error()).count();
    if errors > 0 {
        f.reason(
            Completeness::Incomplete,
            "SNAPSHOT_INVALID",
            false,
            &format!("the snapshot has {errors} validation error(s)"),
        );
    }
    if !task.repos.is_known() {
        f.reason(
            Completeness::Partial,
            "REPO_UNKNOWN",
            false,
            "no --repo given and the host repository was not identified",
        );
    }
}

struct Packed {
    units: Vec<Unit>,
    footer: Footer,
    dropped: Vec<Excluded>,
}

/// Pack whole units: the header and mandatory tier must fit (else
/// `CONTEXT_BUDGET_EXCEEDED` with the required amount); optional units are added first-fit
/// in tier order. The receipt summary is reserved at its largest possible size, so the
/// measured total never exceeds the limit.
fn pack_units(
    header: &Header,
    mandatory: Vec<Unit>,
    optional: Vec<Unit>,
    sections: SectionsMode,
    excluded_other: usize,
    (limit, unit, format): (u64, BudgetUnit, Format),
) -> Result<Packed> {
    let cost = |u: &Unit| measure(&present::unit(u, format), unit);
    let header_cost = measure(&present::header(header, format), unit);
    let mandatory: Vec<(Unit, u64)> = mandatory
        .into_iter()
        .map(|u| {
            let c = cost(&u);
            (u, c)
        })
        .collect();
    let mandatory_cost: u64 = mandatory.iter().map(|(_, c)| c).sum();

    let potential_sections: Vec<Unit> = match sections {
        SectionsMode::None => Vec::new(),
        SectionsMode::Mandatory => mandatory.iter().flat_map(|(u, _)| u.sections()).collect(),
        SectionsMode::All => mandatory
            .iter()
            .map(|(u, _)| u)
            .chain(optional.iter())
            .flat_map(Unit::sections)
            .collect(),
    };
    let max_counts = TierCounts::of(
        mandatory
            .iter()
            .map(|(u, _)| u)
            .chain(optional.iter())
            .chain(potential_sections.iter()),
    );
    let all_optional: Vec<String> = optional
        .iter()
        .chain(potential_sections.iter())
        .map(|u| u.id.clone())
        .collect();
    let footer_for =
        |counts: &TierCounts, excluded_budget: &[String], used: u64, limit: u64| Footer {
            snapshot_digest: header.snapshot.content_digest.clone(),
            receipt_id: receipt_placeholder(),
            counts: counts.clone(),
            excluded_budget: excluded_budget.to_vec(),
            excluded_other,
            budget: BudgetReport {
                limit,
                unit,
                used,
                format,
            },
        };
    let footer_cost = |counts: &TierCounts, excluded: &[String], used: u64, limit: u64| {
        measure(
            &present::footer(&footer_for(counts, excluded, used, limit), format),
            unit,
        )
    };
    let reserve = |l: u64| footer_cost(&max_counts, &all_optional, l, l);
    let fixed = header_cost + mandatory_cost;
    if fixed + reserve(limit) > limit {
        let required = pack::least_fit(|r| fixed + reserve(r));
        return Err(KbError::new(
            ErrorCode::ContextBudgetExceeded,
            format!(
                "the header and mandatory knowledge need {required} {} but the budget is {limit}",
                unit.as_str()
            ),
        )
        .with_details(json!({
            "required": required,
            "limit": limit,
            "unit": unit.as_str(),
            "format": format,
        }))
        .with_hint(format!(
            "re-run with --budget {required} or more; mandatory knowledge is never truncated"
        )));
    }

    let mut avail = limit - fixed - reserve(limit);
    let mut included: Vec<(Unit, u64)> = mandatory;
    let mut dropped: Vec<Unit> = Vec::new();
    let mut first_fit = |units: Vec<Unit>, included: &mut Vec<(Unit, u64)>| {
        for u in units {
            let c = cost(&u);
            if c <= avail {
                avail -= c;
                included.push((u, c));
            } else {
                dropped.push(u);
            }
        }
    };
    first_fit(optional, &mut included);
    let mut section_units: Vec<Unit> = match sections {
        SectionsMode::None => Vec::new(),
        SectionsMode::Mandatory => included
            .iter()
            .filter(|(u, _)| u.tier.is_required())
            .flat_map(|(u, _)| u.sections())
            .collect(),
        SectionsMode::All => included.iter().flat_map(|(u, _)| u.sections()).collect(),
    };
    for unit in &mut section_units {
        delivery::annotate(
            unit,
            &header.delivery,
            header.snapshot.content_digest.is_some(),
        );
    }
    first_fit(section_units, &mut included);

    let units: Vec<Unit> = included.iter().map(|(u, _)| u.clone()).collect();
    let counts = TierCounts::of(&units);
    let excluded_budget: Vec<String> = dropped.iter().map(|u| u.id.clone()).collect();
    let base = header_cost + included.iter().map(|(_, c)| c).sum::<u64>();
    let used = pack::settle(
        |u| base + footer_cost(&counts, &excluded_budget, u, limit),
        limit,
    );
    let footer = footer_for(&counts, &excluded_budget, used, limit);
    let dropped = dropped
        .iter()
        .map(|u| {
            Excluded::new(
                &u.id,
                ExcludedReason::Budget,
                u.score,
                format!("{} unit does not fit the remaining budget", u.tier.as_str()),
            )
        })
        .collect();
    Ok(Packed {
        units,
        footer,
        dropped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_sorts_keys_recursively() {
        let v = json!({"b": [ {"z": 1, "a": "x"} ], "a": null});
        assert_eq!(canonical_json(&v), r#"{"a":null,"b":[{"a":"x","z":1}]}"#);
    }

    #[test]
    fn host_versions_parse() {
        let (r, v) = parse_host_version("mobile=2.3.0").unwrap();
        assert_eq!((r.as_str(), v.to_string().as_str()), ("mobile", "2.3.0"));
        for bad in ["mobile", "mobile=2.x"] {
            assert_eq!(
                parse_host_version(bad).unwrap_err().code,
                ErrorCode::InvalidInput
            );
        }
    }

    #[test]
    fn completeness_order_and_parse() {
        assert!(Completeness::Complete < Completeness::Partial);
        assert!(Completeness::Conflict < Completeness::Incomplete);
        assert_eq!(
            Completeness::parse("conflict"),
            Some(Completeness::Conflict)
        );
        assert_eq!(Completeness::parse("done"), None);
    }
}
