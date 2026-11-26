//! Tool registry adapters: load `<profile>/tools.toml` (or the built-in defaults), probe the
//! connected tools and route code requests to the first tool that can answer.
//!
//! Routing is decided by code and configuration only: the route order from the registry,
//! filtered by availability on this machine, with a fallback to the next tool when a tool
//! fails or does not know the requested symbol. Every answer names the tool that produced
//! it; fallbacks are reported as notes, never silent.

pub mod codegraph;
pub mod command;
pub mod exec;
pub mod grep;
pub mod provider;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;
use serde_json::json;

use crate::answer::{Item, Source};
use crate::error::{ErrorCode, KbError, Result};
use crate::model::ProfileLocation;
use crate::model::tools::{
    AdapterKind, Capability, TOOLS_FILE, ToolSpec, ToolsConfig, parse_tools,
};
use crate::process::{Limits, capture};
use crate::source::{SourceTree, WorkingTreeSource};
use crate::util::PathExt;

/// Where the tools run and what they read.
#[derive(Debug, Clone)]
pub struct Roots {
    /// KB checkout (where the engine and `tools.toml` come from).
    pub kb: PathBuf,
    /// Root of the code the agent works on: the host repository, or the KB checkout itself
    /// when no host was detected.
    pub code: PathBuf,
    /// Registry repo id of the code root, when identified.
    pub repo: Option<String>,
    /// Host-relative path of the KB checkout when it is mounted inside the code root.
    pub kb_mount: Option<String>,
}

/// The loaded registry and where it came from.
#[derive(Debug, Clone)]
pub struct Registry {
    pub config: ToolsConfig,
    /// KB-root-relative `tools.toml`, or `None` for the built-in defaults.
    pub file: Option<String>,
}

impl Registry {
    pub fn origin(&self) -> String {
        self.file
            .clone()
            .unwrap_or_else(|| "built-in defaults (no tools.toml)".into())
    }
}

/// Load the tool registry of `loc` from the KB working tree. Commands that kb runs come
/// from the same checkout as the engine that runs them, never from a fetched snapshot.
pub fn load(kb_root: &Path, loc: &ProfileLocation) -> Result<Registry> {
    let path = format!("{}/{TOOLS_FILE}", loc.dir);
    let Some(bytes) = WorkingTreeSource::new(kb_root).read_path(&path)? else {
        return Ok(Registry {
            config: ToolsConfig::defaults(),
            file: None,
        });
    };
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| KbError::new(ErrorCode::ConfigInvalid, format!("`{path}` is not UTF-8")))?;
    let config = parse_tools(text).map_err(|problems| {
        KbError::new(
            ErrorCode::ConfigInvalid,
            format!("`{path}`: {}", problems.join("; ")),
        )
        .with_details(json!({ "file": path, "problems": problems }))
        .with_hint("see docs/agent-cli.md#tool-registry for the format")
    })?;
    Ok(Registry {
        config,
        file: Some(path),
    })
}

/// What `kb` found out about one tool on this machine.
#[derive(Debug, Clone, Serialize)]
pub struct Probe {
    pub id: String,
    pub adapter: AdapterKind,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// First line of the tool's `--version`, when asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Resolve a registry `program` to an executable file.
pub fn resolve_program(program: &str, roots: &Roots) -> Option<PathBuf> {
    let candidate = if let Some(rest) = program.strip_prefix("{kb}/") {
        roots.kb.join(rest)
    } else if let Some(rest) = program.strip_prefix("{host}/") {
        roots.code.join(rest)
    } else if Path::new(program).is_absolute() {
        PathBuf::from(program)
    } else if program.contains('/') {
        roots.code.join(program)
    } else {
        return search_path(program);
    };
    is_executable(&candidate).then_some(candidate)
}

/// The first executable `name` on `PATH`. On Windows a name without an extension is also
/// tried with each extension of `PATHEXT` (default `.COM;.EXE;.BAT;.CMD`).
pub fn search_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) && Path::new(name).extension().is_none() {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    std::env::split_paths(&path).find_map(|dir| {
        let plain = dir.join(name);
        if is_executable(&plain) {
            return Some(plain);
        }
        exts.iter()
            .map(|e| dir.join(format!("{name}{e}")))
            .find(|p| is_executable(p))
    })
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

/// Probe availability. `ask_version` runs `<program> --version` (bounded) for adapters
/// that report their tool; command tools are only resolved, never started.
pub fn probe(tool: &ToolSpec, roots: &Roots, ask_version: bool) -> Probe {
    let mut p = Probe {
        id: tool.id.clone(),
        adapter: tool.adapter,
        available: false,
        program: None,
        version: None,
        reason: None,
    };
    if tool.adapter == AdapterKind::GitGrep {
        p.available = true;
        if ask_version {
            p.version = version_of(
                Path::new("git"),
                &["--version".to_string()],
                &std::collections::BTreeMap::new(),
            )
            .ok();
        }
        return p;
    }
    let Some(program) = tool.program.as_deref() else {
        p.reason = Some("no program configured".into());
        return p;
    };
    let Some(path) = resolve_program(program, roots) else {
        p.reason = Some(format!(
            "`{program}` is not an executable file on this machine"
        ));
        return p;
    };
    p.program = Some(path.display().to_string());
    p.available = true;
    let version_args: Vec<String> = match tool.adapter {
        AdapterKind::Command => Vec::new(),
        AdapterKind::Exec => tool
            .version_args
            .clone()
            .unwrap_or_else(|| vec!["--version".into()]),
        AdapterKind::CodeProvider => tool
            .args
            .iter()
            .cloned()
            .chain(["--version".to_string()])
            .collect(),
        _ => vec!["--version".into()],
    };
    if ask_version && !version_args.is_empty() {
        match version_of(&path, &version_args, &tool.env) {
            Ok(v) => p.version = Some(v),
            Err(e) => {
                p.available = false;
                p.reason = Some(format!(
                    "`{program} {}` failed: {}",
                    version_args.join(" "),
                    e.message
                ));
            }
        }
    }
    p
}

fn version_of(
    program: &Path,
    args: &[String],
    env: &std::collections::BTreeMap<String, String>,
) -> Result<String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    tool_env(&mut cmd);
    cmd.envs(env);
    let out = capture(
        &mut cmd,
        &[],
        Limits {
            timeout: Duration::from_secs(10),
            stdout: 64 * 1024,
            stderr: 64 * 1024,
        },
    )?;
    if out.exit_code != 0 {
        return Err(KbError::invalid_input(format!("exit {}", out.exit_code)));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let name = program
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let line = line.trim();
    Ok(if line.starts_with(|c: char| c.is_ascii_digit()) {
        format!("{name} {line}")
    } else {
        line.to_string()
    })
}

/// Environment shared by every tool kb starts: no telemetry, update checks or daemons.
pub fn tool_env(cmd: &mut Command) {
    cmd.env("CODEGRAPH_NO_DAEMON", "1")
        .env("CODEGRAPH_TELEMETRY", "0")
        .env("CODEGRAPH_NO_UPDATE_CHECK", "1")
        .env("DO_NOT_TRACK", "1")
        .env("NO_COLOR", "1");
}

/// One code request routed through the registry.
#[derive(Debug, Clone)]
pub struct CodeQuery {
    pub capability: Capability,
    pub name: String,
    /// Narrow definitions to files whose path contains this text.
    pub file: Option<String>,
    /// Narrow definitions to a symbol kind (`function`, `class`, ...).
    pub kind: Option<String>,
    pub depth: u32,
    pub limit: usize,
}

/// What one adapter returned.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub source: Source,
    /// The adapter found the requested symbol itself (a definition or target).
    pub found: bool,
    pub items: Vec<Item>,
    pub notes: Vec<String>,
    /// First call edge with a working-tree source location, if supplied by the adapter.
    pub call_site: Option<(String, u32)>,
    /// Files of the affected code (`impact`), for relating them to knowledge.
    pub files: Vec<String>,
}

impl Outcome {
    pub fn new(source: Source) -> Outcome {
        Outcome {
            source,
            found: false,
            items: Vec::new(),
            notes: Vec::new(),
            call_site: None,
            files: Vec::new(),
        }
    }
}

/// Route `query` through the registry: forced tool, else the configured route in order.
/// The first outcome that found the symbol wins; otherwise the first non-empty outcome,
/// otherwise the first outcome. Failures of earlier tools become notes.
pub fn answer_code(
    registry: &Registry,
    roots: &Roots,
    query: &CodeQuery,
    forced: Option<&str>,
) -> Result<Outcome> {
    let name = query.capability.as_str();
    let route = route_for(registry, name, forced)?;
    let exec_request = exec::ExecRequest {
        capability: name.to_string(),
        subject: query.name.clone(),
        file: query.file.clone(),
        kind: query.kind.clone(),
        depth: query.depth,
        limit: query.limit,
        ..exec::ExecRequest::default()
    };
    run_route(route, roots, name, &query.name, |tool, probe| {
        match tool.adapter {
            AdapterKind::Codegraph => Some(codegraph::query(tool, probe, roots, query)),
            AdapterKind::CodeProvider => Some(provider::query(tool, probe, roots, query)),
            AdapterKind::GitGrep => Some(grep::lexical(tool, probe, roots, query)),
            AdapterKind::Exec => Some(exec::query(tool, probe, roots, &exec_request)),
            AdapterKind::Ripgrep | AdapterKind::Command => None,
        }
    })
}

/// Route a request to `exec` tools (project capabilities of `kb run`, or a built-in request
/// an exec tool answers), with the same fallback rules as [`answer_code`].
pub fn answer_exec(
    registry: &Registry,
    roots: &Roots,
    request: &exec::ExecRequest,
    forced: Option<&str>,
) -> Result<Outcome> {
    let route = route_for(registry, &request.capability, forced)?;
    run_route(
        route,
        roots,
        &request.capability,
        &request.subject,
        |tool, probe| {
            (tool.adapter == AdapterKind::Exec).then(|| exec::query(tool, probe, roots, request))
        },
    )
}

/// Try `route` in order: skip unavailable tools, return the first outcome that found what
/// was asked; otherwise the first non-empty outcome, otherwise the first outcome. Every
/// hand-over is a note; with no outcome at all the request is `TOOL_UNAVAILABLE`.
fn run_route(
    route: Vec<&ToolSpec>,
    roots: &Roots,
    request: &str,
    subject: &str,
    mut call: impl FnMut(&ToolSpec, &Probe) -> Option<Result<Outcome>>,
) -> Result<Outcome> {
    let mut notes = Vec::new();
    let mut outcomes: Vec<Outcome> = Vec::new();
    for tool in route {
        let probe = probe(tool, roots, true);
        if !probe.available {
            notes.push(format!(
                "{} skipped: {}",
                tool.id,
                probe.reason.unwrap_or_default()
            ));
            continue;
        }
        let Some(result) = call(tool, &probe) else {
            continue;
        };
        match result {
            Ok(outcome) if outcome.found => return Ok(finish(outcome, notes)),
            Ok(outcome) => {
                notes.push(if subject.is_empty() {
                    format!("{} returned nothing", tool.id)
                } else {
                    format!("{} did not find `{subject}`", tool.id)
                });
                outcomes.push(outcome);
            }
            Err(e) => notes.push(format!("{} failed: {}", tool.id, e.message)),
        }
    }
    if let Some(i) = outcomes
        .iter()
        .position(|o| !o.items.is_empty())
        .or((!outcomes.is_empty()).then_some(0))
    {
        let best = outcomes.remove(i);
        return Ok(finish(best, notes));
    }
    Err(unavailable(request, notes))
}

/// Hand-over notes of earlier tools first, then the answering tool's own notes.
fn finish(mut outcome: Outcome, mut notes: Vec<String>) -> Outcome {
    notes.append(&mut outcome.notes);
    outcome.notes = notes;
    outcome
}

/// The tools to try for a request (built-in name or project capability), in order.
pub fn route_for<'a>(
    registry: &'a Registry,
    request: &str,
    forced: Option<&str>,
) -> Result<Vec<&'a ToolSpec>> {
    if let Some(id) = forced {
        let tool = registry.config.tool(id).ok_or_else(|| {
            KbError::invalid_input(format!(
                "--tool `{id}` is not in the tool registry ({})",
                registry.origin()
            ))
        })?;
        if !tool.answers(request) {
            return Err(KbError::invalid_input(format!(
                "tool `{id}` (adapter `{}`) cannot answer `{request}`",
                tool.adapter.as_str()
            )));
        }
        return Ok(vec![tool]);
    }
    Ok(registry.config.route_named(request))
}

pub fn unavailable(request: &str, attempts: Vec<String>) -> KbError {
    KbError::new(
        ErrorCode::ToolUnavailable,
        format!(
            "no connected tool could answer `{}`{}",
            request,
            if attempts.is_empty() {
                " (the route is empty)".to_string()
            } else {
                format!(": {}", attempts.join("; "))
            }
        ),
    )
    .with_details(json!({ "capability": request, "attempts": attempts }))
    .with_hint(
        "run `kb tools` to see the registry and what is installed; `kb doctor` checks the setup",
    )
}

/// Bounded run of a tool in the code root.
pub fn run_tool(
    program: &Path,
    args: &[String],
    roots: &Roots,
    timeout: u64,
) -> Result<crate::process::Captured> {
    run_with_env(
        program,
        args,
        roots,
        timeout,
        &std::collections::BTreeMap::new(),
    )
}

/// [`run_tool`] with the tool's configured environment on top of the shared one.
pub fn run_with_env(
    program: &Path,
    args: &[String],
    roots: &Roots,
    timeout: u64,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<crate::process::Captured> {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(&roots.code);
    tool_env(&mut cmd);
    cmd.envs(env);
    capture(
        &mut cmd,
        &[],
        Limits {
            timeout: Duration::from_secs(timeout),
            stdout: 16 * 1024 * 1024,
            stderr: 1024 * 1024,
        },
    )
}

/// `path` relative to the code root when it lies inside it.
pub fn rel_to_code(roots: &Roots, path: &str) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        if let Some(rel) = crate::util::rel_string(&roots.code, p) {
            return rel;
        }
        if let Ok(canonical) = roots.code.canonical()
            && let Some(rel) = crate::util::rel_string(&canonical, p)
        {
            return rel;
        }
    }
    path.trim_start_matches("./").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `path` (written `/code/...`) as an absolute path on this platform, where Windows
    /// needs a drive.
    pub(super) fn abs(path: &str) -> String {
        if cfg!(windows) {
            format!("C:{path}")
        } else {
            path.to_string()
        }
    }

    #[test]
    fn programs_resolve_by_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // Windows runs a batch file, not a script with a shebang.
        let (name, script) = if cfg!(windows) {
            ("tool.cmd", "@echo tool 1.0\r\n")
        } else {
            ("tool", "#!/bin/sh\necho tool 1.0\n")
        };
        let tool = bin.join(name);
        std::fs::write(&tool, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let roots = Roots {
            kb: dir.path().into(),
            code: dir.path().into(),
            repo: None,
            kb_mount: None,
        };
        let program = format!("{{kb}}/bin/{name}");
        assert_eq!(resolve_program(&program, &roots), Some(tool.clone()));
        assert_eq!(
            resolve_program(&format!("bin/{name}"), &roots),
            Some(tool.clone())
        );
        assert_eq!(resolve_program("{host}/bin/missing", &roots), None);
        let spec = ToolSpec {
            id: "t".into(),
            adapter: AdapterKind::Ripgrep,
            program: Some(program),
            ..ToolSpec::default()
        };
        let p = probe(&spec, &roots, true);
        assert!(p.available, "{p:?}");
        assert_eq!(p.version.as_deref(), Some("tool 1.0"));
    }
}
