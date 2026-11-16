//! Tool registry (`<profile>/tools.toml`): the tools kb connects and how requests are
//! routed to them. Pure model and validation; adapters live in `crate::tools`.
//!
//! The registry is trusted configuration of the KB checkout that also supplies the engine:
//! kb runs exactly the argv written here (no shell). Knowledge records never supply
//! commands (ADR 0015, ADR 0016).
//!
//! Any command-line tool can be connected: the built-in adapters cover CodeGraph, `kb.code.v1`
//! providers, ripgrep, `git grep` and check commands; the generic `exec` adapter connects
//! any other program through an argv template and an output mapping, either to a built-in
//! request (`sym`, `callers`, `impact`, `find`) or to a capability the project defines and
//! agents call with `kb run <capability>`.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Current `tools.toml` schema.
pub const TOOLS_SCHEMA: u32 = 1;
/// File name inside the profile directory.
pub const TOOLS_FILE: &str = "tools.toml";
pub const MAX_TOOLS: usize = 64;
pub const MAX_TIMEOUT_SECONDS: u64 = 3600;
/// Placeholders an `exec` argument may contain. `{args}` and `{globs}` must be whole
/// arguments (they expand to several); the others are substituted inside an argument, and
/// an argument whose optional value (`{file}`, `{kind}`) is absent is dropped.
pub const EXEC_PLACEHOLDERS: [&str; 9] = [
    "subject", "args", "file", "kind", "depth", "limit", "globs", "root", "kb",
];
/// Names a project capability cannot take: the knowledge and check commands.
pub const RESERVED_CAPABILITIES: [&str; 4] = ["check", "doc", "context", "run"];

/// A built-in request kb answers through the registry.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Locate a symbol's definition.
    Sym,
    /// Find the code that calls or references a symbol.
    Callers,
    /// Determine the code affected by changing a symbol.
    Impact,
    /// Search the repository text.
    Find,
    /// Run the checks for changed files.
    Check,
}

impl Capability {
    /// Capabilities resolved through `routes` (ordered fallback); `check` runs every
    /// matching command tool instead.
    pub const ROUTED: [Capability; 4] = [
        Capability::Sym,
        Capability::Callers,
        Capability::Impact,
        Capability::Find,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Sym => "sym",
            Capability::Callers => "callers",
            Capability::Impact => "impact",
            Capability::Find => "find",
            Capability::Check => "check",
        }
    }

    pub fn routed(name: &str) -> Option<Capability> {
        Capability::ROUTED.into_iter().find(|c| c.as_str() == name)
    }
}

/// Engine adapter that speaks to a tool. A new tool of a supported kind is a registry entry;
/// any other command-line tool is an `exec` entry.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterKind {
    /// CodeGraph CLI over the code root's live index (`query`, `callers`, `impact`).
    Codegraph,
    /// Any `kb.code.v1` provider (ADR 0011), e.g. `kb-code-provider --backend ast-index`.
    /// Commit-bound: answers describe the code root's `HEAD` commit.
    CodeProvider,
    /// Any command-line tool: argv template (`args`), output mapping (`output`, `items`,
    /// `fields`), declared `capabilities`.
    #[default]
    Exec,
    /// ripgrep text search.
    Ripgrep,
    /// Built-in `git grep` over tracked and untracked files; lexical symbol fallback.
    GitGrep,
    /// A configured check command (argv, no shell).
    Command,
}

impl AdapterKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AdapterKind::Codegraph => "codegraph",
            AdapterKind::CodeProvider => "code-provider",
            AdapterKind::Exec => "exec",
            AdapterKind::Ripgrep => "ripgrep",
            AdapterKind::GitGrep => "git-grep",
            AdapterKind::Command => "command",
        }
    }

    /// Fixed capabilities of the built-in adapters (`exec` declares its own).
    pub fn capabilities(self) -> &'static [Capability] {
        match self {
            AdapterKind::Codegraph | AdapterKind::CodeProvider => {
                &[Capability::Sym, Capability::Callers, Capability::Impact]
            }
            AdapterKind::Ripgrep => &[Capability::Find],
            AdapterKind::GitGrep => &[
                Capability::Sym,
                Capability::Callers,
                Capability::Impact,
                Capability::Find,
            ],
            AdapterKind::Command => &[Capability::Check],
            AdapterKind::Exec => &[],
        }
    }

    /// Adapters that run an external program (all but the built-in `git grep`, which uses
    /// the Git that kb itself requires).
    pub fn needs_program(self) -> bool {
        !matches!(self, AdapterKind::GitGrep)
    }
}

/// When the codegraph adapter brings the live index up to date.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SyncMode {
    /// Run the tool's incremental sync before every query.
    #[default]
    Auto,
    /// Query the index as it is; the answer reports pending changes.
    Never,
}

/// How an `exec` tool's stdout becomes answer items.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// Every non-empty line is one item (its `detail`).
    #[default]
    Lines,
    /// `path:line:text` or `path:line:column:text` lines (grep, ripgrep, compilers).
    Grep,
    /// One JSON document; `items` selects the array, `fields` maps item fields.
    Json,
}

/// A field of an answer item, as `fields` keys and `strings` name them.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ItemField {
    Kind,
    Name,
    Path,
    Line,
    EndLine,
    Detail,
    Confidence,
}

impl ItemField {
    pub const ALL: [ItemField; 7] = [
        ItemField::Kind,
        ItemField::Name,
        ItemField::Path,
        ItemField::Line,
        ItemField::EndLine,
        ItemField::Detail,
        ItemField::Confidence,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ItemField::Kind => "kind",
            ItemField::Name => "name",
            ItemField::Path => "path",
            ItemField::Line => "line",
            ItemField::EndLine => "end_line",
            ItemField::Detail => "detail",
            ItemField::Confidence => "confidence",
        }
    }
}

/// One connected tool.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    /// Registry id (lowercase kebab-case), referenced by `routes`.
    #[schemars(regex(pattern = r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$"), length(max = 64))]
    pub id: String,
    pub adapter: AdapterKind,
    /// Program: a bare name is looked up on PATH; `{kb}/...` is relative to the KB root,
    /// `{host}/...` and other relative paths to the code root. Not used by `git-grep`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// `code-provider`: arguments before the provider protocol. `exec`: the argv template
    /// after `program` (placeholders: [`EXEC_PLACEHOLDERS`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// `exec` only: requests the tool answers: `sym`, `callers`, `impact`, `find`, or a
    /// project capability (kebab-case) called with `kb run <capability>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// `exec` only: `lines` (default), `grep` or `json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<OutputFormat>,
    /// `exec` with `output = "json"`: path of the item array, JSON pointer segments where
    /// `*` flattens every element of an array (`/definitions/*/callers`). Default: the
    /// document itself when it is an array, else `/items`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<String>,
    /// `exec` with `output = "json"`: item field ← key of an element object, or a JSON
    /// pointer into it (`/location/line`). Unmapped fields use their own names.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<ItemField, String>,
    /// `exec` with `output = "json"`: the field a string element fills (default `detail`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strings: Option<ItemField>,
    /// `exec` only: item kind; it wins over an element's own `kind` key unless `fields.kind`
    /// maps one (default per request: `definition`, `caller`, `affected`, `match`, else
    /// `item`, or the element's `kind`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `exec` only: confidence label of every item without one (`resolved`, `possible`,
    /// `lexical`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    /// `exec` only: exit codes that mean success (default `[0]`; grep-like tools use
    /// `[0, 1]`, where 1 is "nothing found").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ok_exit: Vec<i32>,
    /// `exec` only: arguments of a step run before every request with the same program
    /// and placeholders, e.g. `["sync", "{root}"]` to bring an index up to date.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prepare: Vec<String>,
    /// `exec` only: arguments that print the tool's version (default `["--version"]`; `[]`
    /// checks only that the program exists).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_args: Option<Vec<String>>,
    /// Environment variables set for the tool (not `git-grep`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// `codegraph` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncMode>,
    /// `command` only: argv after `program`; the whole argument `{files}` expands to the
    /// matched files, `{kb}`/`{host}` prefixes expand to the KB and code roots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub run: Vec<String>,
    /// `command` only: globs (optionally `repo:`-qualified) of files that trigger the check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// `command` and `exec`: one-line purpose shown by `kb tools`, `kb check` and `kb run`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Deadline of one invocation, 1..=3600 seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

impl ToolSpec {
    pub fn timeout(&self) -> u64 {
        self.timeout_seconds.unwrap_or(match self.adapter {
            AdapterKind::Command => 900,
            AdapterKind::CodeProvider => 300,
            _ => 60,
        })
    }

    /// Does the tool answer the built-in `capability`?
    pub fn supports(&self, capability: Capability) -> bool {
        match self.adapter {
            AdapterKind::Exec => self.capabilities.iter().any(|c| c == capability.as_str()),
            a => a.capabilities().contains(&capability),
        }
    }

    /// Does the tool answer the request named `name` (built-in or project capability)?
    pub fn answers(&self, name: &str) -> bool {
        match Capability::routed(name) {
            Some(c) => self.supports(c),
            None => {
                self.adapter == AdapterKind::Exec && self.capabilities.iter().any(|c| c == name)
            }
        }
    }

    pub fn ok_exit(&self) -> &[i32] {
        if self.ok_exit.is_empty() {
            &[0]
        } else {
            &self.ok_exit
        }
    }
}

/// `tools.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolsConfig {
    #[schemars(extend("const" = 1))]
    pub schema: u32,
    /// Connected tools, in declaration order (the order `kb check` runs commands in).
    #[serde(default, rename = "tool")]
    #[schemars(length(max = MAX_TOOLS))]
    pub tools: Vec<ToolSpec>,
    /// Ordered fallback per request (`sym`, `callers`, `impact`, `find` or a project
    /// capability); an omitted request uses the default order of the tools answering it.
    #[serde(default)]
    pub routes: BTreeMap<String, Vec<String>>,
}

impl ToolsConfig {
    /// The registry used when the profile has no `tools.toml`.
    pub fn defaults() -> ToolsConfig {
        let tool = |id: &str, adapter, program: Option<&str>| ToolSpec {
            id: id.into(),
            adapter,
            program: program.map(str::to_string),
            ..ToolSpec::default()
        };
        ToolsConfig {
            schema: TOOLS_SCHEMA,
            tools: vec![
                tool("codegraph", AdapterKind::Codegraph, Some("codegraph")),
                tool("rg", AdapterKind::Ripgrep, Some("rg")),
                tool("git-grep", AdapterKind::GitGrep, None),
            ],
            routes: BTreeMap::new(),
        }
    }

    pub fn tool(&self, id: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|t| t.id == id)
    }

    /// The configured order for the built-in `capability`.
    pub fn route(&self, capability: Capability) -> Vec<&ToolSpec> {
        self.route_named(capability.as_str())
    }

    /// The configured order for a request: the explicit route, else every declared tool
    /// answering it, structural adapters first, then in declaration order.
    pub fn route_named(&self, name: &str) -> Vec<&ToolSpec> {
        if let Some(ids) = self.routes.get(name) {
            return ids.iter().filter_map(|id| self.tool(id)).collect();
        }
        let mut tools: Vec<&ToolSpec> = self.tools.iter().filter(|t| t.answers(name)).collect();
        tools.sort_by_key(|t| default_rank(t.adapter));
        tools
    }

    /// Project capabilities: names `exec` tools declare beyond the built-in requests.
    pub fn custom_capabilities(&self) -> BTreeSet<&str> {
        self.tools
            .iter()
            .filter(|t| t.adapter == AdapterKind::Exec)
            .flat_map(|t| t.capabilities.iter().map(String::as_str))
            .filter(|c| Capability::routed(c).is_none())
            .collect()
    }

    /// Command tools whose `paths` match one of `files` (repo-qualified globs match only
    /// files of `repo`), with the matched files, in declaration order.
    pub fn checks_for<'a>(
        &'a self,
        repo: Option<&str>,
        files: &[String],
    ) -> Vec<(&'a ToolSpec, Vec<String>)> {
        let mut out = Vec::new();
        for tool in self
            .tools
            .iter()
            .filter(|t| t.adapter == AdapterKind::Command)
        {
            let globs: Vec<_> = tool
                .paths
                .iter()
                .filter_map(|p| crate::glob::RepoGlob::parse(p).ok())
                .collect();
            let matched: Vec<String> = files
                .iter()
                .filter(|f| globs.iter().any(|g| g.matches(repo, f)))
                .cloned()
                .collect();
            if !matched.is_empty() {
                out.push((tool, matched));
            }
        }
        out
    }
}

fn default_rank(adapter: AdapterKind) -> u8 {
    match adapter {
        AdapterKind::Codegraph => 0,
        AdapterKind::CodeProvider => 1,
        AdapterKind::Exec => 2,
        AdapterKind::Ripgrep => 3,
        AdapterKind::GitGrep => 4,
        AdapterKind::Command => 5,
    }
}

fn valid_id(id: &str) -> bool {
    id.len() <= 64
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

fn single_line(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control)
}

/// The `{name}` placeholders in `arg`, or the text of an unterminated one.
pub fn placeholders(arg: &str) -> Result<Vec<&str>, String> {
    let mut out = Vec::new();
    let mut rest = arg;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("unterminated placeholder in `{arg}`"))?;
        out.push(&after[..end]);
        rest = &after[end + 1..];
    }
    Ok(out)
}

/// Parse and validate `tools.toml`. Problems are returned together, each prefixed with the
/// offending tool or route.
pub fn parse_tools(text: &str) -> Result<ToolsConfig, Vec<String>> {
    let config: ToolsConfig = toml::from_str(text).map_err(|e| vec![e.to_string()])?;
    let problems = validate_tools(&config);
    if problems.is_empty() {
        Ok(config)
    } else {
        Err(problems)
    }
}

pub fn validate_tools(config: &ToolsConfig) -> Vec<String> {
    let mut problems = Vec::new();
    if config.schema != TOOLS_SCHEMA {
        problems.push(format!(
            "schema {} is not supported (this engine reads tools schema {TOOLS_SCHEMA})",
            config.schema
        ));
    }
    if config.tools.len() > MAX_TOOLS {
        problems.push(format!("at most {MAX_TOOLS} tools may be declared"));
    }
    let mut ids = BTreeSet::new();
    for t in &config.tools {
        let at = format!("tool `{}`", t.id);
        let mut only = |ok: bool, field: &str, adapters: &str| {
            if !ok {
                problems.push(format!("{at}: `{field}` applies to {adapters} only"));
            }
        };
        let exec = t.adapter == AdapterKind::Exec;
        let command = t.adapter == AdapterKind::Command;
        only(
            t.sync.is_none() || t.adapter == AdapterKind::Codegraph,
            "sync",
            "the codegraph adapter",
        );
        only(
            t.args.is_empty() || exec || t.adapter == AdapterKind::CodeProvider,
            "args",
            "the exec and code-provider adapters (commands use `run`)",
        );
        only(t.run.is_empty() || command, "run", "the command adapter");
        only(
            t.paths.is_empty() || command,
            "paths",
            "the command adapter",
        );
        only(
            t.description.is_none() || exec || command,
            "description",
            "the exec and command adapters",
        );
        only(
            t.env.is_empty() || t.adapter.needs_program(),
            "env",
            "adapters that start a program",
        );
        for (field, set) in [
            ("capabilities", !t.capabilities.is_empty()),
            ("output", t.output.is_some()),
            ("items", t.items.is_some()),
            ("fields", !t.fields.is_empty()),
            ("strings", t.strings.is_some()),
            ("kind", t.kind.is_some()),
            ("confidence", t.confidence.is_some()),
            ("ok_exit", !t.ok_exit.is_empty()),
            ("prepare", !t.prepare.is_empty()),
            ("version_args", t.version_args.is_some()),
        ] {
            only(!set || exec, field, "the exec adapter");
        }
        if !valid_id(&t.id) {
            problems.push(format!("{at}: id must be lowercase kebab-case (≤64 bytes)"));
        }
        if !ids.insert(t.id.as_str()) {
            problems.push(format!("{at}: duplicate id"));
        }
        match (&t.program, t.adapter.needs_program()) {
            (None, true) => problems.push(format!(
                "{at}: adapter `{}` needs `program`",
                t.adapter.as_str()
            )),
            (Some(_), false) => problems.push(format!(
                "{at}: adapter `{}` is built in and takes no `program`",
                t.adapter.as_str()
            )),
            (Some(p), true) => {
                if let Err(e) = check_program(p) {
                    problems.push(format!("{at}: program: {e}"));
                }
            }
            (None, false) => {}
        }
        if command {
            if t.paths.is_empty() {
                problems.push(format!(
                    "{at}: a command tool needs `paths` that trigger it"
                ));
            }
            for p in &t.paths {
                if let Err(e) = crate::glob::RepoGlob::parse(p) {
                    problems.push(format!("{at}: {e}"));
                }
            }
        }
        if exec {
            validate_exec(t, &at, &mut problems);
        }
        let argv = t
            .run
            .iter()
            .chain(&t.args)
            .chain(t.version_args.iter().flatten());
        if argv.clone().any(|a| a.contains('\0') || a.len() > 4096) {
            problems.push(format!("{at}: arguments must be ≤4096 bytes without NUL"));
        }
        for (k, v) in &t.env {
            let name_ok = k.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !name_ok || v.contains('\0') || v.len() > 4096 {
                problems.push(format!(
                    "{at}: env `{k}` needs a NAME of [A-Za-z0-9_] and a value without NUL"
                ));
            }
        }
        if t.description.as_deref().is_some_and(|d| !single_line(d)) {
            problems.push(format!("{at}: description must be one non-empty line"));
        }
        if t.timeout_seconds
            .is_some_and(|s| !(1..=MAX_TIMEOUT_SECONDS).contains(&s))
        {
            problems.push(format!(
                "{at}: timeout_seconds must be 1..={MAX_TIMEOUT_SECONDS}"
            ));
        }
        if t.adapter == AdapterKind::CodeProvider && t.timeout_seconds.is_some_and(|s| s > 600) {
            problems.push(format!(
                "{at}: a code-provider invocation is limited to 600 seconds (kb.code.v1)"
            ));
        }
    }
    let by_id: BTreeMap<&str, &ToolSpec> =
        config.tools.iter().map(|t| (t.id.as_str(), t)).collect();
    let custom = config.custom_capabilities();
    for (name, route) in &config.routes {
        let at = format!("routes.{name}");
        if Capability::routed(name).is_none() && !custom.contains(name.as_str()) {
            problems.push(format!(
                "{at}: `{name}` is neither a built-in request (sym, callers, impact, find) nor a capability an exec tool declares"
            ));
            continue;
        }
        let mut seen = BTreeSet::new();
        for id in route {
            match by_id.get(id.as_str()) {
                None => problems.push(format!("{at}: unknown tool `{id}`")),
                Some(t) if !t.answers(name) => problems.push(format!(
                    "{at}: tool `{id}` (adapter `{}`) cannot answer `{name}`",
                    t.adapter.as_str()
                )),
                Some(_) => {}
            }
            if !seen.insert(id.as_str()) {
                problems.push(format!("{at}: tool `{id}` is listed twice"));
            }
        }
    }
    problems
}

fn validate_exec(t: &ToolSpec, at: &str, problems: &mut Vec<String>) {
    if t.capabilities.is_empty() {
        problems.push(format!("{at}: an exec tool needs `capabilities`"));
    }
    let mut seen = BTreeSet::new();
    for c in &t.capabilities {
        if !valid_id(c) {
            problems.push(format!(
                "{at}: capability `{c}` must be lowercase kebab-case"
            ));
        } else if RESERVED_CAPABILITIES.contains(&c.as_str()) {
            problems.push(format!(
                "{at}: `{c}` is a kb command, not a capability (checks use the command adapter)"
            ));
        }
        if !seen.insert(c) {
            problems.push(format!("{at}: capability `{c}` is listed twice"));
        }
    }
    for a in t.args.iter().chain(&t.prepare) {
        match placeholders(a) {
            Err(e) => problems.push(format!("{at}: {e}")),
            Ok(names) => {
                for n in &names {
                    if !EXEC_PLACEHOLDERS.contains(n) {
                        problems.push(format!(
                            "{at}: unknown placeholder `{{{n}}}` (known: {})",
                            EXEC_PLACEHOLDERS.join(", ")
                        ));
                    }
                    if matches!(*n, "args" | "globs") && a != &format!("{{{n}}}") {
                        problems.push(format!("{at}: `{{{n}}}` must be a whole argument"));
                    }
                }
            }
        }
    }
    let json = t.output == Some(OutputFormat::Json);
    if !json && (t.items.is_some() || !t.fields.is_empty() || t.strings.is_some()) {
        problems.push(format!(
            "{at}: `items`, `fields` and `strings` need output = \"json\""
        ));
    }
    if let Some(path) = &t.items
        && !(path.is_empty() || path.starts_with('/'))
    {
        problems.push(format!(
            "{at}: `items` is a JSON pointer path starting with `/`"
        ));
    }
    if t.fields
        .values()
        .any(|v| v.trim().is_empty() || v.contains('*'))
    {
        problems.push(format!(
            "{at}: `fields` values are keys or JSON pointers without `*`"
        ));
    }
    if t.kind.as_deref().is_some_and(|k| !valid_id(k)) {
        problems.push(format!("{at}: `kind` must be lowercase kebab-case"));
    }
    if t.confidence
        .as_deref()
        .is_some_and(|c| !matches!(c, "resolved" | "possible" | "lexical"))
    {
        problems.push(format!(
            "{at}: `confidence` is resolved, possible or lexical"
        ));
    }
    if t.ok_exit.iter().any(|c| !(0..=255).contains(c)) {
        problems.push(format!("{at}: `ok_exit` codes are 0..=255"));
    }
}

/// A program is a bare name, a relative path (optionally starting with `{kb}/` or
/// `{host}/`) or an absolute path; never empty, never with NUL or `..` segments.
pub fn check_program(p: &str) -> Result<(), String> {
    if p.trim().is_empty() || p.contains('\0') || p.len() > 1024 {
        return Err("must be a non-empty path of at most 1024 bytes".into());
    }
    let rest = p
        .strip_prefix("{kb}/")
        .or_else(|| p.strip_prefix("{host}/"))
        .unwrap_or(p);
    if rest.contains('{') || rest.contains('}') {
        return Err("only a leading `{kb}/` or `{host}/` placeholder is supported".into());
    }
    if rest.split('/').any(|s| s == "..") {
        return Err("`..` segments are not allowed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_route_structural_before_lexical() {
        let c = ToolsConfig::defaults();
        assert!(validate_tools(&c).is_empty());
        let ids = |cap| {
            c.route(cap)
                .iter()
                .map(|t| t.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(Capability::Sym), ["codegraph", "git-grep"]);
        assert_eq!(ids(Capability::Find), ["rg", "git-grep"]);
        assert!(c.route(Capability::Check).is_empty());
        assert!(c.custom_capabilities().is_empty());
    }

    #[test]
    fn explicit_routes_win_and_are_checked() {
        let text = r#"
schema = 1
[[tool]]
id = "grep"
adapter = "git-grep"
[[tool]]
id = "tests"
adapter = "command"
program = "cargo"
run = ["test", "{files}"]
paths = ["src/**/*.rs", "app:lib/**"]
[routes]
sym = ["grep"]
find = ["tests"]
"#;
        let problems = parse_tools(text).unwrap_err();
        assert_eq!(
            problems,
            ["routes.find: tool `tests` (adapter `command`) cannot answer `find`"]
        );
        let ok = parse_tools(&text.replace("find = [\"tests\"]\n", "")).unwrap();
        assert_eq!(ok.route(Capability::Sym)[0].id, "grep");
        let files = vec![
            "src/a.rs".to_string(),
            "lib/b.rs".into(),
            "README.md".into(),
        ];
        let checks = ok.checks_for(None, &files);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].1, ["src/a.rs"]);
        let checks = ok.checks_for(Some("app"), &files);
        assert_eq!(checks[0].1, ["src/a.rs", "lib/b.rs"]);
    }

    #[test]
    fn exec_tools_join_built_in_routes_and_declare_project_capabilities() {
        let text = r#"
schema = 1
[[tool]]
id = "callees"
adapter = "exec"
program = "codegraph"
args = ["callees", "{subject}", "--path", "{root}", "--json"]
capabilities = ["callees"]
output = "json"
items = "/definitions/*/callees"
fields = { path = "filePath", line = "startLine", detail = "kind" }
[[tool]]
id = "symbols"
adapter = "exec"
program = "ast-index"
args = ["symbol", "{subject}", "--in-file={file}", "--format", "json"]
capabilities = ["sym"]
output = "json"
version_args = ["version"]
[[tool]]
id = "grep"
adapter = "git-grep"
[routes]
sym = ["symbols", "grep"]
callees = ["callees"]
"#;
        let c = parse_tools(text).unwrap();
        assert_eq!(c.route(Capability::Sym)[0].id, "symbols");
        assert_eq!(c.route_named("callees")[0].id, "callees");
        assert_eq!(
            c.custom_capabilities().into_iter().collect::<Vec<_>>(),
            ["callees"]
        );
        assert!(c.tool("symbols").unwrap().answers("sym"));
        assert!(!c.tool("symbols").unwrap().answers("callers"));

        let bad = parse_tools(
            r#"
schema = 1
[[tool]]
id = "x"
adapter = "exec"
program = "x"
args = ["{nope}", "--a={args}", "{subject"]
capabilities = ["check", "Bad"]
items = "/a"
confidence = "sure"
ok_exit = [300]
[routes]
undeclared = ["x"]
"#,
        )
        .unwrap_err();
        for needle in [
            "unknown placeholder `{nope}`",
            "`{args}` must be a whole argument",
            "unterminated placeholder",
            "`check` is a kb command",
            "capability `Bad`",
            "need output = \"json\"",
            "`confidence` is",
            "`ok_exit` codes",
            "routes.undeclared",
        ] {
            assert!(bad.iter().any(|p| p.contains(needle)), "{needle}: {bad:#?}");
        }
        let missing =
            parse_tools("schema = 1\n[[tool]]\nid = \"x\"\nadapter = \"exec\"\nprogram = \"x\"\n")
                .unwrap_err();
        assert!(missing[0].contains("needs `capabilities`"), "{missing:?}");
    }

    #[test]
    fn adapter_fields_are_strict() {
        let problems = parse_tools(
            r#"
schema = 1
[[tool]]
id = "Bad"
adapter = "ripgrep"
sync = "never"
run = ["x"]
output = "json"
[[tool]]
id = "git"
adapter = "git-grep"
program = "git"
env = { A = "1" }
"#,
        )
        .unwrap_err();
        assert!(problems.iter().any(|p| p.contains("lowercase kebab-case")));
        assert!(problems.iter().any(|p| p.contains("needs `program`")));
        assert!(problems.iter().any(|p| p.contains("`sync` applies")));
        assert!(problems.iter().any(|p| p.contains("`run` applies")));
        assert!(
            problems
                .iter()
                .any(|p| p.contains("`output` applies to the exec adapter"))
        );
        assert!(problems.iter().any(|p| p.contains("`env` applies")));
        assert!(problems.iter().any(|p| p.contains("takes no `program`")));
        let slow = parse_tools(
            "schema = 1\n[[tool]]\nid = \"p\"\nadapter = \"code-provider\"\nprogram = \"p\"\ntimeout_seconds = 900\n",
        )
        .unwrap_err();
        assert!(slow[0].contains("limited to 600 seconds"), "{slow:?}");
        assert!(parse_tools("schema = 1\nunknown = 1\n").is_err());
        assert!(check_program("{kb}/../x").is_err());
        assert!(check_program("{kb}/target/release/kb-code-provider").is_ok());
        assert!(check_program("{home}/x").is_err());
        assert_eq!(placeholders("--a={file}x{kind}").unwrap(), ["file", "kind"]);
    }
}
