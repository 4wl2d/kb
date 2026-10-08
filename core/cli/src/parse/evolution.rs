//! Context-free checks for schema-2 additions.

use std::collections::BTreeSet;

use super::{check_items, check_text, check_text_list, check_unique_ids, check_unique_list};
use crate::diag::Diagnostic;
use crate::glob::RepoGlob;
use crate::model::ids::{check_local_id, check_registry_id};
use crate::model::*;
use crate::parse::MAX_LIST_LEN;
use crate::util::check_rel_path;

pub(super) fn schema_two_field(table: &toml::Table) -> Option<String> {
    if table
        .get("selectors")
        .and_then(toml::Value::as_table)
        .and_then(|s| s.get("intents"))
        .and_then(toml::Value::as_array)
        .is_some_and(|values| values.iter().any(|v| v.as_str() == Some("diagnose")))
    {
        return Some("selectors.intents.diagnose".into());
    }
    for key in [
        "introduced",
        "retired",
        "verified_at",
        "review_by",
        "delivery",
        "states",
        "transitions",
        "scenarios",
        "clocks",
        "data_sources",
        "consumers",
        "terms",
    ] {
        if table.contains_key(key) {
            return Some(key.into());
        }
    }
    if table
        .get("scope")
        .and_then(toml::Value::as_table)
        .is_some_and(|s| s.contains_key("change_types"))
    {
        return Some("scope.change_types".into());
    }
    for (list, field) in [
        ("anchors", "stamp"),
        ("rules", "verify"),
        ("statements", "verify"),
        ("obligations", "verify"),
    ] {
        if table
            .get(list)
            .and_then(toml::Value::as_array)
            .is_some_and(|a| {
                a.iter()
                    .any(|v| v.as_table().is_some_and(|t| t.contains_key(field)))
            })
        {
            return Some(format!("{list}.{field}"));
        }
    }
    None
}

pub(super) fn check(record: &Record) -> Vec<Diagnostic> {
    let mut d = Vec::new();
    let c = record.common();
    for (field, value, date_only) in [
        ("introduced", c.introduced, false),
        ("retired", c.retired, false),
        ("verified_at", c.verified_at, true),
        ("review_by", c.review_by, true),
    ] {
        if let Some(value) = value
            && date_days(value).is_none()
            && (date_only || !is_commit(value))
        {
            d.push(Diagnostic::error(
                "TEMPORAL_INVALID",
                format!(
                    "{field} must be {}",
                    if date_only {
                        "an ISO calendar date"
                    } else {
                        "an ISO calendar date or 7..=64 hex commit id"
                    }
                ),
            ));
        }
    }
    if let (Some(start), Some(end)) = (c.introduced, c.retired) {
        match (date_days(start), date_days(end)) {
            (Some(a), Some(b)) if a >= b => d.push(Diagnostic::error(
                "TEMPORAL_ORDER",
                "retired must follow introduced (exclusive upper bound)",
            )),
            (Some(_), None) | (None, Some(_)) => d.push(Diagnostic::error(
                "TEMPORAL_KIND_MISMATCH",
                "introduced and retired must use the same kind of bound",
            )),
            _ => {}
        }
    }
    if let (Some(a), Some(b)) = (
        c.verified_at.and_then(date_days),
        c.review_by.and_then(date_days),
    ) && b < a
    {
        d.push(Diagnostic::error(
            "FRESHNESS_ORDER",
            "review_by must not precede verified_at",
        ));
    }
    check_unique_list(&mut d, "scope.change_types", &c.scope.change_types);
    for id in &c.scope.change_types {
        if let Err(e) = check_local_id(id) {
            d.push(Diagnostic::error(
                "CHANGE_TYPE_INVALID",
                format!("{id}: {e}"),
            ));
        }
    }
    if record.delivery() == Delivery::Always
        && (!c.scope.product || !c.scope.change_types.is_empty() || c.applicability.is_some())
    {
        d.push(Diagnostic::error("DELIVERY_SCOPE_INVALID",
            "always delivery requires unconditional product scope without version or change-type constraints"));
    }
    for a in c.anchors {
        if let Some(s) = &a.stamp {
            if !is_commit(&s.commit)
                || s.start_line == 0
                || s.end_line < s.start_line
                || s.sha256.len() != 64
                || !s
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || (a.repo.is_none() && a.kind != crate::model::AnchorKind::Doc)
                || a.path.is_none()
            {
                d.push(Diagnostic::error("ANCHOR_STAMP_INVALID",
                    "stamp needs a path (and repo except for KB docs), a commit, an inclusive nonzero line range and a lowercase SHA-256"));
            }
            if let Some(commit) = &a.commit
                && !s.commit.starts_with(commit)
                && !commit.starts_with(&s.commit)
            {
                d.push(Diagnostic::error(
                    "ANCHOR_STAMP_COMMIT_MISMATCH",
                    "anchor commit and stamp commit disagree",
                ));
            }
        }
    }
    for statement in record.normative() {
        if statement.verify.len() > MAX_LIST_LEN {
            d.push(Diagnostic::error(
                "LIST_TOO_LONG",
                "too many verification probes",
            ));
        }
        for probe in statement.verify {
            check_probe(&mut d, probe);
        }
    }
    match record {
        Record::Feature(f) => {
            check_items(&mut d, "states", &f.states);
            check_unique_ids(
                &mut d,
                "states",
                &f.states.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            );
            let states: BTreeSet<_> = f.states.iter().map(|s| s.id.as_str()).collect();
            check_unique_ids(
                &mut d,
                "transitions",
                &f.transitions
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>(),
            );
            check_len(&mut d, "states", f.states.len());
            check_len(&mut d, "transitions", f.transitions.len());
            for t in &f.transitions {
                check_id(&mut d, "transitions.id", &t.id);
                check_text(&mut d, "transitions.when", &t.when);
                if !states.contains(t.from.as_str()) || !states.contains(t.to.as_str()) {
                    d.push(Diagnostic::error(
                        "TRANSITION_STATE_UNKNOWN",
                        format!("transition {} must name declared from/to states", t.id),
                    ));
                }
            }
            check_scenarios(&mut d, &f.scenarios);
            check_text_list(&mut d, "clocks", &f.clocks);
            check_text_list(&mut d, "data_sources", &f.data_sources);
        }
        Record::Contract(k) => {
            check_scenarios(&mut d, &k.scenarios);
            check_len(&mut d, "consumers", k.consumers.len());
            let mut seen = BTreeSet::new();
            for c in &k.consumers {
                if check_registry_id(&c.repo).is_err() || check_rel_path(&c.path).is_err() {
                    d.push(Diagnostic::error(
                        "CONSUMER_INVALID",
                        "consumer needs a repo id and safe repo-relative path",
                    ));
                }
                if let Some(s) = &c.symbol {
                    check_text(&mut d, "consumers.symbol", s);
                }
                if !seen.insert((&c.repo, &c.path, &c.symbol)) {
                    d.push(Diagnostic::error(
                        "CONSUMER_DUPLICATE",
                        "duplicate contract consumer",
                    ));
                }
            }
        }
        Record::Reference(r) => {
            check_len(&mut d, "terms", r.terms.len());
            let mut seen = BTreeSet::new();
            for t in &r.terms {
                check_text(&mut d, "terms.term", &t.term);
                check_text(&mut d, "terms.meaning", &t.meaning);
                check_text(&mut d, "terms.source", &t.source);
                if !seen.insert(crate::normalize::normalize(&t.term)) {
                    d.push(Diagnostic::error(
                        "TERM_DUPLICATE",
                        format!("duplicate glossary term {}", t.term),
                    ));
                }
            }
        }
        _ => {}
    }
    d
}

fn check_id(d: &mut Vec<Diagnostic>, field: &str, id: &str) {
    if let Err(e) = check_local_id(id) {
        d.push(Diagnostic::error(
            "LOCAL_ID_INVALID",
            format!("{field}: {e}"),
        ));
    }
}

fn check_len(d: &mut Vec<Diagnostic>, field: &str, len: usize) {
    if len > MAX_LIST_LEN {
        d.push(Diagnostic::error(
            "LIST_TOO_LONG",
            format!("{field} exceeds {MAX_LIST_LEN} items"),
        ));
    }
}

fn check_scenarios(d: &mut Vec<Diagnostic>, scenarios: &[Scenario]) {
    check_len(d, "scenarios", scenarios.len());
    check_unique_ids(
        d,
        "scenarios",
        &scenarios.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
    );
    for s in scenarios {
        check_id(d, "scenarios.id", &s.id);
        check_text(d, "scenarios.given", &s.given);
        check_text(d, "scenarios.expect", &s.expect);
    }
}

fn check_probe(d: &mut Vec<Diagnostic>, probe: &VerifyProbe) {
    let (globs, pattern): (Vec<&String>, Option<&str>) = match probe {
        VerifyProbe::CommitMessage { pattern } | VerifyProbe::BranchName { pattern } => {
            (vec![], Some(pattern))
        }
        VerifyProbe::ForbiddenImport { from, to } => {
            if from.is_empty() || to.is_empty() {
                d.push(Diagnostic::error(
                    "VERIFY_INVALID",
                    "forbidden-import needs nonempty from/to globs",
                ));
            }
            check_len(d, "verify.from", from.len());
            check_len(d, "verify.to", to.len());
            (from.iter().chain(to).collect(), None)
        }
        VerifyProbe::Naming { paths, pattern } | VerifyProbe::BannedApi { paths, pattern } => {
            if paths.is_empty() {
                d.push(Diagnostic::error(
                    "VERIFY_INVALID",
                    "probe needs nonempty paths",
                ));
            }
            check_len(d, "verify.paths", paths.len());
            (paths.iter().collect(), Some(pattern))
        }
    };
    for glob in globs {
        if let Err(e) = RepoGlob::parse(glob) {
            d.push(Diagnostic::error("VERIFY_GLOB_INVALID", e));
        }
    }
    if let Some(pattern) = pattern {
        check_text(d, "verify.pattern", pattern);
        if let Err(e) = regex::RegexBuilder::new(pattern)
            .size_limit(1 << 20)
            .build()
        {
            d.push(Diagnostic::error("VERIFY_REGEX_INVALID", e.to_string()));
        }
    }
}
