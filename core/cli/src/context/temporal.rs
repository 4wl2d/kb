//! Pure historical view. Every lookup route observes the same validity filter.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::diag::Diagnostic;
use crate::error::{KbError, Result};
use crate::knowledge::{
    KnowledgeView, MetaEntry, Origin, ProposalEntry, RawEntry, RecordEntry, TaskPath,
};
use crate::model::{Kind, ProfileConfig, Registry, Status, date_days};

use super::{Excluded, ExcludedReason};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AsOf {
    pub requested: String,
    pub date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_revision: Option<String>,
    /// Git facts supplied by the host adapter; not a second source of authority.
    #[serde(skip)]
    pub commit_ancestry: BTreeMap<String, bool>,
}

impl AsOf {
    pub fn on_date(date: &str) -> Result<Self> {
        date_days(date).ok_or_else(|| KbError::invalid_input("--as-of date must be YYYY-MM-DD"))?;
        Ok(Self {
            requested: date.into(),
            date: date.into(),
            host_revision: None,
            commit_ancestry: BTreeMap::new(),
        })
    }

    fn reached(&self, bound: &str) -> Result<bool> {
        if let Some(day) = date_days(bound) {
            let cutoff = date_days(&self.date)
                .ok_or_else(|| KbError::invalid_input("invalid resolved temporal date"))?;
            return Ok(day <= cutoff);
        }
        self.commit_ancestry.get(bound).copied().ok_or_else(|| {
            KbError::invalid_input(format!(
                "no host ancestry evidence for temporal bound {bound}"
            ))
        })
    }
}

pub struct TemporalView<'a> {
    inner: &'a dyn KnowledgeView,
    allowed: BTreeSet<String>,
    fulltext: BTreeMap<String, super::lexical::FtsColumns>,
    pub excluded: Vec<Excluded>,
    pub undated_accepted: usize,
}

impl<'a> TemporalView<'a> {
    pub fn new(inner: &'a dyn KnowledgeView, point: &AsOf) -> Result<Self> {
        let metas = inner.metas_by_kind(&Kind::ALL, Origin::Accepted)?;
        let ids: Vec<_> = metas.iter().map(|e| e.meta.id.clone()).collect();
        let mut allowed = BTreeSet::new();
        let mut excluded = Vec::new();
        let mut fulltext = BTreeMap::new();
        let mut undated_accepted = 0;
        for entry in inner.records(&ids, Origin::Accepted)? {
            let c = entry.parsed.record.common();
            let why = match c.introduced {
                None => {
                    if c.status == Status::Accepted {
                        undated_accepted += 1;
                    }
                    Some("introduced is missing; historical availability is unverified")
                }
                Some(start) if !point.reached(start)? => {
                    Some("not introduced at the requested point")
                }
                _ => match c.retired {
                    Some(end) if point.reached(end)? => Some("retired at the requested point"),
                    _ => None,
                },
            };
            if let Some(why) = why {
                excluded.push(Excluded {
                    id: c.id.into(),
                    reason: ExcludedReason::Temporal,
                    score: None,
                    detail: why.into(),
                });
            } else {
                allowed.insert(c.id.into());
                fulltext.insert(c.id.into(), super::lexical::fts_columns(&entry.parsed));
            }
        }
        excluded.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Self {
            inner,
            allowed,
            fulltext,
            excluded,
            undated_accepted,
        })
    }

    fn filter(&self, entries: Vec<MetaEntry>) -> Vec<MetaEntry> {
        entries
            .into_iter()
            .filter(|e| self.allowed.contains(&e.meta.id))
            .collect()
    }
}

impl KnowledgeView for TemporalView<'_> {
    fn config(&self) -> &ProfileConfig {
        self.inner.config()
    }
    fn registry(&self) -> &Registry {
        self.inner.registry()
    }
    fn diagnostics(&self) -> &[Diagnostic] {
        self.inner.diagnostics()
    }
    fn metas_by_kind(&self, kinds: &[Kind], origin: Origin) -> Result<Vec<MetaEntry>> {
        if origin != Origin::Accepted {
            return Ok(Vec::new());
        }
        Ok(self.filter(self.inner.metas_by_kind(kinds, origin)?))
    }
    fn metas_by_ids(&self, ids: &[String], origin: Origin) -> Result<Vec<MetaEntry>> {
        if origin != Origin::Accepted {
            return Ok(Vec::new());
        }
        Ok(self.filter(self.inner.metas_by_ids(ids, origin)?))
    }
    fn path_candidates(&self, paths: &[TaskPath]) -> Result<Vec<MetaEntry>> {
        Ok(self.filter(self.inner.path_candidates(paths)?))
    }
    fn term_candidates(&self, concepts: &[String], tokens: &[String]) -> Result<Vec<MetaEntry>> {
        Ok(self.filter(self.inner.term_candidates(concepts, tokens)?))
    }
    fn fulltext(&self, terms: &[String], limit: usize) -> Result<Vec<String>> {
        Ok(super::lexical::rank_corpus(&self.fulltext, terms, limit))
    }
    fn records(&self, ids: &[String], origin: Origin) -> Result<Vec<RecordEntry>> {
        if origin != Origin::Accepted {
            return Ok(Vec::new());
        }
        let ids: Vec<_> = ids
            .iter()
            .filter(|id| self.allowed.contains(*id))
            .cloned()
            .collect();
        self.inner.records(&ids, origin)
    }
    fn raw(&self, id: &str, origin: Origin) -> Result<Option<RawEntry>> {
        if origin != Origin::Accepted || !self.allowed.contains(id) {
            return Ok(None);
        }
        self.inner.raw(id, origin)
    }
    fn proposals(&self) -> Result<Vec<ProposalEntry>> {
        Ok(Vec::new())
    }
}
