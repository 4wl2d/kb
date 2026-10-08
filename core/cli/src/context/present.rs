//! Rendering of context results and records in compact text, human text and JSON.
//!
//! Each counted piece (header, every unit, receipt summary) is rendered by its own function
//! so that budget measurement and final output use identical bytes. Normative statements,
//! conditions, exceptions and section Markdown (including code blocks) are emitted verbatim
//! and never truncated.

use std::fmt::Write as _;

use serde_json::{Map, Value, json};

use crate::diag::{Diagnostic, Severity};
use crate::knowledge::{Freshness, Origin};
use crate::model::{
    Anchor, AnchorKind, BudgetUnit, GapKind, Item, NormativeRef, ParsedRecord, Record, Scope,
    Setting,
};
use crate::output::Format;
use crate::versions::{ENGINE_VERSION, PROTOCOL_ID, SKILL_PROTOCOL};

use super::settings::{mode_name, stricter_name, type_name};
use super::{
    ContextResult, EffectiveSetting, Footer, Header, SettingSource, TierCounts, Unit, UnitBody,
};

/// Reminder printed with every receipt (docs/architecture.md §5.2).
pub(super) const RECEIPT_NOTE: &str = "a receipt proves delivery, not understanding or \
     compliance; re-request context after compaction, a new session, a hand-off, or a scope \
     or snapshot change";

macro_rules! w {
    ($o:expr) => {{
        $o.push('\n');
    }};
    ($o:expr, $($arg:tt)*) => {{
        let _ = writeln!($o, $($arg)*);
    }};
}

fn short(s: &str) -> &str {
    match s.char_indices().nth(12) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

// ---------------------------------------------------------------------------------------
// JSON pieces as printed
// ---------------------------------------------------------------------------------------
//
// In JSON mode the counted payload is the `result` member of the `kb.cli.v1` envelope, which
// the CLI prints with `serde_json::to_string_pretty` (two-space indentation): `result` is a
// member of the top-level object, so its members are indented by two levels and its units by
// three. Each counted piece renders its part of that text at its final indentation, so the
// pieces sum to exactly the bytes of the printed `result` (from `{` to `}`):
//
// * header: `{`, then `,\n<indent>"key": <value>` per header member, then the same prefix for
//   `"units": [`;
// * unit: `,\n<indent><unit object>`;
// * footer: the units closing bracket, the footer members, and the closing brace.
//
// A pretty-printed object or array with n ≥ 1 entries has n − 1 commas and n + 1 newlines;
// counting `,\n` per entry leaves exactly the newline before the closing bracket unaccounted,
// so a closing bracket is rendered as `<indent>]` without its newline (and as a bare `]`
// after zero units, which print as `[]`). Key order does not change the byte count.

/// Nesting level of the `result` object in the pretty-printed CLI envelope.
const JSON_RESULT_LEVEL: usize = 1;

fn indent(level: usize) -> String {
    "  ".repeat(level)
}

/// `v` pretty-printed as a value nested at `level` (continuation lines indented).
fn pretty_at(v: &Value, level: usize) -> String {
    let s = serde_json::to_string_pretty(v).unwrap_or_default();
    // JSON strings escape newlines, so every newline is layout.
    s.replace('\n', &format!("\n{}", indent(level)))
}

/// `,\n<indent>"key": <value>` for a member of an object whose members are at `level`.
fn json_members(m: &Map<String, Value>, level: usize) -> String {
    let mut s = String::new();
    for (k, v) in m {
        s.push_str(",\n");
        s.push_str(&indent(level));
        s.push_str(&Value::String(k.clone()).to_string());
        s.push_str(": ");
        s.push_str(&pretty_at(v, level));
    }
    s
}

/// Characters that could manipulate a terminal or reorder displayed text: control
/// characters other than newline and tab, and Unicode bidirectional embedding/override/
/// isolate controls.
fn is_unsafe_char(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t')
        || ('\u{202A}'..='\u{202E}').contains(&c)
        || ('\u{2066}'..='\u{2069}').contains(&c)
}

/// Neutralize unsafe characters in text output as `\u{..}` escapes. Records are untrusted
/// data: they must not inject terminal escape sequences or visually reorder normative text.
/// JSON output escapes control characters by itself; `show --raw` stays byte-exact.
pub(super) fn safe_text(s: String) -> String {
    if !s.chars().any(is_unsafe_char) {
        return s;
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if is_unsafe_char(c) {
            let _ = write!(out, "\\u{{{:x}}}", c as u32);
        } else {
            out.push(c);
        }
    }
    out
}

fn origin_str(o: Origin) -> &'static str {
    match o {
        Origin::Accepted => "accepted",
        Origin::Proposal => "proposal",
    }
}

fn format_str(f: Format) -> &'static str {
    match f {
        Format::Compact => "compact",
        Format::Terse => "terse",
        Format::Human => "human",
        Format::Json => "json",
    }
}

fn severity_str(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

fn diag_text(d: &Diagnostic) -> String {
    let mut s = format!("{} {}", severity_str(d.severity), d.code);
    if let Some(p) = &d.path {
        s.push_str(&format!(" {p}"));
    }
    if let Some(r) = &d.record {
        s.push_str(&format!(" [{r}]"));
    }
    s.push_str(&format!(": {}", d.message));
    s
}

/// `product` or `repos=a,b; modules=m; features=f`.
pub(super) fn scope_text(s: &Scope) -> String {
    if s.product && s.change_types.is_empty() {
        return "product".into();
    }
    let mut parts = if s.product {
        vec!["product".to_string()]
    } else {
        Vec::new()
    };
    for (name, v) in [
        ("repos", &s.repos),
        ("modules", &s.modules),
        ("features", &s.features),
        ("change_types", &s.change_types),
    ] {
        if !v.is_empty() {
            parts.push(format!("{name}={}", v.join(",")));
        }
    }
    parts.join("; ")
}

fn setting_value_text(s: &EffectiveSetting) -> String {
    let mut line = format!("{} = ", s.target());
    match (&s.value, &s.source) {
        (Some(v), SettingSource::Base) => line.push_str(&format!("{} (base)", v.display())),
        (Some(v), SettingSource::Override { by, reason }) => line.push_str(&format!(
            "{} (override by {by}; base {}; reason: {reason})",
            v.display(),
            s.setting.value.display()
        )),
        (_, SettingSource::Conflict { candidates }) => {
            let c: Vec<String> = candidates
                .iter()
                .map(|(p, v)| format!("{p} = {}", v.display()))
                .collect();
            line.push_str(&format!(
                "CONFLICT between {} (no unique most specific override; base {})",
                c.join(", "),
                s.setting.value.display()
            ));
        }
        (None, _) => line.push_str("unknown"),
    }
    if !s.undetermined_overrides.is_empty() {
        line.push_str(&format!(
            "; may be overridden by {} (applicability undetermined)",
            s.undetermined_overrides.join(", ")
        ));
    }
    line
}

// ---------------------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------------------

pub(super) fn header(h: &Header, format: Format) -> String {
    match format {
        Format::Json => {
            let level = JSON_RESULT_LEVEL + 1;
            let mut s = String::from("{");
            s.push_str(&json_members(&header_json(h), level));
            s.push_str(&format!(",\n{}\"units\": [", indent(level)));
            s
        }
        Format::Compact => safe_text(header_text(h, false)),
        Format::Terse => safe_text(terse_header(h)),
        Format::Human => safe_text(header_text(h, true)),
    }
}

fn header_text(h: &Header, human: bool) -> String {
    let mut o = String::new();
    let s = &h.scope;
    let snap = &h.snapshot;
    let item = if human { "  - " } else { "- " };
    if human {
        w!(o, "KB CONTEXT (intent: {})", h.request.intent.as_str());
        w!(
            o,
            "engine {ENGINE_VERSION}, protocol {PROTOCOL_ID}, skill protocol {SKILL_PROTOCOL}"
        );
        w!(o);
    } else {
        w!(
            o,
            "# kb context intent={} engine={ENGINE_VERSION} skill_protocol={SKILL_PROTOCOL} protocol={PROTOCOL_ID}",
            h.request.intent.as_str()
        );
    }
    let approved = match snap.approved {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    };
    let mut line = format!(
        "snapshot: {}; approved={approved}; ref={} {}; source={}",
        snap.label(),
        snap.remote,
        snap.approved_ref,
        snap.source
    );
    if let Some(l) = &snap.latest_approved {
        line.push_str(&format!("; latest={}", short(l)));
    }
    if let Some(p) = &snap.pin {
        line.push_str(&format!("; pin={}", short(&p.revision)));
    }
    if let Some(ov) = &snap.overlay {
        line.push_str(&format!(
            "; overlay={} ({} files)",
            short(&ov.digest),
            ov.files
        ));
    }
    line.push_str(&format!("; key={}", short(&snap.key)));
    w!(o, "{line}");
    let versions: Vec<String> = s
        .host_versions
        .iter()
        .map(|(r, v)| format!("{r}={v}"))
        .collect();
    w!(
        o,
        "host: repo={}{}; head={}; versions={}",
        s.host_repo.as_deref().unwrap_or("unknown"),
        s.host_repo_source
            .as_deref()
            .map(|x| format!(" ({x})"))
            .unwrap_or_default(),
        s.host_head.as_deref().map(short).unwrap_or("unknown"),
        if versions.is_empty() {
            "unknown".to_string()
        } else {
            versions.join(",")
        }
    );
    let concepts: Vec<String> = s
        .concepts
        .iter()
        .map(|(c, src)| {
            format!(
                "{c} ({})",
                src.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    w!(
        o,
        "scope: repos={}; modules={}; features={}; concepts={}",
        s.repos.label(),
        s.modules.label(),
        s.features.label(),
        if concepts.is_empty() {
            "none".to_string()
        } else {
            concepts.join(", ")
        }
    );
    if s.change_types.is_known() {
        w!(o, "change types: {}", s.change_types.label());
    }
    if !s.change_type_hints.is_empty() {
        w!(
            o,
            "change type hints: {} (positive evidence; unmentioned categories stay unresolved)",
            s.change_type_hints
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    if !s.inferred_paths.is_empty() {
        w!(o, "identifier paths: {}", s.inferred_paths.join(", "));
    }
    if !s.inferred_features.is_empty() {
        w!(o, "candidate features: {}", s.inferred_features.join(", "));
    }
    if let Some(point) = &h.request.as_of {
        w!(
            o,
            "as-of: {} (UTC date {}; host revision {})",
            point.requested,
            point.date,
            point.host_revision.as_deref().unwrap_or("none")
        );
    }
    if let Some(change) = &h.request.change {
        w!(
            o,
            "change: merge-base {}; head {}",
            change.merge_base,
            change.head.as_deref().unwrap_or("working-tree")
        );
    }
    for p in &s.paths {
        let modules = if p.modules.is_empty() {
            "no registry module".to_string()
        } else {
            format!("modules {}", p.modules.join(","))
        };
        let features = if p.features.is_empty() {
            String::new()
        } else {
            format!("; features {}", p.features.join(","))
        };
        w!(o, "path: {} -> {modules}{features}", p.display());
    }
    if human {
        w!(o);
    }
    w!(
        o,
        "status: {}",
        h.completeness.as_str().to_ascii_uppercase()
    );
    for r in &h.reasons {
        w!(o, "{item}[{}] {}: {}", r.status.as_str(), r.code, r.message);
    }
    if !h.settings.is_empty() {
        w!(o, "effective settings:");
        for st in &h.settings {
            w!(o, "{item}{}", setting_value_text(st));
        }
    }
    if !h.undetermined.is_empty() {
        w!(
            o,
            "undetermined obligations (listed, not included; narrow the scope to decide):"
        );
        for u in &h.undetermined {
            w!(
                o,
                "{item}{} ({}): {} - {}",
                u.id,
                u.kind.as_str(),
                u.title,
                u.detail
            );
        }
    }
    for a in &s.ambiguities {
        let offered: Vec<String> = a
            .offered
            .iter()
            .map(|(c, id)| format!("{c}={id}"))
            .collect();
        w!(
            o,
            "ambiguous: \"{}\" (alias {}) -> {}; offered: {}; pass --concept or a --path to disambiguate",
            a.phrase,
            a.aliases.join(", "),
            a.candidates.join(", "),
            if offered.is_empty() {
                "none".to_string()
            } else {
                offered.join(", ")
            }
        );
    }
    if !h.issues.is_empty() {
        w!(o, "issues:");
        for d in &h.issues {
            w!(o, "{item}{}", diag_text(d));
        }
    }
    let errors: Vec<&Diagnostic> = h.diagnostics.iter().filter(|d| d.is_error()).collect();
    let warnings = h
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    if !h.diagnostics.is_empty() {
        w!(
            o,
            "snapshot diagnostics: {} error(s), {warnings} warning(s); run `kb validate`",
            errors.len()
        );
        for d in errors {
            w!(o, "{item}{}", diag_text(d));
        }
    }
    if h.delivery.core_receipt.is_some() || h.delivery.since_receipt.is_some() {
        w!(
            o,
            "delivery: core={}; since={}",
            h.delivery.core_receipt.as_deref().unwrap_or("none"),
            h.delivery.since_receipt.as_deref().unwrap_or("none")
        );
    }
    if let Some(date) = &h.reference_date {
        w!(o, "freshness reference: {} ({})", date.date, date.source);
    }
    if !h.pruned_change_types.is_empty() {
        w!(
            o,
            "not applicable to the declared change types: {}",
            h.pruned_change_types.join(", ")
        );
    }
    if let Some(code) = &h.code_info {
        w!(
            o,
            "code provider: {} {} at {} (static query complete={}); facts are non-normative",
            code.tool.name,
            code.tool.version,
            code.commit,
            code.complete
        );
        for limitation in &code.limitations {
            w!(o, "code limitation: {limitation}");
        }
    }
    for n in &h.notes {
        w!(o, "note: {n}");
    }
    o
}

pub(super) fn header_json(h: &Header) -> Map<String, Value> {
    let r = &h.request;
    let s = &h.scope;
    let versions = |v: &mut dyn Iterator<Item = (&String, String)>| -> Value {
        Value::Object(v.map(|(k, x)| (k.clone(), Value::String(x))).collect())
    };
    let errors: Vec<&Diagnostic> = h.diagnostics.iter().filter(|d| d.is_error()).collect();
    let warnings = h
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    let mut m = Map::new();
    if h.delivery.core_receipt.is_some() || h.delivery.since_receipt.is_some() {
        m.insert("delivery".into(), json!({"core_receipt": h.delivery.core_receipt, "since_receipt": h.delivery.since_receipt}));
    }
    if let Some(date) = &h.reference_date {
        m.insert(
            "freshness_reference".into(),
            json!({"on":date, "max_age_days":h.request.stale}),
        );
    }
    if !h.pruned_change_types.is_empty() {
        m.insert("pruned_change_types".into(), json!(h.pruned_change_types));
    }
    if let Some(code) = &h.code_info {
        m.insert("code".into(), json!(code));
    }
    m.insert("engine_version".into(), json!(ENGINE_VERSION));
    m.insert("skill_protocol".into(), json!(SKILL_PROTOCOL));
    m.insert("protocol".into(), json!(PROTOCOL_ID));
    m.insert("intent".into(), json!(r.intent.as_str()));
    m.insert(
        "request".into(),
        json!({
            "task": r.task,
            "repos": r.repos,
            "paths": r.all_paths(),
            "modules": r.modules,
            "features": r.features,
            "concepts": r.concepts,
            "include_proposals": r.include_proposals,
            "sections": r.sections.as_str(),
            "max_supplementary": h.max_supplementary,
            "host_versions": versions(&mut r.host_versions.iter().map(|(k, v)| (k, v.to_string()))),
        }),
    );
    m.insert(
        "snapshot".into(),
        serde_json::to_value(&h.snapshot).unwrap_or(Value::Null),
    );
    m.insert(
        "freshness".into(),
        json!(match h.snapshot.freshness {
            Freshness::Verified => "verified",
            Freshness::Unverified => "unverified",
        }),
    );
    m.insert(
        "host".into(),
        json!({
            "repo": s.host_repo,
            "repo_source": s.host_repo_source,
            "head": s.host_head,
            "versions": versions(&mut s.host_versions.iter().map(|(k, v)| (k, v.to_string()))),
        }),
    );
    m.insert(
        "scope".into(),
        json!({
            "repos": s.repos.to_json(),
            "modules": s.modules.to_json(),
            "features": s.features.to_json(),
            "concepts": s.concepts.iter().map(|(c, src)| json!({"id": c, "sources": src})).collect::<Vec<_>>(),
            "paths": s.paths.iter().map(|p| json!({
                "repo": p.repo, "path": p.path, "modules": p.modules, "features": p.features,
            })).collect::<Vec<_>>(),
        }),
    );
    if s.change_types.is_known() {
        m.get_mut("scope").unwrap()["change_types"] = s.change_types.to_json();
        m.get_mut("request").unwrap()["change_types"] = json!(r.change_types);
    }
    if !s.change_type_hints.is_empty() {
        m.get_mut("scope").unwrap()["change_type_hints"] = json!(s.change_type_hints);
    }
    if !s.inferred_paths.is_empty() {
        m.get_mut("scope").unwrap()["inferred_paths"] = json!(s.inferred_paths);
    }
    if !s.inferred_features.is_empty() {
        m.get_mut("scope").unwrap()["inferred_features"] = json!(s.inferred_features);
    }
    if let Some(point) = &r.as_of {
        m.get_mut("request").unwrap()["as_of"] = json!(point);
    }
    if let Some(change) = &r.change {
        m.get_mut("request").unwrap()["change"] = json!(change);
    }
    m.insert("completeness".into(), json!(h.completeness.as_str()));
    m.insert(
        "status_reasons".into(),
        Value::Array(
            h.reasons
                .iter()
                .map(|x| {
                    json!({"status": x.status.as_str(), "code": x.code, "message": x.message, "provenance": x.provenance})
                })
                .collect(),
        ),
    );
    m.insert(
        "effective_settings".into(),
        Value::Array(h.settings.iter().map(setting_json).collect()),
    );
    m.insert(
        "undetermined".into(),
        Value::Array(
            h.undetermined
                .iter()
                .map(|u| json!({"id": u.id, "kind": u.kind, "title": u.title, "detail": u.detail}))
                .collect(),
        ),
    );
    m.insert(
        "ambiguities".into(),
        Value::Array(
            s.ambiguities
                .iter()
                .map(|a| {
                    json!({
                        "phrase": a.phrase,
                        "aliases": a.aliases,
                        "candidates": a.candidates,
                        "offered": a.offered.iter().map(|(c, id)| json!({"concept": c, "id": id})).collect::<Vec<_>>(),
                    })
                })
                .collect(),
        ),
    );
    m.insert("issues".into(), json!(h.issues));
    m.insert(
        "diagnostics".into(),
        json!({"errors": errors.len(), "warnings": warnings, "items": errors}),
    );
    m.insert("notes".into(), json!(h.notes));
    m
}

fn setting_json(s: &EffectiveSetting) -> Value {
    let (source, by, reason, conflict) = match &s.source {
        SettingSource::Base => ("base", None, None, Vec::new()),
        SettingSource::Override { by, reason } => (
            "override",
            Some(by.clone()),
            Some(reason.clone()),
            Vec::new(),
        ),
        SettingSource::Conflict { candidates } => (
            "conflict",
            None,
            None,
            candidates
                .iter()
                .map(|(p, v)| json!({"policy": p, "value": v}))
                .collect(),
        ),
    };
    json!({
        "policy": s.policy,
        "setting": s.setting.name,
        "type": type_name(s.setting.value_type),
        "value": s.value,
        "base": s.setting.value,
        "source": source,
        "override_by": by,
        "reason": reason,
        "conflict": conflict,
        "undetermined_overrides": s.undetermined_overrides,
    })
}

// ---------------------------------------------------------------------------------------
// Units
// ---------------------------------------------------------------------------------------

pub(super) fn unit(u: &Unit, format: Format) -> String {
    match format {
        Format::Json => {
            let level = JSON_RESULT_LEVEL + 2;
            format!(",\n{}{}", indent(level), pretty_at(&unit_json(u), level))
        }
        Format::Compact => safe_text(unit_text(u, false)),
        Format::Terse => safe_text(terse_unit(u)),
        Format::Human => safe_text(unit_text(u, true)),
    }
}

fn unit_text(u: &Unit, human: bool) -> String {
    if let Some(reuse) = u.reuse {
        return format!("{} {} [{}]\n", u.tier.as_str(), u.id, reuse.as_str());
    }
    let mut o = String::new();
    let ind = if human { "   " } else { "" };
    let labels = if u.labels.is_empty() {
        String::new()
    } else {
        format!("; labels: {}", u.labels.join(", "))
    };
    match &u.body {
        UnitBody::Code(e) => {
            w!(o, "### code {}: {}", u.id, e.symbol.name);
            w!(
                o,
                "source: {}:{}:{}-{} at {}; provider {} {}; role={}; non-normative",
                e.repo,
                e.symbol.path,
                e.symbol.start_line,
                e.symbol.end_line,
                e.commit,
                e.tool.name,
                e.tool.version,
                e.role
            );
            w!(o, "why: {}", e.reason);
            let max_ticks = e
                .source
                .split(|c| c != '`')
                .map(str::len)
                .max()
                .unwrap_or(0);
            let fence = "`".repeat(3.max(max_ticks + 1));
            w!(
                o,
                "{fence}text\n{}\n{fence}",
                e.source.trim_end_matches('\n')
            );
        }
        UnitBody::Section(s) => {
            let heading = if s.heading.is_empty() {
                "(intro)"
            } else {
                s.heading.as_str()
            };
            if human {
                w!(o);
                w!(o, "== SECTION {}: {heading}", u.id);
            } else {
                w!(o, "### section {}: {heading}", u.id);
            }
            w!(o, "{ind}source: {}{labels}", u.path);
            w!(o, "{}", s.markdown);
        }
        UnitBody::Removal => {
            if human {
                w!(o);
                w!(o, "== PROPOSAL {}: {}", u.record_id, u.title);
            } else {
                w!(
                    o,
                    "### proposal {} ({}, removal): {}",
                    u.record_id,
                    u.kind.as_str(),
                    u.title
                );
            }
            w!(o, "{ind}why: {}{labels}; source: {}", u.why, u.path);
        }
        UnitBody::Record(p) => {
            let c = p.record.common();
            if human {
                w!(o);
                w!(
                    o,
                    "== {} {}: {}",
                    u.tier.as_str().to_ascii_uppercase(),
                    u.record_id,
                    u.title
                );
                w!(
                    o,
                    "{ind}kind {} | status {} | origin {} | owner {}",
                    u.kind.as_str(),
                    u.status.as_str(),
                    origin_str(u.origin),
                    c.owner
                );
                w!(o, "{ind}why: {}{labels}", u.why);
                w!(o, "{ind}source: {}", u.path);
            } else {
                w!(
                    o,
                    "### {} {} ({}, {}): {}",
                    u.tier.as_str(),
                    u.record_id,
                    u.kind.as_str(),
                    u.status.as_str(),
                    u.title
                );
                w!(o, "why: {}{labels}; source: {}", u.why, u.path);
            }
            w!(o, "{ind}scope: {}", scope_text(c.scope));
            record_core(&mut o, &p.record, ind);
            link_lines(&mut o, &p.record, ind, false);
        }
    }
    o
}

pub(super) fn unit_json(u: &Unit) -> Value {
    if let Some(reuse) = u.reuse {
        return json!({"id":u.id, "record":u.record_id, "kind": if matches!(u.body, UnitBody::Code(_)) { "code" } else { u.kind.as_str() },
            "tier":u.tier.as_str(), "delivery":reuse, "content_sha256":u.content_digest});
    }
    let mut m = Map::new();
    if let Some(digest) = &u.content_digest {
        m.insert("content_sha256".into(), json!(digest));
    }
    m.insert("tier".into(), json!(u.tier.as_str()));
    m.insert("id".into(), json!(u.id));
    m.insert("record".into(), json!(u.record_id));
    m.insert("kind".into(), json!(u.kind));
    m.insert("status".into(), json!(u.status));
    m.insert("title".into(), json!(u.title));
    m.insert("origin".into(), json!(origin_str(u.origin)));
    m.insert("path".into(), json!(u.path));
    m.insert("why".into(), json!(u.why));
    m.insert("labels".into(), json!(u.labels));
    if let Some(s) = u.score {
        m.insert("score".into(), json!(s));
    }
    match &u.body {
        UnitBody::Code(e) => {
            m.insert("kind".into(), json!("code"));
            m.insert("status".into(), json!("observed"));
            m.insert("origin".into(), json!("provider"));
            m.insert("record".into(), Value::Null);
            m.insert("code".into(), json!(e));
        }
        UnitBody::Record(p) => {
            m.insert("content".into(), record_content_json(&p.record));
        }
        UnitBody::Section(s) => {
            m.insert("section".into(), json!(s.id));
            m.insert("heading".into(), json!(s.heading));
            m.insert("markdown".into(), json!(s.markdown));
        }
        UnitBody::Removal => {
            m.insert("content".into(), Value::Null);
        }
    }
    Value::Object(m)
}

/// Typed record fields without routing-only metadata (selectors, anchors) and without the
/// fields already present on the unit.
fn record_content_json(r: &Record) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
    if let Some(m) = v.as_object_mut() {
        for k in [
            "schema",
            "id",
            "kind",
            "title",
            "status",
            "selectors",
            "anchors",
        ] {
            m.remove(k);
        }
    }
    v
}

// ---------------------------------------------------------------------------------------
// Footer (receipt summary) and explain
// ---------------------------------------------------------------------------------------

fn counts_text(c: &TierCounts) -> String {
    format!(
        "mandatory {}, dependencies {}, proposals {}, supplementary {}, sections {}",
        c.mandatory, c.dependencies, c.proposals, c.supplementary, c.sections
    )
}

fn counts_json(c: &TierCounts) -> Value {
    json!({
        "mandatory": c.mandatory,
        "dependencies": c.dependencies,
        "proposals": c.proposals,
        "supplementary": c.supplementary,
        "sections": c.sections,
    })
}

pub(super) fn footer(f: &Footer, format: Format) -> String {
    let b = &f.budget;
    let estimate = if b.unit == BudgetUnit::TokensEst {
        " (estimate, not a tokenizer count)"
    } else {
        ""
    };
    let excluded = if f.excluded_budget.is_empty() {
        "none".to_string()
    } else {
        f.excluded_budget.join(", ")
    };
    match format {
        Format::Json => {
            let c = &f.counts;
            let units = c.mandatory + c.dependencies + c.proposals + c.supplementary + c.sections;
            let mut s = if units == 0 {
                String::new()
            } else {
                indent(JSON_RESULT_LEVEL + 1)
            };
            s.push(']');
            s.push_str(&json_members(&footer_json(f), JSON_RESULT_LEVEL + 1));
            s.push_str(&indent(JSON_RESULT_LEVEL));
            s.push('}');
            s
        }
        Format::Terse => {
            let mut o = format!(
                "receipt {} | budget {}/{} {} | included {} | excluded-budget {}\n",
                f.receipt_id,
                b.used,
                b.limit,
                b.unit.as_str(),
                counts_text(&f.counts),
                excluded
            );
            if let Some(digest) = &f.snapshot_digest {
                w!(o, "kb.receipt.v2 snapshot-content={digest}");
            }
            w!(
                o,
                "Receipt proves delivery only. After compaction or a new session, request full context."
            );
            o
        }
        Format::Compact => {
            let mut o = String::new();
            w!(
                o,
                "-- receipt {}; included: {}; excluded by budget: {excluded}; other exclusions: {} (--explain); budget: {}/{} {}{estimate}, measured as {}",
                f.receipt_id,
                counts_text(&f.counts),
                f.excluded_other,
                b.used,
                b.limit,
                b.unit.as_str(),
                format_str(b.format)
            );
            w!(o, "-- {RECEIPT_NOTE}");
            if let Some(digest) = &f.snapshot_digest {
                w!(o, "-- kb.receipt.v2 snapshot-content={digest}");
            }
            o
        }
        Format::Human => {
            let mut o = String::new();
            w!(o);
            w!(o, "receipt: {}", f.receipt_id);
            w!(o, "included: {}", counts_text(&f.counts));
            w!(o, "excluded by budget: {excluded}");
            w!(o, "other exclusions: {} (use --explain)", f.excluded_other);
            w!(
                o,
                "budget: used {} of {} {}{estimate}, measured as {}",
                b.used,
                b.limit,
                b.unit.as_str(),
                format_str(b.format)
            );
            w!(o, "{RECEIPT_NOTE}");
            if let Some(digest) = &f.snapshot_digest {
                w!(o, "kb.receipt.v2 snapshot-content={digest}");
            }
            o
        }
    }
}

pub(super) fn footer_json(f: &Footer) -> Map<String, Value> {
    let b = &f.budget;
    let mut m = Map::new();
    m.insert(
        "budget".into(),
        json!({
            "limit": b.limit,
            "unit": b.unit.as_str(),
            "used": b.used,
            "format": format_str(b.format),
            "estimate": b.unit == BudgetUnit::TokensEst,
            "counted": ["header", "units", "receipt"],
            "not_counted": ["explain", "meta"],
        }),
    );
    m.insert(
        "receipt".into(),
        json!({
            "id": f.receipt_id,
            "included": counts_json(&f.counts),
            "excluded_budget": f.excluded_budget,
            "excluded_other": f.excluded_other,
            "note": RECEIPT_NOTE,
        }),
    );
    if let Some(digest) = &f.snapshot_digest {
        m.get_mut("receipt").unwrap()["protocol"] = json!("kb.receipt.v2");
        m.get_mut("receipt").unwrap()["snapshot_content_digest"] = json!(digest);
    }
    m
}

fn terse_header(h: &Header) -> String {
    let mut text = format!(
        "kb {} | {} | {}\n{}\n",
        h.request.intent.as_str(),
        h.completeness.as_str(),
        PROTOCOL_ID,
        h.snapshot.summary_line()
    );
    w!(
        text,
        "scope repos={} modules={} features={}",
        h.scope.repos.label(),
        h.scope.modules.label(),
        h.scope.features.label()
    );
    if let Some(date) = &h.reference_date {
        w!(text, "freshness on={} ({})", date.date, date.source);
    }
    if h.scope.change_types.is_known() {
        w!(text, "change-types={}", h.scope.change_types.label());
    }
    if !h.pruned_change_types.is_empty() {
        w!(
            text,
            "pruned-change-types: {}",
            h.pruned_change_types.join(",")
        );
    }
    if let Some(as_of) = &h.request.as_of {
        w!(text, "as-of={}", as_of.requested);
    }
    for reason in &h.reasons {
        w!(text, "{}: {}", reason.code, reason.message);
    }
    for setting in &h.settings {
        w!(text, "setting {}", setting_value_text(setting));
    }
    for unit in &h.undetermined {
        w!(text, "undetermined {}: {}", unit.id, unit.detail);
    }
    for issue in h.issues.iter().chain(h.diagnostics.iter()) {
        w!(text, "{}", diag_text(issue));
    }
    if let Some(code) = &h.code_info {
        w!(
            text,
            "code {} {} at {} complete={}",
            code.tool.name,
            code.tool.version,
            code.commit,
            code.complete
        );
        for limitation in &code.limitations {
            w!(text, "code: {limitation}");
        }
    }
    if let Some(id) = &h.delivery.core_receipt {
        w!(text, "core={id}");
    }
    if let Some(id) = &h.delivery.since_receipt {
        w!(text, "since={id}");
    }
    for note in &h.notes {
        w!(text, "note: {note}");
    }
    text
}

fn terse_unit(u: &Unit) -> String {
    if u.reuse.is_some() {
        return unit_text(u, false);
    }
    let code = matches!(u.body, UnitBody::Code(_));
    let mut out = format!(
        "[{}] {} {}/{}\n",
        u.tier.as_str(),
        u.id,
        if code { "code" } else { u.kind.as_str() },
        if code { "observed" } else { u.status.as_str() },
    );
    if !u.labels.is_empty() {
        w!(out, "labels: {}", u.labels.join(", "));
    }
    match &u.body {
        UnitBody::Record(p) => {
            w!(out, "scope: {}", scope_text(p.record.common().scope));
            // Share the normative renderer: repeated empty JSON keys are not useful context.
            // Keep all conditions, exceptions, settings, domain fields and validity metadata.
            record_core(&mut out, &p.record, "");
            link_lines(&mut out, &p.record, "", false);
        }
        UnitBody::Code(c) => {
            w!(out, "provider evidence; non-normative");
            w!(out, "{}", canonical_compact(&json!({"code":c})));
        }
        UnitBody::Section(s) => {
            w!(
                out,
                "{}",
                canonical_compact(&json!({"heading":s.heading,"markdown":s.markdown}))
            );
        }
        UnitBody::Removal => w!(out, "proposal removal; accepted record still applies"),
    }
    out
}

fn canonical_compact(value: &Value) -> String {
    super::canonical_json(value)
}

fn anchors(r: &ContextResult) -> Vec<(&str, &Anchor)> {
    r.units
        .iter()
        .filter_map(|u| match &u.body {
            UnitBody::Record(p) => Some((u.id.as_str(), p.record.common().anchors)),
            _ => None,
        })
        .flat_map(|(id, list)| list.iter().map(move |a| (id, a)))
        .collect()
}

fn anchor_kind(k: AnchorKind) -> &'static str {
    match k {
        AnchorKind::Source => "source",
        AnchorKind::Test => "test",
        AnchorKind::Change => "change",
        AnchorKind::Doc => "doc",
    }
}

pub(super) fn anchor_text(a: &Anchor) -> String {
    let mut s = anchor_kind(a.kind).to_string();
    match (&a.repo, &a.path) {
        (Some(r), Some(p)) => s.push_str(&format!(" {r}:{p}")),
        (None, Some(p)) => s.push_str(&format!(" {p}")),
        (Some(r), None) => s.push_str(&format!(" {r}")),
        (None, None) => {}
    }
    if let Some(x) = &a.symbol {
        s.push_str(&format!(" symbol {x}"));
    }
    if let Some(x) = &a.commit {
        s.push_str(&format!(" commit {x}"));
    }
    if let Some(x) = &a.change {
        let label = if a.kind == AnchorKind::Change {
            ""
        } else {
            " change"
        };
        s.push_str(&format!("{label} {x}"));
    }
    if let Some(x) = &a.note {
        s.push_str(&format!(" ({x})"));
    }
    s
}

/// Explain part (text formats). Not counted in the budget.
pub(super) fn explain_text(r: &ContextResult) -> String {
    let mut o = String::new();
    w!(o);
    w!(o, "== explain (not counted in the budget) ==");
    w!(o, "included:");
    for u in &r.units {
        w!(o, "- {} [{}] {}", u.id, u.tier.as_str(), u.why);
        if !u.signals.is_empty() {
            let s: Vec<String> = u
                .signals
                .iter()
                .map(|s| format!("{} +{} ({})", s.kind.as_str(), s.points, s.detail))
                .collect();
            w!(o, "  signals: {}", s.join(", "));
        }
    }
    if !r.requires.is_empty() {
        w!(o, "requires:");
        for (a, b) in &r.requires {
            w!(o, "- {a} -> {b}");
        }
    }
    if !r.excluded.is_empty() {
        w!(o, "excluded:");
        for e in &r.excluded {
            w!(o, "- {} [{}] {}", e.id, e.reason.as_str(), e.detail);
        }
    }
    let anchors = anchors(r);
    if !anchors.is_empty() {
        w!(o, "anchors:");
        for (id, a) in anchors {
            w!(o, "- {id}: {}", anchor_text(a));
        }
    }
    let warnings: Vec<&Diagnostic> = r
        .header
        .issues
        .iter()
        .chain(r.header.diagnostics.iter())
        .collect();
    if !warnings.is_empty() {
        w!(o, "warnings:");
        for d in warnings {
            w!(o, "- {}", diag_text(d));
        }
    }
    safe_text(o)
}

pub(super) fn explain_json(r: &ContextResult) -> Value {
    json!({
        "counted_in_budget": false,
        "included": r.units.iter().map(|u| json!({
            "id": u.id,
            "tier": u.tier.as_str(),
            "why": u.why,
            "score": u.score,
            "signals": u.signals.iter().map(|s| json!({
                "signal": s.kind.as_str(), "points": s.points, "detail": s.detail,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "requires": r.requires.iter().map(|(a, b)| json!({"from": a, "to": b})).collect::<Vec<_>>(),
        "excluded": r.excluded.iter().map(|e| json!({
            "id": e.id, "reason": e.reason.as_str(), "score": e.score, "detail": e.detail,
        })).collect::<Vec<_>>(),
        "anchors": anchors(r).into_iter().map(|(id, a)| json!({"record": id, "anchor": a})).collect::<Vec<_>>(),
        "undetermined": r.header.undetermined.iter().map(|u| u.id.clone()).collect::<Vec<_>>(),
        "warnings": r.header.issues.iter().chain(r.header.diagnostics.iter()).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------------------

fn normative(o: &mut String, ind: &str, n: &NormativeRef) {
    let party = n.party.map(|p| format!("{p} ")).unwrap_or_default();
    w!(
        o,
        "{ind}- {party}{} [{}] {}",
        n.level.keyword(),
        n.id,
        n.text
    );
    for c in n.conditions {
        w!(o, "{ind}  if: {c}");
    }
    for e in n.exceptions {
        w!(o, "{ind}  except [{}]: {}", e.id, e.text);
    }
    for probe in n.verify {
        if let Ok(json) = serde_json::to_string(probe) {
            w!(o, "{ind}  verify: {json}");
        }
    }
}

fn setting_line(o: &mut String, ind: &str, s: &Setting) {
    let mut line = format!(
        "{ind}- setting {} ({}) = {}; override {}",
        s.name,
        type_name(s.value_type),
        s.value.display(),
        mode_name(s.r#override)
    );
    if let Some(d) = s.stricter {
        line.push_str(&format!(" ({})", stricter_name(d)));
    }
    if !s.override_owners.is_empty() {
        line.push_str(&format!("; owners: {}", s.override_owners.join(", ")));
    }
    if let Some(d) = &s.description {
        line.push_str(&format!(" - {d}"));
    }
    w!(o, "{line}");
}

fn list(o: &mut String, ind: &str, label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    w!(o, "{ind}{label}:");
    for i in items {
        w!(o, "{ind}- {i}");
    }
}

fn items(o: &mut String, ind: &str, label: &str, items: &[Item]) {
    if items.is_empty() {
        return;
    }
    w!(o, "{ind}{label}:");
    for i in items {
        w!(o, "{ind}- [{}] {}", i.id, i.text);
    }
}

fn gap_kind(g: GapKind) -> &'static str {
    match g {
        GapKind::Missing => "missing",
        GapKind::Ambiguity => "ambiguity",
        GapKind::Contradiction => "contradiction",
    }
}

/// All typed content of a record (the "core unit"), one item per line.
pub(super) fn record_core(o: &mut String, rec: &Record, ind: &str) {
    match rec {
        Record::Policy(p) => {
            for n in rec.normative() {
                normative(o, ind, &n);
            }
            for s in &p.settings {
                setting_line(o, ind, s);
            }
            for ov in &p.overrides {
                w!(
                    o,
                    "{ind}- override {} = {}: {}",
                    ov.target,
                    ov.value.display(),
                    ov.reason
                );
            }
        }
        Record::Invariant(_) => {
            for n in rec.normative() {
                normative(o, ind, &n);
            }
        }
        Record::Contract(k) => {
            scenarios(o, ind, &k.scenarios);
            for c in &k.consumers {
                w!(
                    o,
                    "{ind}consumer: {}:{}{}",
                    c.repo,
                    c.path,
                    c.symbol
                        .as_ref()
                        .map(|s| format!("#{s}"))
                        .unwrap_or_default()
                );
            }
            for p in &k.parties {
                let modules = if p.modules.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", p.modules.join(", "))
                };
                w!(
                    o,
                    "{ind}party {}: repo {}{modules}; role: {}",
                    p.id,
                    p.repo,
                    p.role
                );
            }
            if let Some(i) = &k.interface {
                w!(o, "{ind}interface: {i}");
            }
            for n in rec.normative() {
                normative(o, ind, &n);
            }
        }
        Record::Feature(f) => {
            w!(o, "{ind}feature: {}", f.feature);
            w!(o, "{ind}summary: {}", f.summary);
            items(o, ind, "behaviors", &f.behaviors);
            items(o, ind, "boundaries", &f.boundaries);
            items(o, ind, "states", &f.states);
            for t in &f.transitions {
                w!(
                    o,
                    "{ind}transition [{}]: {} -> {} when {}",
                    t.id,
                    t.from,
                    t.to,
                    t.when
                );
            }
            list(o, ind, "clocks", &f.clocks);
            list(o, ind, "data sources", &f.data_sources);
            scenarios(o, ind, &f.scenarios);
        }
        Record::Decision(d) => {
            w!(o, "{ind}context: {}", d.context);
            w!(o, "{ind}decision: {}", d.decision);
            list(o, ind, "reasons", &d.reasons);
            if !d.alternatives.is_empty() {
                w!(o, "{ind}alternatives:");
                for a in &d.alternatives {
                    w!(o, "{ind}- {} (rejected: {})", a.option, a.rejected_because);
                }
            }
            list(o, ind, "consequences", &d.consequences);
        }
        Record::Procedure(p) => {
            list(o, ind, "preconditions", &p.preconditions);
            w!(o, "{ind}steps (data; kb never executes them):");
            for (i, s) in p.steps.iter().enumerate() {
                w!(o, "{ind}{}. [{}] {}", i + 1, s.id, s.text);
            }
            list(o, ind, "expected", &p.expected);
        }
        Record::Reference(r) => {
            w!(o, "{ind}summary: {}", r.summary);
            for t in &r.terms {
                w!(
                    o,
                    "{ind}term {}: {} (source: {})",
                    t.term,
                    t.meaning,
                    t.source
                );
            }
            if !r.sources.is_empty() {
                w!(o, "{ind}sources:");
                for s in &r.sources {
                    let mut line = format!("{ind}- {}", s.title);
                    if let Some(u) = &s.url {
                        line.push_str(&format!(" <{u}>"));
                    }
                    if let Some(p) = &s.path {
                        line.push_str(&format!(" ({p})"));
                    }
                    w!(o, "{line}");
                }
            }
        }
        Record::Gap(g) => {
            w!(o, "{ind}gap ({}): {}", gap_kind(g.gap), g.description);
            if !g.affects.is_empty() {
                w!(o, "{ind}affects: {}", g.affects.join(", "));
            }
            list(o, ind, "questions", &g.questions);
        }
    }
}

fn scenarios(o: &mut String, ind: &str, scenarios: &[crate::model::Scenario]) {
    for s in scenarios {
        w!(
            o,
            "{ind}scenario [{}]: given {}; expect {}",
            s.id,
            s.given,
            s.expect
        );
    }
}

/// Link and version lines. `all` adds `related` (full record view).
fn link_lines(o: &mut String, rec: &Record, ind: &str, all: bool) {
    let c = rec.common();
    for (field, value) in [
        ("introduced", c.introduced),
        ("retired", c.retired),
        ("verified_at", c.verified_at),
        ("review_by", c.review_by),
    ] {
        if let Some(value) = value {
            w!(o, "{ind}{field}: {value}");
        }
    }
    let l = c.links;
    for (name, v, show) in [
        ("requires", &l.requires, true),
        ("rationale", &l.rationale, true),
        ("related", &l.related, all),
        ("supersedes", &l.supersedes, true),
    ] {
        if show && !v.is_empty() {
            w!(o, "{ind}{name}: {}", v.join(", "));
        }
    }
    if let Some(a) = c.applicability
        && !a.versions.is_empty()
    {
        let v: Vec<String> = a.versions.iter().map(|(r, q)| format!("{r} {q}")).collect();
        w!(o, "{ind}versions: {}", v.join(", "));
    }
}

/// Full typed record with metadata, anchors and verbatim sections (`kb show`).
pub(super) fn record_full(p: &ParsedRecord, path: &str, origin: Origin, human: bool) -> String {
    let mut o = String::new();
    let c = p.record.common();
    let ind = if human { "  " } else { "" };
    w!(
        o,
        "# {} ({}, {}): {}",
        c.id,
        c.kind.as_str(),
        c.status.as_str(),
        c.title
    );
    w!(
        o,
        "{ind}owner: {}; origin: {}; source: {path}",
        c.owner,
        origin_str(origin)
    );
    w!(o, "{ind}scope: {}", scope_text(c.scope));
    let s = c.selectors;
    let intents: Vec<&str> = s.intents.iter().map(|i| i.as_str()).collect();
    for (name, v) in [
        ("selector paths", s.paths.join(", ")),
        ("selector concepts", s.concepts.join(", ")),
        ("selector intents", intents.join(", ")),
        ("selector aliases", s.aliases.join(", ")),
    ] {
        if !v.is_empty() {
            w!(o, "{ind}{name}: {v}");
        }
    }
    link_lines(&mut o, &p.record, ind, true);
    record_core(&mut o, &p.record, ind);
    if !c.anchors.is_empty() {
        w!(o, "{ind}anchors:");
        for a in c.anchors {
            w!(o, "{ind}- {}", anchor_text(a));
        }
    }
    for sec in &p.sections {
        w!(o);
        if sec.heading.is_empty() {
            w!(o, "## (intro)");
        } else {
            w!(o, "## {}", sec.heading);
        }
        w!(o, "{}", sec.markdown);
    }
    safe_text(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_characters_are_escaped() {
        assert_eq!(safe_text("a\tb\nc".into()), "a\tb\nc");
        assert_eq!(safe_text("x\u{1b}[31my\r".into()), "x\\u{1b}[31my\\u{d}");
        assert_eq!(safe_text("MUST\u{202e}TON".into()), "MUST\\u{202e}TON");
        assert_eq!(safe_text("кириллица ✓".into()), "кириллица ✓");
    }
}
