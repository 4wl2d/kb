//! Record applicability to a task (docs/architecture.md §3.3).
//!
//! Scope: for every non-empty record dimension, a Known task dimension applies iff the sets
//! intersect (otherwise NotApplicable), an Unknown task dimension is Undetermined. AND across
//! dimensions, OR within a dimension; product-wide records apply to every task.
//!
//! Registry narrowing: when the task's modules (features) are Unknown but its repos are
//! Known, a record whose modules (features) all belong to repos outside the task repos is
//! NotApplicable instead of Undetermined — the registry proves it cannot apply.
//!
//! Versions: each `applicability.versions` entry is checked against the host version of that
//! repo (`--host-version` or detected; unknown version → Undetermined, non-matching →
//! NotApplicable). Whenever a version of the entry's repo is known it is checked. For
//! mandatory selection ([`evaluate`]) an entry whose repo is outside Known task repos and has
//! no known version is skipped (the task does not touch that repo, and only the host repo's
//! version is detected). Required dependencies ([`evaluate_required`]) never skip an entry:
//! they are included regardless of scope, so an unknown version stays Undetermined.

use std::collections::BTreeSet;

use crate::model::RecordMeta;

use super::task::{DimScope, TaskScope};

/// A scope or version dimension of applicability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dim {
    Repos,
    Modules,
    Features,
    ChangeTypes,
    Version,
}

impl Dim {
    pub fn as_str(self) -> &'static str {
        match self {
            Dim::Repos => "repos",
            Dim::Modules => "modules",
            Dim::Features => "features",
            Dim::ChangeTypes => "change_types",
            Dim::Version => "version",
        }
    }
}

/// Overall applicability verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verdict {
    Applies,
    Undetermined,
    NotApplicable,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Applies => "applies",
            Verdict::Undetermined => "undetermined",
            Verdict::NotApplicable => "not-applicable",
        }
    }
}

/// Per-dimension applicability of one record to one task.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applicability {
    pub product: bool,
    /// Dimensions the record constrains and the task satisfies.
    pub matched: BTreeSet<Dim>,
    pub undetermined: BTreeSet<Dim>,
    pub not_applicable: BTreeSet<Dim>,
    /// Scope findings from registry narrowing, e.g. `modules belong to other repos`.
    pub scope_notes: Vec<String>,
    /// Version findings, e.g. `mobile 1.9.0 does not satisfy <2.0.0`.
    pub version_notes: Vec<String>,
}

impl Applicability {
    pub fn verdict(&self) -> Verdict {
        if !self.not_applicable.is_empty() {
            Verdict::NotApplicable
        } else if !self.undetermined.is_empty() {
            Verdict::Undetermined
        } else {
            Verdict::Applies
        }
    }

    /// Verdict of the version dimension alone. Required dependencies ignore scope filters
    /// but not version applicability.
    pub fn version_verdict(&self) -> Verdict {
        if self.not_applicable.contains(&Dim::Version) {
            Verdict::NotApplicable
        } else if self.undetermined.contains(&Dim::Version) {
            Verdict::Undetermined
        } else {
            Verdict::Applies
        }
    }

    /// Unknown dimensions as a comma-separated list.
    pub fn undetermined_dims(&self) -> String {
        join_dims(&self.undetermined)
    }

    /// Short explanation, e.g. `applies (repos, modules)` or `undetermined: modules unknown`.
    pub fn describe(&self) -> String {
        let mut out = match self.verdict() {
            Verdict::Applies if self.product && self.matched.is_empty() => {
                "applies (product-wide)".to_string()
            }
            Verdict::Applies if self.product => {
                format!("applies (product-wide, {})", join_dims(&self.matched))
            }
            Verdict::Applies => format!("applies ({})", join_dims(&self.matched)),
            Verdict::Undetermined => format!("undetermined: {} unknown", self.undetermined_dims()),
            Verdict::NotApplicable => {
                format!("not applicable: {}", join_dims(&self.not_applicable))
            }
        };
        for n in self.scope_notes.iter().chain(&self.version_notes) {
            out.push_str("; ");
            out.push_str(n);
        }
        out
    }
}

fn join_dims(s: &BTreeSet<Dim>) -> String {
    s.iter().map(|d| d.as_str()).collect::<Vec<_>>().join(", ")
}

/// Evaluate a record against the task scope.
pub fn evaluate(meta: &RecordMeta, task: &TaskScope) -> Applicability {
    evaluate_with(meta, task, true)
}

/// Evaluate a record reached through `requires`: like [`evaluate`], but every version
/// constraint is checked, including those on repos outside the task repos.
pub fn evaluate_required(meta: &RecordMeta, task: &TaskScope) -> Applicability {
    evaluate_with(meta, task, false)
}

fn evaluate_with(meta: &RecordMeta, task: &TaskScope, skip_foreign_unknown: bool) -> Applicability {
    evaluate_parts(
        &meta.scope,
        meta.applicability.as_ref(),
        task,
        skip_foreign_unknown,
    )
}

/// Evaluate a saved declaration without fabricating a full record (local usage joins).
pub fn evaluate_saved_scope(
    scope: &crate::model::Scope,
    versions: Option<&crate::model::Applicability>,
    task: &TaskScope,
) -> Applicability {
    evaluate_parts(scope, versions, task, true)
}

fn evaluate_parts(
    s: &crate::model::Scope,
    versions: Option<&crate::model::Applicability>,
    task: &TaskScope,
    skip_foreign_unknown: bool,
) -> Applicability {
    let mut a = Applicability::default();
    if s.product {
        a.product = true;
    } else {
        dimension(&mut a, Dim::Repos, &s.repos, &task.repos, None);
        dimension(
            &mut a,
            Dim::Modules,
            &s.modules,
            &task.modules,
            Some(&task.foreign_modules),
        );
        dimension(
            &mut a,
            Dim::Features,
            &s.features,
            &task.features,
            Some(&task.foreign_features),
        );
    }
    if !task.change_types.is_known()
        && s.change_types
            .iter()
            .any(|t| task.change_type_hints.contains_key(t))
    {
        a.matched.insert(Dim::ChangeTypes);
        a.scope_notes
            .push("change type inferred from positive task/path evidence".into());
    } else {
        dimension(
            &mut a,
            Dim::ChangeTypes,
            &s.change_types,
            &task.change_types,
            None,
        );
    }
    if let Some(app) = versions {
        for (repo, req) in &app.versions {
            let host = task.host_versions.get(repo);
            if skip_foreign_unknown
                && host.is_none()
                && let Some(known) = task.repos.known()
                && !known.contains(repo)
            {
                continue;
            }
            let Ok(parsed) = semver::VersionReq::parse(req) else {
                a.undetermined.insert(Dim::Version);
                a.version_notes
                    .push(format!("{repo}: invalid version requirement `{req}`"));
                continue;
            };
            match host {
                Some(v) if parsed.matches(v) => {
                    a.matched.insert(Dim::Version);
                    a.version_notes.push(format!("{repo} {v} satisfies {req}"));
                }
                Some(v) => {
                    a.not_applicable.insert(Dim::Version);
                    a.version_notes
                        .push(format!("{repo} {v} does not satisfy {req}"));
                }
                None => {
                    a.undetermined.insert(Dim::Version);
                    a.version_notes
                        .push(format!("{repo} version unknown (requires {req})"));
                }
            }
        }
    }
    a
}

fn dimension(
    a: &mut Applicability,
    dim: Dim,
    record: &[String],
    task: &DimScope,
    foreign: Option<&BTreeSet<String>>,
) {
    if record.is_empty() {
        return;
    }
    match task {
        DimScope::Known(set) => {
            if record.iter().any(|x| set.contains(x)) {
                a.matched.insert(dim);
            } else {
                a.not_applicable.insert(dim);
            }
        }
        DimScope::Unknown => {
            if let Some(foreign) = foreign
                && record.iter().all(|x| foreign.contains(x))
            {
                a.not_applicable.insert(dim);
                a.scope_notes
                    .push(format!("{} belong to other repos", dim.as_str()));
                return;
            }
            a.undetermined.insert(dim);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::model::{Applicability as RecordApplicability, Kind, Scope, Status};

    fn meta(scope: Scope, versions: &[(&str, &str)]) -> RecordMeta {
        RecordMeta {
            id: "acme.x".into(),
            kind: Kind::Policy,
            status: Status::Accepted,
            title: "x".into(),
            owner: "o".into(),
            scope,
            selectors: Default::default(),
            links: Default::default(),
            applicability: (!versions.is_empty()).then(|| RecordApplicability {
                versions: versions
                    .iter()
                    .map(|(r, v)| (r.to_string(), v.to_string()))
                    .collect(),
            }),
            anchors: vec![],
            consumers: vec![],
            probe_globs: vec![],
            settings: vec![],
            overrides: vec![],
            parties: vec![],
            feature: None,
            gap_affects: vec![],
            sections: vec![],
        }
    }

    fn task(repos: Option<&[&str]>, modules: Option<&[&str]>) -> TaskScope {
        let dim = |v: Option<&[&str]>| match v {
            Some(v) => DimScope::Known(v.iter().map(|s| s.to_string()).collect()),
            None => DimScope::Unknown,
        };
        TaskScope {
            repos: dim(repos),
            modules: dim(modules),
            features: DimScope::Unknown,
            change_types: DimScope::Unknown,
            change_type_hints: BTreeMap::new(),
            inferred_paths: Vec::new(),
            inferred_features: Vec::new(),
            concepts: BTreeMap::new(),
            paths: vec![],
            host_repo: None,
            host_repo_source: None,
            host_head: None,
            host_versions: BTreeMap::new(),
            foreign_modules: Default::default(),
            foreign_features: Default::default(),
            tokens: vec![],
            ambiguities: vec![],
            notes: vec![],
        }
    }

    fn scope(repos: &[&str], modules: &[&str]) -> Scope {
        Scope {
            product: false,
            repos: repos.iter().map(|s| s.to_string()).collect(),
            modules: modules.iter().map(|s| s.to_string()).collect(),
            features: vec![],
            change_types: vec![],
        }
    }

    #[test]
    fn change_types_are_conservative_when_unknown_and_precise_when_explicit() {
        let mut s = Scope {
            product: true,
            ..Default::default()
        };
        s.change_types = vec!["migration".into()];
        let record = meta(s, &[]);
        let mut request = task(Some(&["mobile"]), Some(&["mobile.auth"]));
        let unresolved = evaluate(&record, &request);
        assert_eq!(unresolved.verdict(), Verdict::Undetermined);
        assert!(unresolved.undetermined.contains(&Dim::ChangeTypes));
        request.change_types = DimScope::Known(["migration".into()].into_iter().collect());
        assert_eq!(evaluate(&record, &request).verdict(), Verdict::Applies);
        request.change_types = DimScope::Known(["ui-test".into()].into_iter().collect());
        let excluded = evaluate(&record, &request);
        assert_eq!(excluded.verdict(), Verdict::NotApplicable);
        assert!(excluded.not_applicable.contains(&Dim::ChangeTypes));
    }

    #[test]
    fn and_across_or_within_dimensions() {
        let t = task(Some(&["mobile"]), Some(&["mobile.auth"]));
        let m = meta(scope(&["mobile", "backend"], &["mobile.auth"]), &[]);
        assert_eq!(evaluate(&m, &t).verdict(), Verdict::Applies);
        let m = meta(scope(&["mobile"], &["mobile.ui"]), &[]);
        let a = evaluate(&m, &t);
        assert_eq!(a.verdict(), Verdict::NotApplicable);
        assert!(a.not_applicable.contains(&Dim::Modules));
        let t = task(Some(&["mobile"]), None);
        assert_eq!(evaluate(&m, &t).verdict(), Verdict::Undetermined);
        let product = meta(
            Scope {
                product: true,
                ..Default::default()
            },
            &[],
        );
        assert_eq!(
            evaluate(&product, &task(None, None)).verdict(),
            Verdict::Applies
        );
    }

    #[test]
    fn registry_narrowing_of_foreign_modules() {
        let mut t = task(Some(&["mobile"]), None);
        t.foreign_modules.insert("backend.api".into());
        let a = evaluate(&meta(scope(&[], &["backend.api"]), &[]), &t);
        assert_eq!(a.verdict(), Verdict::NotApplicable);
        assert!(a.describe().contains("modules belong to other repos"));
        let a = evaluate(&meta(scope(&[], &["mobile.auth"]), &[]), &t);
        assert_eq!(a.verdict(), Verdict::Undetermined);
    }

    #[test]
    fn version_applicability() {
        let m = meta(scope(&["mobile"], &[]), &[("mobile", ">=2.0.0")]);
        let mut t = task(Some(&["mobile"]), None);
        assert_eq!(evaluate(&m, &t).version_verdict(), Verdict::Undetermined);
        t.host_versions
            .insert("mobile".into(), semver::Version::new(2, 3, 0));
        assert_eq!(evaluate(&m, &t).verdict(), Verdict::Applies);
        t.host_versions
            .insert("mobile".into(), semver::Version::new(1, 9, 0));
        let a = evaluate(&m, &t);
        assert_eq!(a.verdict(), Verdict::NotApplicable);
        assert!(a.describe().contains("1.9.0 does not satisfy >=2.0.0"));
        // A constraint on a repo outside the known task repos is skipped for mandatory
        // selection while that repo's version is unknown ...
        let other = meta(scope(&["backend"], &[]), &[("mobile", ">=2.0.0")]);
        let mut t = task(Some(&["backend"]), None);
        assert_eq!(evaluate(&other, &t).verdict(), Verdict::Applies);
        // ... but never for a required dependency ...
        assert_eq!(
            evaluate_required(&other, &t).version_verdict(),
            Verdict::Undetermined
        );
        // ... and always checked once the version is known.
        t.host_versions
            .insert("mobile".into(), semver::Version::new(1, 9, 0));
        assert_eq!(evaluate(&other, &t).verdict(), Verdict::NotApplicable);
        assert_eq!(
            evaluate_required(&other, &t).version_verdict(),
            Verdict::NotApplicable
        );
    }
}
