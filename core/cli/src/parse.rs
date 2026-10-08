//! Strict, context-free record parsing: `+++` TOML front matter + optional Markdown body.
//!
//! Everything checked here depends only on the file bytes, so results can be cached by
//! content id. Registry, namespace and cross-record rules live in `validate`.

use std::collections::BTreeSet;

use crate::diag::Diagnostic;
use crate::glob::RepoGlob;
use crate::model::ids::{check_local_id, check_record_id};
use crate::model::record::*;
use crate::util::check_rel_path;
use crate::versions::DOCUMENT_SCHEMA;

mod evolution;

/// Maximum size of a record file.
pub const MAX_RECORD_BYTES: usize = 512 * 1024;
/// Maximum size of front matter.
pub const MAX_FRONT_MATTER_BYTES: usize = 128 * 1024;
/// Maximum length of any single text field.
pub const MAX_TEXT_BYTES: usize = 16 * 1024;
/// Maximum number of entries in any list field.
pub const MAX_LIST_LEN: usize = 500;
pub const MAX_TITLE_CHARS: usize = 200;

const DELIM: &str = "+++";

/// Result of parsing one file: the record or the list of problems.
pub type ParseResult = Result<ParsedRecord, Vec<Diagnostic>>;

/// Split `+++` front matter from the body. Returns (front matter, body, body start line).
pub fn split_front_matter(text: &str) -> Result<(&str, &str, u32), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let first_end = text.find('\n').unwrap_or(text.len());
    if text[..first_end].trim_end_matches('\r') != DELIM {
        return Err("record must start with a `+++` line".into());
    }
    let fm_start = (first_end + 1).min(text.len());
    let mut pos = fm_start;
    let mut line_no: u32 = 2;
    while pos <= text.len() {
        let end = text[pos..]
            .find('\n')
            .map(|i| pos + i)
            .unwrap_or(text.len());
        let line = text[pos..end].trim_end_matches('\r');
        if line == DELIM {
            let fm = &text[fm_start..pos];
            let body_start = (end + 1).min(text.len());
            return Ok((fm, &text[body_start..], line_no + 1));
        }
        if end == text.len() {
            break;
        }
        pos = end + 1;
        line_no += 1;
    }
    Err("front matter is not closed by a `+++` line".into())
}

/// Parse a record file. `path` is used only for diagnostics.
pub fn parse_record(path: &str, bytes: &[u8]) -> ParseResult {
    let diag =
        |code: &str, msg: String| vec![Diagnostic::error(code, msg).at_path(path.to_string())];
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(diag(
            "RECORD_TOO_LARGE",
            format!(
                "{} bytes exceeds the {MAX_RECORD_BYTES}-byte limit",
                bytes.len()
            ),
        ));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| diag("RECORD_NOT_UTF8", "record is not valid UTF-8".into()))?;
    let (fm, body, body_line) =
        split_front_matter(text).map_err(|e| diag("FRONT_MATTER_MISSING", e))?;
    if fm.len() > MAX_FRONT_MATTER_BYTES {
        return Err(diag(
            "FRONT_MATTER_TOO_LARGE",
            format!("front matter exceeds {MAX_FRONT_MATTER_BYTES} bytes"),
        ));
    }
    let toml_diag = |code: &str, e: &toml::de::Error| {
        vec![front_matter_error(code, fm, e).at_path(path.to_string())]
    };
    // Pass 1: generic TOML (syntax + duplicate keys) to read schema and kind.
    let table: toml::Table =
        toml::from_str(fm).map_err(|e| toml_diag("FRONT_MATTER_SYNTAX", &e))?;
    match table.get("schema") {
        Some(toml::Value::Integer(v))
            if u32::try_from(*v).is_ok_and(crate::versions::supports_document_schema) => {}
        Some(toml::Value::Integer(v)) => {
            return Err(diag(
                "UNSUPPORTED_SCHEMA_VERSION",
                format!(
                    "document schema {v} is not supported by this engine (expects {DOCUMENT_SCHEMA}); see `kb migrate`"
                ),
            ));
        }
        Some(_) => {
            return Err(diag(
                "SCHEMA_FIELD_INVALID",
                "`schema` must be an integer".into(),
            ));
        }
        None => {
            return Err(diag(
                "SCHEMA_FIELD_MISSING",
                "missing `schema` field".into(),
            ));
        }
    }
    let kind = match table.get("kind") {
        Some(toml::Value::String(k)) => Kind::parse(k).ok_or_else(|| {
            diag(
                "KIND_UNKNOWN",
                format!(
                    "unknown kind `{k}`; expected one of: {}",
                    Kind::ALL
                        .iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        })?,
        Some(_) => return Err(diag("KIND_INVALID", "`kind` must be a string".into())),
        None => return Err(diag("KIND_MISSING", "missing `kind` field".into())),
    };
    // Pass 2: strict typed parse of the same text (span-aware errors).
    if table.get("schema").and_then(toml::Value::as_integer) == Some(1)
        && let Some(field) = evolution::schema_two_field(&table)
    {
        return Err(diag(
            "SCHEMA_FIELD_UNAVAILABLE",
            format!("`{field}` requires schema 2; run `kb migrate`"),
        ));
    }
    let record = parse_kind(kind, fm).map_err(|e| toml_diag("FRONT_MATTER_INVALID", &e))?;
    let sections = split_sections(body).map_err(|e| diag("BODY_INVALID", e))?;
    let mut problems = check_record(&record);
    problems.extend(lint_body(&sections, body_line));
    for p in &mut problems {
        p.path = Some(path.to_string());
        if p.record.is_none() {
            p.record = Some(record.id().to_string());
        }
    }
    if problems.iter().any(Diagnostic::is_error) {
        return Err(problems);
    }
    // Warnings are not fatal; they are re-derived by `lint_record` during validation.
    Ok(ParsedRecord { record, sections })
}

fn parse_kind(kind: Kind, fm: &str) -> Result<Record, toml::de::Error> {
    fn p<T: serde::de::DeserializeOwned>(fm: &str) -> Result<T, toml::de::Error> {
        toml::from_str::<T>(fm)
    }
    Ok(match kind {
        Kind::Policy => Record::Policy(p(fm)?),
        Kind::Feature => Record::Feature(p(fm)?),
        Kind::Invariant => Record::Invariant(p(fm)?),
        Kind::Contract => Record::Contract(p(fm)?),
        Kind::Decision => Record::Decision(p(fm)?),
        Kind::Procedure => Record::Procedure(p(fm)?),
        Kind::Reference => Record::Reference(p(fm)?),
        Kind::Gap => Record::Gap(p(fm)?),
    })
}

/// File line of the first front-matter line (line 1 is the opening `+++`).
const FRONT_MATTER_FIRST_LINE: u32 = 2;

/// A front-matter TOML error as one diagnostic line. When the error has a span, the
/// diagnostic's `line` and the message's `line N, column M` are positions in the record
/// file, not in the front matter.
fn front_matter_error(code: &str, fm: &str, e: &toml::de::Error) -> Diagnostic {
    let Some(span) = e.span() else {
        return Diagnostic::error(code, clean_toml_error(&e.to_string()));
    };
    let (line, column) = front_matter_position(fm, span.start);
    let message = e
        .message()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Diagnostic::error(
        code,
        format!("TOML parse error at line {line}, column {column}: {message}"),
    )
    .at_line(line)
}

/// 1-based file line and character column of byte `offset` of the front matter. An offset
/// at or past the end (errors at end of input) is placed after the last character, as the
/// `toml` crate places it.
fn front_matter_position(fm: &str, offset: usize) -> (u32, usize) {
    let bytes = fm.as_bytes();
    let idx = offset.min(bytes.len().saturating_sub(1));
    let past_end = offset - idx;
    let line_start = bytes[..idx]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1);
    let line = bytes[..line_start].iter().filter(|b| **b == b'\n').count();
    let column = String::from_utf8_lossy(&bytes[line_start..idx])
        .chars()
        .count()
        + past_end
        + 1;
    let line = u32::try_from(line)
        .unwrap_or(u32::MAX)
        .saturating_add(FRONT_MATTER_FIRST_LINE);
    (line, column)
}

/// Collapse the multi-line TOML error rendering into one line with the location.
fn clean_toml_error(e: &str) -> String {
    let mut lines = e.lines().filter(|l| !l.trim().is_empty());
    let head = lines.next().unwrap_or("").trim().to_string();
    let tail: Vec<&str> = lines
        .filter(|l| !l.trim_start().starts_with('|') && !l.contains(" | "))
        .map(str::trim)
        .collect();
    if tail.is_empty() {
        head
    } else {
        format!("{head}: {}", tail.join(" "))
    }
}

/// Slug for section ids: lowercase, Unicode alphanumerics kept, other runs → `-`.
pub fn slug(heading: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in heading.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Split a Markdown body into sections at `## ` headings (outside fenced code blocks).
pub fn split_sections(body: &str) -> Result<Vec<Section>, String> {
    let mut sections: Vec<Section> = Vec::new();
    let mut cur_heading: Option<String> = None;
    let mut cur: Vec<&str> = Vec::new();
    let mut fence: Option<String> = None;
    let flush = |heading: &Option<String>, lines: &[&str], out: &mut Vec<Section>| {
        let text = trim_blank_lines(lines);
        match heading {
            None if text.is_empty() => {}
            None => out.push(Section {
                id: "intro".into(),
                heading: String::new(),
                markdown: text,
            }),
            Some(h) => out.push(Section {
                id: slug(h),
                heading: h.clone(),
                markdown: text,
            }),
        }
    };
    for line in body.lines() {
        let t = line.trim_start();
        if let Some(f) = &fence {
            if t.starts_with(f.as_str()) {
                fence = None;
            }
            cur.push(line);
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            let marker: String = t.chars().take_while(|c| *c == '`' || *c == '~').collect();
            fence = Some(marker);
            cur.push(line);
            continue;
        }
        if let Some(h) = line.strip_prefix("## ") {
            flush(&cur_heading, &cur, &mut sections);
            cur.clear();
            cur_heading = Some(h.trim().to_string());
            continue;
        }
        cur.push(line);
    }
    flush(&cur_heading, &cur, &mut sections);
    let mut seen = BTreeSet::new();
    for s in &sections {
        if s.id.is_empty() {
            return Err(format!("section heading `{}` has an empty slug", s.heading));
        }
        if !seen.insert(s.id.clone()) {
            return Err(format!("duplicate section id `{}`", s.id));
        }
    }
    Ok(sections)
}

fn trim_blank_lines(lines: &[&str]) -> String {
    let start = lines.iter().position(|l| !l.trim().is_empty());
    let end = lines.iter().rposition(|l| !l.trim().is_empty());
    match (start, end) {
        (Some(s), Some(e)) => lines[s..=e].join("\n"),
        _ => String::new(),
    }
}

const NORMATIVE_KEYWORDS: [&str; 6] = [
    "MUST NOT",
    "MUST",
    "SHALL",
    "SHOULD NOT",
    "REQUIRED",
    "SHALL NOT",
];

/// Warn when optional body sections contain uppercase RFC 2119 keywords: normative content
/// (including exceptions) must be typed, not hidden in optional explanations.
pub fn lint_body(sections: &[Section], body_line: u32) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for s in sections {
        let mut in_code = false;
        for line in s.markdown.lines() {
            let t = line.trim_start();
            if t.starts_with("```") || t.starts_with("~~~") {
                in_code = !in_code;
                continue;
            }
            if in_code {
                continue;
            }
            if let Some(k) = NORMATIVE_KEYWORDS.iter().find(|k| contains_word(line, k)) {
                out.push(
                    Diagnostic::warning(
                        "NORMATIVE_LANGUAGE_IN_BODY",
                        format!(
                            "section `{}` uses `{k}`; move obligations and exceptions into typed front matter",
                            s.id
                        ),
                    )
                    .at_line(body_line),
                );
                break;
            }
        }
    }
    out
}

fn contains_word(hay: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(i) = hay[start..].find(word) {
        let a = start + i;
        let b = a + word.len();
        let before = hay[..a].chars().next_back();
        let after = hay[b..].chars().next();
        let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if boundary(before) && boundary(after) {
            return true;
        }
        start = b;
    }
    false
}

/// Context-free semantic checks of one record.
pub fn check_record(r: &Record) -> Vec<Diagnostic> {
    let mut d = evolution::check(r);
    let c = r.common();
    let err =
        |d: &mut Vec<Diagnostic>, code: &str, msg: String| d.push(Diagnostic::error(code, msg));

    if let Err(e) = check_record_id(c.id) {
        err(&mut d, "ID_INVALID", format!("id `{}`: {e}", c.id));
    }
    let title_chars = c.title.chars().count();
    if c.title.trim().is_empty() || title_chars > MAX_TITLE_CHARS || c.title.contains('\n') {
        err(
            &mut d,
            "TITLE_INVALID",
            format!("title must be a single line of 1..={MAX_TITLE_CHARS} characters"),
        );
    }
    if c.owner.trim().is_empty() {
        err(&mut d, "OWNER_MISSING", "owner must not be empty".into());
    }
    // Scope shape.
    if c.scope.product && !c.scope.is_unconstrained() {
        err(
            &mut d,
            "SCOPE_INVALID",
            "`scope.product = true` requires empty repos/modules/features".into(),
        );
    }
    if !c.scope.product && c.scope.is_unconstrained() {
        err(
            &mut d,
            "SCOPE_EMPTY",
            "scope must set `product = true` or constrain at least one of repos/modules/features"
                .into(),
        );
    }
    for (dim, vals) in [
        ("repos", &c.scope.repos),
        ("modules", &c.scope.modules),
        ("features", &c.scope.features),
    ] {
        check_unique_list(&mut d, &format!("scope.{dim}"), vals);
    }
    // Selectors.
    for p in &c.selectors.paths {
        if let Err(e) = RepoGlob::parse(p) {
            err(&mut d, "SELECTOR_PATH_INVALID", e);
        }
    }
    check_unique_list(&mut d, "selectors.concepts", &c.selectors.concepts);
    check_unique_list(&mut d, "selectors.aliases", &c.selectors.aliases);
    for a in &c.selectors.aliases {
        if crate::normalize::AliasPattern::compile(a).is_none() {
            err(
                &mut d,
                "SELECTOR_ALIAS_INVALID",
                format!("alias `{a}` normalizes to nothing"),
            );
        }
    }
    // Links.
    for (rel, targets) in [
        ("requires", &c.links.requires),
        ("rationale", &c.links.rationale),
        ("related", &c.links.related),
        ("supersedes", &c.links.supersedes),
    ] {
        check_unique_list(&mut d, &format!("links.{rel}"), targets);
        for t in targets {
            if t == c.id {
                err(
                    &mut d,
                    "LINK_SELF",
                    format!("links.{rel} references the record itself"),
                );
            } else if let Err(e) = check_record_id(t) {
                err(
                    &mut d,
                    "LINK_ID_INVALID",
                    format!("links.{rel} target `{t}`: {e}"),
                );
            }
        }
    }
    // Applicability.
    if let Some(a) = c.applicability {
        for (repo, req) in &a.versions {
            if semver::VersionReq::parse(req).is_err() {
                err(
                    &mut d,
                    "APPLICABILITY_INVALID",
                    format!("applicability.versions.{repo}: `{req}` is not a semver requirement"),
                );
            }
        }
    }
    // Anchors.
    for (i, a) in c.anchors.iter().enumerate() {
        match a.kind {
            AnchorKind::Source | AnchorKind::Test => {
                if a.repo.is_none() || a.path.is_none() {
                    err(
                        &mut d,
                        "ANCHOR_INVALID",
                        format!(
                            "anchors[{i}]: `{}` anchors need `repo` and `path`",
                            kind_name(a.kind)
                        ),
                    );
                }
            }
            AnchorKind::Change => {
                if a.change.is_none() && a.commit.is_none() {
                    err(
                        &mut d,
                        "ANCHOR_INVALID",
                        format!("anchors[{i}]: `change` anchors need `change` or `commit`"),
                    );
                }
            }
            AnchorKind::Doc => {
                if a.path.is_none() {
                    err(
                        &mut d,
                        "ANCHOR_INVALID",
                        format!("anchors[{i}]: `doc` anchors need `path`"),
                    );
                }
            }
        }
        if let Some(p) = &a.path
            && let Err(e) = check_rel_path(p)
        {
            err(
                &mut d,
                "ANCHOR_PATH_INVALID",
                format!("anchors[{i}].path: {e}"),
            );
        }
        if let Some(cm) = &a.commit
            && (cm.len() < 7 || cm.len() > 64 || !cm.chars().all(|ch| ch.is_ascii_hexdigit()))
        {
            err(
                &mut d,
                "ANCHOR_COMMIT_INVALID",
                format!("anchors[{i}].commit must be 7..=64 hex characters"),
            );
        }
    }
    // Kind-specific.
    match r {
        Record::Policy(p) => {
            if p.rules.is_empty() && p.settings.is_empty() && p.overrides.is_empty() {
                err(
                    &mut d,
                    "POLICY_EMPTY",
                    "policy needs at least one of rules, settings, overrides".into(),
                );
            }
            check_statements(&mut d, "rules", &p.rules);
            let mut names = BTreeSet::new();
            for s in &p.settings {
                if let Err(e) = check_local_id(&s.name) {
                    err(
                        &mut d,
                        "SETTING_NAME_INVALID",
                        format!("setting `{}`: {e}", s.name),
                    );
                }
                if !names.insert(s.name.as_str()) {
                    err(
                        &mut d,
                        "SETTING_DUPLICATE",
                        format!("duplicate setting `{}`", s.name),
                    );
                }
                if s.value.type_of() != s.value_type {
                    err(
                        &mut d,
                        "SETTING_TYPE_MISMATCH",
                        format!(
                            "setting `{}` value does not match type `{:?}`",
                            s.name, s.value_type
                        ),
                    );
                }
                match (s.r#override, s.stricter) {
                    (OverrideMode::Stricter, None) => err(
                        &mut d,
                        "SETTING_STRICTER_MISSING",
                        format!(
                            "setting `{}`: `override = \"stricter\"` requires `stricter`",
                            s.name
                        ),
                    ),
                    (OverrideMode::Stricter, Some(dir)) if !stricter_fits(s.value_type, dir) => {
                        err(
                            &mut d,
                            "SETTING_STRICTER_INVALID",
                            format!("setting `{}`: direction does not fit type", s.name),
                        )
                    }
                    (OverrideMode::Forbidden | OverrideMode::Any, Some(_)) => err(
                        &mut d,
                        "SETTING_STRICTER_UNUSED",
                        format!(
                            "setting `{}`: `stricter` is only valid with `override = \"stricter\"`",
                            s.name
                        ),
                    ),
                    _ => {}
                }
                if s.r#override == OverrideMode::Forbidden && !s.override_owners.is_empty() {
                    err(
                        &mut d,
                        "SETTING_OWNERS_UNUSED",
                        format!(
                            "setting `{}`: override_owners on a non-overridable setting",
                            s.name
                        ),
                    );
                }
                if let SettingValue::StringSet(v) = &s.value {
                    check_unique_list(&mut d, &format!("settings.{}.value", s.name), v);
                }
            }
            let mut targets = BTreeSet::new();
            for o in &p.overrides {
                match o.split_target() {
                    Some((id, name)) => {
                        if check_record_id(id).is_err() || check_local_id(name).is_err() {
                            err(
                                &mut d,
                                "OVERRIDE_TARGET_INVALID",
                                format!(
                                    "override target `{}` must be `<policy-id>#<setting>`",
                                    o.target
                                ),
                            );
                        }
                        if id == c.id {
                            err(
                                &mut d,
                                "OVERRIDE_SELF",
                                format!("override `{}` targets its own policy", o.target),
                            );
                        }
                    }
                    None => err(
                        &mut d,
                        "OVERRIDE_TARGET_INVALID",
                        format!(
                            "override target `{}` must be `<policy-id>#<setting>`",
                            o.target
                        ),
                    ),
                }
                if !targets.insert(o.target.as_str()) {
                    err(
                        &mut d,
                        "OVERRIDE_DUPLICATE",
                        format!("duplicate override of `{}`", o.target),
                    );
                }
                check_text(&mut d, "overrides.reason", &o.reason);
            }
        }
        Record::Feature(f) => {
            check_text(&mut d, "summary", &f.summary);
            if f.behaviors.is_empty() {
                err(
                    &mut d,
                    "FEATURE_BEHAVIORS_EMPTY",
                    "feature needs at least one behavior".into(),
                );
            }
            check_items(&mut d, "behaviors", &f.behaviors);
            check_items(&mut d, "boundaries", &f.boundaries);
            let mut ids: Vec<&str> = f.behaviors.iter().map(|i| i.id.as_str()).collect();
            ids.extend(f.boundaries.iter().map(|i| i.id.as_str()));
            check_unique_ids(&mut d, "behaviors/boundaries", &ids);
        }
        Record::Invariant(i) => {
            if i.statements.is_empty() {
                err(
                    &mut d,
                    "INVARIANT_EMPTY",
                    "invariant needs at least one statement".into(),
                );
            }
            check_statements(&mut d, "statements", &i.statements);
        }
        Record::Contract(k) => {
            if k.parties.len() < 2 {
                err(
                    &mut d,
                    "CONTRACT_PARTIES",
                    "contract needs at least two parties".into(),
                );
            }
            let party_ids: Vec<&str> = k.parties.iter().map(|p| p.id.as_str()).collect();
            check_unique_ids(&mut d, "parties", &party_ids);
            for p in &k.parties {
                if let Err(e) = check_local_id(&p.id) {
                    err(&mut d, "LOCAL_ID_INVALID", format!("party `{}`: {e}", p.id));
                }
                check_text(&mut d, "parties.role", &p.role);
            }
            if k.obligations.is_empty() {
                err(
                    &mut d,
                    "CONTRACT_OBLIGATIONS_EMPTY",
                    "contract needs at least one obligation".into(),
                );
            }
            let mut ids = Vec::new();
            for o in &k.obligations {
                if !party_ids.contains(&o.party.as_str()) {
                    err(
                        &mut d,
                        "CONTRACT_PARTY_UNKNOWN",
                        format!("obligation `{}` names undeclared party `{}`", o.id, o.party),
                    );
                }
                check_statement_parts(
                    &mut d,
                    "obligations",
                    &o.id,
                    &o.text,
                    &o.conditions,
                    &o.exceptions,
                );
                ids.push(o.id.as_str());
            }
            check_unique_ids(&mut d, "obligations", &ids);
            if let Some(i) = &k.interface {
                check_text(&mut d, "interface", i);
            }
        }
        Record::Decision(x) => {
            check_text(&mut d, "context", &x.context);
            check_text(&mut d, "decision", &x.decision);
            if x.reasons.is_empty() {
                err(
                    &mut d,
                    "DECISION_REASONS_EMPTY",
                    "decision needs at least one reason".into(),
                );
            }
            check_text_list(&mut d, "reasons", &x.reasons);
            check_text_list(&mut d, "consequences", &x.consequences);
            for a in &x.alternatives {
                check_text(&mut d, "alternatives.option", &a.option);
                check_text(&mut d, "alternatives.rejected_because", &a.rejected_because);
            }
        }
        Record::Procedure(x) => {
            if x.steps.is_empty() {
                err(
                    &mut d,
                    "PROCEDURE_STEPS_EMPTY",
                    "procedure needs at least one step".into(),
                );
            }
            if x.expected.is_empty() {
                err(
                    &mut d,
                    "PROCEDURE_EXPECTED_EMPTY",
                    "procedure needs at least one expected result".into(),
                );
            }
            check_items(&mut d, "steps", &x.steps);
            check_unique_ids(
                &mut d,
                "steps",
                &x.steps.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            );
            check_text_list(&mut d, "preconditions", &x.preconditions);
            check_text_list(&mut d, "expected", &x.expected);
        }
        Record::Reference(x) => {
            check_text(&mut d, "summary", &x.summary);
            for s in &x.sources {
                check_text(&mut d, "sources.title", &s.title);
                if let Some(p) = &s.path
                    && let Err(e) = check_rel_path(p)
                {
                    err(&mut d, "SOURCE_PATH_INVALID", format!("sources.path: {e}"));
                }
            }
        }
        Record::Gap(g) => {
            check_text(&mut d, "description", &g.description);
            check_unique_list(&mut d, "affects", &g.affects);
            for a in &g.affects {
                if check_record_id(a).is_err() {
                    err(
                        &mut d,
                        "LINK_ID_INVALID",
                        format!("affects target `{a}` is not a record id"),
                    );
                }
            }
            check_text_list(&mut d, "questions", &g.questions);
        }
    }
    d
}

fn kind_name(k: AnchorKind) -> &'static str {
    match k {
        AnchorKind::Source => "source",
        AnchorKind::Test => "test",
        AnchorKind::Change => "change",
        AnchorKind::Doc => "doc",
    }
}

fn stricter_fits(t: SettingType, dir: Stricter) -> bool {
    matches!(
        (t, dir),
        (SettingType::Integer, Stricter::Lower | Stricter::Higher)
            | (SettingType::Boolean, Stricter::True | Stricter::False)
            | (
                SettingType::StringSet,
                Stricter::Superset | Stricter::Subset
            )
    )
}

fn check_text(d: &mut Vec<Diagnostic>, field: &str, text: &str) {
    if text.trim().is_empty() {
        d.push(Diagnostic::error(
            "TEXT_EMPTY",
            format!("`{field}` must not be empty"),
        ));
    } else if text.len() > MAX_TEXT_BYTES {
        d.push(Diagnostic::error(
            "TEXT_TOO_LONG",
            format!("`{field}` exceeds {MAX_TEXT_BYTES} bytes"),
        ));
    }
}

fn check_text_list(d: &mut Vec<Diagnostic>, field: &str, v: &[String]) {
    if v.len() > MAX_LIST_LEN {
        d.push(Diagnostic::error(
            "LIST_TOO_LONG",
            format!("`{field}` has more than {MAX_LIST_LEN} entries"),
        ));
    }
    for t in v {
        check_text(d, field, t);
    }
}

fn check_unique_list(d: &mut Vec<Diagnostic>, field: &str, v: &[String]) {
    if v.len() > MAX_LIST_LEN {
        d.push(Diagnostic::error(
            "LIST_TOO_LONG",
            format!("`{field}` has more than {MAX_LIST_LEN} entries"),
        ));
    }
    let mut seen = BTreeSet::new();
    for x in v {
        if !seen.insert(x) {
            d.push(Diagnostic::error(
                "LIST_DUPLICATE",
                format!("`{field}` lists `{x}` twice"),
            ));
        }
    }
}

fn check_unique_ids(d: &mut Vec<Diagnostic>, field: &str, ids: &[&str]) {
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(*id) {
            d.push(Diagnostic::error(
                "LOCAL_ID_DUPLICATE",
                format!("`{field}`: duplicate id `{id}`"),
            ));
        }
    }
}

fn check_items(d: &mut Vec<Diagnostic>, field: &str, items: &[Item]) {
    if items.len() > MAX_LIST_LEN {
        d.push(Diagnostic::error(
            "LIST_TOO_LONG",
            format!("`{field}` has more than {MAX_LIST_LEN} entries"),
        ));
    }
    for i in items {
        if let Err(e) = check_local_id(&i.id) {
            d.push(Diagnostic::error(
                "LOCAL_ID_INVALID",
                format!("{field} `{}`: {e}", i.id),
            ));
        }
        check_text(d, field, &i.text);
    }
}

fn check_statement_parts(
    d: &mut Vec<Diagnostic>,
    field: &str,
    id: &str,
    text: &str,
    conditions: &[String],
    exceptions: &[Exception],
) {
    if let Err(e) = check_local_id(id) {
        d.push(Diagnostic::error(
            "LOCAL_ID_INVALID",
            format!("{field} `{id}`: {e}"),
        ));
    }
    check_text(d, field, text);
    check_text_list(d, &format!("{field}.{id}.conditions"), conditions);
    let mut seen = BTreeSet::new();
    for x in exceptions {
        if let Err(e) = check_local_id(&x.id) {
            d.push(Diagnostic::error(
                "LOCAL_ID_INVALID",
                format!("{field} `{id}` exception `{}`: {e}", x.id),
            ));
        }
        if !seen.insert(&x.id) {
            d.push(Diagnostic::error(
                "LOCAL_ID_DUPLICATE",
                format!("{field} `{id}`: duplicate exception `{}`", x.id),
            ));
        }
        check_text(d, &format!("{field}.{id}.exceptions"), &x.text);
    }
}

fn check_statements(d: &mut Vec<Diagnostic>, field: &str, v: &[Statement]) {
    if v.len() > MAX_LIST_LEN {
        d.push(Diagnostic::error(
            "LIST_TOO_LONG",
            format!("`{field}` has more than {MAX_LIST_LEN} entries"),
        ));
    }
    for s in v {
        check_statement_parts(d, field, &s.id, &s.text, &s.conditions, &s.exceptions);
    }
    check_unique_ids(
        d,
        field,
        &v.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
    );
}

/// Non-fatal lints for a parsed record (re-derived during validation).
pub fn lint_record(parsed: &ParsedRecord) -> Vec<Diagnostic> {
    lint_body(&parsed.sections, 0)
        .into_iter()
        .map(|mut d| {
            d.line = None;
            d.record = Some(parsed.record.id().to_string());
            d
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: &str = r#"+++
schema = 1
id = "acme.mobile.token-storage"
kind = "policy"
title = "Token storage"
status = "accepted"
owner = "team-mobile"

[scope]
repos = ["mobile"]

[[rules]]
id = "no-plaintext"
level = "must-not"
text = "Store refresh tokens in plaintext storage."

[[rules.exceptions]]
id = "fake-server"
text = "Debug builds against the fake auth server."
+++
Intro text.

## Background
Why this exists.

```text
## not a heading
```
"#;

    #[test]
    fn parses_policy_and_sections() {
        let p = parse_record("k/a.md", POLICY.as_bytes()).unwrap();
        assert_eq!(p.record.id(), "acme.mobile.token-storage");
        assert_eq!(p.record.kind(), Kind::Policy);
        let ids: Vec<_> = p.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["intro", "background"]);
        assert!(p.sections[1].markdown.contains("## not a heading"));
        let meta = p.meta();
        assert_eq!(meta.sections, vec!["intro", "background"]);
    }

    fn with(extra_fm: &str) -> String {
        POLICY.replacen(
            "owner = \"team-mobile\"\n",
            &format!("owner = \"team-mobile\"\n{extra_fm}\n"),
            1,
        )
    }

    #[test]
    fn rejects_unknown_fields_duplicates_and_bad_enums() {
        let unknown = with("surprise = 1");
        let e = parse_record("a.md", unknown.as_bytes()).unwrap_err();
        assert_eq!(e[0].code, "FRONT_MATTER_INVALID");
        let dup = with("title = \"again\"");
        assert_eq!(
            parse_record("a.md", dup.as_bytes()).unwrap_err()[0].code,
            "FRONT_MATTER_SYNTAX"
        );
        let bad_status = POLICY.replace("status = \"accepted\"", "status = \"approved\"");
        assert_eq!(
            parse_record("a.md", bad_status.as_bytes()).unwrap_err()[0].code,
            "FRONT_MATTER_INVALID"
        );
        let bad_schema = POLICY.replace("schema = 1", "schema = 7");
        assert_eq!(
            parse_record("a.md", bad_schema.as_bytes()).unwrap_err()[0].code,
            "UNSUPPORTED_SCHEMA_VERSION"
        );
        let bad_kind = POLICY.replace("kind = \"policy\"", "kind = \"rule\"");
        assert_eq!(
            parse_record("a.md", bad_kind.as_bytes()).unwrap_err()[0].code,
            "KIND_UNKNOWN"
        );
    }

    #[test]
    fn toml_errors_report_file_lines() {
        // `with` inserts the extra key on file line 8 (line 1 is the opening `+++`).
        let at = |text: &str| {
            let e = parse_record("a.md", text.as_bytes()).unwrap_err();
            assert_eq!(e.len(), 1, "{e:?}");
            (e[0].code.clone(), e[0].line, e[0].message.clone())
        };
        let (code, line, msg) = at(&with("surprise = 1"));
        assert_eq!((code.as_str(), line), ("FRONT_MATTER_INVALID", Some(8)));
        assert!(
            msg.starts_with("TOML parse error at line 8, column 1: unknown field `surprise`"),
            "{msg}"
        );
        let (code, line, msg) = at(&with("title = \"again\""));
        assert_eq!((code.as_str(), line), ("FRONT_MATTER_SYNTAX", Some(8)));
        assert!(
            msg.starts_with("TOML parse error at line 8, column 1: "),
            "{msg}"
        );
        let (code, line, msg) = at(&with("broken = "));
        assert_eq!((code.as_str(), line), ("FRONT_MATTER_SYNTAX", Some(8)));
        assert!(
            msg.starts_with("TOML parse error at line 8, column 10: "),
            "{msg}"
        );
        // `status` is on file line 6; the value starts in column 10.
        let bad_status = POLICY.replace("status = \"accepted\"", "status = \"approved\"");
        let (code, line, msg) = at(&bad_status);
        assert_eq!((code.as_str(), line), ("FRONT_MATTER_INVALID", Some(6)));
        assert!(
            msg.starts_with("TOML parse error at line 6, column 10: unknown variant `approved`"),
            "{msg}"
        );
        // A byte-order mark does not shift lines; multi-byte characters count as one column.
        let bom = format!(
            "\u{feff}{}",
            POLICY.replace("title = \"Token storage\"", "title = \"Токен\" x")
        );
        let (code, line, msg) = at(&bom);
        assert_eq!((code.as_str(), line), ("FRONT_MATTER_SYNTAX", Some(5)));
        assert!(
            msg.starts_with("TOML parse error at line 5, column 17: "),
            "{msg}"
        );
    }

    #[test]
    fn rejects_structural_problems() {
        let empty_scope = POLICY.replace("repos = [\"mobile\"]", "");
        assert!(
            parse_record("a.md", empty_scope.as_bytes())
                .unwrap_err()
                .iter()
                .any(|d| d.code == "SCOPE_EMPTY")
        );
        let self_link = with("[links]\nrequires = [\"acme.mobile.token-storage\"]");
        assert!(
            parse_record("a.md", self_link.as_bytes())
                .unwrap_err()
                .iter()
                .any(|d| d.code == "LINK_SELF")
        );
        assert!(parse_record("a.md", b"no front matter").is_err());
        assert!(parse_record("a.md", b"+++\nschema = 1\n").is_err());
    }

    #[test]
    fn lints_normative_body() {
        let text = POLICY.replace(
            "Why this exists.",
            "Tokens MUST NOT be logged, except in tests.",
        );
        let p = parse_record("a.md", text.as_bytes()).unwrap();
        let lints = lint_record(&p);
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].code, "NORMATIVE_LANGUAGE_IN_BODY");
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Background & Rationale!"), "background-rationale");
        assert_eq!(slug("Почему так?"), "почему-так");
    }
}
