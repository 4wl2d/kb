use crate::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub protocol: String,
    pub id: String,
    /// Local repository. Never mounted into an agent sandbox.
    pub repository: PathBuf,
    pub base: String,
    pub golden: String,
    pub as_of: String,
    /// Keep only the base, or also its past ancestry for commit-bound temporal evidence.
    #[serde(default)]
    pub history: History,
    pub prompt: String,
    pub hidden: PathBuf,
    pub hidden_sha256: String,
    pub build: Vec<Program>,
    pub regression: Vec<Program>,
    pub test: Vec<Program>,
    pub quality: Vec<Criterion>,
    pub rules: Vec<Criterion>,
    #[serde(default)]
    pub judge_context: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum History {
    #[default]
    BaseOnly,
    Ancestors,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub id: String,
    pub text: String,
    pub weight: f64,
    #[serde(default)]
    pub metric: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub timeout_seconds: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    Codex,
    Claude,
    Cursor,
    Grok,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    pub family: Family,
    pub program: PathBuf,
    pub binary_sha256: String,
    pub version: String,
    pub model: String,
    pub effort: Option<String>,
    pub timeout_seconds: u64,
    /// Environment variable names, never secret values. Only these secrets are inherited.
    #[serde(default)]
    pub secret_env: Vec<String>,
    pub usage_basis: crate::accounting::Basis,
    pub input_convention: crate::accounting::Convention,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Overlay {
    pub repository: PathBuf,
    pub commit: String,
    /// A new relative directory in the task checkout. Existing paths are never replaced.
    pub target: String,
    /// Evidence cutoff declared when the knowledge was generated, not inferred from mtime.
    pub as_of: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Arm {
    pub id: String,
    pub instructions: String,
    #[serde(default)]
    pub overlays: Vec<Overlay>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub id: String,
    pub agent: Agent,
    /// Each family judges every trial independently. Analysis never pools judges.
    pub judges: Vec<Agent>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRef {
    pub id: String,
    pub file: PathBuf,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Isolation {
    /// Explicit immutable toolchains/dependency caches. No real HOME or study inputs.
    pub read_roots: Vec<PathBuf>,
    /// Nonsecret, reproducible build environment. Reserved launcher variables are rejected.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Exact model API DNS names. Empty disables all networking. No web/tool endpoints.
    #[serde(default)]
    pub api_hosts: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Higher,
    Lower,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Metric {
    pub direction: Direction,
    pub sesoi: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Comparison {
    pub id: String,
    pub baseline: String,
    pub candidate: String,
    pub metric: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Study {
    pub protocol: String,
    pub id: String,
    pub tasks: Vec<TaskRef>,
    pub arms: Vec<Arm>,
    pub blocks: Vec<Block>,
    pub repetitions: u32,
    pub seed: u64,
    pub draws: usize,
    pub alpha: f64,
    pub minimum_tasks: usize,
    pub metrics: BTreeMap<String, Metric>,
    pub comparisons: Vec<Comparison>,
    pub isolation: Isolation,
    /// Human-readable stopping, missingness, exclusions and decision rules fixed beforehand.
    pub decision_rules: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub protocol: String,
    pub sha256: String,
    pub study: Study,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub study_sha256: String,
    pub task: String,
    pub arm: String,
    pub block: String,
    pub repetition: u32,
    /// Explicit family/version/model identity, or "objective" for executable outcomes.
    pub judge: String,
    pub metrics: BTreeMap<String, f64>,
    #[serde(default)]
    pub deviations: Vec<String>,
    pub evidence_sha256: String,
}

pub fn id(value: &str) -> Result<()> {
    ensure(
        !value.is_empty()
            && value != "."
            && value != ".."
            && value.len() <= 100
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)),
        "invalid evaluation identifier",
    )
}
pub fn hash(value: &str) -> Result<()> {
    ensure(
        value.len() == 64
            && value
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "expected lowercase SHA-256",
    )
}
pub fn commit(value: &str) -> Result<()> {
    ensure(
        matches!(value.len(), 40 | 64)
            && value
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "expected full lowercase commit id",
    )
}
fn unique<'a>(values: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        id(value)?;
        ensure(seen.insert(value), format!("duplicate id {value}"))?;
    }
    Ok(())
}
impl Task {
    pub fn validate(&self) -> Result<()> {
        ensure(self.protocol == "kb.eval.task.v1", "unknown task protocol")?;
        id(&self.id)?;
        commit(&self.base)?;
        commit(&self.golden)?;
        ensure(
            self.as_of == self.base,
            "task as_of must be its exact historical base commit",
        )?;
        hash(&self.hidden_sha256)?;
        ensure(
            !self.prompt.trim().is_empty() && self.prompt.len() <= 100_000,
            "empty or oversized task prompt",
        )?;
        ensure(
            !self.test.is_empty() && !self.quality.is_empty(),
            "hidden test commands and quality checklist are required",
        )?;
        unique(
            self.quality
                .iter()
                .chain(&self.rules)
                .map(|c| c.id.as_str()),
        )?;
        for c in self.quality.iter().chain(&self.rules) {
            ensure(
                !c.text.trim().is_empty() && c.weight.is_finite() && c.weight > 0.0,
                "invalid checklist item",
            )?;
            if let Some(metric) = &c.metric {
                id(metric)?;
                ensure(
                    metric != "all" && metric != "Q" && metric != "rule_compliance",
                    "reserved criterion metric",
                )?;
            }
        }
        for path in &self.judge_context {
            kb::util::check_rel_path(path).map_err(|e| e.to_string())?;
            ensure(
                !path.split('/').any(|s| s == ".git"),
                "judge source context cannot expose Git metadata",
            )?;
        }
        for p in self.build.iter().chain(&self.regression).chain(&self.test) {
            p.validate()?;
        }
        Ok(())
    }
}
impl Program {
    pub fn validate(&self) -> Result<()> {
        ensure(
            !self.program.as_os_str().is_empty()
                && self.timeout_seconds > 0
                && self.timeout_seconds <= 86_400,
            "invalid command or deadline",
        )
    }
}
impl Agent {
    pub fn validate(&self) -> Result<()> {
        hash(&self.binary_sha256)?;
        ensure(
            self.program.is_absolute()
                && !self.version.is_empty()
                && !self.model.is_empty()
                && self.timeout_seconds > 0
                && self.timeout_seconds <= 86_400,
            "agent requires absolute executable, exact version/model and bounded deadline",
        )?;
        let allowed = [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "CURSOR_API_KEY",
            "XAI_API_KEY",
            "GROK_API_KEY",
        ];
        for name in &self.secret_env {
            ensure(
                allowed.contains(&name.as_str()),
                "unsupported credential environment variable",
            )?;
        }
        Ok(())
    }
}
impl Study {
    pub fn validate(&self) -> Result<()> {
        ensure(
            self.protocol == "kb.eval.study.v1",
            "unknown study protocol",
        )?;
        id(&self.id)?;
        ensure(
            !self.tasks.is_empty()
                && self.tasks.len() <= 10_000
                && self.arms.len() >= 2
                && !self.blocks.is_empty(),
            "study requires tasks, at least two arms and blocks",
        )?;
        ensure(
            self.repetitions > 0
                && self.repetitions <= 100
                && (1000..=1_000_000).contains(&self.draws)
                && self.alpha > 0.0
                && self.alpha <= 0.1
                && self.minimum_tasks >= 2,
            "invalid preregistered sample/statistics parameters",
        )?;
        ensure(
            !self.decision_rules.trim().is_empty() && !self.comparisons.is_empty(),
            "preregister decision rules and comparisons",
        )?;
        unique(self.tasks.iter().map(|t| t.id.as_str()))?;
        unique(self.arms.iter().map(|a| a.id.as_str()))?;
        unique(self.blocks.iter().map(|b| b.id.as_str()))?;
        unique(self.comparisons.iter().map(|c| c.id.as_str()))?;
        for t in &self.tasks {
            hash(&t.sha256)?;
        }
        for b in &self.blocks {
            b.agent.validate()?;
            ensure(!b.judges.is_empty(), "each block needs a pinned judge")?;
            let mut judges = BTreeSet::new();
            for j in &b.judges {
                j.validate()?;
                ensure(
                    judges.insert(format!("{:?}", j.family)),
                    "use at most one judge per family in each block",
                )?;
            }
        }
        for a in &self.arms {
            let mut paths = Vec::new();
            for o in &a.overlays {
                commit(&o.commit)?;
                commit(&o.as_of)?;
                kb::util::check_rel_path(&o.target).map_err(|e| e.to_string())?;
                ensure(
                    !o.target.split('/').any(|s| s == ".git"),
                    "overlay cannot replace Git metadata",
                )?;
                let p = PathBuf::from(&o.target);
                ensure(
                    paths
                        .iter()
                        .all(|q: &PathBuf| !q.starts_with(&p) && !p.starts_with(q)),
                    "overlapping overlays",
                )?;
                paths.push(p);
            }
        }
        for m in self.metrics.values() {
            ensure(
                m.sesoi.is_finite() && m.sesoi > 0.0,
                "SESOI must be positive and finite",
            )?;
        }
        for c in &self.comparisons {
            ensure(
                c.baseline != c.candidate
                    && self.arms.iter().any(|a| a.id == c.baseline)
                    && self.arms.iter().any(|a| a.id == c.candidate)
                    && self.metrics.contains_key(&c.metric),
                "comparison references an unknown arm or metric",
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod template_tests {
    use super::*;
    #[test]
    fn shipped_synthetic_templates_follow_the_protocol() {
        let task: Task =
            serde_json::from_str(include_str!("../templates/task.template.json")).unwrap();
        let study: Study =
            serde_json::from_str(include_str!("../templates/study.template.json")).unwrap();
        task.validate().unwrap();
        study.validate().unwrap();
        assert_eq!(study.minimum_tasks, 40);
        assert_eq!(study.repetitions, 2);
    }
}
