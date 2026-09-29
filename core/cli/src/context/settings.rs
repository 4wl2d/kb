//! Effective policy settings (docs/architecture.md §3.7).
//!
//! The effective value of a setting is its base value unless applicable overrides exist;
//! then the most specific applicable overrides decide: those for which no other applicable
//! override is strictly more specific (its scope is subsumed by theirs but does not subsume
//! it). If they all carry the same value, that value wins (with one most specific override,
//! it alone); otherwise the setting is a conflict between them. This matches validation's
//! pairwise `OVERRIDE_AMBIGUOUS` rule, so a validated corpus has no setting conflict. String
//! sets are compared as sets. Overrides that break the override rules are ignored with a
//! warning (validation reports them as errors).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::diag::Diagnostic;
use crate::model::{
    Override, OverrideMode, RecordMeta, Registry, Setting, SettingType, SettingValue, Stricter,
};
use crate::scope::subsumes;

/// Where an effective value comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingSource {
    Base,
    Override {
        by: String,
        reason: String,
    },
    /// The most specific applicable overrides (none strictly more specific) disagree.
    Conflict {
        candidates: Vec<(String, SettingValue)>,
    },
}

/// The effective value of one named policy setting for the task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveSetting {
    /// Policy that declares the setting.
    pub policy: String,
    /// The declared setting (base value, override rules).
    pub setting: Setting,
    /// Effective value (`None` for a conflict).
    pub value: Option<SettingValue>,
    pub source: SettingSource,
    /// Policies with overrides of this setting whose applicability is undetermined.
    pub undetermined_overrides: Vec<String>,
}

impl EffectiveSetting {
    /// `<policy-id>#<setting-name>`.
    pub fn target(&self) -> String {
        format!("{}#{}", self.policy, self.setting.name)
    }

    pub fn is_conflict(&self) -> bool {
        matches!(self.source, SettingSource::Conflict { .. })
    }
}

/// Compute effective settings of `targets` (policies in the mandatory tier) under overrides
/// from `applying` policies (accepted, applicable). Returns settings in (policy id,
/// declaration) order plus warnings for ignored overrides.
pub(super) fn effective(
    targets: &[Arc<RecordMeta>],
    applying: &[Arc<RecordMeta>],
    undetermined: &[Arc<RecordMeta>],
    registry: &Registry,
) -> (Vec<EffectiveSetting>, Vec<Diagnostic>) {
    let mut by_target: BTreeMap<&str, Vec<(&RecordMeta, &Override)>> = BTreeMap::new();
    for p in applying {
        for o in &p.overrides {
            by_target.entry(o.target.as_str()).or_default().push((p, o));
        }
    }
    let mut undetermined_by_target: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for p in undetermined {
        for o in &p.overrides {
            undetermined_by_target
                .entry(o.target.as_str())
                .or_default()
                .push(p.id.clone());
        }
    }
    let mut diags = Vec::new();
    let mut out = Vec::new();
    for t in targets {
        for s in &t.settings {
            let key = format!("{}#{}", t.id, s.name);
            let mut valid: Vec<(&RecordMeta, &Override)> = Vec::new();
            for &(p, o) in by_target.get(key.as_str()).into_iter().flatten() {
                match check_override(t, s, p, o, registry) {
                    Ok(()) => valid.push((p, o)),
                    Err((code, msg)) => diags.push(
                        Diagnostic::warning(code, format!("override of `{key}` ignored: {msg}"))
                            .for_record(p.id.clone()),
                    ),
                }
            }
            valid.sort_by(|a, b| a.0.id.cmp(&b.0.id));
            let (value, source) = resolve(s, &valid, registry);
            let mut undetermined_overrides = undetermined_by_target
                .get(key.as_str())
                .cloned()
                .unwrap_or_default();
            undetermined_overrides.sort();
            undetermined_overrides.dedup();
            out.push(EffectiveSetting {
                policy: t.id.clone(),
                setting: s.clone(),
                value,
                source,
                undetermined_overrides,
            });
        }
    }
    (out, diags)
}

fn resolve(
    s: &Setting,
    valid: &[(&RecordMeta, &Override)],
    registry: &Registry,
) -> (Option<SettingValue>, SettingSource) {
    if valid.is_empty() {
        return (Some(s.value.clone()), SettingSource::Base);
    }
    // Minimal elements under strict specificity: no other override is strictly narrower.
    let mut minimal: Vec<&(&RecordMeta, &Override)> = valid
        .iter()
        .filter(|(c, co)| {
            !valid.iter().any(|(d, dov)| {
                !std::ptr::eq(*co, *dov)
                    && subsumes(&c.scope, &d.scope, registry)
                    && !subsumes(&d.scope, &c.scope, registry)
            })
        })
        .collect();
    if minimal.is_empty() {
        // Only possible if scope subsumption were not transitive; decide over all of them.
        minimal = valid.iter().collect();
    }
    if let Some((first, o)) = minimal.first()
        && minimal.iter().all(|(_, x)| same_value(&x.value, &o.value))
    {
        return (
            Some(o.value.clone()),
            SettingSource::Override {
                by: first.id.clone(),
                reason: o.reason.clone(),
            },
        );
    }
    (
        None,
        SettingSource::Conflict {
            candidates: minimal
                .iter()
                .map(|(p, o)| (p.id.clone(), o.value.clone()))
                .collect(),
        },
    )
}

/// Equal setting values; string sets compare as sets (as validation's `OVERRIDE_AMBIGUOUS`
/// check does).
fn same_value(a: &SettingValue, b: &SettingValue) -> bool {
    match (a, b) {
        (SettingValue::StringSet(x), SettingValue::StringSet(y)) => {
            x.iter().collect::<BTreeSet<_>>() == y.iter().collect::<BTreeSet<_>>()
        }
        _ => a == b,
    }
}

/// Context-time re-check of the override rules (§3.7 rules 2–6; rule 1 holds by lookup).
fn check_override(
    target: &RecordMeta,
    s: &Setting,
    source: &RecordMeta,
    o: &Override,
    registry: &Registry,
) -> Result<(), (&'static str, String)> {
    if s.r#override == OverrideMode::Forbidden {
        return Err((
            "OVERRIDE_FORBIDDEN",
            "the setting is not overridable".into(),
        ));
    }
    if o.value.type_of() != s.value_type {
        return Err((
            "OVERRIDE_TYPE_MISMATCH",
            format!("value is not of type {}", type_name(s.value_type)),
        ));
    }
    if s.r#override == OverrideMode::Stricter
        && let Some(dir) = s.stricter
        && weakens(&s.value, &o.value, dir)
    {
        return Err((
            "OVERRIDE_WEAKENS",
            format!("{} is weaker than {}", o.value.display(), s.value.display()),
        ));
    }
    if !subsumes(&target.scope, &source.scope, registry) {
        return Err((
            "OVERRIDE_SCOPE_EXCEEDS",
            format!("scope of {} exceeds the scope of {}", source.id, target.id),
        ));
    }
    if !s.override_owners.is_empty() && !s.override_owners.contains(&source.owner) {
        return Err((
            "OVERRIDE_NOT_AUTHORIZED",
            format!("owner `{}` is not in override_owners", source.owner),
        ));
    }
    Ok(())
}

/// Does `new` weaken `base` in the given stricter direction?
fn weakens(base: &SettingValue, new: &SettingValue, dir: Stricter) -> bool {
    use SettingValue as V;
    match (base, new, dir) {
        (V::Integer(b), V::Integer(n), Stricter::Lower) => n > b,
        (V::Integer(b), V::Integer(n), Stricter::Higher) => n < b,
        (V::Boolean(b), V::Boolean(n), Stricter::True) => *b && !*n,
        (V::Boolean(b), V::Boolean(n), Stricter::False) => !*b && *n,
        (V::StringSet(b), V::StringSet(n), Stricter::Superset) => !b.iter().all(|x| n.contains(x)),
        (V::StringSet(b), V::StringSet(n), Stricter::Subset) => !n.iter().all(|x| b.contains(x)),
        _ => false,
    }
}

pub(super) fn type_name(t: SettingType) -> &'static str {
    match t {
        SettingType::Integer => "integer",
        SettingType::Boolean => "boolean",
        SettingType::String => "string",
        SettingType::StringSet => "string-set",
    }
}

pub(super) fn mode_name(m: OverrideMode) -> &'static str {
    match m {
        OverrideMode::Forbidden => "forbidden",
        OverrideMode::Stricter => "stricter",
        OverrideMode::Any => "any",
    }
}

pub(super) fn stricter_name(s: Stricter) -> &'static str {
    match s {
        Stricter::Lower => "lower",
        Stricter::Higher => "higher",
        Stricter::True => "true",
        Stricter::False => "false",
        Stricter::Superset => "superset",
        Stricter::Subset => "subset",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weakening_rules() {
        use SettingValue as V;
        assert!(weakens(&V::Integer(10), &V::Integer(20), Stricter::Lower));
        assert!(!weakens(&V::Integer(10), &V::Integer(5), Stricter::Lower));
        assert!(weakens(
            &V::Boolean(true),
            &V::Boolean(false),
            Stricter::True
        ));
        let set = |v: &[&str]| V::StringSet(v.iter().map(|s| s.to_string()).collect());
        assert!(weakens(&set(&["a", "b"]), &set(&["a"]), Stricter::Superset));
        assert!(!weakens(
            &set(&["a"]),
            &set(&["a", "b"]),
            Stricter::Superset
        ));
        assert!(weakens(&set(&["a"]), &set(&["a", "b"]), Stricter::Subset));
    }

    #[test]
    fn string_sets_agree_regardless_of_order() {
        use SettingValue as V;
        let set = |v: &[&str]| V::StringSet(v.iter().map(|s| s.to_string()).collect());
        assert!(same_value(&set(&["a", "b"]), &set(&["b", "a", "a"])));
        assert!(!same_value(&set(&["a"]), &set(&["a", "b"])));
        assert!(same_value(&V::Integer(1), &V::Integer(1)));
        assert!(!same_value(&V::Integer(1), &V::String("1".into())));
    }
}
