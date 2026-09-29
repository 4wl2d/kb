//! Trusted configuration: profile config (`project/project.toml` or the maintainer
//! profile), skill settings, host binding and upstream base.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::ids::{MAX_NAMESPACE, NAMESPACE_PATTERN};
use crate::model::record::Intent;

/// Which knowledge profile is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    Project,
    Maintainer,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Project => "project",
            Profile::Maintainer => "maintainer",
        }
    }
}

/// Where a profile lives inside the KB root (all paths KB-root-relative, `/`-separated).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileLocation {
    pub profile: Profile,
    /// Directory of the profile, e.g. `project` or `core/maintainer-knowledge`.
    pub dir: String,
    /// Config file, e.g. `project/project.toml`.
    pub config: String,
}

impl ProfileLocation {
    pub fn for_profile(profile: Profile) -> ProfileLocation {
        match profile {
            Profile::Project => ProfileLocation {
                profile,
                dir: "project".into(),
                config: "project/project.toml".into(),
            },
            Profile::Maintainer => ProfileLocation {
                profile,
                dir: "core/maintainer-knowledge".into(),
                config: "core/maintainer-knowledge/profile.toml".into(),
            },
        }
    }

    pub fn registry_dir(&self) -> String {
        format!("{}/registry", self.dir)
    }
    pub fn routing_tests_dir(&self) -> String {
        format!("{}/routing-tests", self.dir)
    }
    pub fn skill_config_dir(&self) -> String {
        format!("{}/skill-config", self.dir)
    }
    /// KB-root-relative knowledge roots for a config.
    pub fn knowledge_roots(&self, cfg: &ProfileConfig) -> Vec<String> {
        cfg.knowledge
            .roots
            .iter()
            .map(|r| format!("{}/{}", self.dir, r.trim_end_matches('/')))
            .collect()
    }
    /// All KB-root-relative prefixes whose content forms a snapshot of this profile.
    pub fn content_prefixes(&self, cfg: &ProfileConfig) -> Vec<String> {
        let mut v = vec![self.config.clone(), self.registry_dir()];
        v.extend(self.knowledge_roots(cfg));
        v
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    /// Document schema version (must be 1).
    #[schemars(extend("const" = 1))]
    pub schema: u32,
    pub project: ProjectSection,
    pub source: SourceSection,
    #[serde(default)]
    pub knowledge: KnowledgeSection,
    #[serde(default)]
    pub context: ContextSection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSection {
    pub name: String,
    /// First segment of every record id.
    #[schemars(regex(pattern = NAMESPACE_PATTERN), length(max = MAX_NAMESPACE))]
    pub namespace: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceSection {
    /// Git remote name in the KB checkout whose `approved_ref` is the trust boundary.
    #[serde(default = "default_remote")]
    #[schemars(regex(pattern = r"^[^-\s]\S*$"))]
    pub remote: String,
    /// Fully qualified approved ref, e.g. `refs/heads/main`.
    #[schemars(regex(pattern = r"^refs/"), extend("not" = {"pattern": "\\.\\."}))]
    pub approved_ref: String,
    /// Explicit transport policy for fetches (`https`, `ssh`, `git`, `file`, `http`).
    #[serde(default = "default_protocols")]
    #[schemars(inner(regex(pattern = r"^(https|ssh|git|file|http)$")))]
    pub allowed_protocols: Vec<String>,
}

fn default_remote() -> String {
    "origin".into()
}
fn default_protocols() -> Vec<String> {
    vec!["https".into(), "ssh".into()]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeSection {
    /// Knowledge roots relative to the profile directory.
    #[serde(default = "default_roots")]
    #[schemars(length(min = 1))]
    pub roots: Vec<String>,
}

impl Default for KnowledgeSection {
    fn default() -> Self {
        KnowledgeSection {
            roots: default_roots(),
        }
    }
}

fn default_roots() -> Vec<String> {
    vec!["knowledge".into()]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextSection {
    #[serde(default = "default_budget")]
    #[schemars(range(min = 1))]
    pub default_budget: u64,
    #[serde(default)]
    pub default_budget_unit: BudgetUnit,
    #[serde(default = "default_max_supplementary")]
    pub max_supplementary: usize,
}

impl Default for ContextSection {
    fn default() -> Self {
        ContextSection {
            default_budget: default_budget(),
            default_budget_unit: BudgetUnit::default(),
            max_supplementary: default_max_supplementary(),
        }
    }
}

fn default_budget() -> u64 {
    8000
}
fn default_max_supplementary() -> usize {
    12
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum BudgetUnit {
    #[default]
    TokensEst,
    Bytes,
}

impl BudgetUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            BudgetUnit::TokensEst => "tokens-est",
            BudgetUnit::Bytes => "bytes",
        }
    }
}

/// `project/skill-config/skill.toml`: typed settings for generated harness integrations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillConfig {
    /// Document schema version (must be 1).
    #[schemars(extend("const" = 1))]
    pub schema: u32,
    /// Harnesses to integrate: `claude`, `codex`, `cursor`.
    pub harnesses: Vec<Harness>,
    /// Host-relative path of the KB checkout (submodule path), e.g. `.kb`.
    pub kb_path: String,
    /// Default intent suggested in examples.
    #[serde(default = "default_intent")]
    pub default_intent: Intent,
    /// Snapshot selection written as `--snapshot` into every generated kbw command; `latest`
    /// and `pinned` override a host `.kbw.toml` `selection`, `auto` defers to it.
    #[serde(default)]
    pub snapshot: SkillSnapshot,
    /// Extra project notes appended to the skill (plain Markdown, non-normative).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

fn default_intent() -> Intent {
    Intent::Implement
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SkillSnapshot {
    #[default]
    Auto,
    Latest,
    Pinned,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    Claude,
    Codex,
    Cursor,
}

impl Harness {
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Cursor => "cursor",
        }
    }
}

/// Optional host binding file `.kbw.toml` in the host repository root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostBinding {
    /// Document schema version (must be 1).
    #[schemars(extend("const" = 1))]
    pub schema: u32,
    /// Registry repo id of this host (otherwise matched by remote URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Explicit KB revision pin for non-submodule setups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    /// Explicit default selection for `--snapshot auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SkillSnapshot>,
}

pub const HOST_BINDING_FILE: &str = ".kbw.toml";

/// `project/upstream.toml`: the upstream base this downstream was last updated from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpstreamConfig {
    /// Document schema version (must be 1).
    #[schemars(extend("const" = 1))]
    pub schema: u32,
    /// Upstream repository URL (template parameter; never a fabricated default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Upstream commit the engine paths were last taken from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Human-readable ref (tag) of that revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    /// Intentional local engine patches (paths) exempt from divergence errors.
    #[serde(default)]
    pub engine_patches: Vec<EnginePatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnginePatch {
    pub path: String,
    pub reason: String,
}

pub const UPSTREAM_FILE: &str = "project/upstream.toml";
