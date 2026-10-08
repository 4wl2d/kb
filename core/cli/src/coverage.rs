//! Pure coverage/backlog analysis over tracked host facts, registry scope and records.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::knowledge::RecordEntry;
use crate::model::{Kind, Record, Registry, Status};

#[derive(Debug, Clone, Serialize)]
pub struct ModuleCoverage {
    pub id: String,
    pub files: usize,
    pub churn: u64,
    pub fan_in: Option<u64>,
    pub rank_score: u64,
    pub domain_records: Vec<String>,
    pub broad_records: Vec<String>,
    pub only_broad_records: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureCoverage {
    pub id: String,
    pub records: Vec<String>,
    pub scenarios: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoverageReport {
    pub repo: String,
    pub tracked_files: usize,
    pub total_modules: usize,
    pub ranking: String,
    pub modules: Vec<ModuleCoverage>,
    pub features: Vec<FeatureCoverage>,
    pub unmapped_paths: Vec<String>,
    pub contracts_without_consumers: Vec<String>,
    pub features_without_scenarios: Vec<String>,
    pub checklists_by_change_type: BTreeMap<String, Vec<String>>,
    pub glossary_terms: usize,
    pub note: &'static str,
}

pub fn analyze(
    repo: &str,
    paths: &[String],
    changes: &BTreeMap<String, u64>,
    registry: &Registry,
    records: &[RecordEntry],
    fan_in: Option<&BTreeMap<String, u64>>,
    limit: usize,
) -> CoverageReport {
    let accepted: Vec<_> = records
        .iter()
        .filter(|e| e.parsed.record.status() == Status::Accepted)
        .collect();
    let mut modules = Vec::new();
    for module in registry.data.modules.iter().filter(|m| m.repo == repo) {
        let matches = |path: &str| {
            registry
                .modules_for_path(repo, path)
                .iter()
                .any(|m| m.id == module.id)
        };
        let files = paths.iter().filter(|p| matches(p)).count();
        let churn = changes
            .iter()
            .filter(|(p, _)| matches(p))
            .map(|(_, n)| *n)
            .fold(0u64, u64::saturating_add);
        let mut domain_records = BTreeSet::new();
        let mut broad_records = BTreeSet::new();
        for entry in &accepted {
            let r = &entry.parsed.record;
            let c = r.common();
            let s = c.scope;
            if !s.repos.is_empty() && !s.repos.iter().any(|r| r == repo) {
                continue;
            }
            if !s.modules.is_empty() && !s.modules.contains(&module.id) {
                continue;
            }
            if !s.features.is_empty() && !s.features.iter().any(|f| module.features.contains(f)) {
                continue;
            }
            let model_for_module =
                matches!(r, Record::Feature(f) if module.features.contains(&f.feature));
            if !s.modules.is_empty() || !s.features.is_empty() || model_for_module {
                domain_records.insert(c.id.to_string());
            } else {
                broad_records.insert(c.id.to_string());
            }
        }
        let fan_in = fan_in.map(|f| f.get(&module.id).copied().unwrap_or(0));
        modules.push(ModuleCoverage {
            id: module.id.clone(),
            files,
            churn,
            fan_in,
            rank_score: fan_in.map_or(churn, |f| churn.saturating_mul(f)),
            only_broad_records: domain_records.is_empty(),
            domain_records: domain_records.into_iter().collect(),
            broad_records: broad_records.into_iter().collect(),
        });
    }
    modules.sort_by(|a, b| {
        b.only_broad_records
            .cmp(&a.only_broad_records)
            .then_with(|| b.rank_score.cmp(&a.rank_score))
            .then_with(|| b.churn.cmp(&a.churn))
            .then_with(|| a.id.cmp(&b.id))
    });
    let total_modules = modules.len();
    modules.truncate(limit);
    let mut features = Vec::new();
    let mut contracts_without_consumers = BTreeSet::new();
    let mut features_without_scenarios = BTreeSet::new();
    let mut checklists_by_change_type: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut glossary_terms = 0;
    for feature in &registry.data.features {
        if !feature.repos.is_empty() && !feature.repos.iter().any(|r| r == repo) {
            continue;
        }
        let mut ids = BTreeSet::new();
        let mut scenarios = 0;
        for entry in &accepted {
            if let Record::Feature(f) = &entry.parsed.record
                && f.feature == feature.id
            {
                ids.insert(f.id.clone());
                scenarios += f.scenarios.len();
                if f.scenarios.is_empty() {
                    features_without_scenarios.insert(f.id.clone());
                }
            }
        }
        features.push(FeatureCoverage {
            id: feature.id.clone(),
            records: ids.into_iter().collect(),
            scenarios,
        });
    }
    features.sort_by(|a, b| a.id.cmp(&b.id));
    for entry in &accepted {
        let r = &entry.parsed.record;
        let meta = r.meta(Vec::new());
        if crate::scope::implied_repos(&meta.scope, registry)
            .is_some_and(|repos| !repos.contains(repo))
        {
            continue;
        }
        if let Record::Contract(c) = r
            && c.consumers.is_empty()
        {
            contracts_without_consumers.insert(c.id.clone());
        }
        if let Record::Reference(r) = r {
            glossary_terms += r.terms.len();
        }
        if r.kind() == Kind::Procedure {
            for kind in &meta.scope.change_types {
                checklists_by_change_type
                    .entry(kind.clone())
                    .or_default()
                    .push(meta.id.clone());
            }
        }
    }
    for ids in checklists_by_change_type.values_mut() {
        ids.sort();
        ids.dedup();
    }
    let mut unmapped_paths: Vec<_> = paths
        .iter()
        .filter(|p| registry.modules_for_path(repo, p).is_empty())
        .cloned()
        .collect();
    unmapped_paths.sort();
    unmapped_paths.dedup();
    CoverageReport {
        repo: repo.into(),
        tracked_files: paths.len(),
        total_modules,
        ranking: if fan_in.is_some() {
            "uncovered first, then churn × fan-in"
        } else {
            "uncovered first, then churn (provider fan-in unavailable)"
        }
        .into(),
        modules,
        features,
        unmapped_paths,
        contracts_without_consumers: contracts_without_consumers.into_iter().collect(),
        features_without_scenarios: features_without_scenarios.into_iter().collect(),
        checklists_by_change_type,
        glossary_terms,
        note: "Registry scope is a coverage signal, not proof of semantic completeness. Drafts do not count as accepted coverage.",
    }
}
