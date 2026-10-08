//! Supplementary candidates and deterministic integer ranking
//! (docs/architecture.md §5 steps 5–6).
//!
//! Candidate sources: explicit id mentions in the task text, path selector matches
//! (re-checked with [`RepoGlob`]), concept matches, record alias matches, feature records of
//! task features, full-text search, `rationale` of mandatory records (scope ignored) and one
//! hop of `related` from mandatory records. Title term matches are found through the
//! full-text index (which covers titles); title term overlap, intent selectors, the kind
//! prior and scope applicability only add points to candidates found by a source.
//!
//! Points (integers, summed):
//!
//! | signal | points |
//! |---|---|
//! | id mention | 1000 |
//! | path selector match | 300 + min(literal prefix length, 100) (best glob) |
//! | concept match | 200 each, at most 2 |
//! | record alias match | 150 |
//! | rationale of a mandatory record | 250 |
//! | feature record for a task feature | 250 |
//! | related one hop | 80 |
//! | intent selector match | 40 |
//! | kind prior by intent | 0–40 ([`kind_prior`]) |
//! | scope Applies (non-product) | 50 |
//! | title token overlap | 20 each, at most 3 (tokens of ≥ 3 characters, not stopwords) |
//! | full-text rank position p (0-based, top 20) | 100 − 5p |
//! | best record for a candidate concept of an ambiguous phrase | 200 |
//!
//! Minimum score 80; order: score descending, then id ascending. Ambiguity candidates are
//! offered even below the minimum score or above the `max_supplementary` cap.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::error::Result;
use crate::glob::RepoGlob;
use crate::knowledge::{KnowledgeView, MetaEntry, Origin, TaskPath};
use crate::model::ids::check_record_id;
use crate::model::{Intent, Kind, RecordMeta, Status};
use crate::normalize::{self, AliasPattern};

use super::applicability::{self, Applicability, Verdict};
use super::task::{Ambiguity, ResolvedPath, TaskScope};
use super::{Excluded, ExcludedReason};

pub const MIN_SCORE: i64 = 80;
pub const ID_MENTION: i64 = 1000;
pub const PATH_SELECTOR: i64 = 300;
pub const PATH_PREFIX_MAX: i64 = 100;
pub const CONCEPT_MATCH: i64 = 200;
pub const CONCEPT_MATCH_MAX: usize = 2;
pub const ALIAS_MATCH: i64 = 150;
pub const RATIONALE: i64 = 250;
pub const FEATURE_RECORD: i64 = 250;
pub const RELATED: i64 = 80;
pub const INTENT_SELECTOR: i64 = 40;
pub const SCOPE_APPLIES: i64 = 50;
pub const TITLE_TOKEN: i64 = 20;
pub const TITLE_TOKEN_MAX: usize = 3;
pub const FTS_TOP: usize = 20;
pub const FTS_BASE: i64 = 100;
pub const FTS_STEP: i64 = 5;
pub const AMBIGUITY_CANDIDATE: i64 = 200;
/// Tokens shorter than this (in characters), and [`STOPWORDS`], are ignored for title overlap
/// and full-text terms.
pub const MIN_SIGNIFICANT_CHARS: usize = 3;
/// Maximum number of full-text terms taken from the task text.
pub const MAX_FTS_TERMS: usize = 32;

/// Kind prior by intent (points 0–40). Columns: implement, refactor, debug, review, explain.
///
/// Rationale: implementing leans on features and rules; refactoring on invariants and past
/// decisions; debugging on procedures and known gaps; reviewing on rules and invariants;
/// explaining on features, decisions and references.
#[rustfmt::skip]
const KIND_PRIOR: [(Kind, [i64; 5]); 8] = [
    //                impl  refac debug review explain
    (Kind::Policy,    [30,   30,   20,   40,    20]),
    (Kind::Feature,   [40,   30,   30,   30,    40]),
    (Kind::Invariant, [30,   40,   30,   40,    20]),
    (Kind::Contract,  [30,   30,   30,   30,    20]),
    (Kind::Decision,  [10,   30,   10,   30,    40]),
    (Kind::Procedure, [20,   10,   40,   10,    10]),
    (Kind::Reference, [10,   10,   20,   10,    30]),
    (Kind::Gap,       [20,   20,   30,   30,    20]),
];

/// Kind prior for a record kind under an intent (see [`KIND_PRIOR`]).
pub fn kind_prior(kind: Kind, intent: Intent) -> i64 {
    let col = match intent {
        Intent::Implement => 0,
        Intent::Refactor => 1,
        Intent::Debug => 2,
        Intent::Diagnose => 2,
        Intent::Review => 3,
        Intent::Explain => 4,
    };
    KIND_PRIOR
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, row)| row[col])
        .unwrap_or(0)
}

/// What contributed points to a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SignalKind {
    IdMention,
    PathSelector,
    Concept,
    Alias,
    Rationale,
    FeatureRecord,
    Related,
    AmbiguityCandidate,
    IntentSelector,
    KindPrior,
    ScopeApplies,
    TitleTokens,
    Fulltext,
}

impl SignalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SignalKind::IdMention => "id-mention",
            SignalKind::PathSelector => "path-selector",
            SignalKind::Concept => "concept",
            SignalKind::Alias => "alias",
            SignalKind::Rationale => "rationale",
            SignalKind::FeatureRecord => "feature-record",
            SignalKind::Related => "related",
            SignalKind::AmbiguityCandidate => "ambiguity-candidate",
            SignalKind::IntentSelector => "intent-selector",
            SignalKind::KindPrior => "kind-prior",
            SignalKind::ScopeApplies => "scope-applies",
            SignalKind::TitleTokens => "title-tokens",
            SignalKind::Fulltext => "full-text",
        }
    }
}

/// One scoring contribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub kind: SignalKind,
    pub points: i64,
    pub detail: String,
}

impl Signal {
    fn new(kind: SignalKind, points: i64, detail: impl Into<String>) -> Signal {
        Signal {
            kind,
            points,
            detail: detail.into(),
        }
    }
}

/// A scored supplementary candidate.
#[derive(Debug, Clone)]
pub struct Ranked {
    pub entry: MetaEntry,
    pub signals: Vec<Signal>,
    pub score: i64,
    pub labels: Vec<String>,
    /// Offered for an ambiguity: bypasses the minimum score and the cap.
    pub exempt: bool,
}

pub(super) struct RankInput<'a> {
    pub intent: Intent,
    pub task_text: Option<&'a str>,
    pub task: &'a TaskScope,
    /// Mandatory tier (applicable obligations and required dependencies) by id.
    pub mandatory: &'a BTreeMap<String, Arc<RecordMeta>>,
    pub max_supplementary: usize,
}

pub(super) struct Supplementary {
    /// Selected candidates in rank order.
    pub selected: Vec<Ranked>,
    pub excluded: Vec<Excluded>,
}

type Accumulator = BTreeMap<String, (MetaEntry, Vec<Signal>)>;

fn add(acc: &mut Accumulator, e: &MetaEntry, s: Signal) {
    acc.entry(e.meta.id.clone())
        .or_insert_with(|| (e.clone(), Vec::new()))
        .1
        .push(s);
}

/// Collect, score, order and cap supplementary candidates. Offers the best record per
/// candidate concept of each ambiguity (recorded in `ambiguities[].offered`).
pub(super) fn supplementary(
    view: &dyn KnowledgeView,
    input: &RankInput,
    ambiguities: &mut [Ambiguity],
) -> Result<Supplementary> {
    let task = input.task;
    let mut acc = Accumulator::new();

    let ids = mentioned_ids(input.task_text.unwrap_or(""));
    if !ids.is_empty() {
        for e in view.metas_by_ids(&ids, Origin::Accepted)? {
            let s = Signal::new(
                SignalKind::IdMention,
                ID_MENTION,
                "id mentioned in the task",
            );
            add(&mut acc, &e, s);
        }
    }

    if !task.paths.is_empty() {
        let task_paths: Vec<TaskPath> = task
            .paths
            .iter()
            .map(|p| TaskPath {
                repo: p.repo.clone(),
                path: p.path.clone(),
            })
            .collect();
        for e in view.path_candidates(&task_paths)? {
            if let Some((glob, prefix)) = best_path_match(&e.meta.selectors.paths, &task.paths) {
                let points = PATH_SELECTOR + prefix.min(PATH_PREFIX_MAX);
                let s = Signal::new(
                    SignalKind::PathSelector,
                    points,
                    format!("path selector `{glob}`"),
                );
                add(&mut acc, &e, s);
            }
        }
    }

    let concepts: Vec<String> = task.concepts.keys().cloned().collect();
    if !concepts.is_empty() || !task.tokens.is_empty() {
        for e in view.term_candidates(&concepts, &task.tokens)? {
            let hits: BTreeSet<&String> = e
                .meta
                .selectors
                .concepts
                .iter()
                .filter(|c| task.concepts.contains_key(c.as_str()))
                .collect();
            for c in hits.into_iter().take(CONCEPT_MATCH_MAX) {
                let s = Signal::new(SignalKind::Concept, CONCEPT_MATCH, format!("concept `{c}`"));
                add(&mut acc, &e, s);
            }
            let alias = e
                .meta
                .selectors
                .aliases
                .iter()
                .find(|a| AliasPattern::compile(a).is_some_and(|p| p.matches(&task.tokens)));
            if let Some(a) = alias {
                let s = Signal::new(SignalKind::Alias, ALIAS_MATCH, format!("alias `{a}`"));
                add(&mut acc, &e, s);
            }
        }
    }

    if let Some(features) = task.features.known()
        && !features.is_empty()
    {
        for e in view.metas_by_kind(&[Kind::Feature], Origin::Accepted)? {
            if let Some(f) = &e.meta.feature
                && features.contains(f)
            {
                let s = Signal::new(
                    SignalKind::FeatureRecord,
                    FEATURE_RECORD,
                    format!("feature record for task feature `{f}`"),
                );
                add(&mut acc, &e, s);
            }
        }
    }

    let terms = fts_terms(&task.tokens);
    if !terms.is_empty() {
        let ids = view.fulltext(&terms, FTS_TOP)?;
        let metas: BTreeMap<String, MetaEntry> = view
            .metas_by_ids(&ids, Origin::Accepted)?
            .into_iter()
            .map(|e| (e.meta.id.clone(), e))
            .collect();
        for (p, id) in ids.iter().take(FTS_TOP).enumerate() {
            if let Some(e) = metas.get(id) {
                let points = FTS_BASE - FTS_STEP * p as i64;
                let s = Signal::new(
                    SignalKind::Fulltext,
                    points,
                    format!("full-text rank {}", p + 1),
                );
                add(&mut acc, e, s);
            }
        }
    }

    let mut rationale: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut related: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (id, m) in input.mandatory {
        for t in &m.links.rationale {
            rationale.entry(t.clone()).or_default().insert(id.clone());
        }
        for t in &m.links.related {
            related.entry(t.clone()).or_default().insert(id.clone());
        }
    }
    let targets: Vec<String> = rationale
        .keys()
        .chain(related.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !targets.is_empty() {
        for e in view.metas_by_ids(&targets, Origin::Accepted)? {
            if let Some(src) = rationale.get(&e.meta.id) {
                let s = Signal::new(
                    SignalKind::Rationale,
                    RATIONALE,
                    format!("rationale of {}", join(src)),
                );
                add(&mut acc, &e, s);
            }
            if let Some(src) = related.get(&e.meta.id) {
                let s = Signal::new(
                    SignalKind::Related,
                    RELATED,
                    format!("related to {}", join(src)),
                );
                add(&mut acc, &e, s);
            }
        }
    }

    let mut scored: BTreeMap<String, Ranked> = BTreeMap::new();
    let mut excluded = Vec::new();
    for (id, (entry, mut signals)) in acc {
        if input.mandatory.contains_key(&id) {
            continue;
        }
        if entry.meta.status != Status::Accepted {
            excluded.push(Excluded::new(
                &id,
                ExcludedReason::NotAccepted,
                None,
                format!("status {}", entry.meta.status.as_str()),
            ));
            continue;
        }
        let app = applicability::evaluate(&entry.meta, task);
        let is_rationale = signals.iter().any(|s| s.kind == SignalKind::Rationale);
        if app.verdict() == Verdict::NotApplicable && !is_rationale {
            excluded.push(Excluded::new(
                &id,
                ExcludedReason::NotApplicable,
                None,
                app.describe(),
            ));
            continue;
        }
        signals.extend(bonuses(&entry.meta, &app, input.intent, &task.tokens));
        let ranked = new_ranked(entry, signals, &app);
        scored.insert(id, ranked);
    }

    offer_ambiguity_candidates(view, input, ambiguities, &mut scored)?;

    let mut ranked: Vec<Ranked> = scored.into_values().collect();
    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.entry.meta.id.cmp(&b.entry.meta.id))
    });
    let mut selected = Vec::new();
    let mut normal = 0usize;
    for r in ranked {
        if r.exempt {
            selected.push(r);
        } else if r.score < MIN_SCORE {
            excluded.push(Excluded::new(
                &r.entry.meta.id,
                ExcludedReason::BelowMinScore,
                Some(r.score),
                format!("score {} < {MIN_SCORE}", r.score),
            ));
        } else if normal >= input.max_supplementary {
            excluded.push(Excluded::new(
                &r.entry.meta.id,
                ExcludedReason::MaxSupplementary,
                Some(r.score),
                format!("max_supplementary = {}", input.max_supplementary),
            ));
        } else {
            normal += 1;
            selected.push(r);
        }
    }
    Ok(Supplementary { selected, excluded })
}

fn new_ranked(entry: MetaEntry, signals: Vec<Signal>, app: &Applicability) -> Ranked {
    let mut labels = Vec::new();
    match app.verdict() {
        Verdict::Undetermined => labels.push("applicability-undetermined".to_string()),
        Verdict::NotApplicable => labels.push("outside-task-scope".to_string()),
        Verdict::Applies => {}
    }
    Ranked {
        score: signals.iter().map(|s| s.points).sum(),
        entry,
        signals,
        labels,
        exempt: false,
    }
}

/// For each candidate concept of each ambiguity, offer the best-scoring applicable record
/// carrying that concept (ties by id).
fn offer_ambiguity_candidates(
    view: &dyn KnowledgeView,
    input: &RankInput,
    ambiguities: &mut [Ambiguity],
    scored: &mut BTreeMap<String, Ranked>,
) -> Result<()> {
    for amb in ambiguities.iter_mut() {
        for concept in amb.candidates.clone() {
            let mut best: Option<(i64, Ranked)> = None;
            for e in view.term_candidates(std::slice::from_ref(&concept), &[])? {
                let m = &e.meta;
                if !m.selectors.concepts.contains(&concept)
                    || m.status != Status::Accepted
                    || input.mandatory.contains_key(&m.id)
                {
                    continue;
                }
                let candidate = match scored.get(&m.id) {
                    Some(r) => r.clone(),
                    None => {
                        let app = applicability::evaluate(m, input.task);
                        if app.verdict() == Verdict::NotApplicable {
                            continue;
                        }
                        let signals = bonuses(m, &app, input.intent, &input.task.tokens);
                        new_ranked(e.clone(), signals, &app)
                    }
                };
                let better = best.as_ref().is_none_or(|(s, b)| {
                    candidate.score > *s
                        || (candidate.score == *s && candidate.entry.meta.id < b.entry.meta.id)
                });
                if better {
                    best = Some((candidate.score, candidate));
                }
            }
            if let Some((_, candidate)) = best {
                let id = candidate.entry.meta.id.clone();
                let r = scored.entry(id.clone()).or_insert(candidate);
                r.signals.push(Signal::new(
                    SignalKind::AmbiguityCandidate,
                    AMBIGUITY_CANDIDATE,
                    format!(
                        "best record for concept `{concept}` of ambiguous `{}`",
                        amb.phrase
                    ),
                ));
                r.score += AMBIGUITY_CANDIDATE;
                r.exempt = true;
                r.labels.push(format!("ambiguity-candidate:{concept}"));
                amb.offered.push((concept, id));
            }
        }
    }
    Ok(())
}

/// Bonus signals for a candidate found by a source.
fn bonuses(
    meta: &RecordMeta,
    app: &Applicability,
    intent: Intent,
    tokens: &[String],
) -> Vec<Signal> {
    let mut v = Vec::new();
    if meta.selectors.intents.contains(&intent) {
        v.push(Signal::new(
            SignalKind::IntentSelector,
            INTENT_SELECTOR,
            format!("intent `{}` selector", intent.as_str()),
        ));
    }
    let prior = kind_prior(meta.kind, intent);
    if prior > 0 {
        v.push(Signal::new(
            SignalKind::KindPrior,
            prior,
            format!("{} for {}", meta.kind.as_str(), intent.as_str()),
        ));
    }
    if app.verdict() == Verdict::Applies && !meta.scope.product {
        v.push(Signal::new(
            SignalKind::ScopeApplies,
            SCOPE_APPLIES,
            "scope applies",
        ));
    }
    let overlap = title_overlap(&meta.title, tokens);
    if !overlap.is_empty() {
        v.push(Signal::new(
            SignalKind::TitleTokens,
            TITLE_TOKEN * overlap.len() as i64,
            format!("title terms: {}", overlap.join(", ")),
        ));
    }
    v
}

/// Common English and Russian function words (normalized) ignored for title overlap and
/// full-text terms.
pub const STOPWORDS: [&str; 44] = [
    "and",
    "are",
    "but",
    "can",
    "for",
    "from",
    "has",
    "have",
    "how",
    "into",
    "its",
    "not",
    "our",
    "that",
    "the",
    "their",
    "then",
    "there",
    "this",
    "was",
    "were",
    "what",
    "when",
    "where",
    "which",
    "who",
    "why",
    "will",
    "with",
    "you",
    "без",
    "был",
    "где",
    "для",
    "его",
    "если",
    "как",
    "или",
    "при",
    "про",
    "так",
    "что",
    "чтобы",
    "это",
];

fn significant(t: &str) -> bool {
    t.chars().count() >= MIN_SIGNIFICANT_CHARS && !STOPWORDS.contains(&t)
}

/// Distinct significant title tokens that occur in the task (at most 3, title order).
pub(super) fn title_overlap(title: &str, tokens: &[String]) -> Vec<String> {
    let task: BTreeSet<&str> = tokens
        .iter()
        .filter(|t| significant(t))
        .map(String::as_str)
        .collect();
    let mut seen = BTreeSet::new();
    normalize::tokens(title)
        .into_iter()
        .filter(|t| significant(t) && task.contains(t.as_str()) && seen.insert(t.clone()))
        .take(TITLE_TOKEN_MAX)
        .collect()
}

/// Distinct significant task tokens in order of appearance (at most [`MAX_FTS_TERMS`]).
pub(super) fn fts_terms(tokens: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    tokens
        .iter()
        .filter(|t| significant(t) && seen.insert(t.as_str()))
        .take(MAX_FTS_TERMS)
        .cloned()
        .collect()
}

/// Record ids mentioned verbatim in free text (sorted, deduplicated).
pub fn mentioned_ids(text: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for raw in text.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))) {
        let cand = raw.trim_matches(|c: char| matches!(c, '.' | '-' | '_'));
        if cand.contains('.') && check_record_id(cand).is_ok() {
            out.insert(cand.to_string());
        }
    }
    out.into_iter().collect()
}

/// Best matching selector glob: longest literal prefix, then lexicographically smallest.
fn best_path_match(selectors: &[String], paths: &[ResolvedPath]) -> Option<(String, i64)> {
    let mut best: Option<(i64, String)> = None;
    for spec in selectors {
        let Ok(g) = RepoGlob::parse(spec) else {
            continue;
        };
        if paths.iter().any(|p| g.matches(p.repo.as_deref(), &p.path)) {
            let lp = g.literal_prefix_len() as i64;
            let better = best
                .as_ref()
                .is_none_or(|(b, s)| lp > *b || (lp == *b && spec < s));
            if better {
                best = Some((lp, spec.clone()));
            }
        }
    }
    best.map(|(lp, s)| (s, lp))
}

fn join(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_prior_table_is_complete_and_bounded() {
        for k in Kind::ALL {
            for i in Intent::ALL {
                let p = kind_prior(k, i);
                assert!((0..=40).contains(&p), "{k:?} {i:?}");
            }
        }
        assert_eq!(kind_prior(Kind::Procedure, Intent::Debug), 40);
        assert_eq!(kind_prior(Kind::Decision, Intent::Explain), 40);
    }

    #[test]
    fn id_mentions() {
        let ids = mentioned_ids(
            "See acme.mobile.token-storage, and (acme.contract.token-refresh#rotate). Not Acme.x or TokenStore.kt",
        );
        assert_eq!(
            ids,
            vec!["acme.contract.token-refresh", "acme.mobile.token-storage"]
        );
    }

    #[test]
    fn title_overlap_and_terms() {
        let toks = normalize::tokens("Fix token storage on logout; token again");
        assert_eq!(
            title_overlap("Token storage", &toks),
            vec!["token", "storage"]
        );
        assert_eq!(
            fts_terms(&toks),
            vec!["fix", "token", "storage", "logout", "again"]
        );
    }
}
