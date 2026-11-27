//! `kb engine ...`: the runtimes that run each KB checkout. These commands are always handled
//! by the `kb` that was started, never by the checkout's engine, so they also work for a
//! checkout whose engine is missing, older or newer.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::archive::{self, Member};
use super::resolve::{self, RootSource};
use super::store::{self, BuildInfo, Store, TempDir};
use super::{cargo, fetch, fingerprint, platform, stamp};
use crate::cli::args::{
    EngineArgs, EngineCommand, EngineInstallArgs, EnginePackageArgs, GlobalOpts,
};
use crate::error::{ErrorCode, KbError, Result};
use crate::output::CommandOutput;
use crate::versions::{BUILD_FINGERPRINT, ENGINE_VERSION, MANIFEST_PATH};

/// A KB checkout as far as engine selection needs it.
#[derive(Debug, Clone)]
pub struct Checkout {
    pub root: PathBuf,
    pub found_by: RootSource,
    pub engine_version: String,
    pub rust_toolchain: String,
}

impl Checkout {
    /// The checkout of this invocation (`--root`, `KB_ROOT`, `--host` or the current
    /// directory); its manifest is read leniently, since it may belong to another engine.
    pub fn locate(cwd: &Path, global: &GlobalOpts) -> Result<Checkout> {
        let env = resolve::env_root();
        let found = resolve::find(
            cwd,
            global.root.as_deref(),
            env.as_deref().map(Path::new),
            global.host.as_deref(),
        )
        .ok_or_else(|| {
            KbError::new(
                ErrorCode::ConfigInvalid,
                format!(
                    "no KB checkout here: neither {} nor a directory above it has {MANIFEST_PATH} or .kb/{MANIFEST_PATH}",
                    cwd.display()
                ),
            )
            .with_hint("run kb inside a KB checkout or a host repository that mounts one at .kb, or pass --root <dir>")
        })?;
        Checkout::at(found.dir, found.source)
    }

    pub fn at(dir: PathBuf, found_by: RootSource) -> Result<Checkout> {
        let manifest = dir.join(MANIFEST_PATH);
        let text = std::fs::read_to_string(&manifest).map_err(|e| {
            KbError::new(
                ErrorCode::ConfigInvalid,
                format!(
                    "cannot read {}: {e} (is {} a KB checkout?)",
                    manifest.display(),
                    dir.display()
                ),
            )
        })?;
        let table: toml::Table = toml::from_str(&text).map_err(|e| {
            KbError::new(
                ErrorCode::ConfigInvalid,
                format!("cannot parse {}: {e}", manifest.display()),
            )
        })?;
        let value = |key: &str, extra: &str| -> Result<String> {
            table
                .get(key)
                .and_then(toml::Value::as_str)
                .filter(|v| {
                    !v.is_empty()
                        && v.chars()
                            .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
                })
                .map(str::to_string)
                .ok_or_else(|| {
                    KbError::new(
                        ErrorCode::ConfigInvalid,
                        format!("{MANIFEST_PATH}: {key} is missing or malformed"),
                    )
                })
        };
        let root = crate::util::canonicalize(&dir).unwrap_or(dir);
        Ok(Checkout {
            engine_version: value("engine_version", ".+-")?,
            rust_toolchain: value("rust_toolchain", "._-")?,
            root,
            found_by,
        })
    }
}

/// Where the engine of a checkout comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Serving {
    /// The running `kb` is that engine.
    ThisBinary,
    /// A runtime in the store.
    Store(PathBuf),
    /// A runtime the retired `kbw` launcher built inside the checkout.
    Legacy(PathBuf),
    Missing,
}

pub fn serving(store: &Store, root: &Path, fp: &str) -> Serving {
    if fp == BUILD_FINGERPRINT {
        Serving::ThisBinary
    } else if let Some(p) = store.binary(fp) {
        Serving::Store(p)
    } else if let Some(p) = store::legacy_binary(root, fp) {
        Serving::Legacy(p)
    } else {
        Serving::Missing
    }
}

fn progress(global: &GlobalOpts, msg: impl AsRef<str>) {
    if !global.quiet {
        eprintln!("kb: {}", msg.as_ref());
    }
}

fn cwd() -> Result<PathBuf> {
    std::env::current_dir().map_err(|e| KbError::io("current directory", e))
}

fn fingerprint_of(root: &Path) -> Result<String> {
    stamp::fingerprint(root).map_err(|e| {
        KbError::new(
            ErrorCode::ConfigInvalid,
            format!(
                "cannot hash the engine build inputs under {}: {e} (missing Cargo.toml, Cargo.lock, rust-toolchain.toml, core/release.toml or core/cli?)",
                root.display()
            ),
        )
    })
}

pub fn run(global: &GlobalOpts, args: &EngineArgs) -> Result<CommandOutput> {
    match &args.command {
        EngineCommand::Info => info(global),
        EngineCommand::Fingerprint => {
            let co = Checkout::locate(&cwd()?, global)?;
            let fp = fingerprint_of(&co.root)?;
            Ok(CommandOutput::new(
                json!({"fingerprint": fp, "root": co.root}),
                format!("{fp}\n"),
            ))
        }
        EngineCommand::Bootstrap => {
            let co = Checkout::locate(&cwd()?, global)?;
            let fp = fingerprint_of(&co.root)?;
            let (status, binary) = ensure_runtime(global, &co, &fp)?;
            let dir = binary.parent().map(Path::to_path_buf).unwrap_or_default();
            Ok(CommandOutput::new(
                json!({"fingerprint": fp, "status": status, "runtime": dir, "binary": binary}),
                format!("{}\n", dir.display()),
            ))
        }
        EngineCommand::Install(a) => install(global, a),
        EngineCommand::Package(a) => package(global, a),
    }
}

fn info(global: &GlobalOpts) -> Result<CommandOutput> {
    let cwd = cwd()?;
    let store = Store::locate(&cwd)?;
    let exe = std::env::current_exe().ok();
    let mut text = format!(
        "kb {ENGINE_VERSION} ({}) build {}\n  command: {}\n  runtime store: {}\n",
        platform::BUILD_TARGET,
        short(BUILD_FINGERPRINT),
        exe.as_deref()
            .map_or_else(|| "(unknown)".into(), |p| p.display().to_string()),
        store.dir.display()
    );
    let mut result = json!({
        "kb": {
            "engine_version": ENGINE_VERSION,
            "build_fingerprint": BUILD_FINGERPRINT,
            "target": platform::BUILD_TARGET,
            "path": exe,
        },
        "store": store.dir,
        "checkout": Value::Null,
    });
    match Checkout::locate(&cwd, global) {
        Err(e) => text.push_str(&format!("  checkout: none ({})\n", e.message)),
        Ok(co) => {
            let fp = fingerprint_of(&co.root)?;
            let serving = serving(&store, &co.root, &fp);
            let (status, path) = match &serving {
                Serving::ThisBinary => ("this-binary", exe.clone()),
                Serving::Store(p) => ("store", Some(p.clone())),
                Serving::Legacy(p) => ("legacy", Some(p.clone())),
                Serving::Missing => ("missing", None),
            };
            let build_info = path
                .as_deref()
                .filter(|_| serving != Serving::ThisBinary)
                .and_then(|p| BuildInfo::read(&p.with_file_name("BUILD-INFO")));
            let source = build_info.as_ref().and_then(|b| b.get("source")).unwrap_or(
                if serving == Serving::ThisBinary {
                    "this-binary"
                } else {
                    "unknown"
                },
            );
            text.push_str(&format!(
                "  checkout: {} (found by {}), engine {}, fingerprint {}\n",
                co.root.display(),
                co.found_by.as_str(),
                co.engine_version,
                fp
            ));
            text.push_str(&match &path {
                None => "  runtime: missing; install it with `kb engine install` or build it with `kb engine bootstrap`\n".to_string(),
                Some(p) if serving == Serving::ThisBinary => format!("  runtime: this kb ({})\n", p.display()),
                Some(p) => format!("  runtime: {} ({source})\n", p.display()),
            });
            result["checkout"] = json!({
                "root": co.root,
                "found_by": co.found_by.as_str(),
                "engine_version": co.engine_version,
                "fingerprint": fp,
                "runtime": {"status": status, "path": path, "source": source},
            });
        }
    }
    Ok(CommandOutput::new(result, text))
}

fn short(fp: &str) -> &str {
    fp.get(..12).unwrap_or(fp)
}

/// Make sure a runtime serves `fp`, building it from source when none exists. Returns the
/// status (`this-binary`, `store`, `legacy`, `built`) and the binary.
pub fn ensure_runtime(
    global: &GlobalOpts,
    co: &Checkout,
    fp: &str,
) -> Result<(&'static str, PathBuf)> {
    let cwd = cwd()?;
    let store = Store::locate(&cwd)?;
    match serving(&store, &co.root, fp) {
        Serving::ThisBinary => {
            let exe = std::env::current_exe().map_err(|e| KbError::io("current executable", e))?;
            return Ok(("this-binary", exe));
        }
        Serving::Store(p) => return Ok(("store", p)),
        Serving::Legacy(p) => return Ok(("legacy", p)),
        Serving::Missing => {}
    }
    let fail = |msg: String| {
        KbError::new(
            ErrorCode::BootstrapFailed,
            format!("{msg}; any previously built runtime is unchanged"),
        )
    };
    let rustc = cargo::check_toolchain(&co.root, &co.rust_toolchain, global.quiet)?;
    let target_dir = cargo::target_dir(&cwd, &co.root);
    progress(
        global,
        format!(
            "building the engine of {} from source (engine {}, fingerprint {fp})",
            co.root.display(),
            co.engine_version
        ),
    );
    let bin = cargo::build(&co.root, fp, None, &target_dir)?;
    // An input edited while cargo ran may or may not be compiled in.
    let after = fingerprint::compute(&co.root)
        .map_err(|e| fail(format!("cannot hash the engine inputs: {e}")))?;
    if after != fp {
        return Err(fail(format!(
            "the engine build inputs changed during the build (fingerprint {fp} before, {after} after); bootstrap again once the edits are finished"
        )));
    }
    let staged = store.temp("build")?;
    let scratch = store.temp("smoke")?;
    let exe = staged.path().join(platform::EXE);
    std::fs::copy(&bin, &exe).map_err(|e| fail(format!("cannot stage the runtime: {e}")))?;
    let info = build_info(
        co,
        fp,
        if rustc.host.is_empty() {
            platform::BUILD_TARGET
        } else {
            &rustc.host
        },
        "source-build",
        &rustc,
    );
    std::fs::write(staged.path().join("BUILD-INFO"), info.render())
        .map_err(|e| fail(format!("cannot stage the runtime: {e}")))?;
    store::smoke_expect(&exe, scratch.path(), &co.engine_version, fp)
        .map_err(|e| fail(format!("the built binary failed the smoke test ({e})")))?;
    let dir = store.activate(staged.path(), fp, false)?;
    progress(global, format!("runtime ready: {}", dir.display()));
    Ok(("built", dir.join(platform::EXE)))
}

fn build_info(
    co: &Checkout,
    fp: &str,
    target: &str,
    source: &str,
    rustc: &cargo::Rustc,
) -> BuildInfo {
    let mut b = BuildInfo::default();
    b.set("engine_version", &co.engine_version);
    b.set("fingerprint", fp);
    b.set("target", target);
    b.set("source", source);
    b.set("rustc", &rustc.version_line);
    b.set("built_from", &cargo::built_from(&co.root));
    b
}

pub fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| KbError::io(path.display(), e))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| KbError::io(path.display(), e))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn invalid(msg: impl Into<String>) -> KbError {
    let mut msg = msg.into();
    msg.push_str("; the active runtime is unchanged");
    KbError::new(ErrorCode::ArtifactInvalid, msg)
}

/// Verify a downloaded or local release archive and return the extracted top directory:
/// the digest is checked before anything is read, then entries, the binary and BUILD-INFO
/// (`engine_version`, `target` and, when given, `fingerprint` must match).
pub fn verify_archive(
    archive: &Path,
    expected_sha256: &str,
    version: &str,
    target: &str,
    fingerprint: Option<&str>,
    into: &Path,
) -> Result<(PathBuf, BuildInfo, String)> {
    let actual = sha256_file(archive)?;
    if actual != expected_sha256 {
        return Err(invalid(format!(
            "sha256 mismatch: expected {expected_sha256}, got {actual} (nothing was extracted)"
        )));
    }
    let top = archive::top_dir(version, target);
    let dir = archive::extract(archive, &top, into).map_err(|e| invalid(e.message))?;
    let exe = platform::exe_for(target);
    if !std::fs::symlink_metadata(dir.join(exe)).is_ok_and(|m| m.is_file()) {
        return Err(invalid(format!(
            "{top}/{exe} is missing or not a regular file"
        )));
    }
    let info = BuildInfo::read(&dir.join("BUILD-INFO"))
        .filter(|_| std::fs::symlink_metadata(dir.join("BUILD-INFO")).is_ok_and(|m| m.is_file()))
        .ok_or_else(|| invalid(format!("{top}/BUILD-INFO is missing")))?;
    let mut wanted = vec![("engine_version", version), ("target", target)];
    if let Some(fp) = fingerprint {
        wanted.push(("fingerprint", fp));
    }
    for (key, want) in wanted {
        let got = info
            .get(key)
            .ok_or_else(|| invalid(format!("BUILD-INFO must contain exactly one {key}= line")))?;
        if got != want {
            let why = if key == "fingerprint" {
                " (the checkout's engine sources differ from that release: build them with `kb engine bootstrap`)"
            } else {
                ""
            };
            return Err(invalid(format!(
                "BUILD-INFO {key} is '{got}' but '{want}' is needed{why}"
            )));
        }
    }
    Ok((dir, info, actual))
}

fn install(global: &GlobalOpts, a: &EngineInstallArgs) -> Result<CommandOutput> {
    let cwd = cwd()?;
    let co = Checkout::locate(&cwd, global)?;
    let fp = fingerprint_of(&co.root)?;
    let targets = platform::release_targets();
    if targets.is_empty() {
        return Err(invalid(format!(
            "no release archives exist for this platform ({})",
            platform::BUILD_TARGET
        )));
    }
    let store = Store::locate(&cwd)?;
    let work = store.temp("install")?;
    let version = co.engine_version.as_str();
    let pinned = a.sha256.as_deref().map(fetch::check_digest).transpose()?;
    let (location, target, expected) = match &a.archive {
        Some(src) => {
            let name = fetch::source_name(src);
            let target = targets
                .iter()
                .copied()
                .find(|t| name == archive::file_name(version, t))
                .ok_or_else(|| {
                    invalid(format!(
                        "{name} is not a release archive of engine {version} for this platform (expected one of: {})",
                        targets.iter().map(|t| archive::file_name(version, t)).collect::<Vec<_>>().join(", ")
                    ))
                })?;
            let expected = match (&pinned, &a.sha256_file) {
                (Some(d), _) => d.clone(),
                (None, Some(sums)) => {
                    let text = std::fs::read_to_string(sums)
                        .map_err(|e| invalid(format!("checksum file {}: {e}", sums.display())))?;
                    fetch::digest_for(&text, name)?
                }
                (None, None) => {
                    return Err(KbError::new(
                        ErrorCode::Usage,
                        "an explicit archive needs --sha256 <hex> or --sha256-file <SHA256SUMS>",
                    ));
                }
            };
            (src.clone(), target, expected)
        }
        None => {
            let releases = fetch::releases();
            let sums_path = work.path().join(fetch::SUMS);
            fetch::fetch(
                &fetch::asset(&releases, Some(version), fetch::SUMS),
                &sums_path,
                global.quiet,
            )?;
            let sums = std::fs::read_to_string(&sums_path)
                .map_err(|e| KbError::io(sums_path.display(), e))?;
            let target = targets
                .iter()
                .copied()
                .find(|t| fetch::digest_for(&sums, &archive::file_name(version, t)).is_ok())
                .ok_or_else(|| {
                    invalid(format!(
                        "release v{version} has no archive for this platform ({})",
                        targets.join(", ")
                    ))
                })?;
            let name = archive::file_name(version, target);
            let expected = fetch::digest_for(&sums, &name)?;
            if let Some(p) = &pinned
                && p != &expected
            {
                return Err(invalid(format!(
                    "--sha256 {p} differs from the release's {expected} for {name}"
                )));
            }
            (
                fetch::asset(&releases, Some(version), &name),
                target,
                expected,
            )
        }
    };
    let archive_path = work.path().join("archive.tar.gz");
    fetch::fetch(&location, &archive_path, global.quiet)?;
    let x = work.path().join("x");
    std::fs::create_dir(&x).map_err(|e| KbError::io(x.display(), e))?;
    let (dir, mut info, actual) =
        verify_archive(&archive_path, &expected, version, target, Some(&fp), &x)?;
    info.set("source", "artifact");
    info.set("artifact_sha256", &actual);
    std::fs::write(dir.join("BUILD-INFO"), info.render())
        .map_err(|e| KbError::io(dir.display(), e))?;
    let exe = dir.join(platform::exe_for(target));
    if platform::exe_for(target) != platform::EXE {
        return Err(invalid(format!(
            "{target} binaries do not run on this platform"
        )));
    }
    let scratch = store.temp("smoke")?;
    store::smoke_expect(&exe, scratch.path(), version, &fp)
        .map_err(|e| invalid(format!("the archived kb failed the smoke test ({e})")))?;
    let runtime = store.activate(&dir, &fp, true)?;
    progress(
        global,
        format!(
            "installed {} (sha256 {actual}) as the runtime of {}",
            archive::top_dir(version, target),
            co.root.display()
        ),
    );
    Ok(CommandOutput::new(
        json!({"fingerprint": fp, "runtime": runtime, "archive": location, "sha256": actual, "target": target}),
        format!("{}\n", runtime.display()),
    ))
}

fn package(global: &GlobalOpts, a: &EnginePackageArgs) -> Result<CommandOutput> {
    let cwd = cwd()?;
    let co = Checkout::locate(&cwd, global)?;
    let fail = |msg: String| KbError::new(ErrorCode::PackageFailed, msg);
    let fp = fingerprint::compute(&co.root)
        .map_err(|e| fail(format!("cannot hash the engine inputs: {e}")))?;
    let rustc = cargo::check_toolchain(&co.root, &co.rust_toolchain, global.quiet)?;
    let target = a.target.clone().unwrap_or_else(|| {
        if rustc.host.is_empty() {
            platform::BUILD_TARGET.to_string()
        } else {
            rustc.host.clone()
        }
    });
    progress(
        global,
        format!(
            "building engine {} ({target}, fingerprint {fp})",
            co.engine_version
        ),
    );
    let bin = cargo::build(
        &co.root,
        &fp,
        a.target.as_deref(),
        &cargo::target_dir(&cwd, &co.root),
    )
    .map_err(|e| fail(e.message))?;
    let after = fingerprint::compute(&co.root)
        .map_err(|e| fail(format!("cannot hash the engine inputs: {e}")))?;
    if after != fp {
        return Err(fail(format!(
            "the engine build inputs changed during the build ({fp} before, {after} after)"
        )));
    }
    let out = resolve::absolutize(&cwd, &a.out_dir);
    std::fs::create_dir_all(&out)
        .map_err(|e| fail(format!("cannot create {}: {e}", out.display())))?;
    let work = TempDir::create_in(&out, ".kb-package")?;
    let scratch = TempDir::create_in(&out, ".kb-smoke")?;
    store::smoke_expect(&bin, scratch.path(), &co.engine_version, &fp)
        .map_err(|e| fail(format!("the built binary failed the smoke test ({e})")))?;
    let info_path = work.path().join("BUILD-INFO");
    std::fs::write(
        &info_path,
        build_info(&co, &fp, &target, "source-build", &rustc).render(),
    )
    .map_err(|e| fail(format!("cannot write BUILD-INFO: {e}")))?;
    let exe = platform::exe_for(&target);
    let mut members = vec![
        Member {
            name: exe,
            source: &bin,
            executable: true,
        },
        Member {
            name: "BUILD-INFO",
            source: &info_path,
            executable: false,
        },
    ];
    let docs: Vec<(&str, PathBuf)> = ["LICENSE", "README.md"]
        .into_iter()
        .map(|d| (d, co.root.join(d)))
        .filter(|(_, p)| p.is_file())
        .collect();
    for (name, path) in &docs {
        members.push(Member {
            name,
            source: path,
            executable: false,
        });
    }
    let file = archive::file_name(&co.engine_version, &target);
    let tmp = work.path().join(&file);
    archive::create(
        &tmp,
        &archive::top_dir(&co.engine_version, &target),
        &members,
    )?;
    let sum = sha256_file(&tmp)?;
    let dest = out.join(&file);
    std::fs::rename(&tmp, &dest)
        .map_err(|e| fail(format!("cannot write {}: {e}", dest.display())))?;
    let sum_path = out.join(format!("{file}.sha256"));
    crate::util::atomic_write(&sum_path, format!("{sum}  {file}\n").as_bytes())?;
    Ok(CommandOutput::new(
        json!({"archive": dest, "sha256_file": sum_path, "sha256": sum, "target": target, "fingerprint": fp}),
        format!("{}\n{}\n", dest.display(), sum_path.display()),
    ))
}
