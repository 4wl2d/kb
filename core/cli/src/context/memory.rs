//! In-memory [`KnowledgeView`] over a loaded [`Corpus`]: a brute-force implementation for
//! tests and small corpora. Every query scans the records; the SQLite index
//! (`index::IndexView`) serves warm CLI queries.
//!
//! Full-text search here is a simple deterministic stand-in for FTS5: a record matches when
//! a query term equals one of its normalized tokens; points are 3 per term found in the
//! title, 2 in aliases, 1 in any other text (typed fields and sections). Ordered by points,
//! then id.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::Value;

use crate::corpus::Corpus;
use crate::diag::{Diagnostic, normalize as normalize_diags};
use crate::error::Result;
use crate::knowledge::{
    KnowledgeView, MetaEntry, Origin, ProposalEntry, RawEntry, RecordEntry, TaskPath,
};
use crate::model::{Kind, ParsedRecord, ProfileConfig, RecordMeta, Registry};
use crate::normalize::{self, AliasPattern};

struct Stored {
    path: String,
    parsed: Arc<ParsedRecord>,
    meta: Arc<RecordMeta>,
    raw: Option<Arc<str>>,
    aliases: Vec<AliasPattern>,
    title_tokens: BTreeSet<String>,
    alias_tokens: BTreeSet<String>,
    body_tokens: BTreeSet<String>,
}

impl Stored {
    fn new(path: String, parsed: Arc<ParsedRecord>, raw: Option<Arc<str>>) -> Stored {
        let meta = Arc::new(parsed.meta());
        let aliases = meta
            .selectors
            .aliases
            .iter()
            .filter_map(|a| AliasPattern::compile(a))
            .collect();
        let title_tokens = normalize::tokens(&meta.title).into_iter().collect();
        let alias_tokens = meta
            .selectors
            .aliases
            .iter()
            .flat_map(|a| normalize::tokens(a))
            .collect();
        let mut text = String::new();
        if let Ok(v) = serde_json::to_value(&parsed.record) {
            collect_strings(&v, &mut text);
        }
        for s in &parsed.sections {
            text.push(' ');
            text.push_str(&s.heading);
            text.push(' ');
            text.push_str(&s.markdown);
        }
        let body_tokens = normalize::tokens(&text).into_iter().collect();
        Stored {
            path,
            parsed,
            meta,
            raw,
            aliases,
            title_tokens,
            alias_tokens,
            body_tokens,
        }
    }

    fn meta_entry(&self, origin: Origin) -> MetaEntry {
        MetaEntry {
            path: self.path.clone(),
            origin,
            meta: self.meta.clone(),
        }
    }
}

/// Every string in a JSON value except object keys (typed record text).
fn collect_strings(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
            out.push(' ');
            out.push_str(s);
        }
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        Value::Object(m) => m.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

/// Brute-force in-memory knowledge view.
pub struct MemoryView {
    config: ProfileConfig,
    registry: Registry,
    diagnostics: Vec<Diagnostic>,
    accepted: BTreeMap<String, Stored>,
    proposal_records: BTreeMap<String, Stored>,
    proposals: Vec<ProposalEntry>,
}

impl MemoryView {
    /// Build a view of the corpus's parsed records. Snapshot diagnostics are the corpus
    /// diagnostics plus `diagnostics` (e.g. validation findings), sorted and deduplicated.
    /// When several files declare the same id, the first by path wins (validation reports
    /// the duplicate).
    pub fn from_corpus(corpus: &Corpus, diagnostics: Vec<Diagnostic>) -> MemoryView {
        let mut diags = corpus.diagnostics.clone();
        diags.extend(diagnostics);
        normalize_diags(&mut diags);
        let mut records: Vec<_> = corpus.records().collect();
        records.sort_by(|a, b| a.0.path.cmp(&b.0.path));
        let mut accepted = BTreeMap::new();
        for (entry, parsed) in records {
            accepted
                .entry(parsed.record.id().to_string())
                .or_insert_with(|| {
                    Stored::new(entry.path.clone(), parsed.clone(), Some(entry.raw.clone()))
                });
        }
        MemoryView {
            config: corpus.config.clone(),
            registry: corpus.registry.clone(),
            diagnostics: diags,
            accepted,
            proposal_records: BTreeMap::new(),
            proposals: Vec::new(),
        }
    }

    /// Add a proposal overlay: entries plus the parsed proposal record (for new/modified
    /// records).
    pub fn with_proposals(
        mut self,
        proposals: Vec<(ProposalEntry, Option<Arc<ParsedRecord>>)>,
    ) -> MemoryView {
        for (p, parsed) in proposals {
            if let Some(parsed) = parsed {
                self.proposal_records
                    .entry(parsed.record.id().to_string())
                    .or_insert_with(|| Stored::new(p.path.clone(), parsed, None));
            }
            self.proposals.push(p);
        }
        self.proposals.sort_by(|a, b| a.path.cmp(&b.path));
        self
    }

    fn store(&self, origin: Origin) -> &BTreeMap<String, Stored> {
        match origin {
            Origin::Accepted => &self.accepted,
            Origin::Proposal => &self.proposal_records,
        }
    }
}

fn sorted_ids(ids: &[String]) -> BTreeSet<&String> {
    ids.iter().collect()
}

impl KnowledgeView for MemoryView {
    fn config(&self) -> &ProfileConfig {
        &self.config
    }

    fn registry(&self) -> &Registry {
        &self.registry
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    fn metas_by_kind(&self, kinds: &[Kind], origin: Origin) -> Result<Vec<MetaEntry>> {
        Ok(self
            .store(origin)
            .values()
            .filter(|s| kinds.contains(&s.meta.kind))
            .map(|s| s.meta_entry(origin))
            .collect())
    }

    fn metas_by_ids(&self, ids: &[String], origin: Origin) -> Result<Vec<MetaEntry>> {
        let store = self.store(origin);
        Ok(sorted_ids(ids)
            .into_iter()
            .filter_map(|id| store.get(id))
            .map(|s| s.meta_entry(origin))
            .collect())
    }

    fn path_candidates(&self, paths: &[TaskPath]) -> Result<Vec<MetaEntry>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self
            .accepted
            .values()
            .filter(|s| !s.meta.selectors.paths.is_empty())
            .map(|s| s.meta_entry(Origin::Accepted))
            .collect())
    }

    fn term_candidates(&self, concepts: &[String], tokens: &[String]) -> Result<Vec<MetaEntry>> {
        Ok(self
            .accepted
            .values()
            .filter(|s| {
                s.meta
                    .selectors
                    .concepts
                    .iter()
                    .any(|c| concepts.contains(c))
                    || s.aliases.iter().any(|a| a.matches(tokens))
            })
            .map(|s| s.meta_entry(Origin::Accepted))
            .collect())
    }

    fn fulltext(&self, terms: &[String], limit: usize) -> Result<Vec<String>> {
        let mut hits: Vec<(u64, &String)> = self
            .accepted
            .iter()
            .filter_map(|(id, s)| {
                let points: u64 = terms
                    .iter()
                    .map(|t| {
                        if s.title_tokens.contains(t) {
                            3
                        } else if s.alias_tokens.contains(t) {
                            2
                        } else if s.body_tokens.contains(t) {
                            1
                        } else {
                            0
                        }
                    })
                    .sum();
                (points > 0).then_some((points, id))
            })
            .collect();
        hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        Ok(hits
            .into_iter()
            .take(limit)
            .map(|(_, id)| id.clone())
            .collect())
    }

    fn records(&self, ids: &[String], origin: Origin) -> Result<Vec<RecordEntry>> {
        let store = self.store(origin);
        Ok(sorted_ids(ids)
            .into_iter()
            .filter_map(|id| store.get(id))
            .map(|s| RecordEntry {
                path: s.path.clone(),
                origin,
                parsed: s.parsed.clone(),
            })
            .collect())
    }

    fn raw(&self, id: &str, origin: Origin) -> Result<Option<RawEntry>> {
        Ok(self.store(origin).get(id).and_then(|s| {
            s.raw.as_ref().map(|text| RawEntry {
                path: s.path.clone(),
                origin,
                text: text.clone(),
            })
        }))
    }

    fn proposals(&self) -> Result<Vec<ProposalEntry>> {
        Ok(self.proposals.clone())
    }
}
