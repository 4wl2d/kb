//! Shared plumbing of the agent commands (`sym`, `callers`, `impact <symbol>`, `doc`,
//! `find`, `check`, `tools`): the code root, the tool registry, the knowledge lookup that
//! relates paths to feature documentation, and bounded output.

use std::collections::{BTreeMap, BTreeSet};

use super::Ctx;
use super::session::{Options, Session};
use crate::answer::{Answer, DEFAULT_LINES, Item, MAX_LINES, Source};
use crate::context::{self, ContextRequest, Tier, UnitBody};
use crate::error::{ErrorCode, KbError, Result};
use crate::host::{self, HostContext};
use crate::knowledge::{Freshness, KnowledgeView, Origin, RecordEntry};
use crate::model::{Intent, Kind, Level, Record, Registry, Status};
use crate::output::{CommandOutput, Format};
use crate::source::WorkingTreeSource;
use crate::tools::{self, Roots};
use crate::util::PathExt;

/// Where the agent commands operate.
pub struct Workspace {
    pub roots: Roots,
    pub host: Option<HostContext>,
    pub registry: tools::Registry,
}

/// Resolve the code root (host, else the KB checkout), its registry repo and the tool
/// registry of the profile.
pub fn workspace(ctx: &Ctx) -> Result<Workspace> {
    let host = super::session::detect_host(ctx)?;
    let kb = ctx
        .env
        .kb_root
        .canonical()
        .unwrap_or_else(|_| ctx.env.kb_root.clone());
    let code = host
        .as_ref()
        .map(|h| h.root.clone())
        .unwrap_or_else(|| kb.clone());
    let loc = ctx.location()?;
    let repo = host.as_ref().and_then(|h| {
        let (registry, _) =
            crate::corpus::load_registry(&WorkingTreeSource::new(&ctx.env.kb_root), &loc).ok()?;
        host::identify_repo(h, &registry).map(|(id, _)| id)
    });
    let kb_mount = host
        .as_ref()
        .and_then(|h| h.kb_submodule_path.clone())
        .or_else(|| {
            (kb != code)
                .then(|| crate::util::rel_string(&code, &kb))
                .flatten()
        });
    let registry = tools::load(&ctx.env.kb_root, &loc)?;
    Ok(Workspace {
        roots: Roots {
            kb,
            code,
            repo,
            kb_mount,
        },
        host,
        registry,
    })
}

pub fn lines(requested: Option<u64>) -> usize {
    requested.map_or(DEFAULT_LINES, |n| n.min(MAX_LINES as u64) as usize)
}

/// Bound and render an answer; `failure` keeps the printed result (like `CONTEXT_INCOMPLETE`).
pub fn output(
    mut answer: Answer,
    requested: Option<u64>,
    failure: Option<KbError>,
) -> CommandOutput {
    let lines = lines(requested);
    if let Some(n) = requested.filter(|&n| n > lines as u64) {
        answer
            .notes
            .insert(0, format!("--lines {n} capped at {lines}"));
    }
    let answer = answer.bound(lines);
    let mut out = CommandOutput::new(answer.to_json(), answer.render());
    if let Some(f) = failure {
        out = out.with_failure(f);
    }
    out
}

pub fn not_found(what: impl Into<String>) -> KbError {
    KbError::new(ErrorCode::NotFound, what.into())
}

/// How much knowledge detail to add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// Feature documentation: purpose, invariant statements, contracts, entry points,
    /// limits and gaps.
    Doc,
    /// Source locations, record IDs and documentation anchors.
    CompactDoc,
    /// One line per affected feature and governing record.
    Impact,
}

/// Relate `paths` (code-root-relative) to the knowledge that governs them, using the same
/// scope selection as `context --intent implement`, and add the result to `answer`.
pub fn knowledge(
    ctx: &Ctx,
    ws: &Workspace,
    paths: &[String],
    detail: Detail,
    answer: &mut Answer,
) -> Result<()> {
    let mut session = Session::open(ctx, ws.host.clone(), Options::reading(ctx, false))?;
    let info = session.info.clone();
    let host = session.host.clone();
    let repo_hint = ws.roots.repo.clone();
    let paths = paths.to_vec();
    let code_is_kb = ws.roots.code == ws.roots.kb;
    let (result, anchored, registry) = session.with_view(|view| {
        let mut env = super::cmd_context::task_env(host.as_ref(), view.registry(), info.clone());
        if env.host_repo.is_none() {
            let only = match view.registry().data.repos.as_slice() {
                [only] => Some(only.id.clone()),
                _ => repo_hint.clone(),
            };
            if let Some(repo) = only {
                env.host_repo = Some(repo);
                env.host_repo_source = Some("argument".into());
            }
        }
        let mut req = ContextRequest::new(Intent::Implement);
        req.paths = paths.clone();
        if host.is_none()
            && let Some(repo) = &env.host_repo
        {
            req.repos = vec![repo.clone()];
        }
        req.max_supplementary = Some(20);
        req.budget = Some(10_000_000);
        req.budget_unit = Some(crate::model::BudgetUnit::Bytes);
        let r = context::assemble(&req, &env, view, Format::Compact)?;
        let anchored = if detail == Detail::Impact {
            Vec::new()
        } else {
            anchored_records(view, &r, code_is_kb)?
        };
        Ok((r, anchored, view.registry().clone()))
    })?;
    answer.source(Source {
        id: "knowledge".into(),
        adapter: "knowledge".into(),
        version: Some(format!(
            "knowledge {}",
            short(info.revision.as_deref().unwrap_or("working-tree"))
        )),
        state: format!(
            "{}, {}",
            if info.freshness == Freshness::Verified {
                "verified"
            } else {
                "unverified"
            },
            result.status().as_str()
        ),
    });
    if result.status() != context::Completeness::Complete {
        let codes: Vec<&str> = result
            .header
            .reasons
            .iter()
            .map(|r| r.code.as_str())
            .collect();
        answer.notes.insert(
            0,
            format!(
                "knowledge is {}: {}",
                result.status().as_str(),
                codes.join(", ")
            ),
        );
    }
    match detail {
        Detail::CompactDoc => {
            super::cmd_doc::render_compact_knowledge(&result, &anchored, &ws.roots, answer)
        }
        _ => render_knowledge(&result, &anchored, &registry, detail, answer),
    }
    Ok(())
}

/// Accepted records that name one of the requested paths as their source, test or
/// document although their scope does not select them for it: the records a document
/// about code elsewhere was written from. Sorted by id.
fn anchored_records(
    view: &dyn KnowledgeView,
    result: &context::ContextResult,
    code_is_kb: bool,
) -> Result<Vec<RecordEntry>> {
    let paths = &result.header.scope.paths;
    let selected: BTreeSet<&str> = result.units.iter().map(|u| u.record_id.as_str()).collect();
    let mut ids: Vec<String> = view
        .metas_by_kind(&Kind::ALL, Origin::Accepted)?
        .into_iter()
        .filter(|e| e.meta.status == Status::Accepted && !selected.contains(e.meta.id.as_str()))
        .filter(|e| super::cmd_doc::anchored(&e.meta.anchors, paths, code_is_kb))
        .map(|e| e.meta.id.clone())
        .collect();
    ids.sort();
    let mut records = view.records(&ids, Origin::Accepted)?;
    records.sort_by(|a, b| a.parsed.record.common().id.cmp(b.parsed.record.common().id));
    Ok(records)
}

fn short(rev: &str) -> &str {
    rev.get(..12).unwrap_or(rev)
}

fn statement_line(level: Level, text: &str, conditions: &[String], exceptions: usize) -> String {
    let mut s = format!("{} {text}", level.keyword());
    if !conditions.is_empty() {
        s.push_str(&format!(" (if: {})", conditions.join("; ")));
    }
    if exceptions > 0 {
        s.push_str(&format!(" (+{exceptions} exception(s))"));
    }
    s
}

/// Pure rendering of a context result into feature-centric items.
pub fn render_knowledge(
    result: &context::ContextResult,
    anchored: &[RecordEntry],
    registry: &Registry,
    detail: Detail,
    answer: &mut Answer,
) {
    const SRC: &str = "knowledge";
    let features: BTreeSet<String> = result
        .header
        .scope
        .features
        .known()
        .cloned()
        .unwrap_or_default();
    let mut feature_records = BTreeMap::new();
    let mut required = Vec::new();
    let mut supplementary = Vec::new();
    for unit in &result.units {
        let UnitBody::Record(parsed) = &unit.body else {
            continue;
        };
        if let Record::Feature(f) = &parsed.record {
            feature_records
                .entry(f.feature.clone())
                .or_insert_with(|| (unit.id.clone(), f.clone()));
            continue;
        }
        if unit.tier.is_required() {
            required.push((unit, parsed));
        } else if unit.tier == Tier::Supplementary {
            supplementary.push((unit, parsed));
        }
    }
    let mut head = Vec::new();
    let mut rules = Vec::new();
    let mut tail = Vec::new();
    for fid in &features {
        let title = registry
            .feature(fid)
            .map(|f| f.title.clone())
            .unwrap_or_default();
        let record = feature_records.get(fid);
        let mut item = Item::new("feature", SRC).name(fid.clone()).detail(title);
        if let Some((id, _)) = record {
            item.detail = Some(format!("{} [{id}]", item.detail.unwrap_or_default()));
        }
        head.push(item);
        if detail == Detail::Impact {
            continue;
        }
        let Some((_, f)) = record else {
            answer.note(format!(
                "feature `{fid}` has no feature document; the kb-document skill creates one"
            ));
            continue;
        };
        head.push(Item::new("purpose", SRC).detail(f.summary.clone()));
        for a in f
            .anchors
            .iter()
            .filter(|a| a.kind == crate::model::AnchorKind::Source)
        {
            let path = match (&a.repo, &a.path) {
                (Some(r), Some(p)) => format!("{r}:{p}"),
                (None, Some(p)) => p.clone(),
                _ => continue,
            };
            let mut it = Item::new("entry", SRC).at(path, None);
            if let Some(sym) = &a.symbol {
                it = it.name(sym.clone());
            }
            if let Some(n) = &a.note {
                it = it.detail(n.clone());
            }
            tail.push(it);
        }
        for b in &f.boundaries {
            tail.push(
                Item::new("limit", SRC)
                    .name(b.id.clone())
                    .detail(b.text.clone()),
            );
        }
    }
    let mut policies = Vec::new();
    for (unit, parsed) in &required {
        match &parsed.record {
            Record::Policy(_) => policies.push(unit.id.clone()),
            Record::Invariant(inv) if detail == Detail::Doc => {
                for st in &inv.statements {
                    rules.push(
                        Item::new("invariant", SRC)
                            .name(format!("{}#{}", unit.id, st.id))
                            .detail(statement_line(
                                st.level,
                                &st.text,
                                &st.conditions,
                                st.exceptions.len(),
                            )),
                    );
                }
            }
            Record::Contract(c) => rules.push(
                Item::new("contract", SRC)
                    .name(unit.id.clone())
                    .detail(format!(
                        "{} ({} obligation(s))",
                        unit.title,
                        c.obligations.len()
                    )),
            ),
            Record::Gap(g) => tail.push(
                Item::new("gap", SRC)
                    .name(unit.id.clone())
                    .detail(g.description.clone()),
            ),
            _ => rules.push(
                Item::new(unit.kind.as_str(), SRC)
                    .name(unit.id.clone())
                    .detail(unit.title.clone()),
            ),
        }
    }
    if detail == Detail::Doc {
        for entry in anchored {
            let record = entry.parsed.record.common();
            rules.push(
                Item::new(record.kind.as_str(), SRC)
                    .name(record.id)
                    .detail(format!("{} (cites this path)", record.title)),
            );
        }
    }
    for (unit, parsed) in &supplementary {
        match &parsed.record {
            Record::Gap(g) => tail.push(
                Item::new("gap", SRC)
                    .name(unit.id.clone())
                    .detail(g.description.clone()),
            ),
            Record::Decision(_) if detail == Detail::Doc => tail.push(
                Item::new("decision", SRC)
                    .name(unit.id.clone())
                    .detail(unit.title.clone()),
            ),
            _ => {}
        }
    }
    if !policies.is_empty() {
        answer.note(format!(
            "{} polic{} also apply: {}",
            policies.len(),
            if policies.len() == 1 { "y" } else { "ies" },
            policies.join(", ")
        ));
    }
    if features.is_empty() {
        answer.note("no registry feature covers these paths; the kb-document skill adds one");
    }
    answer.items.extend(head);
    answer.items.extend(rules);
    answer.items.extend(tail);
}
