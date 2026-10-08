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
        std::process::exit(1);
    }
}
