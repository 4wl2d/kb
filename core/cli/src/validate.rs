//! Corpus-level validation: namespace, registry references, scope satisfiability, contract
//! parties, links, cycles, lifecycle, overrides and authority, lints, base comparison and
//! the upstream template check. See docs/architecture.md §3–§4.
//!
//! Context-free checks of a single file live in [`crate::parse`]; everything here needs the
//! profile config, the registry or other records. All functions are pure except
//! [`validate_templates`], which reads the KB checkout, and every result is ordered
//! deterministically ([`crate::diag::normalize`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;

use crate::corpus::{Corpus, is_record_path, load_corpus, parse_config};
use crate::diag::{Diagnostic, Severity, normalize};
use crate::error::{KbError, Result};
use crate::glob::RepoGlob;
use crate::model::ids::namespace_of;
use crate::model::registry::{
    ConceptsFile, FeaturesFile, ModulesFile, OwnersFile, ReposFile, parse_registry_file,
};
use crate::model::routing::RoutingTestFile;
use crate::model::{
    Kind, OverrideMode, ParsedRecord, Profile, ProfileConfig, ProfileLocation, RecordMeta,
    Registry, Scope, Setting, SettingValue, SkillConfig, Status, Stricter, UpstreamConfig,
};
use crate::output::Format;
use crate::parse::{lint_record, parse_record};
use crate::scope::{implied_repos, overlap, subsumes};
use crate::source::{SourceTree, WorkingTreeSource};

/// One successfully parsed record for cross-record validation.
#[derive(Debug, Clone)]
pub struct MetaInput {
    pub path: String,
    pub meta: Arc<RecordMeta>,
}

/// Metadata of every successfully parsed record of a corpus, in path order.
pub fn meta_inputs(corpus: &Corpus) -> Vec<MetaInput> {
    corpus
        .records()
        .map(|(entry, parsed)| MetaInput {
            path: entry.path.clone(),
            meta: Arc::new(parsed.meta()),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Cross-record validation
// ---------------------------------------------------------------------------------------

/// Validate all records of a snapshot (metadata only).
///
/// Implements the cross-record rules of docs/architecture.md §3.1–§3.7 and §4. Every
/// diagnostic carries the offending file (`path`) and record id (`record`). The result is
/// sorted and deduplicated.
pub fn validate_metas(
    config: &ProfileConfig,
    registry: &Registry,
    records: &[MetaInput],
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let index = RecordIndex::new(records, &mut out);
    for r in &index.ordered {
        check_namespace(config, r, &mut out);
        check_owner(registry, r, &mut out);
        check_registry_refs(registry, r, &mut out);
        check_scope_satisfiable(registry, r, &mut out);
        check_contract_parties(registry, r, &mut out);
        check_links(&index, r, &mut out);
    }
    check_cycles(&index, Relation::Requires, &mut out);
    check_cycles(&index, Relation::Supersedes, &mut out);
    check_supersession(&index, &mut out);
    check_overrides(registry, &index, &mut out);
    normalize(&mut out);
    out
}

/// An error diagnostic located at a record.
fn record_error(r: &MetaInput, code: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(code, message)
        .at_path(r.path.clone())
        .for_record(r.meta.id.clone())
}

fn record_warning(r: &MetaInput, code: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::warning(code, message)
        .at_path(r.path.clone())
        .for_record(r.meta.id.clone())
}

/// `[a, b]` rendering of an id list for messages.
fn list<'a>(items: impl IntoIterator<Item = &'a String>) -> String {
    let v: Vec<&str> = items.into_iter().map(String::as_str).collect();
    format!("[{}]", v.join(", "))
}

/// Records ordered by (path, id) plus an id lookup. When an id is defined more than once,
/// the first file in path order is the one used for graph checks; every definition is
/// reported as `DUPLICATE_ID`.
struct RecordIndex<'a> {
    ordered: Vec<&'a MetaInput>,
    by_id: BTreeMap<&'a str, &'a MetaInput>,
}

impl<'a> RecordIndex<'a> {
    fn new(records: &'a [MetaInput], out: &mut Vec<Diagnostic>) -> RecordIndex<'a> {
        let mut ordered: Vec<&MetaInput> = records.iter().collect();
        ordered.sort_by(|a, b| (&a.path, &a.meta.id).cmp(&(&b.path, &b.meta.id)));
        let mut all: BTreeMap<&str, Vec<&MetaInput>> = BTreeMap::new();
        for r in &ordered {
            all.entry(r.meta.id.as_str()).or_default().push(r);
        }
        for (id, defs) in &all {
            if defs.len() > 1 {
                let paths: Vec<&str> = defs.iter().map(|d| d.path.as_str()).collect();
                for d in defs {
                    out.push(record_error(
                        d,
                        "DUPLICATE_ID",
                        format!(
                            "id `{id}` is defined in more than one file: {}",
                            paths.join(", ")
                        ),
                    ));
                }
            }
        }
        let by_id = all.into_iter().map(|(id, defs)| (id, defs[0])).collect();
        RecordIndex { ordered, by_id }
    }

    fn get(&self, id: &str) -> Option<&'a MetaInput> {
        self.by_id.get(id).copied()
    }
}

fn check_namespace(config: &ProfileConfig, r: &MetaInput, out: &mut Vec<Diagnostic>) {
    let ns = namespace_of(&r.meta.id);
    if ns != config.project.namespace {
        out.push(record_error(
            r,
            "NAMESPACE_MISMATCH",
            format!(
                "id `{}` is in namespace `{ns}`, but this profile's namespace is `{}`",
                r.meta.id, config.project.namespace
            ),
        ));
    }
}

/// Owner existence and authority (docs/architecture.md §4).
fn check_owner(registry: &Registry, r: &MetaInput, out: &mut Vec<Diagnostic>) {
    let m = &r.meta;
    let Some(owner) = registry.owner(&m.owner) else {
        out.push(record_error(
            r,
            "OWNER_UNKNOWN",
            format!("owner `{}` is not in registry/owners.toml", m.owner),
        ));
        return;
    };
    if m.scope.product {
        if !owner.product {
            out.push(record_error(
                r,
                "OWNER_NOT_AUTHORIZED",
                format!(
                    "product-wide records need an owner with `product = true`; `{}` is not one",
                    owner.id
                ),
            ));
        }
        return;
    }
    if owner.repos.is_empty() {
        return;
    }
    match implied_repos(&m.scope, registry) {
        None => out.push(record_error(
            r,
            "OWNER_NOT_AUTHORIZED",
            format!(
                "owner `{}` may only own records within repos {}, but this scope does not constrain repos",
                owner.id,
                list(&owner.repos)
            ),
        )),
        Some(repos) => {
            let outside: Vec<&String> = repos.iter().filter(|x| !owner.repos.contains(x)).collect();
            if !outside.is_empty() {
                out.push(record_error(
                    r,
                    "OWNER_NOT_AUTHORIZED",
                    format!(
                        "owner `{}` may only own records within repos {}, but the scope implies {}",
                        owner.id,
                        list(&owner.repos),
                        list(outside)
                    ),
                ));
            }
        }
    }
}

/// Every registry id named by a record must exist.
fn check_registry_refs(registry: &Registry, r: &MetaInput, out: &mut Vec<Diagnostic>) {
    let m = &r.meta;
    let mut unknown = |code: &str, what: String| out.push(record_error(r, code, what));
    for x in &m.scope.repos {
        if registry.repo(x).is_none() {
            unknown("UNKNOWN_REPO", format!("scope.repos: unknown repo `{x}`"));
        }
    }
    for x in &m.scope.modules {
        if registry.module(x).is_none() {
            unknown(
                "UNKNOWN_MODULE",
                format!("scope.modules: unknown module `{x}`"),
            );
        }
    }
    for x in &m.scope.features {
        if registry.feature(x).is_none() {
            unknown(
                "UNKNOWN_FEATURE",
                format!("scope.features: unknown feature `{x}`"),
            );
        }
    }
    for x in &m.selectors.concepts {
        if registry.concept(x).is_none() {
            unknown(
                "UNKNOWN_CONCEPT",
                format!("selectors.concepts: unknown concept `{x}`"),
            );
        }
    }
    for p in &m.selectors.paths {
        // Invalid globs are reported by the parser; only the repo qualifier is checked here.
        if let Ok(g) = RepoGlob::parse(p)
            && let Some(repo) = &g.repo
            && registry.repo(repo).is_none()
        {
            unknown(
                "UNKNOWN_REPO",
                format!("selectors.paths: `{p}` names unknown repo `{repo}`"),
            );
        }
    }
    if let Some(a) = &m.applicability {
        for repo in a.versions.keys() {
            if registry.repo(repo).is_none() {
                unknown(
                    "UNKNOWN_REPO",
                    format!("applicability.versions: unknown repo `{repo}`"),
                );
            }
        }
    }
    for (i, a) in m.anchors.iter().enumerate() {
        if let Some(repo) = &a.repo
            && registry.repo(repo).is_none()
        {
            unknown(
                "UNKNOWN_REPO",
                format!("anchors[{i}].repo: unknown repo `{repo}`"),
            );
        }
    }
    if let Some(f) = &m.feature
        && registry.feature(f).is_none()
    {
        unknown(
            "UNKNOWN_FEATURE",
            format!("feature: `{f}` is not in registry/features.toml"),
        );
    }
    for s in &m.settings {
        for o in &s.override_owners {
            if registry.owner(o).is_none() {
                unknown(
                    "OWNER_UNKNOWN",
                    format!("settings.{}.override_owners: unknown owner `{o}`", s.name),
                );
            }
        }
    }
}

/// Scope combinations that no task can satisfy (AND across dimensions).
fn check_scope_satisfiable(registry: &Registry, r: &MetaInput, out: &mut Vec<Diagnostic>) {
    let s = &r.meta.scope;
    if s.product {
        return;
    }
    let mut item_problem = false;
    if !s.repos.is_empty() {
        for id in &s.modules {
            if let Some(module) = registry.module(id)
                && !s.repos.contains(&module.repo)
            {
                item_problem = true;
                out.push(record_error(
                    r,
                    "SCOPE_UNSATISFIABLE",
                    format!(
                        "module `{id}` belongs to repo `{}`, which is not in scope.repos {}",
                        module.repo,
                        list(&s.repos)
                    ),
                ));
            }
        }
        for id in &s.features {
            if let Some(feature) = registry.feature(id)
                && !feature.repos.is_empty()
                && !feature.repos.iter().any(|x| s.repos.contains(x))
            {
                item_problem = true;
                out.push(record_error(
                    r,
                    "SCOPE_UNSATISFIABLE",
                    format!(
                        "feature `{id}` is implemented in repos {}, none of which is in scope.repos {}",
                        list(&feature.repos),
                        list(&s.repos)
                    ),
                ));
            }
        }
    }
    // Dimensions must share at least one repo. Only decidable when every id is known.
    let all_known = s.modules.iter().all(|m| registry.module(m).is_some())
        && s.features.iter().all(|f| registry.feature(f).is_some());
    if item_problem || !all_known {
        return;
    }
    let mut dims: Vec<(&str, BTreeSet<String>)> = Vec::new();
    if !s.repos.is_empty() {
        dims.push(("repos", s.repos.iter().cloned().collect()));
    }
    if !s.modules.is_empty() {
        dims.push(("modules", registry.repos_of_modules(&s.modules)));
    }
    let feature_repos = registry.repos_of_features(&s.features);
    if !feature_repos.is_empty() {
        dims.push(("features", feature_repos));
    }
    if dims.len() < 2 {
        return;
    }
    let common = dims
        .iter()
        .skip(1)
        .fold(dims[0].1.clone(), |acc, (_, set)| {
            acc.intersection(set).cloned().collect()
        });
    if common.is_empty() {
        let detail: Vec<String> = dims
            .iter()
            .map(|(name, set)| format!("{name} → {}", list(set)))
            .collect();
        out.push(record_error(
            r,
            "SCOPE_UNSATISFIABLE",
            format!(
                "scope dimensions imply disjoint repos ({}); no task can match all of them",
                detail.join("; ")
            ),
        ));
    }
}

/// Contract parties name existing repos and modules, modules belong to their party's repo,
/// and every party repo is covered by the contract's scope (unless product-wide), so the
/// mandatory contract reaches every party.
fn check_contract_parties(registry: &Registry, r: &MetaInput, out: &mut Vec<Diagnostic>) {
    let m = &r.meta;
    if m.parties.is_empty() {
        return;
    }
    let scope_repos = implied_repos(&m.scope, registry);
    for p in &m.parties {
        if registry.repo(&p.repo).is_none() {
            out.push(record_error(
                r,
                "UNKNOWN_REPO",
                format!("parties.{}: unknown repo `{}`", p.id, p.repo),
            ));
        } else if let Some(repos) = &scope_repos
            && !repos.contains(&p.repo)
        {
            out.push(record_error(
                r,
                "CONTRACT_PARTY_OUT_OF_SCOPE",
                format!(
                    "party `{}` is in repo `{}`, which the contract scope {} does not cover",
                    p.id,
                    p.repo,
                    list(repos)
                ),
            ));
        }
        for id in &p.modules {
            match registry.module(id) {
                None => out.push(record_error(
                    r,
                    "UNKNOWN_MODULE",
                    format!("parties.{}: unknown module `{id}`", p.id),
                )),
                Some(module) if module.repo != p.repo => out.push(record_error(
                    r,
                    "CONTRACT_PARTY_MODULE_MISMATCH",
                    format!(
                        "party `{}`: module `{id}` belongs to repo `{}`, not `{}`",
                        p.id, module.repo, p.repo
                    ),
                )),
                Some(_) => {}
            }
        }
    }
}

/// Link targets exist (any status); accepted records only require accepted knowledge.
fn check_links(index: &RecordIndex<'_>, r: &MetaInput, out: &mut Vec<Diagnostic>) {
    let m = &r.meta;
    let relations = [
        ("links.requires", &m.links.requires),
        ("links.rationale", &m.links.rationale),
        ("links.related", &m.links.related),
        ("links.supersedes", &m.links.supersedes),
        ("affects", &m.gap_affects),
    ];
    for (field, targets) in relations {
        for t in targets {
            if index.get(t).is_none() {
                out.push(record_error(
                    r,
                    "DANGLING_LINK",
                    format!("{field}: target `{t}` does not exist"),
                ));
            }
        }
    }
    if m.status != Status::Accepted {
        return;
    }
    for t in &m.links.requires {
        let Some(target) = index.get(t) else { continue };
        match target.meta.status {
            Status::Draft | Status::Superseded => out.push(record_error(
                r,
                "REQUIRES_NOT_ACCEPTED",
                format!(
                    "accepted record requires `{t}`, which is {}; required knowledge must be accepted",
                    target.meta.status.as_str()
                ),
            )),
            Status::Deprecated => out.push(record_warning(
                r,
                "REQUIRES_DEPRECATED",
                format!("accepted record requires deprecated record `{t}`"),
            )),
            Status::Accepted => {}
        }
    }
}

#[derive(Clone, Copy)]
enum Relation {
    Requires,
    Supersedes,
}

impl Relation {
    fn targets(self, m: &RecordMeta) -> &[String] {
        match self {
            Relation::Requires => &m.links.requires,
            Relation::Supersedes => &m.links.supersedes,
        }
    }
    fn code(self) -> &'static str {
        match self {
            Relation::Requires => "REQUIRES_CYCLE",
            Relation::Supersedes => "SUPERSEDES_CYCLE",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Relation::Requires => "requires",
            Relation::Supersedes => "supersedes",
        }
    }
}

/// Report every cycle of a relation once: one diagnostic per strongly connected component,
/// located at its smallest id and naming a shortest cycle through it.
fn check_cycles(index: &RecordIndex<'_>, rel: Relation, out: &mut Vec<Diagnostic>) {
    let nodes: Vec<(&str, &MetaInput)> = index.by_id.iter().map(|(id, r)| (*id, *r)).collect();
    let pos: BTreeMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (*id, i))
        .collect();
    let adj: Vec<Vec<usize>> = nodes
        .iter()
        .map(|(_, r)| {
            let mut v: Vec<usize> = rel
                .targets(&r.meta)
                .iter()
                .filter_map(|t| pos.get(t.as_str()).copied())
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        })
        .collect();
    for scc in strongly_connected(&adj) {
        // Components are sorted and non-empty; the first node is the smallest id.
        let Some(&start) = scc.first() else { continue };
        if scc.len() == 1 && !adj[start].contains(&start) {
            continue;
        }
        let members: BTreeSet<usize> = scc.iter().copied().collect();
        let cycle = shortest_cycle(&adj, &members, start);
        let names: Vec<&str> = cycle.iter().map(|&i| nodes[i].0).collect();
        out.push(record_error(
            nodes[start].1,
            rel.code(),
            format!("{} cycle: {}", rel.name(), names.join(" -> ")),
        ));
    }
}

/// Tarjan's strongly connected components, iterative (no recursion depth limit).
fn strongly_connected(adj: &[Vec<usize>]) -> Vec<Vec<usize>> {
    const UNSEEN: usize = usize::MAX;
    let n = adj.len();
    let mut index = vec![UNSEEN; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut sccs = Vec::new();
    for root in 0..n {
        if index[root] != UNSEEN {
            continue;
        }
        let mut calls: Vec<(usize, usize)> = Vec::new();
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        calls.push((root, 0));
        while let Some(&(v, edge)) = calls.last() {
            if let Some(&w) = adj[v].get(edge) {
                if let Some(top) = calls.last_mut() {
                    top.1 += 1;
                }
                if index[w] == UNSEEN {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    calls.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            calls.pop();
            if let Some(&(parent, _)) = calls.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                let mut scc = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    scc.push(w);
                    if w == v {
                        break;
                    }
                }
                scc.sort_unstable();
                sccs.push(scc);
            }
        }
    }
    sccs.sort();
    sccs
}

/// Shortest cycle through `start` inside `members` (BFS over sorted adjacency), as a node
/// list that begins and ends with `start`.
fn shortest_cycle(adj: &[Vec<usize>], members: &BTreeSet<usize>, start: usize) -> Vec<usize> {
    let mut parent: BTreeMap<usize, usize> = BTreeMap::new();
    let mut queue = std::collections::VecDeque::from([start]);
    let mut seen = BTreeSet::from([start]);
    while let Some(v) = queue.pop_front() {
        for &w in &adj[v] {
            if w == start {
                // Walk BFS parents back from `v` to `start`, then close the cycle.
                let mut path = vec![v];
                let mut cur = v;
                while let Some(&p) = parent.get(&cur) {
                    path.push(p);
                    cur = p;
                }
                path.reverse();
                path.push(start);
                return path;
            }
            if members.contains(&w) && seen.insert(w) {
                parent.insert(w, v);
                queue.push_back(w);
            }
        }
    }
    vec![start, start]
}

/// Supersession consistency: targets are `superseded`, and every `superseded` record has a
/// successor.
fn check_supersession(index: &RecordIndex<'_>, out: &mut Vec<Diagnostic>) {
    let mut successors: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for r in &index.ordered {
        for t in &r.meta.links.supersedes {
            successors
                .entry(t.as_str())
                .or_default()
                .insert(r.meta.id.as_str());
        }
    }
    for (target, by) in &successors {
        if let Some(t) = index.get(target)
            && t.meta.status != Status::Superseded
        {
            let by: Vec<&str> = by.iter().copied().collect();
            out.push(record_error(
                t,
                "SUPERSEDED_STATUS",
                format!(
                    "record is superseded by {} but has status `{}`; set `status = \"superseded\"`",
                    by.join(", "),
                    t.meta.status.as_str()
                ),
            ));
        }
    }
    for r in &index.ordered {
        if r.meta.status == Status::Superseded && !successors.contains_key(r.meta.id.as_str()) {
            out.push(record_error(
                r,
                "SUPERSEDED_ORPHAN",
                "record has status `superseded`, but no record lists it in `links.supersedes`",
            ));
        }
    }
}

/// A valid override from an accepted policy, collected for ambiguity detection.
struct EffectiveOverride<'a> {
    record: &'a MetaInput,
    value: &'a SettingValue,
}

/// Override rules 1–6 and `OVERRIDE_AMBIGUOUS` (docs/architecture.md §3.7).
fn check_overrides(registry: &Registry, index: &RecordIndex<'_>, out: &mut Vec<Diagnostic>) {
    let mut effective: BTreeMap<&str, Vec<EffectiveOverride<'_>>> = BTreeMap::new();
    for r in &index.ordered {
        for ov in &r.meta.overrides {
            // Malformed targets are reported by the parser.
            let Some((target_id, name)) = ov.split_target() else {
                continue;
            };
            let target = &ov.target;
            let Some(policy) = index.get(target_id) else {
                out.push(record_error(
                    r,
                    "OVERRIDE_TARGET_UNKNOWN",
                    format!("override `{target}`: record `{target_id}` does not exist"),
                ));
                continue;
            };
            if policy.meta.kind != Kind::Policy {
                out.push(record_error(
                    r,
                    "OVERRIDE_TARGET_UNKNOWN",
                    format!(
                        "override `{target}`: `{target_id}` is a {}, not a policy",
                        policy.meta.kind.as_str()
                    ),
                ));
                continue;
            }
            let Some(setting) = policy.meta.settings.iter().find(|s| s.name == name) else {
                out.push(record_error(
                    r,
                    "OVERRIDE_TARGET_UNKNOWN",
                    format!("override `{target}`: policy `{target_id}` has no setting `{name}`"),
                ));
                continue;
            };
            if setting.r#override == OverrideMode::Forbidden {
                out.push(record_error(
                    r,
                    "OVERRIDE_FORBIDDEN",
                    format!("override `{target}`: the setting does not allow overrides"),
                ));
                continue;
            }
            if ov.value.type_of() != setting.value_type {
                out.push(record_error(
                    r,
                    "OVERRIDE_TYPE_MISMATCH",
                    format!(
                        "override `{target}`: value {} does not have the setting's type `{}`",
                        ov.value.display(),
                        type_name(setting)
                    ),
                ));
                continue;
            }
            let mut valid = true;
            if setting.r#override == OverrideMode::Stricter
                && let Some(dir) = setting.stricter
                && weakens(dir, &setting.value, &ov.value)
            {
                valid = false;
                out.push(record_error(
                    r,
                    "OVERRIDE_WEAKENS",
                    format!(
                        "override `{target}`: {} is weaker than the base value {} (stricter = {})",
                        ov.value.display(),
                        setting.value.display(),
                        stricter_name(dir)
                    ),
                ));
            }
            if !subsumes(&policy.meta.scope, &r.meta.scope, registry) {
                valid = false;
                out.push(record_error(
                    r,
                    "OVERRIDE_SCOPE_EXCEEDS",
                    format!(
                        "override `{target}`: this scope ({}) is not within the target policy's scope ({})",
                        scope_label(&r.meta.scope),
                        scope_label(&policy.meta.scope)
                    ),
                ));
            }
            if !setting.override_owners.is_empty()
                && !setting.override_owners.contains(&r.meta.owner)
            {
                valid = false;
                out.push(record_error(
                    r,
                    "OVERRIDE_NOT_AUTHORIZED",
                    format!(
                        "override `{target}`: owner `{}` is not in override_owners {}",
                        r.meta.owner,
                        list(&setting.override_owners)
                    ),
                ));
            }
            if valid && r.meta.status == Status::Accepted {
                effective
                    .entry(target.as_str())
                    .or_default()
                    .push(EffectiveOverride {
                        record: r,
                        value: &ov.value,
                    });
            }
        }
    }
    for (target, ovs) in &effective {
        for (i, a) in ovs.iter().enumerate() {
            for b in &ovs[i + 1..] {
                if a.record.meta.id == b.record.meta.id || same_value(a.value, b.value) {
                    continue;
                }
                let (sa, sb) = (&a.record.meta.scope, &b.record.meta.scope);
                // Ambiguous unless one scope is strictly more specific: incomparable scopes
                // (neither subsumes the other) and equivalent scopes (each subsumes the
                // other) both leave context without a unique most specific override.
                if overlap(sa, sb, registry)
                    && subsumes(sa, sb, registry) == subsumes(sb, sa, registry)
                {
                    out.push(record_error(
                        a.record,
                        "OVERRIDE_AMBIGUOUS",
                        format!(
                            "overrides of `{target}` by `{}` ({}) and `{}` ({}, {}) have overlapping scopes and neither is strictly more specific",
                            a.record.meta.id,
                            a.value.display(),
                            b.record.meta.id,
                            b.value.display(),
                            b.record.path
                        ),
                    ));
                }
            }
        }
    }
}

/// Is `new` weaker than `base` in the setting's stricter direction? Equal values are not
/// weaker. Direction/type combinations that do not fit are rejected by the parser.
fn weakens(dir: Stricter, base: &SettingValue, new: &SettingValue) -> bool {
    use SettingValue as V;
    match (dir, base, new) {
        (Stricter::Lower, V::Integer(b), V::Integer(n)) => n > b,
        (Stricter::Higher, V::Integer(b), V::Integer(n)) => n < b,
        (Stricter::True, V::Boolean(b), V::Boolean(n)) => *b && !*n,
        (Stricter::False, V::Boolean(b), V::Boolean(n)) => !*b && *n,
        // Stricter = more elements: the override must contain every base element.
        (Stricter::Superset, V::StringSet(b), V::StringSet(n)) => b.iter().any(|x| !n.contains(x)),
        // Stricter = fewer elements: the override may only contain base elements.
        (Stricter::Subset, V::StringSet(b), V::StringSet(n)) => n.iter().any(|x| !b.contains(x)),
        _ => false,
    }
}

/// Value equality with string sets compared as sets.
fn same_value(a: &SettingValue, b: &SettingValue) -> bool {
    match (a, b) {
        (SettingValue::StringSet(x), SettingValue::StringSet(y)) => {
            x.iter().collect::<BTreeSet<_>>() == y.iter().collect::<BTreeSet<_>>()
        }
        _ => a == b,
    }
}

fn type_name(s: &Setting) -> &'static str {
    match s.value_type {
        crate::model::SettingType::Integer => "integer",
        crate::model::SettingType::Boolean => "boolean",
        crate::model::SettingType::String => "string",
        crate::model::SettingType::StringSet => "string-set",
    }
}

fn stricter_name(d: Stricter) -> &'static str {
    match d {
        Stricter::Lower => "lower",
        Stricter::Higher => "higher",
        Stricter::True => "true",
        Stricter::False => "false",
        Stricter::Superset => "superset",
        Stricter::Subset => "subset",
    }
}

fn scope_label(s: &Scope) -> String {
    if s.product {
        return "product".into();
    }
    let mut parts = Vec::new();
    for (name, v) in [
        ("repos", &s.repos),
        ("modules", &s.modules),
        ("features", &s.features),
    ] {
        if !v.is_empty() {
            parts.push(format!("{name}={}", list(v)));
        }
    }
    parts.join(" ")
}

// ---------------------------------------------------------------------------------------
// Lints and base comparison
// ---------------------------------------------------------------------------------------

/// Non-fatal lints (`parse::lint_record`) for parsed records, located at their files.
pub fn lint_records(records: &[(String, &ParsedRecord)]) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = records
        .iter()
        .flat_map(|(path, parsed)| {
            lint_record(parsed)
                .into_iter()
                .map(|d| d.at_path(path.clone()))
        })
        .collect();
    normalize(&mut out);
    out
}

/// Compare the current records with those at a base revision (`kb validate --base`):
/// ids present at base must still exist (`ID_REMOVED`; historical ids stay addressable) and
/// keep their kind (`ID_KIND_CHANGED`; ids are never reused for other facts).
pub fn validate_against_base(current: &[MetaInput], base: &[MetaInput]) -> Vec<Diagnostic> {
    fn first_by_id(records: &[MetaInput]) -> BTreeMap<&str, &MetaInput> {
        let mut sorted: Vec<&MetaInput> = records.iter().collect();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        let mut map: BTreeMap<&str, &MetaInput> = BTreeMap::new();
        for r in sorted {
            map.entry(r.meta.id.as_str()).or_insert(r);
        }
        map
    }
    let now = first_by_id(current);
    let mut out = Vec::new();
    for (id, was) in first_by_id(base) {
        match now.get(id) {
            None => out.push(record_error(
                was,
                "ID_REMOVED",
                format!(
                    "{} `{id}` existed at the base revision but is gone; historical ids stay addressable: keep the record and set `status = \"deprecated\"` or supersede it",
                    was.meta.kind.as_str()
                ),
            )),
            Some(cur) if cur.meta.kind != was.meta.kind => out.push(record_error(
                cur,
                "ID_KIND_CHANGED",
                format!(
                    "id `{id}` was a {} at the base revision and is now a {}; ids must not be reused for other facts",
                    was.meta.kind.as_str(),
                    cur.meta.kind.as_str()
                ),
            )),
            Some(_) => {}
        }
    }
    normalize(&mut out);
    out
}

// ---------------------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------------------

/// Result of validating one profile. Serialized as the `result` of `kb validate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationReport {
    pub profile: String,
    /// Record files found under the knowledge roots.
    pub files: usize,
    /// Records that parsed successfully.
    pub records: usize,
    pub errors: usize,
    pub warnings: usize,
    /// Sorted, deduplicated findings.
    pub diagnostics: Vec<Diagnostic>,
}

impl ValidationReport {
    pub fn new(
        profile: impl Into<String>,
        files: usize,
        records: usize,
        diagnostics: Vec<Diagnostic>,
    ) -> ValidationReport {
        let mut report = ValidationReport {
            profile: profile.into(),
            files,
            records,
            errors: 0,
            warnings: 0,
            diagnostics: Vec::new(),
        };
        report.extend(diagnostics);
        report
    }

    /// Add findings (e.g. base comparison, templates, routing fixtures) and recount.
    pub fn extend(&mut self, more: impl IntoIterator<Item = Diagnostic>) {
        self.diagnostics.extend(more);
        normalize(&mut self.diagnostics);
        self.errors = self.count(Severity::Error);
        self.warnings = self.count(Severity::Warning);
    }

    fn count(&self, severity: Severity) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .count()
    }

    /// No errors (and, when `strict`, no warnings).
    pub fn is_ok(&self, strict: bool) -> bool {
        self.errors == 0 && (!strict || self.warnings == 0)
    }
}

/// Full validation of a loaded profile: load/parse diagnostics, cross-record rules and lints.
pub fn validate_corpus(corpus: &Corpus) -> ValidationReport {
    let metas = meta_inputs(corpus);
    let mut diagnostics = corpus.diagnostics.clone();
    diagnostics.extend(validate_metas(&corpus.config, &corpus.registry, &metas));
    let parsed: Vec<(String, &ParsedRecord)> = corpus
        .records()
        .map(|(entry, p)| (entry.path.clone(), p.as_ref()))
        .collect();
    diagnostics.extend(lint_records(&parsed));
    ValidationReport::new(
        corpus.location.profile.as_str(),
        corpus.entries.len(),
        metas.len(),
        diagnostics,
    )
}

/// Render a report for the given output format (JSON is the serialized report).
pub fn render_report(report: &ValidationReport, format: Format) -> String {
    match format {
        Format::Json => {
            let mut s = serde_json::to_string_pretty(report).unwrap_or_default();
            s.push('\n');
            s
        }
        Format::Compact => {
            let mut s = format!(
                "validate {}: records: {}, files: {}, errors: {}, warnings: {}\n",
                report.profile, report.records, report.files, report.errors, report.warnings
            );
            for d in &report.diagnostics {
                s.push_str(&format!("{} {}", severity_name(d.severity), d.code));
                if let Some(p) = &d.path {
                    s.push_str(&format!(" {p}"));
                    if let Some(l) = d.line {
                        s.push_str(&format!(":{l}"));
                    }
                }
                if let Some(r) = &d.record {
                    s.push_str(&format!(" [{r}]"));
                }
                s.push_str(&format!(": {}\n", d.message));
            }
            s
        }
        Format::Human => {
            let mut s = format!(
                "Validation of profile `{}`\n  record files: {}\n  parsed records: {}\n  errors: {}\n  warnings: {}\n",
                report.profile, report.files, report.records, report.errors, report.warnings
            );
            for d in &report.diagnostics {
                s.push_str(&format!(
                    "\n{}[{}]: {}\n",
                    severity_name(d.severity),
                    d.code,
                    d.message
                ));
                if let Some(p) = &d.path {
                    match d.line {
                        Some(l) => s.push_str(&format!("  --> {p}:{l}\n")),
                        None => s.push_str(&format!("  --> {p}\n")),
                    }
                }
                if let Some(r) = &d.record {
                    s.push_str(&format!("  record: {r}\n"));
                }
            }
            s.push_str(if report.errors == 0 {
                "\nResult: valid\n"
            } else {
                "\nResult: INVALID\n"
            });
            s
        }
    }
}

fn severity_name(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

// ---------------------------------------------------------------------------------------
// Upstream templates
// ---------------------------------------------------------------------------------------

/// KB-root-relative directory of shipped templates.
pub const TEMPLATES_DIR: &str = "core/templates";

/// Validate shipped templates (`kb validate --templates`, an upstream check):
///
/// * every `core/templates/records/**/*.md` (except `README.md`) must be a valid record and
///   every record kind needs at least one template;
/// * files under `core/templates/project/` that are plain TOML (or `.md` records under its
///   `knowledge/`) are parsed strictly with the type of their destination path; `*.tmpl`
///   files containing `{{` placeholders are rendered by `kb init` and skipped here;
/// * every example under `core/templates/examples/<name>/project` is loaded and validated as
///   a full corpus (parse, cross-record rules, lints).
///
/// Diagnostic paths are KB-root-relative.
pub fn validate_templates(kb_root: &Path) -> Result<Vec<Diagnostic>> {
    let mut out = Vec::new();
    if !is_real_dir(&kb_root.join(TEMPLATES_DIR)) {
        out.push(
            Diagnostic::error(
                "TEMPLATES_MISSING",
                "the upstream template directory is missing",
            )
            .at_path(TEMPLATES_DIR),
        );
        return Ok(out);
    }
    let source = WorkingTreeSource::new(kb_root);
    check_record_templates(&source, &mut out)?;
    check_project_templates(&source, &mut out)?;
    check_examples(kb_root, &mut out)?;
    normalize(&mut out);
    Ok(out)
}

fn is_real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// List and read all files under a KB-root-relative directory; listing issues become errors.
fn read_tree(
    source: &WorkingTreeSource,
    dir: &str,
    out: &mut Vec<Diagnostic>,
) -> Result<Vec<(String, Vec<u8>)>> {
    let (entries, issues) = source.list(&[dir.to_string()])?;
    out.extend(
        issues
            .into_iter()
            .map(|i| Diagnostic::error(i.code, i.message).at_path(i.path)),
    );
    let contents = source.read(&entries)?;
    Ok(entries.into_iter().map(|e| e.path).zip(contents).collect())
}

fn check_record_templates(source: &WorkingTreeSource, out: &mut Vec<Diagnostic>) -> Result<()> {
    let dir = format!("{TEMPLATES_DIR}/records");
    if !is_real_dir(&source.root().join(&dir)) {
        out.push(
            Diagnostic::error("TEMPLATES_MISSING", "record templates directory is missing")
                .at_path(dir),
        );
        return Ok(());
    }
    let mut kinds = BTreeSet::new();
    for (path, bytes) in read_tree(source, &dir, out)? {
        if !is_record_path(&path) {
            continue;
        }
        match parse_record(&path, &bytes) {
            Ok(parsed) => {
                kinds.insert(parsed.record.kind());
                out.extend(lint_records(&[(path, &parsed)]));
            }
            Err(ds) => out.extend(ds),
        }
    }
    for k in Kind::ALL {
        if !kinds.contains(&k) {
            out.push(
                Diagnostic::error(
                    "TEMPLATE_KIND_MISSING",
                    format!("no valid record template of kind `{}`", k.as_str()),
                )
                .at_path(dir.clone()),
            );
        }
    }
    Ok(())
}

fn check_project_templates(source: &WorkingTreeSource, out: &mut Vec<Diagnostic>) -> Result<()> {
    let dir = format!("{TEMPLATES_DIR}/project");
    for (path, bytes) in read_tree(source, &dir, out)? {
        let rel = path.strip_prefix(&format!("{dir}/")).unwrap_or(&path);
        let (dest, is_tmpl) = match rel.strip_suffix(".tmpl") {
            Some(d) => (d, true),
            None => (rel, false),
        };
        let text = match std::str::from_utf8(&bytes) {
            Ok(t) => t,
            Err(_) => {
                out.push(
                    Diagnostic::error("TEMPLATE_INVALID", "template is not UTF-8").at_path(path),
                );
                continue;
            }
        };
        if is_tmpl && text.contains("{{") {
            continue;
        }
        if let Some(d) = check_profile_toml(&path, dest, text) {
            out.push(d);
        }
        if dest.starts_with("knowledge/") && is_record_path(dest) {
            match parse_record(&path, &bytes) {
                Ok(parsed) => out.extend(lint_records(&[(path.clone(), &parsed)])),
                Err(ds) => out.extend(ds),
            }
        }
    }
    Ok(())
}

/// Strictly parse one plain TOML profile file (or template) by its profile-relative path.
fn check_profile_toml(path: &str, dest: &str, text: &str) -> Option<Diagnostic> {
    fn typed<T: serde::de::DeserializeOwned>(path: &str, text: &str) -> Option<Diagnostic> {
        toml::from_str::<T>(text)
            .err()
            .map(|e| Diagnostic::error("TEMPLATE_INVALID", e.to_string()).at_path(path))
    }
    fn registry<T: serde::de::DeserializeOwned>(path: &str, text: &str) -> Option<Diagnostic> {
        parse_registry_file::<T>(path, text).err()
    }
    if !dest.ends_with(".toml") {
        return None;
    }
    match dest {
        "project.toml" => parse_config(path, text.as_bytes())
            .err()
            .map(|e: KbError| Diagnostic::error(e.code.as_str(), e.message).at_path(path)),
        "registry/owners.toml" => registry::<OwnersFile>(path, text),
        "registry/repos.toml" => registry::<ReposFile>(path, text),
        "registry/modules.toml" => registry::<ModulesFile>(path, text),
        "registry/features.toml" => registry::<FeaturesFile>(path, text),
        "registry/concepts.toml" => registry::<ConceptsFile>(path, text),
        "skill-config/skill.toml" => typed::<SkillConfig>(path, text),
        "upstream.toml" => typed::<UpstreamConfig>(path, text),
        d if d.starts_with("routing-tests/") => typed::<RoutingTestFile>(path, text),
        _ => typed::<toml::Table>(path, text),
    }
}

fn check_examples(kb_root: &Path, out: &mut Vec<Diagnostic>) -> Result<()> {
    let rel = format!("{TEMPLATES_DIR}/examples");
    let dir = kb_root.join(&rel);
    if !is_real_dir(&dir) {
        return Ok(());
    }
    let mut names: Vec<(String, bool)> = fs::read_dir(&dir)
        .map_err(|e| KbError::io(dir.display(), e))?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let t = e.file_type().ok()?;
            let name = e.file_name().to_str()?.to_string();
            (t.is_dir() || t.is_symlink()).then_some((name, t.is_symlink()))
        })
        .collect();
    names.sort();
    for (name, symlink) in names {
        let example_rel = format!("{rel}/{name}");
        if symlink {
            out.push(
                Diagnostic::error(
                    "SYMLINK_NOT_ALLOWED",
                    "symlinks are not followed inside templates",
                )
                .at_path(example_rel),
            );
            continue;
        }
        let example = dir.join(&name);
        if !is_real_dir(&example.join("project")) {
            out.push(
                Diagnostic::warning(
                    "TEMPLATE_EXAMPLE_EMPTY",
                    "example directory has no `project/` profile",
                )
                .at_path(example_rel),
            );
            continue;
        }
        // Diagnostics are collected relative to the example root, then prefixed.
        let mut local = Vec::new();
        let loc = ProfileLocation::for_profile(Profile::Project);
        let source = WorkingTreeSource::new(&example);
        match load_corpus(&source, &loc) {
            Ok(corpus) => local.extend(validate_corpus(&corpus).diagnostics),
            Err(e) => local
                .push(Diagnostic::error(e.code.as_str(), e.message).at_path(loc.config.clone())),
        }
        // Profile files the corpus loader does not read (skill config, upstream base,
        // routing fixtures) are parsed strictly as well.
        let dir_prefix = format!("{}/", loc.dir);
        for (path, bytes) in read_tree(&source, &loc.dir, &mut local)? {
            let dest = path.strip_prefix(&dir_prefix).unwrap_or(&path);
            if dest == "project.toml" || dest.starts_with("registry/") {
                continue;
            }
            match std::str::from_utf8(&bytes) {
                Ok(text) => local.extend(check_profile_toml(&path, dest, text)),
                Err(_) => local
                    .push(Diagnostic::error("TEMPLATE_INVALID", "file is not UTF-8").at_path(path)),
            }
        }
        out.extend(local.into_iter().map(|mut d| {
            let p = d.path.take().unwrap_or_else(|| loc.dir.clone());
            d.path = Some(format!("{example_rel}/{p}"));
            d
        }));
    }
    Ok(())
}
