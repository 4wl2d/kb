//! Thin, version-pinned adapters. Parsing belongs to the selected native tool.
mod ast;
mod codegraph;
mod frozen;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use clap::ValueEnum;
use kb::error::{KbError, Result};
use kb::model::*;
use kb::process::{Limits, capture_in_group};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Backend {
    AstIndex,
    Codegraph,
}

impl Backend {
    pub fn program(self) -> &'static str {
        match self {
            Self::AstIndex => "ast-index",
            Self::Codegraph => "codegraph",
        }
    }

    fn version(self) -> &'static str {
        match self {
            Self::AstIndex => "3.54.0",
            Self::Codegraph => "1.6.1",
        }
    }
}

pub struct Options {
    pub backend: Backend,
    pub tool: PathBuf,
    pub timeout_seconds: u64,
}

pub fn run(request: &CodeRequest, options: &Options) -> Result<CodeResponse> {
    if !(1..=600).contains(&options.timeout_seconds) {
        return Err(KbError::invalid_input("timeout must be 1..600 seconds"));
    }
    if !Path::new(&request.root).is_absolute()
        || !matches!(request.commit.len(), 40 | 64)
        || !kb::model::is_commit(&request.commit)
        || !(1..=4).contains(&request.depth)
    {
        return Err(KbError::invalid_input(
            "request needs an absolute root, full commit id and depth 1..4",
        ));
    }
    let frozen = frozen::Frozen::create(request)?;
    let bytes = native(&frozen, options, &["--version"])?;
    let version = String::from_utf8_lossy(&bytes);
    if version.split_whitespace().last() != Some(options.backend.version()) {
        return Err(KbError::invalid_input(format!(
            "{} adapter is verified for version {}; installed version is {}",
            options.backend.program(),
            options.backend.version(),
            version.trim()
        )));
    }
    let mut response = match options.backend {
        Backend::AstIndex => ast::load(&frozen, options, request)?,
        Backend::Codegraph => codegraph::load(&frozen, options, request)?,
    };
    if !frozen.limitations.is_empty() {
        response.complete = false;
        response
            .limitations
            .extend(frozen.limitations.iter().cloned());
    }
    add_similar(&mut response, request);
    kb::code::validate(response, request)
}

fn response(request: &CodeRequest, backend: Backend) -> CodeResponse {
    CodeResponse {
        protocol: CodeProtocol::V1,
        repo: request.repo.clone(),
        commit: request.commit.clone(),
        tool: CodeTool { name: backend.program().into(), version: backend.version().into() },
        capabilities: vec![CodeOperation::Symbols, CodeOperation::Refs, CodeOperation::Dependents, CodeOperation::Similar],
        complete: true,
        limitations: vec!["Static code evidence does not establish runtime reachability or behavioral equivalence.".into()],
        symbols: vec![],
        refs: vec![],
        similar: vec![],
    }
}

fn native(frozen: &frozen::Frozen, options: &Options, args: &[&str]) -> Result<Vec<u8>> {
    let tool = if options.tool.is_absolute() || options.tool.components().count() == 1 {
        options.tool.clone()
    } else {
        std::env::current_dir()
            .map_err(|e| KbError::io("cwd", e))?
            .join(&options.tool)
    };
    let mut command = Command::new(tool);
    command.args(args).current_dir(&frozen.root);
    frozen.environment(&mut command);
    // Native tools and snapshot Git stay in the provider's process group, so the engine's
    // deadline ends them together with the provider. When this deadline stops one instead,
    // `main` ends that group after the frozen tree is removed, if the engine created it.
    let output = capture_in_group(
        &mut command,
        &[],
        Limits {
            timeout: Duration::from_secs(options.timeout_seconds),
            stdout: kb::code::MAX_PROVIDER_BYTES,
            stderr: 1024 * 1024,
        },
    )?;
    if output.exit_code != 0 {
        return Err(KbError::invalid_input(format!(
            "{} {} exited {}: {}",
            options.backend.program(),
            args.first().unwrap_or(&""),
            output.exit_code,
            kb::git::redact(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    Ok(output.stdout)
}

fn test_candidate(path: &str, name: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.split('/')
        .any(|part| matches!(part, "test" | "tests" | "__tests__"))
        || path.contains(".test.")
        || path.contains(".spec.")
        || name.starts_with("test_")
        || name.ends_with("Test")
}

fn add_similar(response: &mut CodeResponse, request: &CodeRequest) {
    let query: BTreeSet<_> = kb::context::discovery::identifier_tokens(&format!(
        "{} {}",
        request.task.as_deref().unwrap_or_default(),
        request.identifiers.join(" ")
    ))
    .into_iter()
    .filter(|w| w.chars().count() >= 3)
    .collect();
    if query.is_empty() {
        return;
    }
    for symbol in &response.symbols {
        if symbol.extent == CodeExtent::File {
            continue;
        }
        let terms: BTreeSet<_> = kb::context::discovery::identifier_tokens(&format!(
            "{} {} {}",
            symbol.name,
            symbol.path,
            symbol.signature.as_deref().unwrap_or_default()
        ))
        .into_iter()
        .collect();
        let shared = query.intersection(&terms).count();
        if shared > 0 {
            response.similar.push(CodeSimilar {
                symbol: symbol.id.clone(),
                score: ((shared * 1000) / query.len()) as u32,
                reason: "lexical overlap with the task; inspect before reusing as a precedent"
                    .into(),
            });
        }
    }
}
