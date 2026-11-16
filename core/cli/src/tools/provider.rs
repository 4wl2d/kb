//! `code-provider` adapter: any `kb.code.v1` provider (ADR 0011) connected through the
//! registry, e.g. `kb-code-provider --backend ast-index`. Answers are commit-bound: they
//! describe the code root's `HEAD` commit, validated against Git objects, and never see
//! uncommitted edits.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::{CodeQuery, Outcome, Probe, Roots, ToolSpec};
use crate::answer::{Item, Source};
use crate::error::Result;
use crate::model::tools::Capability;
use crate::model::{CodeExtent, CodeOperation, CodeResponse};

pub fn query(tool: &ToolSpec, probe: &Probe, roots: &Roots, q: &CodeQuery) -> Result<Outcome> {
    let provider = crate::code::Provider {
        program: probe.program.as_deref().map(PathBuf::from),
        args: tool.args.clone(),
        files: Vec::new(),
        timeout_seconds: tool.timeout(),
    };
    let operation = match q.capability {
        Capability::Sym => CodeOperation::Symbols,
        Capability::Callers => CodeOperation::Refs,
        _ => CodeOperation::Dependents,
    };
    let repo = roots.repo.clone().unwrap_or_else(|| "code".into());
    let mut request = crate::code::request(&roots.code, &repo, "HEAD", operation)?;
    request.identifiers = vec![q.name.clone()];
    request.depth = q.depth.clamp(1, 4);
    let response = provider.load(&request)?;
    let mut outcome = Outcome::new(Source {
        id: tool.id.clone(),
        adapter: "code-provider".into(),
        version: Some(format!("{} {}", response.tool.name, response.tool.version)),
        state: format!(
            "commit {}{}",
            &response.commit[..12.min(response.commit.len())],
            if response.complete {
                ", complete"
            } else {
                ", incomplete"
            }
        ),
    });
    outcome
        .notes
        .push("commit-bound: uncommitted edits are not part of this answer".into());
    outcome
        .notes
        .extend(response.limitations.iter().take(2).cloned());
    answer(&mut outcome, &response, q);
    Ok(outcome)
}

/// Turn a validated response into items. Pure.
pub fn answer(outcome: &mut Outcome, response: &CodeResponse, q: &CodeQuery) {
    let src = outcome.source.id.clone();
    let wanted = super::grep::leaf(&q.name);
    let targets: Vec<_> = response
        .symbols
        .iter()
        .filter(|s| s.extent != CodeExtent::File)
        .filter(|s| s.name == q.name || super::grep::leaf(&s.name) == wanted)
        .filter(|s| q.file.as_deref().is_none_or(|f| s.path.contains(f)))
        .filter(|s| q.kind.as_deref().is_none_or(|k| s.kind == k))
        .collect();
    outcome.found = !targets.is_empty();
    let by_id: BTreeMap<&str, _> = response
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    match q.capability {
        Capability::Sym => {
            outcome.items = targets
                .iter()
                .map(|s| {
                    Item::new("definition", &src)
                        .name(s.name.clone())
                        .at(s.path.clone(), Some(s.start_line))
                        .span(Some(s.end_line))
                        .detail(format!(
                            "{} {}",
                            s.kind,
                            s.signature.as_deref().unwrap_or("")
                        ))
                })
                .collect();
        }
        Capability::Callers => {
            let ids: BTreeSet<&str> = targets.iter().map(|s| s.id.as_str()).collect();
            for r in response.refs.iter().filter(|r| ids.contains(r.to.as_str())) {
                if let Some(from) = by_id.get(r.from.as_str()) {
                    outcome.items.push(
                        Item::new("caller", &src)
                            .name(from.name.clone())
                            .at(from.path.clone(), Some(r.line))
                            .detail(format!("{:?}", r.kind).to_lowercase())
                            .confidence(match r.confidence {
                                crate::model::CodeConfidence::Resolved => "resolved",
                                crate::model::CodeConfidence::Possible => "possible",
                            }),
                    );
                }
            }
        }
        _ => {
            // Incoming references, breadth first, to the requested depth.
            let mut frontier: BTreeSet<&str> = targets.iter().map(|s| s.id.as_str()).collect();
            let mut visited = frontier.clone();
            let mut files: BTreeSet<String> = targets.iter().map(|s| s.path.clone()).collect();
            for depth in 1..=q.depth.clamp(1, 4) {
                let mut next = BTreeSet::new();
                for r in response
                    .refs
                    .iter()
                    .filter(|r| frontier.contains(r.to.as_str()))
                {
                    if visited.insert(r.from.as_str())
                        && let Some(from) = by_id.get(r.from.as_str())
                    {
                        next.insert(r.from.as_str());
                        files.insert(from.path.clone());
                        outcome.items.push(
                            Item::new("affected", &src)
                                .name(from.name.clone())
                                .at(from.path.clone(), Some(from.start_line))
                                .detail(format!("{} depth {depth}", from.kind))
                                .confidence(match r.confidence {
                                    crate::model::CodeConfidence::Resolved => "resolved",
                                    crate::model::CodeConfidence::Possible => "possible",
                                }),
                        );
                    }
                }
                if next.is_empty() {
                    break;
                }
                frontier = next;
            }
            outcome.files = files.into_iter().collect();
        }
    }
}
