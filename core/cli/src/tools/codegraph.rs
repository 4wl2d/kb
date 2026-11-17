//! CodeGraph adapter over the code root's live index (`.codegraph/` of the host).
//!
//! Interactive lookups use the index the developer already keeps, brought up to date by the
//! tool's incremental `sync` (registry `sync = "auto"`, the default) before every query, so
//! answers describe the current work tree. kb never creates the index: a missing index makes
//! the tool unavailable for this code root and routing falls back. The commit-bound
//! `kb.code.v1` path (ADR 0011) remains for historical and composed context.

use serde_json::Value;

use super::{CodeQuery, Outcome, Probe, Roots, ToolSpec, rel_to_code, run_tool};
use crate::answer::{Item, Source};
use crate::error::{KbError, Result};
use crate::model::tools::{Capability, SyncMode};

/// Index state of the code root as `codegraph status --json` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexState {
    pub initialized: bool,
    pub complete: bool,
    pub pending_changes: u64,
    pub pending_refs: u64,
}

impl IndexState {
    pub fn describe(&self, synced: bool) -> String {
        if !self.initialized {
            return "no index".into();
        }
        let mut s = String::from(if synced {
            "index synced"
        } else {
            "index as is"
        });
        if self.pending_changes > 0 {
            s.push_str(&format!(
                ", {} file change(s) not indexed",
                self.pending_changes
            ));
        }
        s.push_str(if self.complete && self.pending_refs == 0 {
            ", complete"
        } else {
            ", incomplete"
        });
        s
    }
}

pub fn parse_status(v: &Value) -> IndexState {
    let pending = &v["pendingChanges"];
    let changes = ["added", "modified", "removed"]
        .iter()
        .map(|k| pending[k].as_u64().unwrap_or(0))
        .sum();
    IndexState {
        initialized: v["initialized"].as_bool().unwrap_or(false),
        complete: v["index"]["state"].as_str() == Some("complete"),
        pending_changes: changes,
        pending_refs: v["index"]["pendingRefs"].as_u64().unwrap_or(0),
    }
}

fn json_of(out: &crate::process::Captured, what: &str) -> Result<Option<Value>> {
    if out.exit_code != 0 {
        return Err(KbError::invalid_input(format!(
            "codegraph {what} exited {}: {}",
            out.exit_code,
            crate::git::redact(String::from_utf8_lossy(&out.stderr).trim())
        )));
    }
    // A symbol the index does not know is reported as plain text, not JSON.
    Ok(serde_json::from_slice(&out.stdout).ok())
}

/// `codegraph status <root> --json`.
pub fn status(program: &std::path::Path, roots: &Roots, timeout: u64) -> Result<IndexState> {
    let root = roots.code.display().to_string();
    let out = run_tool(
        program,
        &["status".into(), root, "--json".into()],
        roots,
        timeout,
    )?;
    let v = json_of(&out, "status")?
        .ok_or_else(|| KbError::invalid_input("codegraph status printed no JSON"))?;
    Ok(parse_status(&v))
}

pub fn query(tool: &ToolSpec, probe: &Probe, roots: &Roots, q: &CodeQuery) -> Result<Outcome> {
    let program = std::path::PathBuf::from(probe.program.as_deref().unwrap_or("codegraph"));
    let timeout = tool.timeout();
    let root = roots.code.display().to_string();
    let mut state = status(&program, roots, timeout)?;
    if !state.initialized {
        return Err(KbError::invalid_input(format!(
            "no CodeGraph index in {root}; create it with `codegraph init --yes {root}` (kb never creates tool indexes)"
        )));
    }
    let synced = tool.sync.unwrap_or_default() == SyncMode::Auto;
    if synced {
        let out = run_tool(&program, &["sync".into(), root.clone()], roots, timeout)?;
        if out.exit_code != 0 {
            return Err(KbError::invalid_input(format!(
                "codegraph sync exited {}",
                out.exit_code
            )));
        }
        state = status(&program, roots, timeout)?;
    }
    let mut outcome = Outcome::new(Source {
        id: tool.id.clone(),
        adapter: "codegraph".into(),
        version: probe.version.clone(),
        state: state.describe(synced),
    });
    if state.pending_changes > 0 {
        outcome.notes.push(format!(
            "{} changed file(s) are not in the index{}",
            state.pending_changes,
            if synced { "" } else { " (sync = \"never\")" }
        ));
    }
    let limit = q.limit.to_string();
    let mut args: Vec<String> = match q.capability {
        Capability::Sym => vec![
            "query".into(),
            q.name.clone(),
            "--limit".into(),
            (q.limit * 3).max(20).to_string(),
        ],
        Capability::Callers => vec!["callers".into(), q.name.clone(), "--limit".into(), limit],
        Capability::Impact => vec![
            "impact".into(),
            q.name.clone(),
            "--depth".into(),
            q.depth.to_string(),
        ],
        _ => return Err(KbError::internal("codegraph cannot answer this capability")),
    };
    args.extend(["--path".into(), root, "--json".into()]);
    if let Some(kind) = &q.kind
        && q.capability == Capability::Sym
    {
        args.extend(["--kind".into(), kind.clone()]);
    }
    if let Some(file) = &q.file
        && q.capability != Capability::Sym
    {
        args.extend(["--file".into(), file.clone()]);
    }
    let out = run_tool(&program, &args, roots, timeout)?;
    let Some(v) = json_of(&out, args[0].as_str())? else {
        if let Some(suggestion) = missing_symbol_suggestion(&out.stdout) {
            outcome.notes.push(suggestion);
        }
        return Ok(outcome);
    };
    match q.capability {
        Capability::Sym => definitions(&mut outcome, &v, roots, q),
        Capability::Callers => {
            callers(&mut outcome, &v, roots);
            if state.pending_changes > 0 {
                outcome.call_site = None;
            }
        }
        Capability::Impact => impact(&mut outcome, &v, roots),
        _ => {}
    }
    Ok(outcome)
}

/// CodeGraph emits a plain-text miss even when `--json` is present.
fn missing_symbol_suggestion(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let names = text
        .lines()
        .find_map(|line| line.split_once(" — did you mean: "))?
        .1
        .trim();
    (!names.is_empty()).then(|| {
        format!(
            "CodeGraph suggests {}",
            crate::answer::clip(&crate::git::redact(names), 160)
        )
    })
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v[key].as_str().filter(|x| !x.is_empty())
}

fn line(v: &Value, key: &str) -> Option<u32> {
    v[key].as_u64().and_then(|n| u32::try_from(n).ok())
}

/// Exact name or qualified-name match (`Type::name`, `pkg.name`, ...).
pub fn exact(name: &str, qualified: Option<&str>, wanted: &str) -> bool {
    name == wanted
        || qualified.is_some_and(|q| {
            q == wanted
                || q.strip_suffix(wanted)
                    .is_some_and(|p| p.ends_with("::") || p.ends_with('.') || p.ends_with('/'))
        })
}

fn definitions(outcome: &mut Outcome, v: &Value, roots: &Roots, q: &CodeQuery) {
    let mut exact_items = Vec::new();
    let mut similar = Vec::new();
    for hit in v.as_array().into_iter().flatten() {
        let node = &hit["node"];
        let Some(name) = s(node, "name") else {
            continue;
        };
        let Some(path) = s(node, "filePath") else {
            continue;
        };
        let kind = s(node, "kind").unwrap_or("symbol");
        if kind == "import" && q.kind.as_deref() != Some("import") {
            continue;
        }
        let path = rel_to_code(roots, path);
        if q.file.as_deref().is_some_and(|f| !path.contains(f)) {
            continue;
        }
        let qualified = s(node, "qualifiedName");
        let item = Item::new("definition", &outcome.source.id)
            .name(qualified.unwrap_or(name))
            .at(path, line(node, "startLine"))
            .span(line(node, "endLine"))
            .detail(format!("{kind} {}", s(node, "signature").unwrap_or("")));
        if exact(name, qualified, &q.name) {
            exact_items.push(item);
        } else {
            similar.push(item);
        }
    }
    outcome.found = !exact_items.is_empty();
    if outcome.found {
        outcome.items = exact_items;
        return;
    }
    if !similar.is_empty() {
        outcome.notes.push(format!(
            "no exact definition of `{}`; similar symbols follow",
            q.name
        ));
    }
    for i in &mut similar {
        i.kind = "similar".into();
    }
    outcome.items = similar;
}

fn ambiguity(outcome: &mut Outcome, v: &Value) {
    let targets = v["targets"].as_array().map_or(0, Vec::len);
    if v["ambiguous"].as_bool() == Some(true) || targets > 1 {
        outcome.notes.push(format!(
            "{targets} definitions share this name; narrow with --file"
        ));
    }
}

fn callers(outcome: &mut Outcome, v: &Value, roots: &Roots) {
    outcome.found = v["targets"].as_array().is_some_and(|t| !t.is_empty());
    ambiguity(outcome, v);
    let mut seen = std::collections::BTreeSet::new();
    for def in v["definitions"].as_array().into_iter().flatten() {
        let edges = def["edges"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        for c in def["callers"].as_array().into_iter().flatten() {
            let Some(path) = s(c, "filePath") else {
                continue;
            };
            let path = rel_to_code(roots, path);
            let id = s(c, "id");
            let mut matching_edges = edges
                .iter()
                .filter(|e| id.is_some() && s(e, "source") == id);
            let call_site = matching_edges
                .clone()
                .filter(|e| s(e, "kind") == Some("calls"))
                .find_map(|e| line(e, "line").filter(|&n| n > 0));
            let call_line = call_site
                .or_else(|| matching_edges.next().and_then(|e| line(e, "line")))
                .or_else(|| line(c, "startLine"));
            let rel = c["relationships"]
                .as_array()
                .map(|r| {
                    r.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let name = s(c, "name").unwrap_or("?").to_string();
            if !seen.insert((path.clone(), call_line, name.clone())) {
                continue;
            }
            if let Some(line) = call_site
                && outcome.call_site.is_none()
            {
                outcome.call_site = Some((path.clone(), line));
            }
            outcome.items.push(
                Item::new("caller", &outcome.source.id)
                    .name(name)
                    .at(path, call_line)
                    .detail(format!("{} {rel}", s(c, "kind").unwrap_or(""))),
            );
        }
        if def["truncated"].as_bool() == Some(true) {
            outcome
                .notes
                .push("the tool truncated the caller list; narrow with --file".into());
        }
    }
    if v["definitions"].as_array().is_none() {
        for c in v["callers"].as_array().into_iter().flatten() {
            let Some(path) = s(c, "filePath") else {
                continue;
            };
            outcome.items.push(
                Item::new("caller", &outcome.source.id)
                    .name(s(c, "name").unwrap_or("?"))
                    .at(rel_to_code(roots, path), line(c, "startLine"))
                    .detail(s(c, "kind").unwrap_or("")),
            );
        }
    }
}

fn impact(outcome: &mut Outcome, v: &Value, roots: &Roots) {
    let targets: Vec<(String, Option<u32>)> = v["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| Some((rel_to_code(roots, s(t, "filePath")?), line(t, "startLine"))))
        .collect();
    outcome.found = !targets.is_empty();
    ambiguity(outcome, v);
    let mut files = std::collections::BTreeSet::new();
    for (path, _) in &targets {
        files.insert(path.clone());
    }
    let mut seen = std::collections::BTreeSet::new();
    for a in v["affected"].as_array().into_iter().flatten() {
        let Some(path) = s(a, "filePath") else {
            continue;
        };
        let path = rel_to_code(roots, path);
        let at = line(a, "startLine");
        files.insert(path.clone());
        if targets.contains(&(path.clone(), at)) || !seen.insert((path.clone(), at)) {
            continue;
        }
        outcome.items.push(
            Item::new("affected", &outcome.source.id)
                .name(s(a, "name").unwrap_or("?"))
                .at(path, at)
                .detail(s(a, "kind").unwrap_or("")),
        );
    }
    outcome.files = files.into_iter().collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roots() -> Roots {
        Roots {
            kb: "/kb".into(),
            code: "/code".into(),
            repo: None,
            kb_mount: None,
        }
    }

    fn outcome() -> Outcome {
        Outcome::new(Source {
            id: "codegraph".into(),
            adapter: "codegraph".into(),
            version: None,
            state: "index synced, complete".into(),
        })
    }

    #[test]
    fn status_reports_pending_work() {
        let st = parse_status(
            &json!({"initialized": true, "pendingChanges": {"added": 1, "modified": 2, "removed": 0}, "index": {"state": "complete", "pendingRefs": 0}}),
        );
        assert_eq!(
            st.describe(false),
            "index as is, 3 file change(s) not indexed, complete"
        );
        assert_eq!(parse_status(&json!({})).describe(true), "no index");
    }

    #[test]
    fn exact_matches_rank_before_similar_symbols() {
        let q = CodeQuery {
            capability: Capability::Sym,
            name: "refresh".into(),
            file: None,
            kind: None,
            depth: 2,
            limit: 10,
        };
        let v = json!([
            {"node": {"name": "refreshAll", "kind": "function", "filePath": "/code/a.kt", "startLine": 3, "endLine": 9}},
            {"node": {"name": "pkg.refresh", "qualifiedName": "Source::pkg.refresh", "kind": "import", "filePath": "/code/a.kt", "startLine": 1}},
            {"node": {"name": "refresh", "qualifiedName": "TokenRefresher::refresh", "kind": "method", "filePath": "/code/b.kt", "startLine": 40, "endLine": 80, "signature": "(): Token"}}
        ]);
        let mut o = outcome();
        definitions(&mut o, &v, &roots(), &q);
        assert!(o.found);
        assert_eq!(o.items[0].name.as_deref(), Some("TokenRefresher::refresh"));
        assert_eq!(o.items[0].path.as_deref(), Some("b.kt"));
        assert_eq!(
            o.items.len(),
            1,
            "imports are not definitions and similar symbols are dropped on exact hits"
        );
        assert!(exact("x", Some("pkg.Type.x"), "x"));
        assert!(!exact("xy", Some("pkg.xy"), "y"));
    }

    #[test]
    fn codegraph_miss_keeps_its_spelling_suggestion() {
        let response = "ℹ Symbol \"refResh\" not found — did you mean: refresh, refreshAll?\n";
        assert_eq!(
            missing_symbol_suggestion(response.as_bytes()).as_deref(),
            Some("CodeGraph suggests refresh, refreshAll?")
        );
        assert_eq!(
            missing_symbol_suggestion("ℹ Symbol \"missing\" not found\n".as_bytes()),
            None
        );
    }

    #[test]
    fn callers_take_call_lines_from_edges() {
        let v = json!({
            "targets": [{"name": "fan_in", "filePath": "core/cli/src/code.rs", "startLine": 422}],
            "ambiguous": false,
            "definitions": [{
                "callers": [{"id": "f:1", "name": "run", "kind": "function", "filePath": "core/cli/src/cli/cmd_coverage.rs", "startLine": 11, "relationships": ["calls"]}],
                "edges": [
                    {"source": "f:1", "target": "f:0", "kind": "references", "line": 12},
                    {"source": "f:1", "target": "f:0", "kind": "calls", "line": 53}
                ],
                "truncated": false
            }]
        });
        let mut o = outcome();
        callers(&mut o, &v, &roots());
        assert!(o.found);
        assert_eq!(o.items.len(), 1);
        assert_eq!(o.items[0].line, Some(53));
        assert_eq!(o.items[0].detail.as_deref(), Some("function calls"));
        assert_eq!(
            o.call_site
                .as_ref()
                .map(|(path, line)| (path.as_str(), *line)),
            Some(("core/cli/src/cli/cmd_coverage.rs", 53))
        );
    }

    #[test]
    fn impact_excludes_targets_and_collects_files() {
        let v = json!({
            "targets": [{"name": "fan_in", "filePath": "a.rs", "startLine": 4}],
            "affected": [
                {"name": "fan_in", "kind": "function", "filePath": "a.rs", "startLine": 4},
                {"name": "run", "kind": "function", "filePath": "b.rs", "startLine": 11},
                {"name": "dispatch", "kind": "function", "filePath": "c.rs", "startLine": 2}
            ]
        });
        let mut o = outcome();
        impact(&mut o, &v, &roots());
        assert_eq!(o.items.len(), 2);
        assert_eq!(o.files, ["a.rs", "b.rs", "c.rs"]);
    }
}
