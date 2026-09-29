//! Pure scope algebra shared by validation and context assembly
//! (docs/architecture.md §3.3 and §3.7).

use std::collections::BTreeSet;

use crate::model::{Registry, Scope};

/// Repos a scope can apply to: `None` = unconstrained (any repo).
///
/// Explicit `repos` win; otherwise the repos implied by constrained modules and features
/// (intersected, since dimensions are ANDed); otherwise unconstrained.
pub fn implied_repos(scope: &Scope, registry: &Registry) -> Option<BTreeSet<String>> {
    if scope.product {
        return None;
    }
    if !scope.repos.is_empty() {
        return Some(scope.repos.iter().cloned().collect());
    }
    let mut acc: Option<BTreeSet<String>> = None;
    if !scope.modules.is_empty() {
        acc = Some(registry.repos_of_modules(&scope.modules));
    }
    if !scope.features.is_empty() {
        let f = registry.repos_of_features(&scope.features);
        // Features without declared repos do not constrain the repo dimension.
        if !f.is_empty() {
            acc = Some(match acc {
                Some(a) => a.intersection(&f).cloned().collect(),
                None => f,
            });
        }
    }
    acc
}

fn subset(a: &[String], b: &[String]) -> bool {
    let bs: BTreeSet<&String> = b.iter().collect();
    a.iter().all(|x| bs.contains(x))
}

/// `a ⊆ b`: every task to which `a` applies is also one to which `b` applies.
pub fn subsumes(b: &Scope, a: &Scope, registry: &Registry) -> bool {
    if b.product {
        return true;
    }
    if a.product {
        return false;
    }
    if !b.repos.is_empty() {
        match implied_repos(a, registry) {
            Some(ra) => {
                if !ra.iter().all(|r| b.repos.contains(r)) {
                    return false;
                }
            }
            None => return false,
        }
    }
    if !b.modules.is_empty() && (a.modules.is_empty() || !subset(&a.modules, &b.modules)) {
        return false;
    }
    if !b.features.is_empty() && (a.features.is_empty() || !subset(&a.features, &b.features)) {
        return false;
    }
    true
}

/// Could some task be in both scopes? Conservative: returns true unless a dimension
/// constrained by both is provably disjoint.
pub fn overlap(a: &Scope, b: &Scope, registry: &Registry) -> bool {
    if a.product || b.product {
        return true;
    }
    if let (Some(ra), Some(rb)) = (implied_repos(a, registry), implied_repos(b, registry))
        && ra.is_disjoint(&rb)
    {
        return false;
    }
    let disjoint = |x: &[String], y: &[String]| {
        !x.is_empty() && !y.is_empty() && !x.iter().any(|v| y.contains(v))
    };
    !(disjoint(&a.modules, &b.modules) || disjoint(&a.features, &b.features))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RegistryData;
    use crate::model::registry::{Feature, Module, Repo};

    fn reg() -> Registry {
        let repo = |id: &str| Repo {
            id: id.into(),
            title: id.into(),
            remotes: vec![],
            version_file: None,
        };
        Registry::new(RegistryData {
            repos: vec![repo("mobile"), repo("backend")],
            modules: vec![
                Module {
                    id: "mobile.auth".into(),
                    repo: "mobile".into(),
                    title: "a".into(),
                    paths: vec![],
                    features: vec![],
                },
                Module {
                    id: "mobile.pay".into(),
                    repo: "mobile".into(),
                    title: "p".into(),
                    paths: vec![],
                    features: vec![],
                },
            ],
            features: vec![Feature {
                id: "login".into(),
                title: "l".into(),
                repos: vec!["mobile".into(), "backend".into()],
                paths: vec![],
            }],
            ..Default::default()
        })
    }

    fn s(repos: &[&str], modules: &[&str], features: &[&str]) -> Scope {
        Scope {
            product: false,
            repos: repos.iter().map(|x| x.to_string()).collect(),
            modules: modules.iter().map(|x| x.to_string()).collect(),
            features: features.iter().map(|x| x.to_string()).collect(),
        }
    }

    #[test]
    fn subsumption() {
        let r = reg();
        let product = Scope {
            product: true,
            ..Default::default()
        };
        assert!(subsumes(&product, &s(&["mobile"], &[], &[]), &r));
        assert!(subsumes(
            &s(&["mobile"], &[], &[]),
            &s(&[], &["mobile.auth"], &[]),
            &r
        ));
        assert!(!subsumes(
            &s(&["mobile"], &[], &[]),
            &s(&[], &[], &["login"]),
            &r
        ));
        assert!(!subsumes(
            &s(&[], &["mobile.auth"], &[]),
            &s(&["mobile"], &[], &[]),
            &r
        ));
        assert!(subsumes(
            &s(&["mobile"], &[], &[]),
            &s(&["mobile"], &["mobile.pay"], &[]),
            &r
        ));
    }

    #[test]
    fn overlaps() {
        let r = reg();
        assert!(!overlap(
            &s(&["mobile"], &[], &[]),
            &s(&["backend"], &[], &[]),
            &r
        ));
        assert!(!overlap(
            &s(&[], &["mobile.auth"], &[]),
            &s(&[], &["mobile.pay"], &[]),
            &r
        ));
        assert!(overlap(
            &s(&["mobile"], &[], &[]),
            &s(&[], &["mobile.pay"], &[]),
            &r
        ));
    }
}
