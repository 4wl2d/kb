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
    /// Verbose text for people
    Human,
    /// Versioned JSON protocol (kb.cli.v1)
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum IntentArg {
    Implement,
    Refactor,
    Debug,
    Review,
    Explain,
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
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create the project structure from templates (dry-run unless --apply)
    Init(InitArgs),
    /// Check environment, configuration, runtime, Git binding and index
    Doctor(DoctorArgs),
    /// Validate records, references, policies, schemas and routing fixtures
    Validate(ValidateArgs),
    /// Build or update the derived index
    Index(IndexArgs),
    /// Assemble task-scoped context
    Context(ContextArgs),
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

#[derive(Debug, Clone, Args)]
pub struct ContextArgs {
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
    /// Base revision in the host repository (diff uses the merge-base with head)
    #[arg(long, value_name = "REV")]
    pub base: Option<String>,
    /// Head revision (default HEAD)
    #[arg(long, value_name = "REV")]
    pub head: Option<String>,
    /// Include staged, unstaged and untracked local changes
    #[arg(long)]
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
