//! CLI dispatch. Each command handler lives in its own module with the signature
//! `fn run(ctx: &Ctx, args: &Args) -> Result<CommandOutput>`.

pub mod args;
mod cmd_context;
mod cmd_doctor;
mod cmd_impact;
mod cmd_index;
mod cmd_init;
mod cmd_integrate;
mod cmd_migrate;
mod cmd_schema;
mod cmd_search;
mod cmd_show;
mod cmd_sync;
mod cmd_update;
mod cmd_validate;
mod session;

use std::time::Instant;

use clap::Parser;
use serde_json::json;

use crate::env::Env;
use crate::error::{ErrorCode, KbError, Result};
use crate::model::{Profile, ProfileLocation};
use crate::output::{CommandOutput, Format, emit};
use crate::versions::{CompiledVersions, SKILL_PROTOCOL, check_runtime};
use args::{Cli, Command, FormatArg, GlobalOpts, ProfileArg};

/// Shared invocation context for command handlers.
pub struct Ctx {
    pub env: Env,
    pub global: GlobalOpts,
    pub format: Format,
}

impl Ctx {
    pub fn profile(&self) -> Profile {
        match self.global.profile {
            ProfileArg::Project => Profile::Project,
            ProfileArg::Maintainer => Profile::Maintainer,
        }
    }

    /// Profile location, honoring `--config`.
    pub fn location(&self) -> Result<ProfileLocation> {
        let mut loc = ProfileLocation::for_profile(self.profile());
        if let Some(c) = &self.global.config {
            crate::util::check_rel_path(c).map_err(KbError::unsafe_path)?;
            loc.config = c.clone();
        }
        Ok(loc)
    }
}

fn format_of(g: &GlobalOpts) -> Format {
    if g.json {
        return Format::Json;
    }
    match g.format {
        Some(FormatArg::Json) => Format::Json,
        Some(FormatArg::Human) => Format::Human,
        _ => Format::Compact,
    }
}

fn command_name(c: &Command) -> &'static str {
    match c {
        Command::Init(_) => "init",
        Command::Doctor(_) => "doctor",
        Command::Validate(_) => "validate",
        Command::Index(_) => "index",
        Command::Context(_) => "context",
        Command::Search(_) => "search",
        Command::Show(_) => "show",
        Command::Sync(_) => "sync",
        Command::Impact(_) => "impact",
        Command::Integrate(_) => "integrate",
        Command::Migrate(_) => "migrate",
        Command::Update(_) => "update",
        Command::Schema(_) => "schema",
        Command::Version => "version",
    }
}

/// Entry point used by `main`. Returns the process exit code.
pub fn run() -> i32 {
    let started = Instant::now();
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => return parse_failure(e),
    };
    let format = format_of(&cli.global);
    let name = command_name(&cli.command);
    let res = dispatch(cli, format);
    emit(name, format, res, started.elapsed().as_millis())
}

/// Report a clap parse result that did not yield a command. In JSON mode (detected from the
/// raw arguments, since parsing failed) stdout still gets one protocol envelope.
fn parse_failure(e: clap::Error) -> i32 {
    use clap::CommandFactory;
    use clap::error::ErrorKind;
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let json_mode = raw.iter().enumerate().any(|(i, a)| {
        a == "--json"
            || a == "--format=json"
            || (a == "--format" && raw.get(i + 1).is_some_and(|v| v == "json"))
    });
    let names: Vec<String> = Cli::command()
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    let command = raw.iter().find(|a| names.contains(a)).cloned();
    let text = e.render().to_string();
    match e.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            if json_mode {
                crate::output::emit_unparsed(command.as_deref(), true, Some(&text), None)
            } else {
                let _ = e.print();
                0
            }
        }
        _ => {
            // clap's rendering (with usage and suggestions) always goes to stderr.
            let _ = e.print();
            let err = KbError::new(ErrorCode::Usage, text.trim().to_string())
                .with_hint("run `kbw <command> --help` for the accepted arguments");
            crate::output::emit_unparsed(command.as_deref(), json_mode, None, Some(&err))
        }
    }
}

fn dispatch(cli: Cli, format: Format) -> Result<CommandOutput> {
    if let Command::Version = cli.command {
        let v = CompiledVersions::current();
        let text = format!(
            "kb {} (document schema {}, protocol {}, index schema {}, skill protocol {}) build {}\n",
            v.engine_version,
            v.document_schema,
            v.protocol,
            v.index_schema,
            v.skill_protocol,
            v.build_fingerprint
        );
        return Ok(CommandOutput::new(json!(v), text));
    }
    if let Some(p) = cli.global.skill_protocol
        && p != SKILL_PROTOCOL
    {
        return Err(KbError::new(
                ErrorCode::SkillOutdated,
                format!("the caller's skill targets skill protocol {p}, this engine serves {SKILL_PROTOCOL}"),
            )
            .with_details(json!({"caller": p, "engine": SKILL_PROTOCOL}))
            .with_hint("re-read the kb skill (SKILL.md) or start a new session so the updated instructions are loaded"));
    }
    let env = Env::discover(cli.global.root.as_deref(), cli.global.quiet)?;
    check_runtime(&env.manifest)?;
    let ctx = Ctx {
        env,
        global: cli.global,
        format,
    };
    match &cli.command {
        Command::Init(a) => cmd_init::run(&ctx, a),
        Command::Doctor(a) => cmd_doctor::run(&ctx, a),
        Command::Validate(a) => cmd_validate::run(&ctx, a),
        Command::Index(a) => cmd_index::run(&ctx, a),
        Command::Context(a) => cmd_context::run(&ctx, a),
        Command::Search(a) => cmd_search::run(&ctx, a),
        Command::Show(a) => cmd_show::run(&ctx, a),
        Command::Sync(a) => cmd_sync::run(&ctx, a),
        Command::Impact(a) => cmd_impact::run(&ctx, a),
        Command::Integrate(a) => cmd_integrate::run(&ctx, a),
        Command::Migrate(a) => cmd_migrate::run(&ctx, a),
        Command::Update(a) => cmd_update::run(&ctx, a),
        Command::Schema(a) => cmd_schema::run(&ctx, a),
        Command::Version => unreachable!("handled above"),
    }
}
