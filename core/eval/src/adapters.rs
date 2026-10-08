//! Native flags are intentionally narrow and checked against an exact executable/version pin.
use crate::{
    Result, ensure, files,
    model::{Agent, Family, Program},
};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub fn identity(agent: &Agent) -> String {
    format!(
        "{}/{}/{}",
        format!("{:?}", agent.family).to_ascii_lowercase(),
        agent.version,
        agent.model
    )
}
pub fn verify(agent: &Agent, home: &Path) -> Result<()> {
    agent.validate()?;
    ensure(
        files::digest(&files::read(&agent.program, 1024 * 1024 * 1024)?) == agent.binary_sha256,
        "agent executable digest changed",
    )?;
    let mut c = Command::new(&agent.program);
    files::isolated_env(&mut c, home);
    c.arg("--version");
    let out = kb::process::capture(
        &mut c,
        &[],
        kb::process::Limits {
            timeout: Duration::from_secs(20),
            stdout: 64 * 1024,
            stderr: 64 * 1024,
        },
    )?;
    ensure(
        out.exit_code == 0 && String::from_utf8_lossy(&out.stdout).trim() == agent.version,
        format!(
            "agent version does not match registration in the isolated HOME: expected {:?}; exit {}; stdout {:?}; stderr {:?}",
            agent.version,
            out.exit_code,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

pub fn invocation(agent: &Agent, prompt: &Path, work: &Path, judge: bool) -> Result<Program> {
    agent.validate()?;
    let prompt_text = String::from_utf8(files::read(prompt, 1024 * 1024)?)?;
    let mut args: Vec<String> = match agent.family {
        Family::Codex => vec![
            "exec",
            "--json",
            "--ignore-user-config",
            "--ignore-rules",
            "--ephemeral",
            "--dangerously-bypass-approvals-and-sandbox",
            "--skip-git-repo-check",
            "--model",
            &agent.model,
            "-c",
            "features.multi_agent=false",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        Family::Claude => vec![
            "--print",
            "--verbose",
            "--output-format",
            "stream-json",
            "--model",
            &agent.model,
            "--dangerously-skip-permissions",
            "--no-session-persistence",
            "--strict-mcp-config",
            "--mcp-config",
            "{}",
            "--setting-sources",
            "project",
            "--no-chrome",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        Family::Cursor => vec![
            "--print",
            "--output-format",
            "stream-json",
            "--model",
            &agent.model,
            "--force",
            "--sandbox",
            "disabled",
            "--trust",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        Family::Grok => vec![
            "--model",
            &agent.model,
            "--output-format",
            "streaming-messages-json",
            "--permission-mode",
            "bypassPermissions",
            "--disable-web-search",
            "--no-subagents",
            "--no-plan",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
    };
    if let Some(effort) = &agent.effort {
        match agent.family {
            Family::Codex => {
                args.push("-c".into());
                args.push(format!(
                    "model_reasoning_effort={}",
                    serde_json::to_string(effort)?
                ));
            }
            Family::Claude => {
                args.push("--effort".into());
                args.push(effort.clone());
            }
            Family::Grok => {
                args.push("--reasoning-effort".into());
                args.push(effort.clone());
            }
            Family::Cursor => {
                return Err("put the pinned Cursor effort parameter in its model string".into());
            }
        }
    }
    if judge && agent.family == Family::Claude {
        args.push("--bare".into());
    }
    match agent.family {
        Family::Grok => {
            args.push("--cwd".into());
            args.push(work.to_string_lossy().into_owned());
            args.push("--prompt-file".into());
            args.push(prompt.to_string_lossy().into_owned());
        }
        Family::Codex => {
            args.push("--cd".into());
            args.push(work.to_string_lossy().into_owned());
            args.push(prompt_text);
        }
        Family::Cursor => {
            args.push("--workspace".into());
            args.push(work.to_string_lossy().into_owned());
            args.push(prompt_text);
        }
        Family::Claude => args.push(prompt_text),
    }
    Ok(Program {
        program: agent.program.clone(),
        args,
        timeout_seconds: agent.timeout_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn judge_cli_does_not_resume_sessions_or_inherit_global_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = dir.path().join("prompt.txt");
        std::fs::write(&prompt, "synthetic judge request").unwrap();
        for family in [Family::Codex, Family::Claude, Family::Cursor, Family::Grok] {
            let a = Agent {
                family,
                program: "/fixture/agent".into(),
                binary_sha256: "0".repeat(64),
                version: "fixture-pinned".into(),
                model: "model-pinned".into(),
                effort: None,
                timeout_seconds: 20,
                secret_env: vec![],
                usage_basis: crate::accounting::Basis::Incremental,
                input_convention: crate::accounting::Convention::Unknown,
            };
            let p = invocation(&a, &prompt, dir.path(), true).unwrap();
            assert!(!p.args.iter().any(|v| v == "--resume" || v == "--continue"));
            if family == Family::Claude {
                assert!(p.args.contains(&"--bare".into()));
            }
            if family == Family::Grok {
                assert!(p.args.contains(&"--no-subagents".into()));
            }
        }
    }
}
