//! Command-line interface definition. Option names are part of the documented contract.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "kb",
    version,
    about = "Typed, verifiable engineering knowledge base for coding agents and people",
    long_about = "Assembles verifiable, task-scoped context from a typed knowledge base.\n\
                  Run through the project launcher `kbw`; see README.md.",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalOpts,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Args)]
pub struct GlobalOpts {
    /// KB root (defaults to $KB_ROOT set by kbw, else the nearest ancestor with core/release.toml)
    #[arg(long, global = true, value_name = "DIR")]
    pub root: Option<PathBuf>,
    /// Profile config file override (KB-root-relative)
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<String>,
    /// Knowledge profile
    #[arg(long, global = true, value_enum, default_value_t = ProfileArg::Project)]
    pub profile: ProfileArg,
    /// Output format
    #[arg(long, global = true, value_enum)]
    pub format: Option<FormatArg>,
    /// Shorthand for --format json
    #[arg(long, global = true, conflicts_with = "format")]
    pub json: bool,
    /// Skip the remote freshness check (result is marked freshness=unverified)
    #[arg(long, global = true)]
    pub offline: bool,
    /// Snapshot selection: auto, latest, pinned, working-tree, or a revision
    #[arg(long, global = true, value_name = "SELECTION")]
    pub snapshot: Option<String>,
    /// Host repository root (defaults to detection from the current directory)
    #[arg(long, global = true, value_name = "DIR")]
    pub host: Option<PathBuf>,
    /// Suppress progress output on stderr
    #[arg(long, short, global = true)]
    pub quiet: bool,
    /// Skill protocol the caller was written for; a mismatch fails with SKILL_OUTDATED
    #[arg(long, global = true, value_name = "N")]
    pub skill_protocol: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProfileArg {
    Project,
    Maintainer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    /// Compact text for agents (default)
    Compact,
    /// Minimal tiered context; preserves every typed obligation and defers Markdown bodies
    Terse,
    /// Verbose text for people
    Human,
    /// Versioned JSON protocol (kb.cli.v1)
    Json,
}

impl From<FormatArg> for crate::output::Format {
    fn from(value: FormatArg) -> Self {
        match value {
            FormatArg::Compact => Self::Compact,
            FormatArg::Terse => Self::Terse,
            FormatArg::Human => Self::Human,
            FormatArg::Json => Self::Json,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum IntentArg {
    Implement,
    Refactor,
    Debug,
    Review,
    Explain,
    Diagnose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BudgetUnitArg {
    TokensEst,
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SectionsArg {
    None,
    Mandatory,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HarnessArg {
    Claude,
    Codex,
    Cursor,
    Grok,
    Copilot,
    Junie,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create the project structure from templates (dry-run unless --apply)
    Init(InitArgs),
    /// Check environment, configuration, runtime, Git binding and index
    Doctor(DoctorArgs),
    /// Validate records, references, policies, schemas and routing fixtures
    Validate(ValidateArgs),
    /// Deterministic routing and retrospective change-scope evaluation
    #[command(subcommand)]
    Eval(EvalCommand),
    /// Build or update the derived index
    Index(IndexArgs),
    /// Assemble task-scoped context
    Context(ContextArgs),
    /// Inventory of context ids, reasons and token costs, capped at 2000 estimated tokens
    Outline(ContextArgs),
    /// Rank host modules needing domain knowledge using tracked files and Git history
    Coverage(CoverageArgs),
    /// Create a change work order or validate/submit a knowledge draft
    #[command(subcommand)]
    Propose(ProposeCommand),
    /// Capture a reported decision, gap, quirk or scenario as a draft
    Capture(CaptureArgs),
    /// Stamp or check Git-backed provenance anchors
    #[command(subcommand)]
    Anchors(AnchorsCommand),
    /// Queue changed or unverifiable anchored knowledge by owner
    Drift(DriftArgs),
    /// Report support for normative statements and sample drafts for human audit
    Ledger(LedgerArgs),
    /// Evaluate declared commit/branch/import/naming/API probes against a host diff
    Verify(VerifyArgs),
    /// Inspect private local context-delivery observations
    #[command(subcommand)]
    Usage(UsageCommand),
    /// Search records (not a substitute for `context`)
    Search(SearchArgs),
    /// Show a record or one of its sections
    Show(ShowArgs),
    /// Fetch the approved ref into the isolated cache (never touches working files)
    Sync(SyncArgs),
    /// Relate a code diff to affected knowledge and coverage gaps
    Impact(ImpactArgs),
    /// Generate or check harness integrations (dry-run unless --apply)
    Integrate(IntegrateArgs),
    /// Show or apply supported schema migrations (dry-run unless --apply)
    Migrate(MigrateArgs),
    /// Check or prepare an explicit upstream engine update
    #[command(subcommand)]
    Update(UpdateCommand),
    /// Export or check the generated JSON Schemas
    Schema(SchemaArgs),
    /// Print engine and contract versions
    Version,
}

#[derive(Debug, Clone, Args)]
pub struct InitArgs {
    /// Write files (default is a dry-run plan)
    #[arg(long)]
    pub apply: bool,
    /// Project display name
    #[arg(long)]
    pub name: Option<String>,
    /// Record id namespace (lowercase kebab-case)
    #[arg(long)]
    pub namespace: Option<String>,
    /// Git remote name of the approved source
    #[arg(long, default_value = "origin")]
    pub remote: String,
    /// Approved ref (trust boundary)
    #[arg(long, default_value = "refs/heads/main")]
    pub approved_ref: String,
    /// Host-relative path where host repositories mount this KB
    #[arg(long, default_value = ".kb")]
    pub kb_path: String,
    /// Harnesses to integrate (repeatable)
    #[arg(long = "harness", value_enum)]
    pub harnesses: Vec<HarnessArg>,
    /// Upstream repository URL recorded in project/upstream.toml
    #[arg(long)]
    pub upstream_url: Option<String>,
    /// Materialize a synthetic example instead of an empty project (demo/testing only)
    #[arg(long, value_name = "NAME")]
    pub example: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct DoctorArgs {
    /// Also check the remote approved ref (network)
    #[arg(long)]
    pub online: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ValidateArgs {
    /// Warn when accepted verification is older than this many days at --on (or HEAD date)
    #[arg(long)]
    pub stale: Option<u32>,
    /// Calendar reference YYYY-MM-DD; defaults to the host/KB commit date, never the clock
    #[arg(long)]
    pub on: Option<String>,
    /// Skip routing fixtures
    #[arg(long)]
    pub no_routing: bool,
    /// Compare against a base revision (historical ids must stay addressable)
    #[arg(long, value_name = "REV")]
    pub base: Option<String>,
    /// Validate shipped templates and examples (upstream check)
    #[arg(long)]
    pub templates: bool,
    /// Treat warnings as errors
    #[arg(long)]
    pub strict: bool,
}

#[derive(Debug, Clone, Args)]
pub struct IndexArgs {
    /// Drop the cache database and rebuild
    #[arg(long)]
    pub rebuild: bool,
    /// Remove old snapshots from the cache
    #[arg(long)]
    pub gc: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum EvalCommand {
    /// Run routing fixtures with recall, applicability and response-size metrics
    Routing(EvalRoutingArgs),
    /// Evaluate first-parent host changes, including squashed merge commits
    History(EvalHistoryArgs),
}

#[derive(Debug, Clone, Args)]
pub struct EvalRoutingArgs {
    #[arg(long)]
    pub strict: bool,
    /// Evaluate a shipped synthetic example instead of the selected profile
    #[arg(long)]
    pub example: Option<String>,
    /// Renderer measured inside each context case; independent of the report's --format
    #[arg(long, value_enum, default_value = "compact")]
    pub context_format: FormatArg,
}

#[derive(Debug, Clone, Args)]
pub struct EvalHistoryArgs {
    /// Inclusive end, exclusive start: BASE..HEAD (a linear first-parent history)
    #[arg(long)]
    pub range: String,
    #[command(flatten)]
    pub hosts: HostRootsArgs,
    /// Optional kb.history-labels.v1 JSON export of merged MR descriptions by commit
    #[arg(long)]
    pub labels: Option<String>,
    /// Filter records at each change's first parent; requires historical validity metadata
    #[arg(long)]
    pub as_of: bool,
    /// Reject longer ranges instead of silently sampling them
    #[arg(long, default_value_t = 200)]
    pub max_changes: usize,
    /// Fail when context or historical inputs are incomplete
    #[arg(long)]
    pub check: bool,
}

#[derive(Debug, Clone, Default, Args)]
pub struct HostRootsArgs {
    /// Registry identity of --host (otherwise detected from the host binding/remotes)
    #[arg(long)]
    pub repo: Option<String>,
    /// Additional host repository checkout, REPO=DIR (repeatable)
    #[arg(long = "repo-root")]
    pub repo_roots: Vec<String>,
}

#[derive(Debug, Clone, Default, Args)]
pub struct KnowledgeScopeArgs {
    #[command(flatten)]
    pub hosts: HostRootsArgs,
    /// Record ids (default accepted records; repeatable)
    #[arg(long = "id")]
    pub ids: Vec<String>,
    #[arg(long)]
    pub include_drafts: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum AnchorsCommand {
    /// Plan source stamps for explicit records; preserve content, status and comments
    Stamp(AnchorStampArgs),
    /// Check declared anchors against immutable Git objects
    Check(AnchorCheckArgs),
}

#[derive(Debug, Clone, Args)]
pub struct AnchorStampArgs {
    #[command(flatten)]
    pub hosts: HostRootsArgs,
    #[command(flatten)]
    pub code: CodeProviderArgs,
    #[arg(long = "id", required = true)]
    pub ids: Vec<String>,
    /// Host revision; --host remains the checkout directory
    #[arg(long, default_value = "HEAD")]
    pub at: String,
    /// Explicitly reported human review date; never inferred from byte checks
    #[arg(long)]
    pub verified_at: Option<String>,
    #[arg(long)]
    pub review_by: Option<String>,
    #[arg(long)]
    pub apply: bool,
}

#[derive(Debug, Clone, Args)]
pub struct AnchorCheckArgs {
    #[command(flatten)]
    pub scope: KnowledgeScopeArgs,
    #[arg(long, default_value = "HEAD")]
    pub at: String,
    /// Also require anchors on every selected record and stamps on every path anchor
    #[arg(long)]
    pub strict: bool,
}

#[derive(Debug, Clone, Args)]
pub struct DriftArgs {
    #[command(flatten)]
    pub scope: KnowledgeScopeArgs,
    /// Baseline host revision/date, or each record's own verified evidence
    #[arg(long, default_value = "verified")]
    pub since: String,
    #[arg(long, default_value = "HEAD")]
    pub at: String,
    #[arg(long)]
    pub check: bool,
}

#[derive(Debug, Clone, Args)]
pub struct LedgerArgs {
    #[command(flatten)]
    pub scope: KnowledgeScopeArgs,
    /// Host revision to inspect; --snapshot independently selects the KB revision
    #[arg(long, default_value = "HEAD")]
    pub at: String,
    #[arg(long)]
    pub on: Option<String>,
    #[arg(long)]
    pub stale: Option<u32>,
    /// Number of draft records to select reproducibly for a human audit
    #[arg(long, default_value_t = 10)]
    pub sample: usize,
    #[arg(long, default_value = "0")]
    pub seed: String,
    /// Fail when accepted statements are stale or unverifiable
    #[arg(long)]
    pub check: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ProbeKindArg {
    CommitMessage,
    BranchName,
    ForbiddenImport,
    Naming,
    BannedApi,
}

impl ProbeKindArg {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommitMessage => "commit-message",
            Self::BranchName => "branch-name",
            Self::ForbiddenImport => "forbidden-import",
            Self::Naming => "naming",
            Self::BannedApi => "banned-api",
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct VerifyArgs {
    #[arg(long, default_value = "HEAD")]
    pub diff: String,
    /// Verify a committed target (required for import graphs); default is the work tree
    #[arg(long, conflicts_with = "staged")]
    pub head: Option<String>,
    /// Verify the staged index, not unstaged edits (commit-msg hook)
    #[arg(long)]
    pub staged: bool,
    #[arg(long)]
    pub repo: Option<String>,
    #[arg(long = "id")]
    pub ids: Vec<String>,
    /// Pending message file supplied by a commit-msg hook; treated only as text
    #[arg(long)]
    pub commit_message: Option<PathBuf>,
    /// Actual source branch supplied by CI when checking a detached commit
    #[arg(long)]
    pub branch: Option<String>,
    #[arg(long, value_enum)]
    pub only: Vec<ProbeKindArg>,
    /// Assert this record#statement's conditions hold and no exception applies
    #[arg(long)]
    pub applicable: Vec<String>,
    #[arg(long = "change-type")]
    pub change_types: Vec<String>,
    #[arg(long = "host-version")]
    pub host_versions: Vec<String>,
    /// Make advisory-level probe failures and unavailable evidence blocking too
    #[arg(long)]
    pub strict: bool,
    #[command(flatten)]
    pub code: CodeProviderArgs,
}

#[derive(Debug, Clone, Subcommand)]
pub enum UsageCommand {
    Report(UsageReportArgs),
}

#[derive(Debug, Clone, Args)]
pub struct UsageReportArgs {
    #[arg(long, default_value = "HEAD")]
    pub diff: String,
    /// Committed final target; otherwise include current work-tree changes
    #[arg(long)]
    pub head: Option<String>,
    #[arg(long)]
    pub repo: Option<String>,
    /// Restrict to these receipts; default is all local calls for this KB/host/profile
    #[arg(long = "receipt")]
    pub receipts: Vec<String>,
    #[arg(long = "change-type")]
    pub change_types: Vec<String>,
    #[arg(long = "host-version")]
    pub host_versions: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct CodeProviderArgs {
    /// One-shot kb.code.v1 provider program (never invoked through a shell)
    #[arg(long, conflicts_with = "provider_files")]
    pub provider: Option<PathBuf>,
    #[arg(
        long = "provider-arg",
        requires = "provider",
        allow_hyphen_values = true
    )]
    pub provider_args: Vec<String>,
    /// Pinned provider JSON response (repeat for base/head commits)
    #[arg(long = "provider-file", conflicts_with = "provider")]
    pub provider_files: Vec<PathBuf>,
    #[arg(long, default_value_t = 30)]
    pub provider_timeout: u64,
}

#[derive(Debug, Clone, Args)]
pub struct CoverageArgs {
    #[command(flatten)]
    pub code: CodeProviderArgs,
    #[command(flatten)]
    pub hosts: HostRootsArgs,
    /// History window relative to the host HEAD date, or an ISO start date
    #[arg(long, default_value = "180d")]
    pub since: String,
    /// Maximum ranked modules to return
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ProposeCommand {
    /// Emit a work order from a Git revision range or an exported MR JSON file
    Begin(ProposeBeginArgs),
    /// Validate a draft and preview its proposal-overlay write (requires --apply to write)
    Submit(ProposeSubmitArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ProposeBeginArgs {
    #[command(flatten)]
    pub code: CodeProviderArgs,
    /// BASE..HEAD or a kb.change.v1 JSON export path
    #[arg(long, value_name = "RANGE|JSON")]
    pub from_change: String,
    #[command(flatten)]
    pub hosts: HostRootsArgs,
}

#[derive(Debug, Clone, Args)]
pub struct ProposeSubmitArgs {
    #[command(flatten)]
    pub code: CodeProviderArgs,
    /// Add provider-derived consumer candidates to a contract draft (review still required)
    #[arg(long)]
    pub fill_consumers: bool,
    pub file: PathBuf,
    #[command(flatten)]
    pub hosts: HostRootsArgs,
    /// Write the validated draft; accepted status is always rejected
    #[arg(long)]
    pub apply: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CaptureKind {
    Decision,
    Gap,
    Quirk,
    Scenario,
}

#[derive(Debug, Clone, Args)]
pub struct CaptureArgs {
    #[arg(value_enum)]
    pub kind: CaptureKind,
    #[arg(long)]
    pub title: String,
    /// Caller-supplied observation or decision (not inferred by the engine)
    #[arg(long)]
    pub text: String,
    /// Stable record id (default: deterministic id from the capture content)
    #[arg(long)]
    pub id: Option<String>,
    /// Registry owner (can be omitted only with a single registered owner)
    #[arg(long)]
    pub owner: Option<String>,
    #[command(flatten)]
    pub hosts: HostRootsArgs,
    #[arg(long = "module")]
    pub modules: Vec<String>,
    #[arg(long = "feature")]
    pub features: Vec<String>,
    #[arg(long = "change-type")]
    pub change_types: Vec<String>,
    /// Explicit product-wide scope
    #[arg(long)]
    pub product: bool,
    /// Source provenance, REPO:PATH[@REV][#SYMBOL] (repeatable; default REV is HEAD)
    #[arg(long = "anchor")]
    pub anchors: Vec<String>,
    /// Test-source provenance, using the same syntax as --anchor
    #[arg(long = "test-anchor")]
    pub test_anchors: Vec<String>,
    /// Command reported by the caller; recorded verbatim and never executed
    #[arg(long = "test")]
    pub tests: Vec<String>,
    /// Why the decision was chosen (repeatable; required for decisions)
    #[arg(long = "reason")]
    pub reasons: Vec<String>,
    /// Prior situation for a decision, or scenario preconditions
    #[arg(long)]
    pub given: Option<String>,
    /// Expected scenario outcome (required for scenarios)
    #[arg(long)]
    pub expect: Option<String>,
    /// Explicit validity evidence, date or host commit
    #[arg(long)]
    pub introduced: Option<String>,
    #[arg(long)]
    pub apply: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ContextArgs {
    /// Warn when accepted verification is older than this many days at --on (or host HEAD date)
    #[arg(long)]
    pub stale: Option<u32>,
    /// Explicit calendar reference YYYY-MM-DD for review deadlines and age checks
    #[arg(long)]
    pub on: Option<String>,
    /// Reuse unchanged units from this explicitly supplied receipt in the same session
    #[arg(long, value_name = "ID")]
    pub since_receipt: Option<String>,
    /// A loaded managed core block's receipt; verified against this snapshot and host
    #[arg(long, value_name = "ID", requires = "core_source")]
    pub core_receipt: Option<String>,
    /// Host-relative instruction file from which the core receipt was loaded
    #[arg(long, value_name = "FILE", requires = "core_receipt")]
    pub core_source: Option<String>,
    #[command(flatten)]
    pub code: CodeProviderArgs,
    /// Add optional, budgeted code evidence from an explicit provider
    #[arg(long)]
    pub with_code: bool,
    /// Task intent
    #[arg(long, value_enum)]
    pub intent: IntentArg,
    /// Task description (natural language, any script)
    #[arg(long)]
    pub task: Option<String>,
    /// Registry repo id (repeatable; default: detected host repo)
    #[arg(long = "repo")]
    pub repos: Vec<String>,
    /// Host-relative path, or `repo:path` (repeatable)
    #[arg(long = "path")]
    pub paths: Vec<String>,
    /// Registry module id (repeatable)
    #[arg(long = "module")]
    pub modules: Vec<String>,
    /// Registry feature id (repeatable)
    #[arg(long = "feature")]
    pub features: Vec<String>,
    /// Registry concept id (repeatable)
    #[arg(long = "concept")]
    pub concepts: Vec<String>,
    /// Explicit change category (repeatable); unknown categories never silently prune rules
    #[arg(long = "change-type")]
    pub change_types: Vec<String>,
    /// Scope context to a host diff (default without --base: local changes since HEAD)
    #[arg(long)]
    pub changed: bool,
    /// Diff base; uses the merge-base with the selected head
    #[arg(long, requires = "changed", value_name = "REV")]
    pub base: Option<String>,
    /// Diff head (default HEAD)
    #[arg(
        long,
        requires = "changed",
        conflicts_with = "working_tree",
        value_name = "REV"
    )]
    pub head: Option<String>,
    /// Include staged, unstaged and untracked files in --changed
    #[arg(long, requires = "changed")]
    pub working_tree: bool,
    /// Withhold records not valid at this ISO date or host revision
    #[arg(long, value_name = "REV|DATE", conflicts_with = "include_proposals")]
    pub as_of: Option<String>,
    /// Numeric budget
    #[arg(long)]
    pub budget: Option<u64>,
    /// Budget unit
    #[arg(long, value_enum)]
    pub budget_unit: Option<BudgetUnitArg>,
    /// Include local proposals (committed-unmerged, staged, unstaged, untracked) as a labeled overlay
    #[arg(long)]
    pub include_proposals: bool,
    /// Show the explain receipt (reasons, excluded candidates, anchors)
    #[arg(long)]
    pub explain: bool,
    /// Include optional Markdown sections
    #[arg(long, value_enum, default_value_t = SectionsArg::None)]
    pub sections: SectionsArg,
    /// Maximum number of supplementary records
    #[arg(long)]
    pub max_supplementary: Option<usize>,
    /// Host code version `repo=x.y.z` (repeatable)
    #[arg(long = "host-version", value_name = "REPO=VERSION")]
    pub host_versions: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct SearchArgs {
    /// Query text
    pub query: String,
    /// Restrict to kinds (repeatable)
    #[arg(long = "kind")]
    pub kinds: Vec<String>,
    /// Maximum results
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// Include proposals
    #[arg(long)]
    pub include_proposals: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ShowArgs {
    /// Show all deferred Markdown sections without repeating the typed record
    #[arg(long, conflicts_with_all = ["section", "raw"])]
    pub sections: bool,
    /// Record id, optionally `id#section`
    pub id: String,
    /// Section id
    #[arg(long)]
    pub section: Option<String>,
    /// Print the authoritative file bytes
    #[arg(long)]
    pub raw: bool,
    /// Look up proposals too
    #[arg(long)]
    pub include_proposals: bool,
}

#[derive(Debug, Clone, Args)]
pub struct SyncArgs {}

#[derive(Debug, Clone, Args)]
pub struct ImpactArgs {
    #[command(flatten)]
    pub code: CodeProviderArgs,
    #[arg(long)]
    pub deep: bool,
    /// Incoming code-reference depth for --deep
    #[arg(long, default_value_t = 2)]
    pub depth: u32,
    /// Explicit registry identity of the host
    #[arg(long)]
    pub repo: Option<String>,
    /// Base revision in the host repository (diff uses the merge-base with head)
    #[arg(long, value_name = "REV")]
    pub base: Option<String>,
    /// Head revision (default HEAD)
    #[arg(long, value_name = "REV")]
    pub head: Option<String>,
    /// Include staged, unstaged and untracked local changes
    #[arg(long, alias = "changed")]
    pub working_tree: bool,
    /// File containing the MR description with a `kb-impact` block
    #[arg(long, value_name = "FILE")]
    pub statement: Option<PathBuf>,
    /// Fail when affected knowledge or unknown coverage is not acknowledged
    #[arg(long)]
    pub check: bool,
}

#[derive(Debug, Clone, Args)]
pub struct IntegrateArgs {
    /// Verify installed files and emit a challenge for a separate real harness load probe
    #[arg(long, conflicts_with_all = ["generate", "apply", "check", "force"])]
    pub probe: bool,
    /// Render the KB-level skill bundle into project/skill-config/generated
    #[arg(long)]
    pub generate: bool,
    /// Exit non-zero on drift
    #[arg(long, conflicts_with = "apply")]
    pub check: bool,
    /// Write changes
    #[arg(long)]
    pub apply: bool,
    /// Overwrite user-modified generated files (explicit)
    #[arg(long, requires = "apply")]
    pub force: bool,
}

#[derive(Debug, Clone, Args)]
pub struct MigrateArgs {
    /// Write changes
    #[arg(long)]
    pub apply: bool,
    /// Target document schema version (default: current)
    #[arg(long)]
    pub to: Option<u32>,
}

#[derive(Debug, Subcommand)]
pub enum UpdateCommand {
    /// Check compatibility of an upstream ref with this downstream
    Check(UpdateSourceArgs),
    /// Prepare a reviewable update branch in an isolated worktree
    Prepare(UpdatePrepareArgs),
    /// Report engine-owned paths that diverge from the recorded upstream base
    Divergence,
    /// Remove an update worktree and branch created by `update prepare` (dry-run unless --apply)
    Abandon(UpdateAbandonArgs),
}

#[derive(Debug, Clone, Args)]
pub struct UpdateSourceArgs {
    /// Upstream repository URL or local path
    #[arg(long)]
    pub upstream: String,
    /// Upstream tag or commit
    #[arg(long = "ref")]
    pub reference: String,
}

#[derive(Debug, Clone, Args)]
pub struct UpdatePrepareArgs {
    #[command(flatten)]
    pub source: UpdateSourceArgs,
    /// Branch name for the prepared update; must start with `kb-update/` (default kb-update/<ref>)
    #[arg(long)]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct UpdateAbandonArgs {
    /// Branch created by `update prepare`
    pub branch: String,
    /// Remove the worktree and delete the branch (default: dry-run)
    #[arg(long)]
    pub apply: bool,
    /// Also discard uncommitted, untracked or conflicted files in the worktree (explicit)
    #[arg(long, requires = "apply")]
    pub force: bool,
}

#[derive(Debug, Clone, Args)]
pub struct SchemaArgs {
    /// Fail if core/schemas differs from the generated schemas
    #[arg(long, conflicts_with = "write")]
    pub check: bool,
    /// Write generated schemas to core/schemas
    #[arg(long)]
    pub write: bool,
}
