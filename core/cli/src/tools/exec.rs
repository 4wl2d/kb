//! `exec` adapter: any command-line tool, connected by configuration alone.
//!
//! The registry gives the argv template (`args`, placeholders [`EXEC_PLACEHOLDERS`]), the
//! exit codes that mean success (`ok_exit`), the environment (`env`) and how stdout becomes
//! answer items (`output`: `lines`, `grep` or `json` with `items`/`fields`/`strings`).
//! The program runs without a shell, in the code root, under its deadline and output limits.
//! A tool may also print the native item protocol directly: a JSON object
//! `{"items": [{kind?, name?, path?, line?, end_line?, detail?, confidence?}], "found"?,
//! "state"?, "notes"?}`, which `output = "json"` reads without any mapping.
//!
//! [`EXEC_PLACEHOLDERS`]: crate::model::tools::EXEC_PLACEHOLDERS

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

use super::{Outcome, Probe, Roots, ToolSpec, rel_to_code, run_with_env};
use crate::answer::{Item, Source};
use crate::error::{KbError, Result};
use crate::model::tools::{ItemField, OutputFormat, placeholders};

/// Items read from one tool output at most; more are dropped with a note.
pub const MAX_ITEMS: usize = 10_000;

/// One request to an `exec` tool.
#[derive(Debug, Clone, Default)]
pub struct ExecRequest {
    /// Built-in request (`sym`, `callers`, `impact`, `find`) or project capability.
    pub capability: String,
    /// The symbol, text or joined arguments of the request.
    pub subject: String,
    /// Extra arguments of `kb run`, for `{args}`.
    pub args: Vec<String>,
    pub file: Option<String>,
    pub kind: Option<String>,
    pub depth: u32,
    pub limit: usize,
    pub globs: Vec<String>,
}

/// The default item kind of a request.
pub fn default_kind(capability: &str) -> &'static str {
    match capability {
        "sym" => "definition",
        "callers" => "caller",
        "impact" => "affected",
        "find" => "match",
        _ => "item",
    }
}

/// Expand the argv template. An argument whose optional value (`{file}`, `{kind}`) is
/// absent is dropped; `{args}` and `{globs}` splat.
pub fn expand(tool: &ToolSpec, roots: &Roots, req: &ExecRequest) -> Result<Vec<String>> {
    expand_template(tool, &tool.args, roots, req, true)
}

/// Expand `template`; with `require_input`, input the template cannot carry is an error
/// instead of being silently dropped.
fn expand_template(
    tool: &ToolSpec,
    template: &[String],
    roots: &Roots,
    req: &ExecRequest,
    require_input: bool,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut uses_input = false;
    for a in template {
        if a == "{args}" {
            uses_input = true;
            out.extend(req.args.iter().cloned());
            continue;
        }
        if a == "{globs}" {
            out.extend(req.globs.iter().cloned());
            continue;
        }
        // One pass over the template: substituted values are never scanned again, so input
        // that contains `{root}` or `{kind}` text reaches the tool verbatim.
        placeholders(a).map_err(KbError::invalid_input)?;
        let mut value = String::new();
        let mut rest = a.as_str();
        let mut keep = true;
        while let Some(start) = rest.find('{') {
            value.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let end = after.find('}').unwrap_or(after.len());
            let n = &after[..end];
            rest = after.get(end + 1..).unwrap_or("");
            let v = match n {
                "subject" => {
                    uses_input = true;
                    Some(req.subject.clone())
                }
                "file" => req.file.clone(),
                "kind" => req.kind.clone(),
                "depth" => Some(req.depth.to_string()),
                "limit" => Some(req.limit.to_string()),
                "root" => Some(roots.code.display().to_string()),
                "kb" => Some(roots.kb.display().to_string()),
                other => {
                    return Err(KbError::invalid_input(format!(
                        "tool `{}`: unknown placeholder `{{{other}}}`",
                        tool.id
                    )));
                }
            };
            match v {
                Some(v) => value.push_str(&v),
                None => keep = false,
            }
        }
        value.push_str(rest);
        if keep {
            out.push(value);
        }
    }
    if require_input && !uses_input && (!req.subject.is_empty() || !req.args.is_empty()) {
        return Err(KbError::invalid_input(format!(
            "tool `{}` takes no input: its `args` use neither `{{subject}}` nor `{{args}}`",
            tool.id
        )));
    }
    Ok(out)
}

pub fn query(tool: &ToolSpec, probe: &Probe, roots: &Roots, req: &ExecRequest) -> Result<Outcome> {
    let program = Path::new(probe.program.as_deref().unwrap_or_default());
    let argv = expand(tool, roots, req)?;
    if !tool.prepare.is_empty() {
        let prepare = expand_template(tool, &tool.prepare, roots, req, false)?;
        let out = run_with_env(program, &prepare, roots, tool.timeout(), &tool.env)?;
        if out.exit_code != 0 {
            return Err(KbError::invalid_input(format!(
                "prepare step `{}` exited {}",
                prepare.join(" "),
                out.exit_code
            )));
        }
    }
    let out = run_with_env(program, &argv, roots, tool.timeout(), &tool.env)?;
    if !tool.ok_exit().contains(&out.exit_code) {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let last = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        return Err(KbError::invalid_input(format!(
            "exited {}{}",
            out.exit_code,
            if last.is_empty() {
                String::new()
            } else {
                format!(": {}", crate::git::redact(last.trim()))
            }
        )));
    }
    let mut outcome = Outcome::new(Source {
        id: tool.id.clone(),
        adapter: "exec".into(),
        version: probe.version.clone(),
        state: if tool.prepare.is_empty() {
            "working tree".into()
        } else {
            "working tree, prepared".into()
        },
    });
    let kind = tool
        .kind
        .clone()
        .unwrap_or_else(|| default_kind(&req.capability).to_string());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut items = match tool.output.unwrap_or_default() {
        OutputFormat::Lines => stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| Item::new(&kind, &tool.id).detail(l))
            .collect(),
        OutputFormat::Grep => stdout
            .lines()
            .filter_map(parse_grep_line)
            .map(|(path, line, text)| {
                Item::new(&kind, &tool.id)
                    .at(rel_to_code(roots, &path), Some(line))
                    .detail(text)
            })
            .collect(),
        OutputFormat::Json => 'json: {
            // Tools often report "not found" as plain text even in JSON mode: that is an
            // empty answer whose message becomes a note, not a tool failure.
            let Ok(doc) = serde_json::from_str::<Value>(stdout.trim()) else {
                if let Some(first) = stdout.lines().map(str::trim).find(|l| !l.is_empty()) {
                    outcome.notes.push(format!(
                        "{} printed no JSON: {}",
                        tool.id,
                        crate::answer::clip(first, 200)
                    ));
                }
                break 'json Vec::new();
            };
            if let Some(o) = doc.as_object() {
                if let Some(s) = o.get("state").and_then(Value::as_str) {
                    outcome.source.state = crate::answer::clip(s, 120);
                }
                for n in o
                    .get("notes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(n) = n.as_str() {
                        outcome.notes.push(crate::answer::clip(n, 300));
                    }
                }
            }
            let selected = select(&doc, tool.items.as_deref())?;
            let found = doc.get("found").and_then(Value::as_bool);
            let items: Vec<Item> = selected
                .into_iter()
                .filter_map(|v| map_item(v, tool, &kind, roots))
                .collect();
            if let Some(found) = found {
                outcome.found = found;
            }
            items
        }
    };
    if items.len() > MAX_ITEMS {
        outcome.notes.push(format!(
            "the tool printed {} items; the first {MAX_ITEMS} were read",
            items.len()
        ));
        items.truncate(MAX_ITEMS);
    }
    if let Some(c) = &tool.confidence {
        for i in items.iter_mut().filter(|i| i.confidence.is_none()) {
            i.confidence = Some(c.clone());
        }
    }
    if !outcome.found {
        outcome.found = !items.is_empty();
    }
    let files: BTreeSet<String> = items.iter().filter_map(|i| i.path.clone()).collect();
    outcome.files = files.into_iter().collect();
    outcome.items = items;
    Ok(outcome)
}

/// `path:line:text`, `path:line:column:text`, or the NUL-delimited forms of grep/ripgrep.
pub fn parse_grep_line(raw: &str) -> Option<(String, u32, String)> {
    if raw.contains('\0') {
        let hit = super::grep::parse_hits(format!("{raw}\n").as_bytes())
            .into_iter()
            .next()?;
        return Some((hit.path, hit.line, hit.text));
    }
    let mut parts = raw.splitn(4, ':');
    let path = parts.next()?.trim();
    let line: u32 = parts.next()?.trim().parse().ok()?;
    let third = parts.next().unwrap_or("");
    let text = match parts.next() {
        Some(rest) if third.trim().parse::<u32>().is_ok() => rest,
        Some(rest) => return Some((path.into(), line, format!("{third}:{rest}").trim().into())),
        None => third,
    };
    (!path.is_empty() && line > 0).then(|| (path.to_string(), line, text.trim().to_string()))
}

/// Select the item values: JSON pointer segments, `*` flattening arrays (and object values).
/// Without a path: the document if it is an array, else its `items` array.
pub fn select<'a>(doc: &'a Value, path: Option<&str>) -> Result<Vec<&'a Value>> {
    let path = match path {
        Some(p) => p,
        None if doc.is_array() => "",
        None if doc.get("items").is_some_and(Value::is_array) => "/items",
        None => {
            return Err(KbError::invalid_input(
                "JSON output is neither an array nor an object with `items`; set `items` in tools.toml",
            ));
        }
    };
    let mut current = vec![doc];
    for seg in path.split('/').skip(1) {
        let seg = seg.replace("~1", "/").replace("~0", "~");
        let mut next = Vec::new();
        for v in current {
            if seg == "*" {
                match v {
                    Value::Array(a) => next.extend(a.iter()),
                    Value::Object(o) => next.extend(o.values()),
                    _ => {}
                }
            } else if let Some(child) = match v {
                Value::Object(o) => o.get(&seg),
                Value::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get(i)),
                _ => None,
            } {
                next.push(child);
            }
        }
        current = next;
    }
    Ok(current
        .into_iter()
        .flat_map(|v| match v {
            Value::Array(a) => a.iter().collect::<Vec<_>>(),
            other => vec![other],
        })
        .collect())
}

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Array(a) => {
            let parts: Vec<String> = a.iter().filter_map(text_of).collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        }
        _ => None,
    }
}

fn line_of(v: &Value) -> Option<u32> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
}

/// Map one selected value to an item; `None` when it carries nothing to show.
pub fn map_item(v: &Value, tool: &ToolSpec, kind: &str, roots: &Roots) -> Option<Item> {
    let mut item = Item::new(kind, &tool.id);
    match v {
        Value::Object(o) => {
            let get = |f: ItemField| -> Option<&Value> {
                let key = tool
                    .fields
                    .get(&f)
                    .map(String::as_str)
                    .unwrap_or(f.as_str());
                if key.starts_with('/') {
                    v.pointer(key)
                } else {
                    o.get(key)
                }
            };
            // An explicit `kind` in the registry wins over an element's own `kind` key, unless
            // `fields.kind` maps one deliberately.
            let read_kind = tool.kind.is_none() || tool.fields.contains_key(&ItemField::Kind);
            if read_kind && let Some(k) = get(ItemField::Kind).and_then(text_of) {
                item.kind = crate::answer::clip(&k, 40);
            }
            item.name = get(ItemField::Name).and_then(text_of);
            item.path = get(ItemField::Path)
                .and_then(text_of)
                .map(|p| rel_to_code(roots, &p));
            item.line = get(ItemField::Line).and_then(line_of);
            item.end_line = get(ItemField::EndLine)
                .and_then(line_of)
                .filter(|e| Some(*e) != item.line);
            item.detail = get(ItemField::Detail)
                .and_then(text_of)
                .filter(|d| !d.trim().is_empty());
            item.confidence = get(ItemField::Confidence)
                .and_then(Value::as_str)
                .filter(|c| matches!(*c, "resolved" | "possible" | "lexical"))
                .map(str::to_string);
        }
        Value::String(_) | Value::Number(_) | Value::Bool(_) => {
            let text = text_of(v)?;
            match tool.strings.unwrap_or(ItemField::Detail) {
                ItemField::Path => item.path = Some(rel_to_code(roots, &text)),
                ItemField::Name => item.name = Some(text),
                ItemField::Kind => item.kind = text,
                ItemField::Line => item.line = line_of(v),
                ItemField::EndLine => item.end_line = line_of(v),
                ItemField::Confidence => item.confidence = Some(text),
                ItemField::Detail => item.detail = Some(text),
            }
        }
        _ => return None,
    }
    (item.name.is_some() || item.path.is_some() || item.detail.is_some()).then_some(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tools::AdapterKind;
    use serde_json::json;

    fn roots() -> Roots {
        Roots {
            kb: "/kb".into(),
            code: "/code".into(),
            repo: None,
            kb_mount: None,
        }
    }

    fn tool(args: &[&str]) -> ToolSpec {
        ToolSpec {
            id: "t".into(),
            adapter: AdapterKind::Exec,
            program: Some("t".into()),
            args: args.iter().map(|s| s.to_string()).collect(),
            capabilities: vec!["deps".into()],
            ..ToolSpec::default()
        }
    }

    #[test]
    fn argv_templates_expand_and_drop_absent_optionals() {
        let req = ExecRequest {
            capability: "sym".into(),
            subject: "refresh".into(),
            file: None,
            kind: Some("method".into()),
            depth: 2,
            limit: 30,
            args: vec!["a b".into(), "--x".into()],
            globs: vec!["*.kt".into()],
        };
        let t = tool(&[
            "q",
            "{subject}",
            "--file={file}",
            "--kind={kind}",
            "--root",
            "{root}",
            "{args}",
            "{globs}",
            "-n{limit}",
        ]);
        assert_eq!(
            expand(&t, &roots(), &req).unwrap(),
            [
                "q",
                "refresh",
                "--kind=method",
                "--root",
                "/code",
                "a b",
                "--x",
                "*.kt",
                "-n30"
            ]
        );
        let literal = ExecRequest {
            subject: "{root} and {kind}".into(),
            ..ExecRequest::default()
        };
        assert_eq!(
            expand(&tool(&["--q={subject}!"]), &roots(), &literal).unwrap(),
            ["--q={root} and {kind}!"],
            "substituted input is never expanded again"
        );
        let silent = tool(&["status"]);
        assert!(
            expand(&silent, &roots(), &req)
                .unwrap_err()
                .message
                .contains("takes no input")
        );
        assert_eq!(
            expand(&silent, &roots(), &ExecRequest::default()).unwrap(),
            ["status"]
        );
    }

    #[test]
    fn grep_lines_parse_with_and_without_columns() {
        assert_eq!(
            parse_grep_line("src/a.rs:12:5:let x = 1;"),
            Some(("src/a.rs".into(), 12, "let x = 1;".into()))
        );
        assert_eq!(
            parse_grep_line("src/a.rs:3:fn a() { x: 1 }"),
            Some(("src/a.rs".into(), 3, "fn a() { x: 1 }".into()))
        );
        assert_eq!(
            parse_grep_line("b.rs\u{0}4:text"),
            Some(("b.rs".into(), 4, "text".into()))
        );
        assert_eq!(parse_grep_line("no line here"), None);
    }

    #[test]
    fn json_paths_flatten_and_fields_map() {
        let doc = json!({"definitions": [
            {"callees": [{"name": "run", "kind": "function", "filePath": "/code/src/a.rs", "startLine": 11, "loc": {"end": 20}}]},
            {"callees": [{"name": "exit", "kind": "function", "filePath": "src/b.rs", "startLine": "4"}]}
        ]});
        let mut t = tool(&[]);
        t.output = Some(OutputFormat::Json);
        t.fields.insert(ItemField::Path, "filePath".into());
        t.fields.insert(ItemField::Line, "startLine".into());
        t.fields.insert(ItemField::EndLine, "/loc/end".into());
        t.fields.insert(ItemField::Detail, "kind".into());
        let items: Vec<Item> = select(&doc, Some("/definitions/*/callees"))
            .unwrap()
            .into_iter()
            .filter_map(|v| map_item(v, &t, "callee", &roots()))
            .collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].path.as_deref(), Some("src/a.rs"));
        assert_eq!((items[0].line, items[0].end_line), (Some(11), Some(20)));
        assert_eq!(items[1].line, Some(4));
        assert_eq!(items[1].kind, "function", "an element's own kind wins");
        let bare = map_item(&json!({"name": "x"}), &t, "callee", &roots()).unwrap();
        assert_eq!(
            bare.kind, "callee",
            "the configured kind applies when the output names none"
        );
        t.kind = Some("callee".into());
        let fixed = map_item(
            &json!({"name": "x", "kind": "function"}),
            &t,
            "callee",
            &roots(),
        )
        .unwrap();
        assert_eq!(
            fixed.kind, "callee",
            "an explicit kind wins over the element's"
        );

        let tests = json!({"affectedTests": ["t/a.rs", "t/b.rs"]});
        t.strings = Some(ItemField::Path);
        let paths: Vec<_> = select(&tests, Some("/affectedTests"))
            .unwrap()
            .into_iter()
            .filter_map(|v| map_item(v, &t, "test", &roots()))
            .map(|i| i.path.unwrap())
            .collect();
        assert_eq!(paths, ["t/a.rs", "t/b.rs"]);

        let native =
            json!({"items": [{"kind": "rule", "name": "R1", "detail": "x"}], "found": true});
        assert_eq!(select(&native, None).unwrap().len(), 1);
        assert!(select(&json!({"x": 1}), None).is_err());
        assert_eq!(select(&json!([1, 2]), None).unwrap().len(), 2);
    }
}
