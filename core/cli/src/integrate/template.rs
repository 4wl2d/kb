//! Strict `{{key}}` templates for `core/templates` (used by `kb init`) and `core/skills`
//! (used by `kb integrate --generate`).
//!
//! Rules:
//! * Only files ending in `.tmpl` are rendered (the suffix is dropped); other files are
//!   copied byte for byte.
//! * A placeholder is `{{name}}` with `name` matching `[a-z][a-z0-9_]*`. Every `{{` in a
//!   template must start a well-formed placeholder whose name is defined; anything else is
//!   an error, so no placeholder can survive rendering.
//! * Substitution is a single pass: a substituted value is never re-scanned.
//! * [`Vars::text`] values are escaped for the output format (TOML basic-string escaping for
//!   `.toml` outputs, verbatim otherwise). [`Vars::fragment`] values are code-generated
//!   fragments (for example a TOML array built from typed values) and are inserted verbatim.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{ErrorCode, KbError, Result};
use crate::source::{SourceTree, WorkingTreeSource};

/// Suffix of files that are rendered rather than copied.
pub const TEMPLATE_SUFFIX: &str = ".tmpl";

/// Escaping applied to [`Vars::text`] values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escape {
    /// Contents of a TOML basic string (`"..."`).
    Toml,
    /// Inserted as is (Markdown, YAML comments, plain text).
    Verbatim,
}

impl Escape {
    /// Escaping for an output path: TOML for `*.toml`, verbatim otherwise.
    pub fn for_output(path: &str) -> Escape {
        if path.ends_with(".toml") {
            Escape::Toml
        } else {
            Escape::Verbatim
        }
    }
}

#[derive(Debug, Clone)]
enum Value {
    Text(String),
    Fragment(String),
}

/// Template variables.
#[derive(Debug, Clone, Default)]
pub struct Vars(BTreeMap<String, Value>);

impl Vars {
    pub fn new() -> Vars {
        Vars::default()
    }

    /// A user-facing value, escaped for the output format.
    pub fn text(&mut self, key: &str, value: impl Into<String>) -> &mut Vars {
        self.0.insert(key.to_string(), Value::Text(value.into()));
        self
    }

    /// A code-generated fragment inserted verbatim (callers escape its parts themselves).
    pub fn fragment(&mut self, key: &str, value: impl Into<String>) -> &mut Vars {
        self.0
            .insert(key.to_string(), Value::Fragment(value.into()));
        self
    }

    /// Defined keys, sorted.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

fn valid_key(key: &str) -> bool {
    key.starts_with(|c: char| c.is_ascii_lowercase())
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn template_error(name: &str, line: usize, msg: String) -> KbError {
    KbError::new(
        ErrorCode::InvalidInput,
        format!("template `{name}` line {line}: {msg}"),
    )
    .with_hint(
        "templates are engine files (core/templates, core/skills); restore them from upstream",
    )
}

/// Render `template` (named `name` in errors) with `vars`.
pub fn render(name: &str, template: &str, vars: &Vars, escape: Escape) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    let mut consumed = 0usize;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let line = template[..consumed + i].matches('\n').count() + 1;
        let after = &rest[i + 2..];
        let end = after
            .find("}}")
            .filter(|e| !after[..*e].contains('\n'))
            .ok_or_else(|| template_error(name, line, "unterminated `{{`".into()))?;
        let key = &after[..end];
        if !valid_key(key) {
            let shown: String = key.chars().take(40).collect();
            return Err(template_error(
                name,
                line,
                format!("malformed placeholder `{{{{{shown}}}}}`"),
            ));
        }
        match vars.0.get(key) {
            Some(Value::Text(v)) => match escape {
                Escape::Toml => out.push_str(&toml_escape(v)),
                Escape::Verbatim => out.push_str(v),
            },
            Some(Value::Fragment(v)) => out.push_str(v),
            None => {
                return Err(template_error(
                    name,
                    line,
                    format!(
                        "unknown placeholder `{{{{{key}}}}}` (defined: {})",
                        vars.keys().collect::<Vec<_>>().join(", ")
                    ),
                ));
            }
        }
        let skip = i + 2 + end + 2;
        consumed += skip;
        rest = &rest[skip..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Escape `s` as the contents of a TOML basic string.
pub fn toml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// A quoted TOML basic string.
pub fn toml_string(s: &str) -> String {
    format!("\"{}\"", toml_escape(s))
}

/// A TOML array of basic strings, e.g. `["a", "b"]`.
pub fn toml_string_array<S: AsRef<str>>(items: &[S]) -> String {
    let parts: Vec<String> = items.iter().map(|s| toml_string(s.as_ref())).collect();
    format!("[{}]", parts.join(", "))
}

/// One output file of a rendered template directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedFile {
    /// Output path relative to the rendered directory (`.tmpl` suffix removed).
    pub path: String,
    pub bytes: Vec<u8>,
}

/// Render every file under the KB-root-relative directory `dir`, sorted by output path.
/// Symlinks and unreadable entries are errors; a missing directory is `NOT_FOUND`.
pub fn render_dir(kb_root: &Path, dir: &str, vars: &Vars) -> Result<Vec<RenderedFile>> {
    let src = WorkingTreeSource::new(kb_root);
    let abs = crate::util::safe_join(kb_root, dir)?;
    if !abs.is_dir() {
        return Err(KbError::new(
            ErrorCode::NotFound,
            format!("template directory `{dir}` does not exist in the KB checkout"),
        )
        .with_hint("the engine checkout is incomplete; restore core/ from upstream"));
    }
    let (entries, issues) = src.list(&[dir.to_string()])?;
    if let Some(i) = issues.first() {
        return Err(KbError::unsafe_path(format!(
            "template file `{}`: {}",
            i.path, i.message
        )));
    }
    let contents = src.read(&entries)?;
    let prefix = format!("{}/", dir.trim_end_matches('/'));
    let mut out = Vec::with_capacity(entries.len());
    for (e, bytes) in entries.iter().zip(contents) {
        let rel = e.path.strip_prefix(&prefix).unwrap_or(&e.path);
        out.push(render_file(&e.path, rel, bytes, vars)?);
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    for w in out.windows(2) {
        if w[0].path == w[1].path {
            return Err(KbError::invalid_input(format!(
                "template directory `{dir}` has both `{0}` and `{0}{TEMPLATE_SUFFIX}`",
                w[0].path
            )));
        }
    }
    Ok(out)
}

/// Render (`.tmpl`) or copy one template file. `name` is used in errors, `rel` is the
/// output-relative source path.
pub fn render_file(name: &str, rel: &str, bytes: Vec<u8>, vars: &Vars) -> Result<RenderedFile> {
    match rel.strip_suffix(TEMPLATE_SUFFIX) {
        Some(out_path) => {
            let text = String::from_utf8(bytes)
                .map_err(|_| KbError::invalid_input(format!("template `{name}` is not UTF-8")))?;
            let rendered = render(name, &text, vars, Escape::for_output(out_path))?;
            Ok(RenderedFile {
                path: out_path.to_string(),
                bytes: rendered.into_bytes(),
            })
        }
        None => Ok(RenderedFile {
            path: rel.to_string(),
            bytes,
        }),
    }
}

/// Read one KB-root-relative template file and render it (its `.tmpl` suffix is dropped).
pub fn render_one(kb_root: &Path, path: &str, vars: &Vars) -> Result<RenderedFile> {
    let src = WorkingTreeSource::new(kb_root);
    let bytes = src.read_path(path)?.ok_or_else(|| {
        KbError::new(
            ErrorCode::NotFound,
            format!("template `{path}` does not exist in the KB checkout"),
        )
        .with_hint("the engine checkout is incomplete; restore core/ from upstream")
    })?;
    let rel = path.rsplit('/').next().unwrap_or(path);
    render_file(path, rel, bytes, vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        let mut v = Vars::new();
        v.text("name", "A \"quoted\" \\ name\n")
            .text("ns", "acme")
            .fragment("list", "[\"a\", \"b\"]");
        v
    }

    #[test]
    fn substitutes_and_escapes_per_format() {
        let v = vars();
        assert_eq!(
            render("t", "name = \"{{name}}\"\nl = {{list}}\n", &v, Escape::Toml).unwrap(),
            "name = \"A \\\"quoted\\\" \\\\ name\\n\"\nl = [\"a\", \"b\"]\n"
        );
        assert_eq!(
            render("t", "# {{ns}} {{name}}", &v, Escape::Verbatim).unwrap(),
            "# acme A \"quoted\" \\ name\n"
        );
        let rendered = render("t", "name = \"{{name}}\"", &v, Escape::Toml).unwrap();
        let t: toml::Table = toml::from_str(&rendered).unwrap();
        assert_eq!(t["name"].as_str().unwrap(), "A \"quoted\" \\ name\n");
    }

    #[test]
    fn rejects_unknown_malformed_and_unterminated_placeholders() {
        let v = vars();
        let e = render("t", "a\n{{nope}}", &v, Escape::Verbatim).unwrap_err();
        assert!(e.message.contains("line 2"), "{}", e.message);
        assert!(e.message.contains("unknown placeholder `{{nope}}`"));
        for bad in [
            "{{ ns }}",
            "{{NS}}",
            "{{}}",
            "${{ github.sha }}",
            "{{ns",
            "{{n\ns}}",
        ] {
            assert!(
                render("t", bad, &v, Escape::Verbatim).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn values_are_not_rescanned() {
        let mut v = Vars::new();
        v.text("a", "{{b}}");
        assert_eq!(
            render("t", "x{{a}}y", &v, Escape::Verbatim).unwrap(),
            "x{{b}}y"
        );
    }

    #[test]
    fn toml_escape_handles_control_characters() {
        let s = "a\u{1}b\u{7f}\t";
        let t: toml::Table = toml::from_str(&format!("v = {}", toml_string(s))).unwrap();
        assert_eq!(t["v"].as_str().unwrap(), s);
        assert_eq!(toml_string_array(&["x", "y\""]), "[\"x\", \"y\\\"\"]");
    }
}
