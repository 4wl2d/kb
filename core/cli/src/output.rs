//! Output envelope and formats. In JSON mode stdout carries only the protocol document;
//! progress and diagnostics go to stderr.

use std::io::Write;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::error::KbError;
use crate::versions::PROTOCOL_ID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    Compact,
    Terse,
    Human,
    Json,
}

/// Result of a command handler.
#[derive(Debug, Clone, Default)]
pub struct CommandOutput {
    /// Deterministic machine result (JSON mode `result`).
    pub result: Value,
    /// Rendered text for compact/human formats (ignored in JSON mode).
    pub text: String,
    /// Non-success status that still carries a result (e.g. VALIDATION_FAILED, CONTEXT_INCOMPLETE).
    pub failure: Option<KbError>,
    /// Non-deterministic metadata (timings, timestamps). Never part of `result`.
    pub meta: Map<String, Value>,
    /// Print `text` byte-exact in text formats (no newline is appended), e.g. `show --raw`.
    pub exact_text: bool,
}

impl CommandOutput {
    pub fn new(result: Value, text: String) -> Self {
        CommandOutput {
            result,
            text,
            failure: None,
            meta: Map::new(),
            exact_text: false,
        }
    }
    pub fn with_failure(mut self, err: KbError) -> Self {
        self.failure = Some(err);
        self
    }
    /// Mark `text` as authoritative bytes that must be printed unchanged.
    pub fn with_exact_text(mut self) -> Self {
        self.exact_text = true;
        self
    }
}

pub fn error_json(e: &KbError) -> Value {
    let mut v = json!({
        "code": e.code.as_str(),
        "exit_code": e.exit_code(),
        "message": e.message,
    });
    if !e.details.is_null() {
        v["details"] = e.details.clone();
    }
    if let Some(h) = &e.hint {
        v["hint"] = json!(h);
    }
    if !e.diagnostics.is_empty() {
        v["diagnostics"] = serde_json::to_value(&e.diagnostics).unwrap_or(Value::Null);
    }
    v
}

/// Build the JSON envelope.
pub fn envelope(
    command: &str,
    out: Option<&CommandOutput>,
    err: Option<&KbError>,
    meta: &Map<String, Value>,
) -> Value {
    let failure = err.or_else(|| out.and_then(|o| o.failure.as_ref()));
    json!({
        "protocol": PROTOCOL_ID,
        "command": command,
        "ok": failure.is_none(),
        "result": out.map(|o| o.result.clone()).unwrap_or(Value::Null),
        "error": failure.map(error_json).unwrap_or(Value::Null),
        "meta": Value::Object(meta.clone()),
    })
}

/// Print a command result and return the process exit code.
pub fn emit(
    command: &str,
    format: Format,
    res: Result<CommandOutput, KbError>,
    elapsed_ms: u128,
) -> i32 {
    let stdout = std::io::stdout();
    let mut so = stdout.lock();
    match res {
        Ok(out) => {
            let mut meta = out.meta.clone();
            meta.insert("elapsed_ms".into(), json!(elapsed_ms as u64));
            match format {
                Format::Json => {
                    let v = envelope(command, Some(&out), None, &meta);
                    let _ = writeln!(
                        so,
                        "{}",
                        serde_json::to_string_pretty(&v).unwrap_or_default()
                    );
                }
                _ => {
                    if !out.text.is_empty() {
                        let _ = write!(so, "{}", out.text);
                        if !out.exact_text && !out.text.ends_with('\n') {
                            let _ = writeln!(so);
                        }
                    }
                    if let Some(f) = &out.failure {
                        print_error_text(f);
                    }
                }
            }
            let _ = so.flush();
            out.failure.as_ref().map(|f| f.exit_code()).unwrap_or(0)
        }
        Err(e) => {
            if format == Format::Json {
                let mut meta = Map::new();
                meta.insert("elapsed_ms".into(), json!(elapsed_ms as u64));
                let v = envelope(command, None, Some(&e), &meta);
                let _ = writeln!(
                    so,
                    "{}",
                    serde_json::to_string_pretty(&v).unwrap_or_default()
                );
                let _ = so.flush();
            }
            print_error_text(&e);
            e.exit_code()
        }
    }
}

/// Human-readable error on stderr.
pub fn print_error_text(e: &KbError) {
    let mut s = format!("error[{}]: {}\n", e.code.as_str(), e.message);
    if !e.details.is_null()
        && let Ok(d) = serde_json::to_string(&e.details)
    {
        s.push_str(&format!("  details: {d}\n"));
    }
    for d in e.diagnostics.iter().take(50) {
        let loc = d.path.clone().unwrap_or_default();
        s.push_str(&format!(
            "  {:?} {} {}: {}\n",
            d.severity, d.code, loc, d.message
        ));
    }
    if e.diagnostics.len() > 50 {
        s.push_str(&format!(
            "  … {} more diagnostics\n",
            e.diagnostics.len() - 50
        ));
    }
    if let Some(h) = &e.hint {
        s.push_str(&format!("  hint: {h}\n"));
    }
    eprint!("{s}");
}

/// Emit the result of argument parsing that did not produce a command: usage errors, and
/// help requested in JSON mode. In JSON mode stdout still carries exactly one protocol
/// envelope (`command` is null when no subcommand could be identified); clap's text goes
/// to stderr. Returns the exit code.
pub fn emit_unparsed(
    command: Option<&str>,
    json_mode: bool,
    help_text: Option<&str>,
    error: Option<&KbError>,
) -> i32 {
    if json_mode {
        let v = json!({
            "protocol": PROTOCOL_ID,
            "command": command,
            "ok": error.is_none(),
            "result": help_text.map(|h| json!({ "help": h })).unwrap_or(Value::Null),
            "error": error.map(error_json).unwrap_or(Value::Null),
            "meta": {},
        });
        let stdout = std::io::stdout();
        let mut so = stdout.lock();
        let _ = writeln!(
            so,
            "{}",
            serde_json::to_string_pretty(&v).unwrap_or_default()
        );
        let _ = so.flush();
    }
    error.map(KbError::exit_code).unwrap_or(0)
}
