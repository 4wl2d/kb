//! `kb search`: ranked lookup of records by id, concept/alias, title and full text.
//!
//! Search is **not** a context assembly: it does not select mandatory obligations, follow
//! `requires`, apply scope or report completeness. Every rendering says so.
//!
//! Points: id mention 1000; concept match (via registry aliases in the query) 200 each, at
//! most 2; record alias match 150; title token overlap 20 each, at most 3; full-text rank
//! position p → max(100 − 5p, 5). Order: points descending, then id. All statuses are
//! searchable and labeled.
//!
//! With a code checkout the handler adds [`file_hits`]: the files whose names read like
//! the query, so that a search in the words of a task also names the code to open.

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
    /// Code files named like the query; empty when no checkout was consulted.
    pub files: Vec<FileHit>,
    /// Files named like the query before [`MAX_FILE_HITS`] was applied.
    pub files_total: usize,
}

/// A code file whose name reads like the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHit {
    /// Code-root-relative path.
    pub path: String,
    /// The query words found in the file name.
    pub matched: Vec<String>,
    /// Where the file declares what it is named after: the line and its text. Filled in
    /// by the handler for files of the checkout; a lexical locator for `kb read`.
    pub declaration: Option<(u32, String)>,
}

/// Files listed with a search result.
pub const MAX_FILE_HITS: usize = 8;
/// Query words shorter than this say too little about a file name.
const MIN_NAME_WORD: usize = 3;

/// The words of a name: `InvoiceLineParser` and `invoice-line_parser` both hold
/// `invoice`, `line`, `parser`. Case changes, digits and punctuation separate words.
pub fn name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut previous: Option<char> = None;
    let chars: Vec<char> = name.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if !ch.is_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            previous = None;
            continue;
        }
        // `fooBar` breaks before `B`; `HTTPServer` breaks before `S`.
        let boundary = previous.is_some_and(|p| {
            (p.is_lowercase() && ch.is_uppercase())
                || (p.is_alphabetic() != ch.is_alphabetic())
                || (p.is_uppercase()
                    && ch.is_uppercase()
                    && chars.get(i + 1).is_some_and(|n| n.is_lowercase()))
        });
        if boundary && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        cur.extend(ch.to_lowercase());
        previous = Some(ch);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Do a query word and a name word mean the same: equal, or forms of one word
/// (`refresh`/`refresher`, `entry`/`entries`)?
fn same_word(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let letters = short.chars().count();
    if let Some(ending) = long.strip_prefix(short) {
        // A long word that begins a longer one is its family (`config`, `configuration`);
        // a short one needs a known ending (`list` is not `listener`).
        return letters >= 6
            || ENDINGS
                .iter()
                .any(|(known, least)| ending == *known && letters >= *least)
            || (short.ends_with('e') && letters >= 4 && matches!(ending, "d" | "r" | "rs"));
    }
    // The short form gives up its last letter: `entry`/`entries`, `migrate`/`migration`.
    let Some((cut, last)) = short.char_indices().next_back() else {
        return false;
    };
    let Some(ending) = long.strip_prefix(&short[..cut]) else {
        return false;
    };
    match last {
        'y' => letters >= 4 && matches!(ending, "ies" | "ied" | "ier"),
        'e' => {
            letters >= 5
                && matches!(
                    ending,
                    "ing" | "ion" | "ions" | "or" | "ors" | "ation" | "ations" | "able"
                )
        }
        _ => false,
    }
}

/// Endings that make a longer form of a word, with the fewest letters the word must have
/// for the ending to count (`auth` is not `author`, `stat` is not `station`).
const ENDINGS: &[(&str, usize)] = &[
    ("s", 3),
    ("es", 3),
    ("ed", 4),
    ("ing", 4),
    ("ings", 4),
    ("ment", 4),
    ("ments", 4),
    ("er", 5),
    ("ers", 5),
    ("or", 5),
    ("ors", 5),
    ("ion", 5),
    ("ions", 5),
    ("al", 5),
    ("ly", 5),
    ("able", 5),
    ("ation", 5),
    ("ations", 5),
];

/// The files whose names read like the query, best first. A file is listed when its
/// name holds at least two words of the query (one, for a one-word query) or consists of
/// query words only. Rarer words weigh more: a word found in few file names says more than
/// one found in hundreds. Ties go to the name with fewer other words, then to the path.
/// Returns the first [`MAX_FILE_HITS`] and the number that matched.
pub fn file_hits<'a>(query: &str, paths: impl Iterator<Item = &'a str>) -> (Vec<FileHit>, usize) {
    let mut wanted: Vec<String> = Vec::new();
    for word in name_words(query) {
        // Two forms of one word (`token`, `tokens`) are one word of the query.
        if word.chars().count() >= MIN_NAME_WORD && !wanted.iter().any(|w| same_word(w, &word)) {
            wanted.push(word);
        }
    }
    let needed = wanted.len().min(2);
    if needed == 0 {
        return (Vec::new(), 0);
    }
    // Per file: which query words its name holds, and how many other words.
    let mut named: Vec<(&str, Vec<usize>, usize)> = Vec::new();
    let mut frequency = vec![0usize; wanted.len()];
    let mut files = 0usize;
    for path in paths {
        files += 1;
        let file = path.rsplit('/').next().unwrap_or(path);
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        let words = name_words(stem);
        let matched: Vec<usize> = (0..wanted.len())
            .filter(|&i| words.iter().any(|n| same_word(&wanted[i], n)))
            .collect();
        if matched.is_empty() {
            continue;
        }
        for &i in &matched {
            frequency[i] += 1;
        }
        let others = words
            .iter()
            .filter(|n| !wanted.iter().any(|w| same_word(w, n)))
            .count();
        if matched.len() >= needed || others == 0 {
            named.push((path, matched, others));
        }
    }
    // Inverse document frequency in thousandths, so that the order is exact.
    let weight = |i: usize| -> u64 {
        let ratio = files as f64 / frequency[i].max(1) as f64;
        ((1.0 + ratio).ln() * 1000.0).round() as u64
    };
    let mut hits: Vec<(u64, usize, FileHit)> = named
        .into_iter()
        .map(|(path, matched, others)| {
            (
                matched.iter().map(|&i| weight(i)).sum(),
                others,
                FileHit {
                    path: path.to_string(),
                    matched: matched.iter().map(|&i| wanted[i].clone()).collect(),
                    declaration: None,
                },
            )
        })
        .collect();
    hits.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.path.cmp(&b.2.path))
    });
    let total = hits.len();
    (
        hits.into_iter()
            .take(MAX_FILE_HITS)
            .map(|(_, _, hit)| hit)
            .collect(),
        total,
    )
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
        files: Vec::new(),
        files_total: 0,
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

/// Render compact, terse or human text (JSON: pretty-printed [`to_json`]).
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
    if human {
        o.push_str(&format!("note: {NOTE}\n"));
    } else {
        o.push_str("note: search is not a context assembly; use `kb context` for obligations\n");
        if !r.hits.is_empty() {
            o.push_str("open: kb show <record-id>\n");
        }
    }
    if r.hits.is_empty() {
        o.push_str("no matching records\n");
    }
    let files = |o: &mut String| {
        if r.files.is_empty() {
            return;
        }
        o.push_str(&format!(
            "code: {} of {} file(s) named like the query; kb sym <name> or kb read <path> opens one\n",
            r.files.len(),
            r.files_total
        ));
        for f in &r.files {
            o.push_str(&f.path);
            if let Some((line, text)) = &f.declaration {
                o.push_str(&format!(":{line}  {text}"));
            }
            o.push('\n');
        }
    };
    for h in &r.hits {
        let origin = match h.origin {
            Origin::Accepted => "",
            Origin::Proposal => ", proposal",
        };
        let kind = h.kind.as_str();
        let status = h.status.as_str();
        if human {
            o.push_str(&format!(
                "{} {} ({kind}, {status}{origin}): {}\n  matched: {}\n  source: {}\n",
                h.score,
                h.id,
                h.title,
                h.matched.join(", "),
                h.path
            ));
        } else {
            let title = h.title.lines().collect::<Vec<_>>().join(" ");
            o.push_str(&format!("{} [{kind}, {status}{origin}] {title}\n", h.id));
        }
    }
    files(&mut o);
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
        "files": r.files.iter().map(|f| {
            let mut file = json!({"path": f.path, "matched": f.matched});
            if let Some((line, text)) = &f.declaration {
                file["line"] = json!(line);
                file["declaration"] = json!(text);
            }
            file
        }).collect::<Vec<_>>(),
        "files_total": r.files_total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_split_into_words_at_case_changes_digits_and_punctuation() {
        assert_eq!(
            name_words("InvoiceLineParser"),
            ["invoice", "line", "parser"]
        );
        assert_eq!(
            name_words("HTTPServer2Config"),
            ["http", "server", "2", "config"]
        );
        assert_eq!(name_words("sync-entries_v2"), ["sync", "entries", "v", "2"]);
        assert_eq!(name_words("Профиль User"), ["профиль", "user"]);
        assert!(name_words("--").is_empty());
    }

    #[test]
    fn longer_forms_of_a_word_match_and_short_words_need_equality() {
        assert!(same_word("refresh", "refresher"));
        assert!(same_word("entry", "entries"));
        assert!(same_word("validate", "validator"));
        assert!(same_word("migrate", "migration"));
        assert!(same_word("config", "configuration"));
        assert!(same_word("process", "processes"));
        assert!(same_word("status", "statuses"));
        assert!(!same_word("store", "story"));
        assert!(!same_word("stat", "station"));
        assert!(!same_word("apply", "apple"));
        assert!(same_word("stored", "store"));
        assert!(same_word("users", "user"));
        assert!(!same_word("map", "mapper"));
        assert!(!same_word("token", "tooling"));
        assert!(!same_word("user", "used"));
        // Words that only begin alike are different words.
        for (a, b) in [
            ("state", "static"),
            ("state", "status"),
            ("config", "confirm"),
            ("list", "listener"),
            ("auth", "author"),
        ] {
            assert!(!same_word(a, b), "{a} {b}");
        }
    }

    #[test]
    fn files_named_like_the_query_come_rarest_words_first() {
        let paths = [
            "app/auth/StoreMigration.kt",
            "app/auth/TokenRefresher.kt",
            "app/auth/TokenStore.kt",
            "app/auth/TokenStoreTest.kt",
            "app/invoices/InvoiceArchiveView.kt",
            "app/invoices/InvoiceArchiveViewMapper.kt",
            "app/token/Token.kt",
            "docs/token-refresh.md",
            "README.md",
        ];
        let (hits, total) = file_hits("token store refresh", paths.into_iter());
        let listed: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        // `refresh` names fewer files than `store`, so it weighs more; among equals the
        // name with fewer other words leads. A name of query words alone is listed with
        // one word; a name with one query word among others is not.
        assert_eq!(
            listed,
            [
                "app/auth/TokenRefresher.kt",
                "docs/token-refresh.md",
                "app/auth/TokenStore.kt",
                "app/auth/TokenStoreTest.kt",
                "app/token/Token.kt"
            ]
        );
        assert_eq!(total, 5);
        assert_eq!(hits[0].matched, ["token", "refresh"]);

        // A name is split like a file name: both words must be found.
        let (hits, _) = file_hits("InvoiceArchive", paths.into_iter());
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "app/invoices/InvoiceArchiveView.kt");
        // A one-word query lists every name that holds the word, shortest name first.
        let (hits, total) = file_hits("migration", paths.into_iter());
        assert_eq!(total, 1);
        assert_eq!(hits[0].path, "app/auth/StoreMigration.kt");
        let (hits, total) = file_hits("token", paths.into_iter());
        assert_eq!(total, 5);
        assert_eq!(hits[0].path, "app/token/Token.kt");
        // Two forms of one word are one word of the query, not the two a name must hold.
        assert_eq!(file_hits("token tokens", paths.into_iter()).1, 5);

        // Words shorter than three characters and words that name nothing find no file.
        assert_eq!(file_hits("an is of", paths.into_iter()).1, 0);
        assert_eq!(file_hits("nothing here", paths.into_iter()).1, 0);

        let many: Vec<String> = (0..20).map(|n| format!("src/TokenStore{n}.kt")).collect();
        let (hits, total) = file_hits("token store", many.iter().map(String::as_str));
        assert_eq!((hits.len(), total), (MAX_FILE_HITS, 20));
        assert_eq!(hits[0].path, "src/TokenStore0.kt");
    }
}
