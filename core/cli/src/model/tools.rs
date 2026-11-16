//! Tool registry (`<profile>/tools.toml`): the code tools kb connects and how requests are
//! routed to them. Pure model and validation; adapters live in `crate::tools`.
//!
//! The registry is trusted configuration of the KB checkout that also supplies the engine:
//! a `command` tool runs exactly the argv written here. Knowledge records never supply
//! commands (ADR 0015, ADR 0016).

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Current `tools.toml` schema.
pub const TOOLS_SCHEMA: u32 = 1;
/// File name inside the profile directory.
pub const TOOLS_FILE: &str = "tools.toml";
pub const MAX_TOOLS: usize = 64;
pub const MAX_TIMEOUT_SECONDS: u64 = 3600;

/// A request kb answers through the registry.
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
}

/// Engine adapter that speaks to a tool. A new kind of tool needs an adapter; a new tool of
/// an existing kind needs only a registry entry.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterKind {
    /// CodeGraph CLI over the code root's live index (`query`, `callers`, `impact`).
    Codegraph,
    /// Any `kb.code.v1` provider (ADR 0011), e.g. `kb-code-provider --backend ast-index`.
    /// Commit-bound: answers describe the code root's `HEAD` commit.
    CodeProvider,
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
            AdapterKind::Ripgrep => "ripgrep",
            AdapterKind::GitGrep => "git-grep",
            AdapterKind::Command => "command",
        }
    }

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

/// One connected tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    /// Extra arguments placed before the adapter's own (`code-provider` only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
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
    /// `command` only: one-line purpose shown by `kb tools` and `kb check`.
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

    pub fn supports(&self, capability: Capability) -> bool {
        self.adapter.capabilities().contains(&capability)
    }
}

/// Ordered fallback per routed capability.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Routes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sym: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub find: Option<Vec<String>>,
}

impl Routes {
    pub fn get(&self, capability: Capability) -> Option<&Vec<String>> {
        match capability {
            Capability::Sym => self.sym.as_ref(),
            Capability::Callers => self.callers.as_ref(),
            Capability::Impact => self.impact.as_ref(),
            Capability::Find => self.find.as_ref(),
            Capability::Check => None,
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
    /// Ordered fallback per capability; an omitted capability uses the default order of the
    /// declared tools that support it.
    #[serde(default)]
    pub routes: Routes,
}

impl ToolsConfig {
    /// The registry used when the profile has no `tools.toml`.
    pub fn defaults() -> ToolsConfig {
        let tool = |id: &str, adapter, program: Option<&str>| ToolSpec {
            id: id.into(),
            adapter,
            program: program.map(str::to_string),
            args: Vec::new(),
            sync: None,
            run: Vec::new(),
            paths: Vec::new(),
            description: None,
            timeout_seconds: None,
        };
        ToolsConfig {
            schema: TOOLS_SCHEMA,
            tools: vec![
                tool("codegraph", AdapterKind::Codegraph, Some("codegraph")),
                tool("rg", AdapterKind::Ripgrep, Some("rg")),
                tool("git-grep", AdapterKind::GitGrep, None),
            ],
            routes: Routes::default(),
        }
    }

    pub fn tool(&self, id: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|t| t.id == id)
    }

    /// The configured order for `capability`: the explicit route, else every declared
    /// tool supporting it, structural adapters before lexical ones.
    pub fn route(&self, capability: Capability) -> Vec<&ToolSpec> {
        if let Some(ids) = self.routes.get(capability) {
            return ids.iter().filter_map(|id| self.tool(id)).collect();
        }
        let mut tools: Vec<&ToolSpec> = self
            .tools
            .iter()
            .filter(|t| t.supports(capability))
            .collect();
        tools.sort_by_key(|t| default_rank(t.adapter));
        tools
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
        AdapterKind::Ripgrep => 2,
        AdapterKind::GitGrep => 3,
        AdapterKind::Command => 4,
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
        if t.sync.is_some() && t.adapter != AdapterKind::Codegraph {
            problems.push(format!(
                "{at}: `sync` applies to the codegraph adapter only"
            ));
        }
        if !t.args.is_empty() && t.adapter != AdapterKind::CodeProvider {
            problems.push(format!(
                "{at}: `args` applies to the code-provider adapter only (commands use `run`)"
            ));
        }
        if t.adapter == AdapterKind::Command {
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
        } else {
            if !t.run.is_empty() || !t.paths.is_empty() {
                problems.push(format!(
                    "{at}: `run` and `paths` apply to the command adapter only"
                ));
            }
            if t.description.is_some() {
                problems.push(format!(
                    "{at}: `description` applies to the command adapter only"
                ));
            }
        }
        if t.run
            .iter()
            .chain(&t.args)
            .any(|a| a.contains('\0') || a.len() > 4096)
        {
            problems.push(format!("{at}: arguments must be ≤4096 bytes without NUL"));
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
    for cap in Capability::ROUTED {
        let Some(route) = config.routes.get(cap) else {
            continue;
        };
        let mut seen = BTreeSet::new();
        for id in route {
            let at = format!("routes.{}", cap.as_str());
            match by_id.get(id.as_str()) {
                None => problems.push(format!("{at}: unknown tool `{id}`")),
                Some(t) if !t.supports(cap) => problems.push(format!(
                    "{at}: tool `{id}` (adapter `{}`) cannot answer `{}`",
                    t.adapter.as_str(),
                    cap.as_str()
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
    fn adapter_fields_are_strict() {
        let problems = parse_tools(
            r#"
schema = 1
[[tool]]
id = "Bad"
adapter = "ripgrep"
sync = "never"
run = ["x"]
[[tool]]
id = "git"
adapter = "git-grep"
program = "git"
"#,
        )
        .unwrap_err();
        assert!(problems.iter().any(|p| p.contains("lowercase kebab-case")));
        assert!(problems.iter().any(|p| p.contains("needs `program`")));
        assert!(problems.iter().any(|p| p.contains("`sync` applies")));
        assert!(problems.iter().any(|p| p.contains("`run` and `paths`")));
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
    }
}
