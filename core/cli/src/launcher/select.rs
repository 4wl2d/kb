//! Engine selection, the first step of every `kb` invocation: knowledge is always read by
//! the engine its checkout pins. The checkout's engine fingerprint (warm path: the stamp)
//! decides: the running binary serves it in-process when it was built from exactly those
//! sources, else the matching runtime from the store (or one the retired `kbw` launcher
//! built in the checkout) runs in place of this process. A missing runtime is an error
//! unless `KB_AUTO_BOOTSTRAP=1`; normal commands never build or download an engine.
//!
//! `kb engine ...`, `kb self ...`, top-level `--help`/`--version`, invocations outside any
//! KB checkout and checkouts without complete engine sources (pinned by their manifest's
//! versions alone, which the engine checks) always run here.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::engine::{self, Checkout, Serving};
use super::resolve::{self, RootSource};
use super::store::Store;
use super::{platform, stamp};
use crate::cli::args::GlobalOpts;
use crate::error::{ErrorCode, KbError};
use crate::versions::{BUILD_FINGERPRINT, ENGINE_VERSION};

/// Outcome of selection.
pub enum Selected {
    /// Run the command in this process.
    Here,
    /// The command already ran (another engine, or an error was reported): exit with this.
    Exit(i32),
}

/// Environment variable naming the fingerprint a started runtime was selected for.
pub const SELECTED_ENV: &str = "KB_SELECTED_FINGERPRINT";

/// Subcommands the running `kb` always handles itself.
pub const LOCAL_COMMANDS: [&str; 2] = ["engine", "self"];

/// Global options that take a value.
const VALUE_OPTIONS: [&str; 7] = [
    "--root",
    "--config",
    "--profile",
    "--format",
    "--snapshot",
    "--host",
    "--skill-protocol",
];

/// What selection needs from the raw arguments, read without the engine's parser (the
/// selected engine may accept different arguments).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Scan {
    pub root: Option<PathBuf>,
    pub host: Option<PathBuf>,
    pub command: Option<String>,
    pub json: bool,
    pub quiet: bool,
}

pub fn scan(args: &[OsString]) -> Scan {
    let mut s = Scan::default();
    let mut it = args.iter().skip(1);
    while let Some(arg) = it.next() {
        let Some(a) = arg.to_str() else {
            if s.command.is_none() {
                s.command = Some(arg.to_string_lossy().into_owned());
            }
            continue;
        };
        if a == "--" {
            break;
        }
        if let Some((name, value)) = a.split_once('=')
            && VALUE_OPTIONS.contains(&name)
        {
            match name {
                "--root" => s.root = Some(PathBuf::from(value)),
                "--host" => s.host = Some(PathBuf::from(value)),
                "--format" => s.json |= value == "json",
                _ => {}
            }
            continue;
        }
        match a {
            "--root" => s.root = it.next().map(PathBuf::from),
            "--host" => s.host = it.next().map(PathBuf::from),
            "--format" => s.json |= it.next().is_some_and(|v| v == "json"),
            "--json" => s.json = true,
            "--quiet" | "-q" => s.quiet = true,
            _ if VALUE_OPTIONS.contains(&a) => {
                if s.command.is_none() {
                    it.next();
                }
            }
            _ if s.command.is_none() && !a.starts_with('-') => s.command = Some(a.to_string()),
            _ => {}
        }
    }
    s
}

fn fail(scan: &Scan, err: &KbError) -> Selected {
    crate::output::emit_unparsed(scan.command.as_deref(), scan.json, None, Some(err));
    crate::output::print_error_text(err);
    Selected::Exit(err.exit_code())
}

pub fn select(args: &[OsString]) -> Selected {
    // Set by the `kb` that chose this binary as the runtime of a fingerprint; it concerns only
    // this process, so it is taken out of the environment before anything else runs (no
    // thread exists yet) and is not inherited by programs the engine starts.
    let chosen_for = std::env::var_os(SELECTED_ENV);
    if chosen_for.is_some() {
        // SAFETY: called first thing in `main`, before any other thread is started.
        unsafe { std::env::remove_var(SELECTED_ENV) };
    }
    let scan = scan(args);
    let Some(command) = scan.command.as_deref() else {
        return Selected::Here;
    };
    if LOCAL_COMMANDS.contains(&command) {
        return Selected::Here;
    }
    let Ok(cwd) = std::env::current_dir() else {
        return Selected::Here;
    };
    let env = resolve::env_root();
    let Some(found) = resolve::find(
        &cwd,
        scan.root.as_deref(),
        env.as_deref().map(Path::new),
        scan.host.as_deref(),
    ) else {
        return Selected::Here;
    };
    // An explicit root without a manifest: the engine reports it (`version` needs none).
    if !found.has_manifest() {
        return Selected::Here;
    }
    let fp = match stamp::fingerprint(&found.dir) {
        Ok(fp) => fp,
        // Without complete engine sources the checkout pins its engine only by the versions
        // in its manifest, which the engine checks itself (RUNTIME_INCOMPATIBLE).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Selected::Here,
        Err(e) => {
            return fail(
                &scan,
                &KbError::new(
                    ErrorCode::ConfigInvalid,
                    format!(
                        "cannot hash the engine build inputs under {}: {e}",
                        found.dir.display()
                    ),
                ),
            );
        }
    };
    if fp == BUILD_FINGERPRINT {
        return Selected::Here;
    }
    if chosen_for.as_deref().and_then(|v| v.to_str()) == Some(fp.as_str()) {
        // Selecting again would start this same binary in a loop.
        let exe = std::env::current_exe().unwrap_or_default();
        return fail(
            &scan,
            &KbError::new(
                ErrorCode::RuntimeIncompatible,
                format!(
                    "the runtime {} was selected for engine fingerprint {fp} but was built from other sources (build {BUILD_FINGERPRINT})",
                    exe.display()
                ),
            )
            .with_hint("replace it: remove that runtime directory, then run `kb engine bootstrap` or `kb engine install`"),
        );
    }
    let store = match Store::locate(&cwd) {
        Ok(s) => s,
        Err(e) => return fail(&scan, &e),
    };
    let binary = match engine::serving(&store, &found.dir, &fp) {
        Serving::ThisBinary => return Selected::Here,
        Serving::Store(p) | Serving::Legacy(p) => p,
        Serving::Missing => match missing(&scan, found.dir.clone(), found.source, &fp) {
            Ok(p) => p,
            Err(e) => return fail(&scan, &e),
        },
    };
    let envs: [(&str, &std::ffi::OsStr); 2] = [
        ("KB_ROOT", found.dir.as_os_str()),
        (SELECTED_ENV, std::ffi::OsStr::new(&fp)),
    ];
    match platform::run_in_place(&binary, &args[1..], &envs) {
        Ok(code) => Selected::Exit(code),
        Err(e) => fail(
            &scan,
            &KbError::new(
                ErrorCode::IoError,
                format!("cannot run the runtime {}: {e}", binary.display()),
            )
            .with_hint(
                "rebuild it with `kb engine bootstrap` or reinstall it with `kb engine install`",
            ),
        ),
    }
}

/// No runtime serves `fp`: build it when `KB_AUTO_BOOTSTRAP=1`, else explain how to get one.
fn missing(scan: &Scan, dir: PathBuf, source: RootSource, fp: &str) -> Result<PathBuf, KbError> {
    let co = Checkout::at(dir, source)?;
    if std::env::var("KB_AUTO_BOOTSTRAP").as_deref() == Ok("1") {
        let global = GlobalOpts {
            root: Some(co.root.clone()),
            config: None,
            profile: crate::cli::args::ProfileArg::Project,
            format: None,
            json: scan.json,
            offline: false,
            snapshot: None,
            host: None,
            quiet: scan.quiet,
            skill_protocol: None,
        };
        return engine::ensure_runtime(&global, &co, fp).map(|(_, p)| p);
    }
    let release_hint = if co.engine_version == ENGINE_VERSION {
        format!(
            "this kb is engine {ENGINE_VERSION} too, so the checkout's engine sources differ from it (local engine edits or patches): build them with `kb engine bootstrap`"
        )
    } else {
        format!(
            "install the release runtime with `kb engine install` (works when the checkout's engine is the unmodified release {}), or build it from source with `kb engine bootstrap`",
            co.engine_version
        )
    };
    Err(KbError::new(
        ErrorCode::RuntimeNotBootstrapped,
        format!(
            "no runtime for engine {} (fingerprint {fp}) of the KB checkout {}; normal commands never build or download an engine",
            co.engine_version,
            co.root.display()
        ),
    )
    .with_details(serde_json::json!({
        "root": co.root,
        "engine_version": co.engine_version,
        "fingerprint": fp,
        "kb": {"engine_version": ENGINE_VERSION, "build_fingerprint": BUILD_FINGERPRINT},
    }))
    .with_hint(format!(
        "{release_hint}; KB_AUTO_BOOTSTRAP=1 builds missing runtimes implicitly (CI)"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Scan {
        let v: Vec<OsString> = std::iter::once("kb")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect();
        scan(&v)
    }

    #[test]
    fn scans_global_options_on_either_side_of_the_command() {
        let r = s(&[
            "--profile",
            "maintainer",
            "--root=x",
            "context",
            "--json",
            "--host",
            "h",
            "--",
            "--root",
            "y",
        ]);
        assert_eq!(r.command.as_deref(), Some("context"));
        assert_eq!(r.root, Some(PathBuf::from("x")));
        assert_eq!(r.host, Some(PathBuf::from("h")));
        assert!(r.json);
        let r = s(&["-q", "--format", "json", "engine", "bootstrap"]);
        assert_eq!(r.command.as_deref(), Some("engine"));
        assert!(r.json && r.quiet);
        assert_eq!(s(&["--version"]).command, None);
        assert_eq!(
            s(&["--snapshot", "pinned", "version"]).command.as_deref(),
            Some("version")
        );
    }
}
