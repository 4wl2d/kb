//! `command` adapter: run a configured check for the files that trigger it. The argv comes
//! only from the registry of the KB checkout that supplies the engine; it is started
//! without a shell, in the code root, under the tool's deadline and output limits.

use super::{Probe, Roots, ToolSpec, run_tool};
use crate::error::Result;

/// Result of one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Passed,
    Failed(i32),
    /// Deadline, output limit or start failure.
    Error(String),
}

#[derive(Debug, Clone)]
pub struct CheckRun {
    pub status: Status,
    /// Last lines of the combined output (stdout, then stderr) of a failed check, at most `tail`.
    pub tail: Vec<String>,
}

/// Expand `run`: the whole argument `{files}` becomes the matched files; leading `{kb}` /
/// `{host}` become the KB and code roots.
pub fn argv(tool: &ToolSpec, roots: &Roots, files: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for a in &tool.run {
        if a == "{files}" {
            out.extend(files.iter().cloned());
        } else if let Some(rest) = a.strip_prefix("{kb}") {
            out.push(format!("{}{rest}", roots.kb.display()));
        } else if let Some(rest) = a.strip_prefix("{host}") {
            out.push(format!("{}{rest}", roots.code.display()));
        } else {
            out.push(a.clone());
        }
    }
    out
}

pub fn run(
    tool: &ToolSpec,
    probe: &Probe,
    roots: &Roots,
    files: &[String],
    tail: usize,
) -> Result<CheckRun> {
    let program = std::path::PathBuf::from(probe.program.as_deref().unwrap_or_default());
    let args = argv(tool, roots, files);
    let out = match run_tool(&program, &args, roots, tool.timeout()) {
        Ok(out) => out,
        Err(e) => {
            return Ok(CheckRun {
                status: Status::Error(e.message),
                tail: Vec::new(),
            });
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let status = if out.exit_code == 0 {
        Status::Passed
    } else {
        Status::Failed(out.exit_code)
    };
    let pick = |text: &str| -> Vec<String> {
        let lines: Vec<String> = text
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        lines[lines.len().saturating_sub(tail)..].to_vec()
    };
    // Tools print their verdict last: stdout's end, then stderr's (e.g. `test result:
    // FAILED` followed by `error: test failed`).
    let tail = if status == Status::Passed {
        Vec::new()
    } else {
        pick(&format!("{stdout}\n{stderr}"))
    };
    Ok(CheckRun { status, tail })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tools::AdapterKind;

    #[test]
    fn argv_expands_placeholders() {
        let tool = ToolSpec {
            id: "lint".into(),
            adapter: AdapterKind::Command,
            program: Some("eslint".into()),
            run: vec![
                "--config".into(),
                "{kb}/lint.json".into(),
                "{files}".into(),
                "x{files}".into(),
            ],
            paths: vec!["**/*.ts".into()],
            ..ToolSpec::default()
        };
        let roots = Roots {
            kb: "/k".into(),
            code: "/c".into(),
            repo: None,
            kb_mount: None,
        };
        assert_eq!(
            argv(&tool, &roots, &["a.ts".into(), "b.ts".into()]),
            ["--config", "/k/lint.json", "a.ts", "b.ts", "x{files}"]
        );
    }
}
