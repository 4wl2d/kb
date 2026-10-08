//! Pure evaluation of declarative probes over adapter-supplied facts. No commands, source
//! parsers, network or filesystem access. Regexes describe lexical checks, not semantics.
pub mod git;

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::context::{TaskScope, Verdict};
use crate::error::{KbError, Result};
use crate::glob::RepoGlob;
use crate::impact::{ChangeStatus, HostDiff};
use crate::knowledge::RecordEntry;
use crate::model::{CodeConfidence, CodeRelation, CodeResponse, Level, Status, VerifyProbe};
use crate::util::FieldHasher;

pub fn kind(probe: &VerifyProbe) -> &'static str {
    match probe {
        VerifyProbe::CommitMessage { .. } => "commit-message",
        VerifyProbe::BranchName { .. } => "branch-name",
        VerifyProbe::ForbiddenImport { .. } => "forbidden-import",
        VerifyProbe::Naming { .. } => "naming",
        VerifyProbe::BannedApi { .. } => "banned-api",
    }
}

#[derive(Debug, Clone)]
pub struct AddedLine {
    pub line: u32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ChangedSource {
    pub path: String,
    pub status: ChangeStatus,
    pub added: Vec<AddedLine>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub commit: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Input {
    pub repo: String,
    pub mode: String,
    pub diff: HostDiff,
    pub branch: Option<String>,
    pub branch_source: &'static str,
    pub messages: Vec<Message>,
    pub files: Vec<ChangedSource>,
    pub code: Option<CodeResponse>,
}

impl Input {
    fn digest(&self) -> Result<String> {
        let mut h = FieldHasher::new();
        h.field("kb-verify-input/1")
            .field(&self.repo)
            .field(&self.mode)
            .field(crate::context::canonical_json(&serde_json::to_value(
                &self.diff,
            )?))
            .field(self.branch.as_deref().unwrap_or(""))
            .field(self.branch_source);
        for message in &self.messages {
            h.field(message.commit.as_deref().unwrap_or(""))
                .field(&message.text);
        }
        for file in &self.files {
            h.field(&file.path)
                .field(file.error.as_deref().unwrap_or(""));
            for line in &file.added {
                h.field(line.line.to_string()).field(&line.text);
            }
        }
        if let Some(code) = &self.code {
            h.field(crate::context::canonical_json(&serde_json::to_value(code)?));
        }
        Ok(h.finish_hex())
    }
}

pub struct Options {
    pub only: BTreeSet<String>,
    /// Explicit assertion that textual conditions hold and no listed exception applies.
    pub applicable: BTreeSet<String>,
    pub strict: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Passed,
    Failed,
    Skipped,
    Unverifiable,
}

#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub path: Option<String>,
    pub line: Option<u32>,
    pub commit: Option<String>,
    pub confidence: Option<CodeConfidence>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub id: String,
    pub kind: &'static str,
    pub level: Level,
    pub blocking: bool,
    pub state: State,
    pub detail: String,
    pub matches: usize,
    pub evidence: Vec<Evidence>,
    pub evidence_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub protocol: &'static str,
    pub repo: String,
    pub mode: String,
    pub diff: HostDiff,
    pub branch: Option<String>,
    pub branch_source: &'static str,
    pub input_digest: String,
    pub code: Option<crate::context::code::CodeInfo>,
    pub messages_checked: usize,
    pub explicit_applicability: BTreeSet<String>,
    pub only: BTreeSet<String>,
    pub probes: Vec<ProbeResult>,
    pub counts: BTreeMap<&'static str, usize>,
    pub blocking_failures: usize,
    pub blocking_unverifiable: usize,
    pub note: &'static str,
}

pub fn evaluate(
    records: &[RecordEntry],
    scope: &TaskScope,
    input: &Input,
    options: &Options,
) -> Result<Report> {
    for name in &options.only {
        if ![
            "commit-message",
            "branch-name",
            "forbidden-import",
            "naming",
            "banned-api",
        ]
        .contains(&name.as_str())
        {
            return Err(KbError::invalid_input(format!("unknown probe kind {name}")));
        }
    }
    let mut probes = Vec::new();
    let mut statements = BTreeSet::new();
    for entry in records {
        let record = &entry.parsed.record;
        if record.status() != Status::Accepted {
            continue;
        }
        let applicability = crate::context::evaluate_applicability(&entry.parsed.meta(), scope);
        for statement in record.normative() {
            let sid = format!("{}#{}", record.id(), statement.id);
            statements.insert(sid.clone());
            for (index, probe) in statement.verify.iter().enumerate() {
                let mut row = ProbeResult {
                    id: format!("{sid}:{index}"),
                    kind: kind(probe),
                    level: statement.level,
                    blocking: options.strict
                        || matches!(statement.level, Level::Must | Level::MustNot),
                    state: State::Skipped,
                    detail: String::new(),
                    matches: 0,
                    evidence: Vec::new(),
                    evidence_truncated: false,
                };
                if !options.only.is_empty() && !options.only.contains(row.kind) {
                    row.detail = "outside explicitly selected probe kinds".into();
                } else if applicability.verdict() == Verdict::NotApplicable {
                    row.detail = applicability.describe();
                } else if applicability.verdict() == Verdict::Undetermined {
                    row.state = State::Unverifiable;
                    row.detail = applicability.describe();
                } else if (!statement.conditions.is_empty() || !statement.exceptions.is_empty())
                    && !options.applicable.contains(&sid)
                {
                    row.state = State::Unverifiable;
                    row.detail = format!(
                        "textual conditions/exceptions need a decision; --applicable {sid} asserts that conditions hold and no exception applies"
                    );
                } else {
                    evaluate_probe(probe, input, &mut row)?;
                }
                probes.push(row);
                if probes.len() > 20_000 {
                    return Err(KbError::invalid_input(
                        "over 20000 verification probes; split the knowledge scope",
                    ));
                }
            }
        }
    }
    if let Some(id) = options
        .applicable
        .iter()
        .find(|id| !statements.contains(*id))
    {
        return Err(KbError::invalid_input(format!(
            "unknown --applicable statement {id}"
        )));
    }
    probes.sort_by(|a, b| a.id.cmp(&b.id));
    let mut counts = BTreeMap::from([
        ("passed", 0),
        ("failed", 0),
        ("skipped", 0),
        ("unverifiable", 0),
    ]);
    for row in &probes {
        *counts
            .entry(match row.state {
                State::Passed => "passed",
                State::Failed => "failed",
                State::Skipped => "skipped",
                State::Unverifiable => "unverifiable",
            })
            .or_default() += 1;
    }
    let blocking_failures = probes
        .iter()
        .filter(|p| p.blocking && p.state == State::Failed)
        .count();
    let blocking_unverifiable = probes
        .iter()
        .filter(|p| p.blocking && p.state == State::Unverifiable)
        .count();
    Ok(Report {
        protocol: "kb.verify.v1",
        repo: input.repo.clone(),
        mode: input.mode.clone(),
        diff: input.diff.clone(),
        branch: input.branch.clone(),
        branch_source: input.branch_source,
        input_digest: input.digest()?,
        code: input.code.as_ref().map(crate::code::info).transpose()?,
        messages_checked: input.messages.len(),
        explicit_applicability: options.applicable.clone(),
        only: options.only.clone(),
        probes,
        counts,
        blocking_failures,
        blocking_unverifiable,
        note: "Declared probes only. Commit/branch/naming regexes must match; banned-api regexes must not match added lines; forbidden-import uses static provider edges. Text conditions require explicit applicability. Regex/API matches are lexical (including comments/strings). No record text is executed. Advisory levels block only with --strict.",
    })
}

fn regex(pattern: &str) -> Result<regex::Regex> {
    regex::RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| KbError::invalid_input(format!("invalid probe regex: {e}")))
}

pub fn globs(patterns: &[String]) -> Result<Vec<RepoGlob>> {
    patterns
        .iter()
        .map(|s| RepoGlob::parse(s).map_err(KbError::invalid_input))
        .collect()
}
pub fn matches(patterns: &[RepoGlob], repo: &str, path: &str) -> bool {
    patterns.iter().any(|p| p.matches(Some(repo), path))
}

fn hit(row: &mut ProbeResult, evidence: Evidence) {
    row.matches += 1;
    if row.evidence.len() < 100 {
        row.evidence.push(evidence);
    } else {
        row.evidence_truncated = true;
        if evidence.confidence == Some(CodeConfidence::Resolved)
            && !row
                .evidence
                .iter()
                .any(|e| e.confidence == Some(CodeConfidence::Resolved))
        {
            // A bounded report must retain a witness that actually justifies failure.
            if let Some(last) = row.evidence.last_mut() {
                *last = evidence;
            }
        }
    }
}

fn evaluate_probe(probe: &VerifyProbe, input: &Input, row: &mut ProbeResult) -> Result<()> {
    row.state = State::Passed;
    match probe {
        VerifyProbe::CommitMessage { pattern } => {
            if input.messages.is_empty() {
                row.state = State::Skipped;
                row.detail="no committed or pending message in this input; the commit-msg hook checks the eventual message".into();
                return Ok(());
            }
            let regex = regex(pattern)?;
            for message in &input.messages {
                if !regex.is_match(message.text.trim_end_matches('\n')) {
                    hit(
                        row,
                        Evidence {
                            path: None,
                            line: None,
                            commit: message.commit.clone(),
                            confidence: None,
                        },
                    );
                }
            }
            row.detail = "checked each supplied commit message against the required regex".into();
        }
        VerifyProbe::BranchName { pattern } => {
            let Some(branch) = &input.branch else {
                row.state = State::Unverifiable;
                row.detail = "detached HEAD: pass the actual source branch with --branch".into();
                return Ok(());
            };
            if !regex(pattern)?.is_match(branch) {
                hit(
                    row,
                    Evidence {
                        path: None,
                        line: None,
                        commit: input.diff.head.clone(),
                        confidence: None,
                    },
                );
            }
            row.detail = "checked the selected branch name against the required regex".into();
        }
        VerifyProbe::Naming { paths, pattern } => {
            let patterns = globs(paths)?;
            let regex = regex(pattern)?;
            let selected: Vec<_> = input
                .files
                .iter()
                .filter(|f| {
                    f.status != ChangeStatus::Deleted && matches(&patterns, &input.repo, &f.path)
                })
                .collect();
            if selected.is_empty() {
                row.state = State::Skipped;
                row.detail = "no changed path matches this probe".into();
                return Ok(());
            }
            for file in selected {
                if !regex.is_match(&file.path) {
                    hit(
                        row,
                        Evidence {
                            path: Some(file.path.clone()),
                            line: None,
                            commit: input.diff.head.clone(),
                            confidence: None,
                        },
                    );
                }
            }
            row.detail = "checked final repo-relative changed paths, excluding deletions".into();
        }
        VerifyProbe::BannedApi { paths, pattern } => {
            let patterns = globs(paths)?;
            let regex = regex(pattern)?;
            let selected: Vec<_> = input
                .files
                .iter()
                .filter(|f| {
                    f.status != ChangeStatus::Deleted && matches(&patterns, &input.repo, &f.path)
                })
                .collect();
            if selected.is_empty() {
                row.state = State::Skipped;
                row.detail = "no changed path matches this probe".into();
                return Ok(());
            }
            let mut missing = Vec::new();
            for file in selected {
                if let Some(error) = &file.error {
                    missing.push(format!("{}: {error}", file.path));
                    continue;
                }
                for line in &file.added {
                    if regex.is_match(&line.text) {
                        hit(
                            row,
                            Evidence {
                                path: Some(file.path.clone()),
                                line: Some(line.line),
                                commit: input.diff.head.clone(),
                                confidence: None,
                            },
                        );
                    }
                }
            }
            row.detail="checked only added source lines (and entire new untracked files) for the forbidden lexical pattern".into();
            if !missing.is_empty() {
                row.state = State::Unverifiable;
                row.detail
                    .push_str(&format!("; unavailable: {}", missing.join("; ")));
            }
        }
        VerifyProbe::ForbiddenImport { from, to } => {
            let from = globs(from)?;
            let to = globs(to)?;
            let changed: BTreeSet<_> = input
                .files
                .iter()
                .filter(|f| {
                    f.status != ChangeStatus::Deleted && matches(&from, &input.repo, &f.path)
                })
                .map(|f| f.path.as_str())
                .collect();
            if changed.is_empty() {
                row.state = State::Skipped;
                row.detail = "no changed source path matches the import probe".into();
                return Ok(());
            }
            if input.mode != "commit" {
                row.state = State::Unverifiable;
                row.detail="an immutable commit graph cannot verify uncommitted import changes; run this probe on --head after commit".into();
                return Ok(());
            }
            let Some(graph) = &input.code else {
                row.state = State::Unverifiable;
                row.detail = "forbidden-import needs a commit-bound provider".into();
                return Ok(());
            };
            let symbols: BTreeMap<_, _> =
                graph.symbols.iter().map(|s| (s.id.as_str(), s)).collect();
            let mut possible = false;
            let mut proven = false;
            for edge in graph.refs.iter().filter(|e| e.kind == CodeRelation::Import) {
                let (Some(source), Some(target)) = (
                    symbols.get(edge.from.as_str()),
                    symbols.get(edge.to.as_str()),
                ) else {
                    continue;
                };
                if changed.contains(source.path.as_str()) && matches(&to, &input.repo, &target.path)
                {
                    possible |= edge.confidence == CodeConfidence::Possible;
                    proven |= edge.confidence == CodeConfidence::Resolved;
                    hit(
                        row,
                        Evidence {
                            path: Some(source.path.clone()),
                            line: Some(edge.line),
                            commit: Some(graph.commit.clone()),
                            confidence: Some(edge.confidence),
                        },
                    );
                }
            }
            if proven {
                row.state = State::Failed;
                row.detail = "resolved static import crosses the forbidden path boundary".into();
                return Ok(());
            }
            if possible
                || !graph.complete
                || to
                    .iter()
                    .any(|p| p.repo.as_deref().is_some_and(|r| r != input.repo))
            {
                row.state = State::Unverifiable;
                row.detail="possible edges, incomplete graph or cross-repository target prevent proving absence of a forbidden import".into();
                return Ok(());
            }
            row.detail="no forbidden import among the provider's complete static edges for changed source paths".into();
        }
    }
    if row.matches > 0 {
        row.state = State::Failed;
    }
    Ok(())
}
