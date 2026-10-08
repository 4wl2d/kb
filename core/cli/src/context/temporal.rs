//! Pure historical view. Every lookup route observes the same validity filter.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::diag::Diagnostic;
use crate::error::{KbError, Result};
use crate::knowledge::{
    KnowledgeView, MetaEntry, Origin, ProposalEntry, RawEntry, RecordEntry, TaskPath,
};
use crate::model::{Kind, ProfileConfig, Registry, Scope, Status, date_days};

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

    /// Is the bound reached at this point? `None` for a commit bound without host ancestry
    /// evidence: the adapter resolves only bounds of records that can apply in the host.
    fn reached(&self, bound: &str) -> Result<Option<bool>> {
        if let Some(day) = date_days(bound) {
            let cutoff = date_days(&self.date)
                .ok_or_else(|| KbError::invalid_input("invalid resolved temporal date"))?;
            return Ok(Some(day <= cutoff));
        }
        Ok(self.commit_ancestry.get(bound).copied())
    }
}

/// Can a record with `scope` apply to a task in registry repo `repo`? Not when its repos, or
/// every registry repo of its modules (features), exclude `repo` (the registry narrowing of
/// applicability). Commit bounds of such a record name another repository's history, so the
/// host adapter does not resolve them; the slice withholds the record instead.
pub fn scope_reaches_repo(scope: &Scope, registry: &Registry, repo: &str) -> bool {
    if scope.product {
        return true;
    }
    let foreign_module = |id: &String| registry.module(id).is_some_and(|m| m.repo != repo);
    let foreign_feature = |id: &String| {
        registry
            .feature(id)
            .is_some_and(|f| !f.repos.is_empty() && f.repos.iter().all(|r| r != repo))
    };
    let other_repos = !scope.repos.is_empty() && !scope.repos.iter().any(|r| r == repo);
    let other_modules = !scope.modules.is_empty() && scope.modules.iter().all(foreign_module);
    let other_features = !scope.features.is_empty() && scope.features.iter().all(foreign_feature);
    !(other_repos || other_modules || other_features)
}

pub struct TemporalView<'a> {
    inner: &'a dyn KnowledgeView,
    allowed: BTreeSet<String>,
    fulltext: BTreeMap<String, super::lexical::FtsColumns>,
    pub excluded: Vec<Excluded>,
    pub undated_accepted: usize,
    /// Accepted records withheld because a commit bound has no host ancestry evidence.
    pub unresolved_accepted: usize,
}

impl<'a> TemporalView<'a> {
    pub fn new(inner: &'a dyn KnowledgeView, point: &AsOf) -> Result<Self> {
        let metas = inner.metas_by_kind(&Kind::ALL, Origin::Accepted)?;
        let ids: Vec<_> = metas.iter().map(|e| e.meta.id.clone()).collect();
        let mut allowed = BTreeSet::new();
        let mut excluded = Vec::new();
        let mut fulltext = BTreeMap::new();
        let mut undated_accepted = 0;
        let mut unresolved_accepted = 0;
        for entry in inner.records(&ids, Origin::Accepted)? {
            let c = entry.parsed.record.common();
            let accepted = usize::from(c.status == Status::Accepted);
            let unresolved = "commit bound not resolvable in this host";
            let why = match c.introduced.map(|b| point.reached(b)).transpose()? {
                None => {
                    undated_accepted += accepted;
                    Some("introduced is missing; historical availability is unverified")
                }
                Some(None) => {
                    unresolved_accepted += accepted;
                    Some(unresolved)
                }
                Some(Some(false)) => Some("not introduced at the requested point"),
                Some(Some(true)) => match c.retired.map(|b| point.reached(b)).transpose()? {
                    Some(None) => {
                        unresolved_accepted += accepted;
                        Some(unresolved)
                    }
                    Some(Some(true)) => Some("retired at the requested point"),
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
            unresolved_accepted,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RegistryData;
    use crate::model::registry::{Feature, Module};

    #[test]
    fn records_scoped_to_other_repositories_do_not_reach_the_host() {
        let module = |id: &str, repo: &str| Module {
            id: id.into(),
            repo: repo.into(),
            title: id.into(),
            paths: vec![],
            features: vec![],
        };
        let feature = |id: &str, repos: &[&str]| Feature {
            id: id.into(),
            title: id.into(),
            repos: repos.iter().map(|r| r.to_string()).collect(),
            paths: vec![],
        };
        let registry = Registry::new(RegistryData {
            modules: vec![
                module("mobile.auth", "mobile"),
                module("backend.api", "backend"),
            ],
            features: vec![feature("billing", &["backend"]), feature("login", &[])],
            ..Default::default()
        });
        let scope = |repos: &[&str], modules: &[&str], features: &[&str]| Scope {
            repos: repos.iter().map(|s| s.to_string()).collect(),
            modules: modules.iter().map(|s| s.to_string()).collect(),
            features: features.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let reaches = |s: Scope| scope_reaches_repo(&s, &registry, "mobile");
        assert!(reaches(Scope {
            product: true,
            ..Default::default()
        }));
        assert!(reaches(scope(&[], &[], &[])));
        assert!(reaches(scope(&["mobile", "backend"], &[], &[])));
        assert!(reaches(scope(&[], &["backend.api", "mobile.auth"], &[])));
        assert!(reaches(scope(&[], &["unregistered"], &["login"])));
        assert!(!reaches(scope(&["backend"], &[], &[])));
        assert!(!reaches(scope(&[], &["backend.api"], &[])));
        assert!(!reaches(scope(&[], &[], &["billing"])));
    }
}
