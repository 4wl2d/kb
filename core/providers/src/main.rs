use std::io::{Read, Write};
use std::path::PathBuf;

use clap::Parser;
use kb::error::{KbError, Result};
use kb::model::CodeRequest;
use kb_code_provider::{Backend, Options};

#[derive(Parser)]
#[command(
    version,
    about = "Read one kb.code.v1 request on stdin; emit one response"
)]
struct Args {
    #[arg(long, value_enum)]
    backend: Backend,
    /// Installed native tool, otherwise the backend name is searched on PATH.
    #[arg(long)]
    tool: Option<PathBuf>,
    /// Deadline for each native indexing/query command (the caller also has a deadline).
    #[arg(long, default_value_t = 120)]
    timeout: u64,
}

fn run(args: Args) -> Result<()> {
    const MAX_REQUEST: u64 = 8 * 1024 * 1024;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_REQUEST + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| KbError::io("read provider request", e))?;
    if bytes.len() as u64 > MAX_REQUEST {
        return Err(KbError::invalid_input("provider request exceeds 8 MiB"));
    }
    let request: CodeRequest = serde_json::from_slice(&bytes)?;
    let response = kb_code_provider::run(
        &request,
        &Options {
            backend: args.backend,
            tool: args.tool.unwrap_or_else(|| args.backend.program().into()),
            timeout_seconds: args.timeout,
        },
    )?;
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &response)?;
    stdout
        .write_all(b"\n")
        .map_err(|e| KbError::io("write provider response", e))
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        // A failed provider emits no partial successful response on stdout.
        eprintln!("{}", kb::git::redact(&error.to_string()));
        if matches!(
            error.details["kind"].as_str(),
            Some("timeout" | "stdout-limit" | "stderr-limit")
        ) {
            end_own_group();
        }
        std::process::exit(1);
    }
}

/// A native command or snapshot Git stopped at its own deadline or byte limit was killed
/// alone, and anything it started still runs in this process group. `run` has returned, so
/// the frozen tree is gone: end the group, this provider included, when the provider leads
/// it, as it does under the engine. A group it merely joined belongs to its caller.
#[cfg(unix)]
fn end_own_group() {
    // SAFETY: getpgrp and getpid cannot fail; kill receives a valid signal and 0, which
    // names the caller's own process group, verified to be led by this provider.
    unsafe {
        if libc::getpgrp() == libc::getpid() {
            libc::kill(0, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn end_own_group() {}
