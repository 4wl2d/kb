//! Harness integrations: KB-level skill bundle generation and host-level installation with
//! managed instruction blocks. See docs/architecture.md §10.
//!
//! * [`template`]: strict `{{key}}` rendering shared with `kb init`.
//! * [`generate`]: renders `core/skills` + `project/skill-config/skill.toml` into the committed
//!   bundle `project/skill-config/generated/` (`kb integrate --generate`).
//! * [`blocks`]: named managed blocks in host Markdown files (`AGENTS.md`, `CLAUDE.md`).
//! * [`install`]: installs the committed bundle into a host repository and tracks installed
//!   hashes in `.kbw/integration.lock` (`kb integrate`).
//!
//! Text instructions do not guarantee that an agent follows them; kb never relies on
//! undocumented harness hooks.

pub mod blocks;
pub mod core;
pub mod generate;
pub mod install;
pub mod template;

use std::collections::BTreeMap;

use serde::Serialize;

pub use generate::{Bundle, BundleManifest, GeneratePlan, parse_skill_config, render_bundle};
pub use install::{
    InstallPlan, IntegrationLock, SkillState, SkillStatus, plan_install, resolve_host_root,
    skill_status,
};

/// What a planned file operation does. Shared by `init`, `integrate --generate` and host
/// installation so that plans read the same everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// The file (or block) does not exist and will be created.
    Create,
    /// Existing managed content will be rewritten.
    Update,
    /// An existing unmanaged file is intentionally replaced (only `project/README.md` at init).
    Replace,
    /// Existing content is kept as is and never overwritten.
    Keep,
    /// Content already matches.
    Unchanged,
    /// Stale managed content will be removed.
    Remove,
    /// Content was modified outside kb; applying requires `--force`.
    Conflict,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Create => "create",
            Action::Update => "update",
            Action::Replace => "replace",
            Action::Keep => "keep",
            Action::Unchanged => "unchanged",
            Action::Remove => "remove",
            Action::Conflict => "conflict",
        }
    }

    /// Does applying this action change bytes on disk?
    pub fn writes(self) -> bool {
        matches!(
            self,
            Action::Create | Action::Update | Action::Replace | Action::Remove
        )
    }
}

/// One planned change to a file or to a managed block inside a file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Change {
    /// Path relative to the root the plan applies to (KB root or host root).
    pub path: String,
    /// Managed block name when the change targets a block inside `path`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
    pub action: Action,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Change {
    pub fn file(path: impl Into<String>, action: Action) -> Change {
        Change {
            path: path.into(),
            block: None,
            action,
            reason: None,
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Change {
        self.reason = Some(reason.into());
        self
    }

    /// One-line rendering for text output, e.g. `update    AGENTS.md [block kb-instructions]`.
    pub fn line(&self) -> String {
        let mut s = format!("{:<9} {}", self.action.as_str(), self.path);
        if let Some(b) = &self.block {
            s.push_str(&format!(" [block {b}]"));
        }
        if let Some(r) = &self.reason {
            s.push_str(&format!(": {r}"));
        }
        s
    }
}

/// Remove now-empty parent directories of `file`, stopping at `stop` (exclusive). Only empty
/// directories are removed (`remove_dir` fails on non-empty ones).
pub(crate) fn prune_empty_dirs(stop: &std::path::Path, file: &std::path::Path) {
    let mut cur = file.parent();
    while let Some(dir) = cur {
        if dir == stop || !dir.starts_with(stop) || std::fs::remove_dir(dir).is_err() {
            break;
        }
        cur = dir.parent();
    }
}

/// Count actions for a plan summary (deterministic key order).
pub fn summarize<I: IntoIterator<Item = Action>>(actions: I) -> BTreeMap<&'static str, usize> {
    let mut m = BTreeMap::new();
    for a in actions {
        *m.entry(a.as_str()).or_insert(0) += 1;
    }
    m
}

/// How a mutating command was invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    DryRun,
    Check,
    Apply,
}

impl Mode {
    pub fn from_flags(check: bool, apply: bool) -> Mode {
        if apply {
            Mode::Apply
        } else if check {
            Mode::Check
        } else {
            Mode::DryRun
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::DryRun => "dry-run",
            Mode::Check => "check",
            Mode::Apply => "apply",
        }
    }
}
