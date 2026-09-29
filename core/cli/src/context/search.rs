//! `kb search`: ranked lookup of records by id, concept/alias, title and full text.
//!
//! Search is **not** a context assembly: it does not select mandatory obligations, follow
//! `requires`, apply scope or report completeness. Every rendering says so.
//!
//! Points: id mention 1000; concept match (via registry aliases in the query) 200 each, at
//! most 2; record alias match 150; title token overlap 20 each, at most 3; full-text rank
//! position p → max(100 − 5p, 5). Order: points descending, then id. All statuses are
//! searchable and labeled.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::error::{KbError, Result};
use crate::knowledge::{KnowledgeView, MetaEntry, Origin, ProposalChange};
use crate::model::{Kind, RecordMeta, Status};
use crate::normalize::{self, AliasPattern};
use crate::output::Format;

use super::present::safe_text;
use super::rank::{
    ALIAS_MATCH, CONCEPT_MATCH, CONCEPT_MATCH_MAX, FTS_BASE, FTS_STEP, FTS_TOP, ID_MENTION,
    TITLE_TOKEN, fts_terms, mentioned_ids, title_overlap,
};

/// Maximum query size in bytes.
pub const MAX_QUERY_BYTES: usize = 8 * 1024;
/// Maximum `limit`.
pub const MAX_LIMIT: usize = 200;
/// Printed with every search result.
pub const NOTE: &str = "search is not a context assembly: mandatory obligations, required \
     dependencies and completeness are not checked; use `kb context` before planning or \
     changing code";

/// One search hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub id: String,
    pub kind: Kind,
    pub status: Status,
    pub title: String,
    pub origin: Origin,
    pub path: String,
    pub score: i64,
    /// What matched, e.g. `concept auth-token`, `full-text rank 2`.
    pub matched: Vec<String>,
}

/// Ranked search hits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub query: String,
    pub kinds: Vec<Kind>,
    pub limit: usize,
    /// Matching records before the limit was applied.
    pub total: usize,
    pub hits: Vec<SearchHit>,
}

type Acc = BTreeMap<(String, Origin), (MetaEntry, i64, Vec<String>)>;

fn add(acc: &mut Acc, e: &MetaEntry, points: i64, what: String) {
    let slot = acc
        .entry((e.meta.id.clone(), e.origin))
        .or_insert_with(|| (e.clone(), 0, Vec::new()));
    slot.1 += points;
    slot.2.push(what);
}

/// Search records. Errors: `INVALID_INPUT` for an empty/oversized query or a bad limit.
pub fn search(
    view: &dyn KnowledgeView,
    query: &str,
    kinds: &[Kind],
    limit: usize,
    include_proposals: bool,
) -> Result<SearchResult> {
    if query.len() > MAX_QUERY_BYTES {
        return Err(KbError::invalid_input(format!(
            "the query is {} bytes; the limit is {MAX_QUERY_BYTES}",
            query.len()
        )));
    }
    if limit == 0 || limit > MAX_LIMIT {
        return Err(KbError::invalid_input(format!(
            "--limit must be between 1 and {MAX_LIMIT}"
        )));
    }
    let tokens = normalize::tokens(query);
    let ids = mentioned_ids(query);
    if tokens.is_empty() && ids.is_empty() {
        return Err(KbError::invalid_input("the query has no searchable terms"));
    }
    let concepts: Vec<String> = view
        .registry()
        .concept_alias_matches(&tokens)
        .into_iter()
        .map(|(_, c, _)| c)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut acc = Acc::new();
    if !ids.is_empty() {
        for e in view.metas_by_ids(&ids, Origin::Accepted)? {
            add(&mut acc, &e, ID_MENTION, "id".into());
        }
    }
    for e in view.term_candidates(&concepts, &tokens)? {
        term_points(&mut acc, &e, &concepts, &tokens);
    }
    let terms = fts_terms(&tokens);
    if !terms.is_empty() {
        let ranked = view.fulltext(&terms, limit.max(FTS_TOP))?;
        let metas: BTreeMap<String, MetaEntry> = view
            .metas_by_ids(&ranked, Origin::Accepted)?
            .into_iter()
            .map(|e| (e.meta.id.clone(), e))
            .collect();
        for (p, id) in ranked.iter().enumerate() {
            if let Some(e) = metas.get(id) {
                let points = (FTS_BASE - FTS_STEP * p as i64).max(FTS_STEP);
                add(&mut acc, e, points, format!("full-text rank {}", p + 1));
            }
        }
    }
    if include_proposals {
        for p in view.proposals()? {
            let (Some(meta), ProposalChange::New { .. } | ProposalChange::Modifies { .. }) =
                (&p.meta, &p.change)
            else {
                continue;
            };
            let e = MetaEntry {
                path: p.path.clone(),
                origin: Origin::Proposal,
                meta: meta.clone(),
            };
            if ids.contains(&meta.id) {
                add(&mut acc, &e, ID_MENTION, "id".into());
            }
            term_points(&mut acc, &e, &concepts, &tokens);
            let overlap = title_overlap(&meta.title, &tokens);
            if !overlap.is_empty() && !acc.contains_key(&(meta.id.clone(), Origin::Proposal)) {
                add(&mut acc, &e, 0, "local proposal".into());
            }
        }
    }

    let mut hits: Vec<SearchHit> = acc
        .into_values()
        .filter(|(e, _, _)| kinds.is_empty() || kinds.contains(&e.meta.kind))
        .map(|(e, mut score, mut matched)| {
            let overlap = title_overlap(&e.meta.title, &tokens);
            if !overlap.is_empty() {
                score += TITLE_TOKEN * overlap.len() as i64;
                matched.push(format!("title terms: {}", overlap.join(", ")));
            }
            hit(&e, score, matched)
        })
        .filter(|h| h.score > 0)
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.origin.cmp(&b.origin))
    });
    let total = hits.len();
    hits.truncate(limit);
    let mut kinds = kinds.to_vec();
    kinds.sort();
    kinds.dedup();
    Ok(SearchResult {
        query: query.to_string(),
        kinds,
        limit,
        total,
        hits,
    })
}

fn term_points(acc: &mut Acc, e: &MetaEntry, concepts: &[String], tokens: &[String]) {
    let m: &RecordMeta = &e.meta;
    let hits: BTreeSet<&String> = m
        .selectors
        .concepts
        .iter()
        .filter(|c| concepts.contains(c))
        .collect();
    for c in hits.into_iter().take(CONCEPT_MATCH_MAX) {
        add(acc, e, CONCEPT_MATCH, format!("concept {c}"));
    }
    if let Some(a) = m
        .selectors
        .aliases
        .iter()
        .find(|a| AliasPattern::compile(a).is_some_and(|p| p.matches(tokens)))
    {
        add(acc, e, ALIAS_MATCH, format!("alias `{a}`"));
    }
}

fn hit(e: &MetaEntry, score: i64, matched: Vec<String>) -> SearchHit {
    SearchHit {
        id: e.meta.id.clone(),
        kind: e.meta.kind,
        status: e.meta.status,
        title: e.meta.title.clone(),
        origin: e.origin,
        path: e.path.clone(),
        score,
        matched,
    }
}

/// Render compact or human text (JSON: pretty-printed [`to_json`]).
pub fn render(r: &SearchResult, format: Format) -> String {
    if format == Format::Json {
        let mut s = serde_json::to_string_pretty(&to_json(r)).unwrap_or_default();
        s.push('\n');
        return s;
    }
    let human = format == Format::Human;
    let mut o = format!(
        "# kb search \"{}\": {} of {} hit(s)\n",
        r.query,
        r.hits.len(),
        r.total
    );
    o.push_str(&format!("note: {NOTE}\n"));
    if r.hits.is_empty() {
        o.push_str("no matching records\n");
    }
    for h in &r.hits {
        let origin = match h.origin {
            Origin::Accepted => "",
            Origin::Proposal => ", proposal",
        };
        o.push_str(&format!(
            "{} {} ({}, {}{origin}): {}\n",
            h.score,
            h.id,
            h.kind.as_str(),
            h.status.as_str(),
            h.title
        ));
        o.push_str(&format!("  matched: {}\n", h.matched.join(", ")));
        if human {
            o.push_str(&format!("  source: {}\n", h.path));
        }
    }
    safe_text(o)
}

/// Deterministic JSON result.
pub fn to_json(r: &SearchResult) -> Value {
    json!({
        "note": NOTE,
        "context_assembly": false,
        "query": r.query,
        "kinds": r.kinds,
        "limit": r.limit,
        "total": r.total,
        "hits": r.hits.iter().map(|h| json!({
            "id": h.id,
            "kind": h.kind,
            "status": h.status,
            "title": h.title,
            "origin": h.origin,
            "path": h.path,
            "score": h.score,
            "matched": h.matched,
        })).collect::<Vec<_>>(),
    })
}
