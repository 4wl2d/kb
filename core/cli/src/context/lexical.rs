//! Pure search data and scoring shared by the SQLite adapter and temporal views.
//! No filesystem or database access occurs here.

use crate::glob::RepoGlob;
use crate::model::{Exception, Item, ParsedRecord, Record};
use crate::normalize::{self, AliasPattern};

/// Term row kinds (`doc_terms.kind`).
pub const TERM_CONCEPT: &str = "concept";
/// First token of an alias whose first token must match exactly.
pub const TERM_ALIAS_FIRST: &str = "alias-first";
/// Single-token prefix alias (`композици*`): the prefix itself; checked in Rust.
pub const TERM_ALIAS_PREFIX: &str = "alias-prefix";

/// FTS5 column values (already normalized) and the total token count.
#[derive(Debug, Default, PartialEq)]
pub struct FtsColumns {
    pub ids: String,
    pub title: String,
    pub aliases: String,
    pub normative: String,
    pub body: String,
    pub tokens: usize,
}

/// One `doc_paths` row.
#[derive(Debug, PartialEq, Eq)]
pub struct PathRow {
    pub repo: Option<String>,
    pub dir_prefix: String,
    pub glob: String,
}

/// Collects normalized tokens for one FTS column.
#[derive(Default)]
struct Column(Vec<String>);

impl Column {
    fn push(&mut self, s: &str) {
        self.0.extend(normalize::tokens(s));
    }
    fn all<'a>(&mut self, items: impl IntoIterator<Item = &'a String>) {
        for s in items {
            self.push(s);
        }
    }
    /// A normative statement: text, conditions and exceptions here, local ids in `ids`.
    fn statement(
        &mut self,
        ids: &mut Column,
        id: &str,
        text: &str,
        conditions: &[String],
        exceptions: &[Exception],
    ) {
        ids.push(id);
        self.push(text);
        self.all(conditions);
        for e in exceptions {
            ids.push(&e.id);
            self.push(&e.text);
        }
    }
    fn items(&mut self, ids: &mut Column, items: &[Item]) {
        for i in items {
            ids.push(&i.id);
            self.push(&i.text);
        }
    }
    fn text(self) -> String {
        self.0.join(" ")
    }
}

/// Build the FTS columns of a record: id tokens (record and local ids), title, aliases
/// (selector aliases and concepts), all typed normative and explanatory text, and the
/// Markdown sections.
pub fn fts_columns(p: &ParsedRecord) -> FtsColumns {
    let mut ids = Column::default();
    let mut title = Column::default();
    let mut aliases = Column::default();
    let mut norm = Column::default();
    let mut body = Column::default();
    let c = p.record.common();
    ids.push(c.id);
    title.push(c.title);
    aliases.all(&c.selectors.aliases);
    aliases.all(&c.selectors.concepts);
    match &p.record {
        Record::Policy(r) => {
            for s in &r.rules {
                norm.statement(&mut ids, &s.id, &s.text, &s.conditions, &s.exceptions);
            }
            for s in &r.settings {
                ids.push(&s.name);
                norm.push(&s.name);
                norm.all(&s.description);
            }
            for o in &r.overrides {
                norm.push(&o.target);
                norm.push(&o.reason);
            }
        }
        Record::Feature(r) => {
            norm.push(&r.feature);
            norm.push(&r.summary);
            norm.items(&mut ids, &r.behaviors);
            norm.items(&mut ids, &r.boundaries);
            norm.items(&mut ids, &r.states);
            norm.all(&r.clocks);
            norm.all(&r.data_sources);
            for t in &r.transitions {
                ids.push(&t.id);
                norm.push(&t.from);
                norm.push(&t.to);
                norm.push(&t.when);
            }
            for s in &r.scenarios {
                ids.push(&s.id);
                norm.push(&s.given);
                norm.push(&s.expect);
            }
        }
        Record::Invariant(r) => {
            for s in &r.statements {
                norm.statement(&mut ids, &s.id, &s.text, &s.conditions, &s.exceptions);
            }
        }
        Record::Contract(r) => {
            norm.all(&r.interface);
            for party in &r.parties {
                ids.push(&party.id);
                norm.push(&party.role);
            }
            for o in &r.obligations {
                norm.statement(&mut ids, &o.id, &o.text, &o.conditions, &o.exceptions);
            }
            for s in &r.scenarios {
                ids.push(&s.id);
                norm.push(&s.given);
                norm.push(&s.expect);
            }
            for c in &r.consumers {
                norm.push(&c.repo);
                norm.push(&c.path);
                norm.all(&c.symbol);
            }
        }
        Record::Decision(r) => {
            norm.push(&r.context);
            norm.push(&r.decision);
            norm.all(&r.reasons);
            for a in &r.alternatives {
                norm.push(&a.option);
                norm.push(&a.rejected_because);
            }
            norm.all(&r.consequences);
        }
        Record::Procedure(r) => {
            norm.all(&r.preconditions);
            norm.items(&mut ids, &r.steps);
            norm.all(&r.expected);
        }
        Record::Reference(r) => {
            norm.push(&r.summary);
            for s in &r.sources {
                norm.push(&s.title);
            }
            for t in &r.terms {
                aliases.push(&t.term);
                norm.push(&t.meaning);
                norm.push(&t.source);
            }
        }
        Record::Gap(r) => {
            norm.push(&r.description);
            norm.all(&r.questions);
            ids.all(&r.affects);
        }
    }
    for s in &p.sections {
        body.push(&s.heading);
        body.push(&s.markdown);
    }
    let tokens = [&ids, &title, &aliases, &norm, &body]
        .iter()
        .map(|c| c.0.len())
        .sum();
    FtsColumns {
        ids: ids.text(),
        title: title.text(),
        aliases: aliases.text(),
        normative: norm.text(),
        body: body.text(),
        tokens,
    }
}

/// `doc_paths` rows for the record's path selectors (invalid globs are skipped; they are
/// reported by parsing/validation).
pub fn path_rows(p: &ParsedRecord) -> Vec<PathRow> {
    let mut rows: Vec<PathRow> = p
        .record
        .common()
        .selectors
        .paths
        .iter()
        .filter_map(|spec| {
            let g = RepoGlob::parse(spec).ok()?;
            Some(PathRow {
                dir_prefix: g.literal_dir_prefix(),
                repo: g.repo,
                glob: spec.clone(),
            })
        })
        .collect();
    rows.sort_by(|a, b| (&a.glob, &a.repo).cmp(&(&b.glob, &b.repo)));
    rows.dedup();
    rows
}

/// One `doc_terms` row: `value` is the lookup key, `pattern` what a match must satisfy
/// (the concept id, or the alias key as produced by [`AliasPattern::key`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TermRow {
    pub kind: &'static str,
    pub value: String,
    pub pattern: String,
}

/// `doc_terms` rows for concept selectors and selector aliases. Aliases are keyed by
/// their first token; single-token prefix aliases by the prefix (a small set scanned in
/// Rust, since a prefix cannot be looked up by token equality).
pub fn term_rows(p: &ParsedRecord) -> Vec<TermRow> {
    let meta = p.meta();
    let sel = &meta.selectors;
    let mut rows: Vec<TermRow> = sel
        .concepts
        .iter()
        .map(|c| TermRow {
            kind: TERM_CONCEPT,
            value: c.clone(),
            pattern: c.clone(),
        })
        .collect();
    for a in &sel.aliases {
        if let Some(pat) = AliasPattern::compile(a) {
            let kind = if pat.prefix && pat.tokens.len() == 1 {
                TERM_ALIAS_PREFIX
            } else {
                TERM_ALIAS_FIRST
            };
            rows.push(TermRow {
                kind,
                value: pat.tokens[0].clone(),
                pattern: pat.key(),
            });
        }
    }
    rows.sort();
    rows.dedup();
    rows
}

/// Normalize caller terms into distinct tokens (sorted), at most `max` of them.
pub fn query_tokens(terms: &[String], max: usize) -> Vec<String> {
    let mut out: Vec<String> = terms.iter().flat_map(|t| normalize::tokens(t)).collect();
    out.sort();
    out.dedup();
    out.truncate(max);
    out
}

/// Quote one token as an FTS5 string literal. User text is never passed as FTS syntax.
pub fn fts_string(token: &str) -> String {
    format!("\"{}\"", token.replace('"', "\"\""))
}

/// BM25 parameters (the FTS5 defaults).
const K1: f64 = 1.2;
const B: f64 = 0.75;

/// Rank a filtered corpus using the same binary-term BM25 formula as IndexView.
/// Statistics are computed after filtering, so future records cannot alter past rankings.
pub fn rank_corpus(
    docs: &std::collections::BTreeMap<String, FtsColumns>,
    terms: &[String],
    limit: usize,
) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};
    let terms = query_tokens(terms, 64);
    if limit == 0 || terms.is_empty() || docs.is_empty() {
        return Vec::new();
    }
    let avg = docs.values().map(|d| d.tokens as f64).sum::<f64>() / docs.len() as f64;
    let vocabulary: BTreeMap<_, BTreeSet<_>> = docs
        .iter()
        .map(|(id, d)| {
            (
                id,
                [&d.ids, &d.title, &d.aliases, &d.normative, &d.body]
                    .into_iter()
                    .flat_map(|s| s.split_whitespace())
                    .collect(),
            )
        })
        .collect();
    let mut scores: BTreeMap<&String, f64> = BTreeMap::new();
    for term in terms {
        let hits: Vec<_> = vocabulary
            .iter()
            .filter(|(_, v)| v.contains(term.as_str()))
            .map(|(id, _)| *id)
            .collect();
        let weight = idf(docs.len(), hits.len());
        for id in hits {
            *scores.entry(id).or_default() += term_weight(weight, docs[id].tokens as f64, avg);
        }
    }
    let mut ranked: Vec<_> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked
        .into_iter()
        .take(limit)
        .map(|(id, _)| id.clone())
        .collect()
}

/// Inverse document frequency of a term matched by `df` of `n` snapshot documents.
pub fn idf(n: usize, df: usize) -> f64 {
    let (n, df) = (n as f64, df as f64);
    (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
}

/// BM25 term weight with binary term frequency for a document of `len` tokens.
///
/// All statistics come from the snapshot being queried, so a ranking never depends on
/// which other snapshots happen to share the cache (FTS5's `bm25()` uses table-wide
/// statistics, which would make results depend on cache history).
pub fn term_weight(idf: f64, len: f64, avglen: f64) -> f64 {
    let norm = if avglen > 0.0 {
        1.0 - B + B * len / avglen
    } else {
        1.0
    };
    idf * (K1 + 1.0) / (1.0 + K1 * norm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_record;

    const REC: &str = r#"+++
schema = 1
id = "acme.mobile.token-storage"
kind = "policy"
title = "Token Storage"
status = "accepted"
owner = "team-mobile"

[scope]
repos = ["mobile"]

[selectors]
paths = ["app/auth/**", "backend:src/api/*.rs", "**/token/**"]
concepts = ["auth-token"]
aliases = ["token refresh", "обновлени*", "refresh*"]

[[rules]]
id = "no-plaintext"
level = "must-not"
text = "Store refresh tokens in plaintext storage."
conditions = ["When the device is rooted"]

[[rules.exceptions]]
id = "fake-server"
text = "Debug builds against the Ёлка server."
+++
## Background
Why this exists.
"#;

    #[test]
    fn columns_cover_typed_text() {
        let p = parse_record("k/a.md", REC.as_bytes()).unwrap();
        let c = fts_columns(&p);
        assert!(c.ids.starts_with("acme mobile token storage"));
        assert!(c.ids.contains("no plaintext") && c.ids.contains("fake server"));
        assert_eq!(c.title, "token storage");
        assert!(c.aliases.contains("обновлени") && c.aliases.contains("auth token"));
        assert!(c.normative.contains("device is rooted"));
        assert!(c.normative.contains("елка server"));
        assert_eq!(c.body, "background why this exists");
        assert!(c.tokens > 20);
    }

    #[test]
    fn selector_rows() {
        let p = parse_record("k/a.md", REC.as_bytes()).unwrap();
        let paths = path_rows(&p);
        let prefixes: Vec<_> = paths
            .iter()
            .map(|r| (r.repo.as_deref(), r.dir_prefix.as_str()))
            .collect();
        assert_eq!(
            prefixes,
            vec![
                (None, ""),
                (None, "app/auth/"),
                (Some("backend"), "src/api/")
            ]
        );
        let rows = term_rows(&p);
        let terms: Vec<(&str, &str, &str)> = rows
            .iter()
            .map(|t| (t.kind, t.value.as_str(), t.pattern.as_str()))
            .collect();
        assert_eq!(
            terms,
            vec![
                (TERM_ALIAS_FIRST, "token", "token refresh"),
                (TERM_ALIAS_PREFIX, "refresh", "refresh*"),
                (TERM_ALIAS_PREFIX, "обновлени", "обновлени*"),
                (TERM_CONCEPT, "auth-token", "auth-token"),
            ]
        );
    }

    #[test]
    fn queries_are_quoted_tokens() {
        let toks = query_tokens(&["\" OR * NEAR( -- ;DROP".into(), "Token".into()], 10);
        assert_eq!(toks, vec!["drop", "near", "or", "token"]);
        assert_eq!(fts_string("a\"b"), "\"a\"\"b\"");
        assert_eq!(query_tokens(&["a b c".into()], 2), vec!["a", "b"]);
    }

    #[test]
    fn rarer_terms_and_shorter_docs_weigh_more() {
        assert!(idf(100, 1) > idf(100, 50));
        assert!(idf(100, 100) > 0.0);
        let w = idf(10, 2);
        assert!(term_weight(w, 10.0, 100.0) > term_weight(w, 1000.0, 100.0));
        assert!(term_weight(w, 5.0, 0.0).is_finite());
    }
}
