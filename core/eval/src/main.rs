use clap::{Parser, Subcommand};
use kb_eval::{
    Result,
    accounting::{self, Basis, Convention},
    files,
    model::{Family, Observation},
    replay,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "kb-eval",
    version,
    about = "Optional isolated replay kit. No model jobs are started by registration, analysis or qualification."
)]
struct Cli {
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Freeze the preregistered study. Refuses to overwrite an existing registration.
    Register {
        #[arg(long)]
        study: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Hash a task file, executable, or hidden-test directory without exposing contents.
    Digest { path: PathBuf },
    /// Check pinned client startup in isolation, without network or model credentials.
    Preflight {
        #[arg(long)]
        registration: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Require base hidden-test failure and golden success in offline sandboxes.
    Qualify {
        #[arg(long)]
        registration: PathBuf,
        #[arg(long)]
        task: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Start one registered, potentially billable model trial. Requires explicit --execute.
    Run {
        #[arg(long)]
        registration: PathBuf,
        #[arg(long)]
        task: String,
        #[arg(long)]
        arm: String,
        #[arg(long)]
        block: String,
        #[arg(long)]
        repetition: u32,
        #[arg(long)]
        qualification: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, required = true)]
        execute: bool,
    },
    /// Start one registered, potentially billable blinded judge. Requires --execute.
    Judge {
        #[arg(long)]
        registration: PathBuf,
        #[arg(long)]
        trial: PathBuf,
        #[arg(long)]
        judge: usize,
        #[arg(long, required = true)]
        execute: bool,
    },
    /// Analyze explicit observation files; missing planned cells remain visible.
    Analyze {
        #[arg(long)]
        registration: PathBuf,
        #[arg(long, required = true)]
        observation: Vec<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Parse one native log into a numbered reconnect segment.
    Segment {
        #[arg(long,value_parser=["codex","claude","cursor","grok"])]
        family: String,
        #[arg(long)]
        log: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        session: String,
        #[arg(long)]
        sequence: u32,
        #[arg(long)]
        cumulative: bool,
        #[arg(long)]
        output: PathBuf,
    },
    /// Sum all independent/incremental segments; never assume missing usage is zero.
    Account {
        #[arg(long, required = true)]
        segment: Vec<PathBuf>,
        #[arg(long,value_parser=["includes-cache","excludes-cache","unknown"],default_value="unknown")]
        input_convention: String,
        #[arg(long)]
        output: PathBuf,
    },
    #[command(name = "_probe", hide = true)]
    Probe {
        allowed: PathBuf,
        forbidden: PathBuf,
    },
    #[command(name = "_proxy-exec", hide = true)]
    ProxyExec {
        socket: PathBuf,
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
}
fn execute(cli: Cli) -> Result<serde_json::Value> {
    match cli.command {
        Action::Register { study, output } => {
            Ok(serde_json::to_value(replay::register(&study, &output)?)?)
        }
        Action::Digest { path } => Ok(
            serde_json::json!({"sha256":if path.is_dir(){files::tree_digest(&path)?}else{files::digest(&files::read(&path,1024*1024*1024)?)}}),
        ),
        Action::Preflight {
            registration,
            output,
        } => replay::preflight(&replay::load_registration(&registration)?, &output),
        Action::Qualify {
            registration,
            task,
            output,
        } => {
            let result =
                replay::qualify(&replay::load_registration(&registration)?, &task, &output)?;
            let accepted = result.accepted;
            println!("{}", serde_json::to_string_pretty(&result)?);
            if !accepted {
                std::process::exit(1);
            }
            Ok(serde_json::Value::Null)
        }
        Action::Run {
            registration,
            task,
            arm,
            block,
            repetition,
            qualification,
            output,
            execute,
        } => {
            kb_eval::ensure(execute, "--execute is required for model work")?;
            Ok(serde_json::to_value(replay::run(
                &replay::load_registration(&registration)?,
                replay::Cell {
                    task: &task,
                    arm: &arm,
                    block: &block,
                    repetition,
                },
                &qualification,
                &output,
            )?)?)
        }
        Action::Judge {
            registration,
            trial,
            judge,
            execute,
        } => {
            kb_eval::ensure(execute, "--execute is required for judge work")?;
            Ok(
                serde_json::json!({"observation":replay::judge(&replay::load_registration(&registration)?,&trial,judge)?}),
            )
        }
        Action::Analyze {
            registration,
            observation,
            output,
        } => {
            let rows: Vec<Observation> = observation
                .iter()
                .map(|p| files::json(p))
                .collect::<Result<_>>()?;
            let report =
                kb_eval::statistics::analyze(&replay::load_registration(&registration)?, &rows)?;
            files::save(&output, &report)?;
            Ok(serde_json::to_value(report)?)
        }
        Action::Segment {
            family,
            log,
            id,
            session,
            sequence,
            cumulative,
            output,
        } => {
            let family = match family.as_str() {
                "codex" => Family::Codex,
                "claude" => Family::Claude,
                "cursor" => Family::Cursor,
                _ => Family::Grok,
            };
            let mut segment = accounting::parse(
                family,
                &files::read(&log, 128 * 1024 * 1024)?,
                if cumulative {
                    Basis::SessionCumulative
                } else {
                    Basis::Incremental
                },
            )?;
            segment.id = id;
            segment.session = session;
            segment.sequence = sequence;
            files::save(&output, &segment)?;
            Ok(serde_json::to_value(segment)?)
        }
        Action::Account {
            segment,
            input_convention,
            output,
        } => {
            let segments = segment
                .iter()
                .map(|p| files::json(p))
                .collect::<Result<Vec<_>>>()?;
            let convention = match input_convention.as_str() {
                "includes-cache" => Convention::InputIncludesCache,
                "excludes-cache" => Convention::InputExcludesCache,
                _ => Convention::Unknown,
            };
            let result = accounting::reconcile(&segments, convention)?;
            files::save(&output, &result)?;
            Ok(serde_json::to_value(result)?)
        }
        Action::Probe { allowed, forbidden } => {
            kb_eval::isolation::probe(&allowed, &forbidden)?;
            Ok(serde_json::Value::Null)
        }
        Action::ProxyExec { socket, command } => {
            let code = kb_eval::isolation::proxy_exec(
                &socket,
                &PathBuf::from(&command[0]),
                &command[1..],
            )?;
            std::process::exit(code);
        }
    }
}
fn main() {
    match execute(Cli::parse()) {
        Ok(v) => {
            if !v.is_null() {
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
        }
        Err(e) => {
            eprintln!("kb-eval: {e}");
            std::process::exit(1);
        }
    }
}
