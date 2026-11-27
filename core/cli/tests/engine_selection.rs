//! The `kb` command as the engine selector (docs/architecture.md, section 9; ADR 0017):
//! which engine serves a checkout, runtimes in the store, source bootstrap, release archive
//! installation, `kb self update`, the installers and the deprecated `kbw` forwarder.
//!
//! Checkouts are copies of this repository's engine build inputs. An exact copy has the
//! fingerprint of the `kb` under test; a copy with a marker comment is another engine. The
//! cases that need a fake toolchain or a shell run on Unix only; the others run everywhere.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{Sandbox, isolated_env, kb_bin, repo_root, stderr, stdout};
use kb::launcher::{archive, fingerprint, platform};
use kb::versions::{BUILD_FINGERPRINT, ENGINE_VERSION};
use serde_json::Value;

/// A copy of this repository's engine build inputs at `dir`; with `marker`, one source file
/// gets a comment, which makes it another engine.
fn checkout(dir: &Path, marker: Option<&str>) {
    let src = repo_root();
    for rel in fingerprint::input_paths(&src).unwrap() {
        let dst = dir.join(&rel);
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::copy(src.join(&rel), dst).unwrap();
    }
    if let Some(m) = marker {
        let lib = dir.join("core/cli/src/lib.rs");
        let text = fs::read_to_string(&lib).unwrap();
        fs::write(&lib, format!("{text}\n// {m}\n")).unwrap();
    }
}

fn kb_at(sb: &Sandbox, cwd: &Path, args: &[&str], envs: &[(&str, &Path)]) -> Output {
    let mut c = Command::new(kb_bin());
    c.current_dir(cwd).args(args);
    isolated_env(&mut c, &sb.home());
    for (k, v) in envs {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn envelope(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON envelope ({e})\n{}", show(o)))
}

fn show(o: &Output) -> String {
    format!(
        "exit {:?}\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        stdout(o),
        stderr(o)
    )
}

fn assert_exit(o: &Output, code: i32) {
    assert_eq!(o.status.code(), Some(code), "{}", show(o));
}

fn store(sb: &Sandbox) -> PathBuf {
    sb.home().join("kb-runtime")
}

fn engine_fingerprint(sb: &Sandbox, root: &Path) -> String {
    let o = kb_at(sb, root, &["engine", "fingerprint"], &[]);
    assert_exit(&o, 0);
    stdout(&o).trim().to_string()
}

/// A release archive of the `kb` under test with this `BUILD-INFO`; returns its path.
fn release_archive(dir: &Path, version: &str, target: &str, fp: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let info = dir.join("BUILD-INFO.src");
    fs::write(
        &info,
        format!(
            "engine_version={version}\nfingerprint={fp}\ntarget={target}\nsource=source-build\n"
        ),
    )
    .unwrap();
    let out = dir.join(archive::file_name(version, target));
    archive::create(
        &out,
        &archive::top_dir(version, target),
        &[
            archive::Member {
                name: platform::exe_for(target),
                source: &kb_bin(),
                executable: true,
            },
            archive::Member {
                name: "BUILD-INFO",
                source: &info,
                executable: false,
            },
        ],
    )
    .unwrap();
    fs::remove_file(info).unwrap();
    out
}

fn sha256(path: &Path) -> String {
    kb::util::sha256_hex(&fs::read(path).unwrap())
}

/// A local release location (`KB_RELEASES_URL`) with `archive` as release `version` and as
/// the latest release.
fn releases(dir: &Path, version: &str, archive: &Path) -> PathBuf {
    let name = archive.file_name().unwrap().to_str().unwrap();
    for sub in [
        format!("download/v{version}"),
        "latest/download".to_string(),
    ] {
        let d = dir.join(sub);
        fs::create_dir_all(&d).unwrap();
        fs::copy(archive, d.join(name)).unwrap();
        fs::write(
            d.join("SHA256SUMS"),
            format!("{}  {name}\n", sha256(archive)),
        )
        .unwrap();
    }
    dir.to_path_buf()
}

fn host_target() -> &'static str {
    platform::release_targets()
        .first()
        .copied()
        .expect("tests need a platform with release archives")
}

#[test]
fn kb_serves_a_checkout_of_its_own_engine_in_process_from_anywhere_in_a_host() {
    let sb = Sandbox::new();
    let host = sb.path().join("host");
    let kb = host.join(".kb");
    checkout(&kb, None);
    let deep = host.join("app/src");
    fs::create_dir_all(&deep).unwrap();
    assert_eq!(engine_fingerprint(&sb, &kb), BUILD_FINGERPRINT);

    let o = kb_at(&sb, &deep, &["--json", "engine", "info"], &[]);
    assert_exit(&o, 0);
    let co = &envelope(&o)["result"]["checkout"];
    assert_eq!(co["found_by"], "mount", "{co:#}");
    assert_eq!(co["fingerprint"], BUILD_FINGERPRINT);
    assert_eq!(co["runtime"]["status"], "this-binary", "{co:#}");
    let o = kb_at(&sb, &deep, &["--json", "version"], &[]);
    assert_exit(&o, 0);
    assert_eq!(
        envelope(&o)["result"]["build_fingerprint"],
        BUILD_FINGERPRINT
    );
    // Nothing was built or installed; the warm-path stamp lives in the checkout's cache.
    assert!(!store(&sb).exists());
    assert!(kb.join(".cache/engine-stamp").is_file());
}

#[test]
fn a_missing_runtime_is_an_error_that_names_how_to_get_one() {
    let sb = Sandbox::new();
    let host = sb.path().join("host");
    let kb = host.join(".kb");
    checkout(&kb, Some("another engine"));
    let fp = engine_fingerprint(&sb, &kb);
    assert_ne!(fp, BUILD_FINGERPRINT);

    let o = kb_at(&sb, &host, &["--json", "validate"], &[]);
    assert_exit(&o, 54);
    let v = envelope(&o);
    assert_eq!(v["command"], "validate");
    assert_eq!(v["error"]["code"], "RUNTIME_NOT_BOOTSTRAPPED");
    assert_eq!(v["error"]["details"]["fingerprint"], fp.as_str());
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("kb engine bootstrap"), "{hint}");
    // Text mode: the error on stderr, nothing on stdout.
    let o = kb_at(&sb, &host, &["validate"], &[]);
    assert_exit(&o, 54);
    assert!(stdout(&o).is_empty());
    assert!(
        stderr(&o).contains("error[RUNTIME_NOT_BOOTSTRAPPED]"),
        "{}",
        show(&o)
    );
    // `kb engine ...` and top-level help/version still work: the running kb handles them.
    let o = kb_at(&sb, &host, &["--json", "engine", "info"], &[]);
    assert_exit(&o, 0);
    assert_eq!(
        envelope(&o)["result"]["checkout"]["runtime"]["status"],
        "missing"
    );
    assert_exit(&kb_at(&sb, &host, &["--version"], &[]), 0);
    assert_exit(&kb_at(&sb, &host, &["--help"], &[]), 0);
    assert!(!store(&sb).exists(), "normal commands never build");
}

#[test]
fn a_runtime_that_reports_another_build_is_refused_instead_of_selected_again() {
    let sb = Sandbox::new();
    let root = sb.path().join("kb");
    checkout(&root, Some("another engine"));
    let fp = engine_fingerprint(&sb, &root);
    // A wrong binary in the store under that fingerprint: the kb under test itself.
    let dir = store(&sb).join(&fp);
    fs::create_dir_all(&dir).unwrap();
    fs::copy(kb_bin(), dir.join(platform::EXE)).unwrap();
    let o = kb_at(&sb, &root, &["--json", "version"], &[]);
    assert_exit(&o, 50);
    let v = envelope(&o);
    assert_eq!(v["error"]["code"], "RUNTIME_INCOMPATIBLE", "{v:#}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&format!("selected for engine fingerprint {fp}")),
        "{v:#}"
    );
}

#[test]
fn engine_install_verifies_release_archives_before_activating_them() {
    let sb = Sandbox::new();
    let root = sb.path().join("kb");
    checkout(&root, None);
    let fp = engine_fingerprint(&sb, &root);
    let target = host_target();
    let good = release_archive(&sb.path().join("dist"), ENGINE_VERSION, target, &fp);
    let digest = sha256(&good);
    let install = |args: &[&str], envs: &[(&str, &Path)]| {
        let mut all = vec!["--json", "engine", "install"];
        all.extend_from_slice(args);
        kb_at(&sb, &root, &all, envs)
    };
    let s = |p: &Path| p.to_str().unwrap().to_string();

    // An explicit archive needs an explicit digest.
    assert_exit(&install(&[&s(&good)], &[]), 2);
    // A wrong digest: nothing is extracted or activated.
    let o = install(&[&s(&good), "--sha256", &"0".repeat(64)], &[]);
    assert_exit(&o, 57);
    assert!(
        envelope(&o)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("sha256 mismatch")
    );
    // BUILD-INFO for another engine.
    let other = release_archive(
        &sb.path().join("other"),
        ENGINE_VERSION,
        target,
        &"f".repeat(64),
    );
    let o = install(&[&s(&other), "--sha256", &sha256(&other)], &[]);
    assert_exit(&o, 57);
    let msg = envelope(&o)["error"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        msg.contains("fingerprint") && msg.contains("kb engine bootstrap"),
        "{msg}"
    );
    // An archive of another engine version (by its name).
    let old = release_archive(&sb.path().join("old"), "0.0.1", target, &fp);
    assert_exit(&install(&[&s(&old), "--sha256", &sha256(&old)], &[]), 57);
    assert!(!store(&sb).join(&fp).exists());

    // A verified archive becomes the runtime of the checkout's fingerprint.
    let o = install(&[&s(&good), "--sha256", &digest], &[]);
    assert_exit(&o, 0);
    let runtime = store(&sb).join(&fp);
    assert_eq!(envelope(&o)["result"]["runtime"], s(&runtime));
    assert!(runtime.join(platform::EXE).is_file());
    let info = fs::read_to_string(runtime.join("BUILD-INFO")).unwrap();
    assert!(info.contains("\nsource=artifact\n"), "{info}");
    assert!(
        info.contains(&format!("\nartifact_sha256={digest}\n")),
        "{info}"
    );

    // Without an archive: the release of the checkout's engine version, by SHA256SUMS.
    fs::remove_dir_all(&runtime).unwrap();
    let rel = releases(&sb.path().join("releases"), ENGINE_VERSION, &good);
    let o = install(&[], &[("KB_RELEASES_URL", &rel)]);
    assert_exit(&o, 0);
    assert!(runtime.join(platform::EXE).is_file());
    // A tampered checksum file is refused.
    fs::write(
        rel.join(format!("download/v{ENGINE_VERSION}/SHA256SUMS")),
        format!(
            "{}  {}\n",
            "1".repeat(64),
            archive::file_name(ENGINE_VERSION, target)
        ),
    )
    .unwrap();
    assert_exit(&install(&[], &[("KB_RELEASES_URL", &rel)]), 57);
    let leftovers: Vec<String> = fs::read_dir(store(&sb))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(leftovers, vec![fp], "temporary directories are removed");
}

#[test]
fn self_update_replaces_the_installed_binary_after_verifying_the_release() {
    let sb = Sandbox::new();
    let bin = sb.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let installed = bin.join(platform::EXE);
    fs::copy(kb_bin(), &installed).unwrap();
    let target = host_target();
    let good = release_archive(
        &sb.path().join("dist"),
        ENGINE_VERSION,
        target,
        BUILD_FINGERPRINT,
    );
    let rel = releases(&sb.path().join("releases"), ENGINE_VERSION, &good);
    let run = |args: &[&str]| {
        let mut c = Command::new(&installed);
        c.current_dir(sb.path()).arg("--json").args(args);
        isolated_env(&mut c, &sb.home());
        c.env("KB_RELEASES_URL", &rel).output().unwrap()
    };

    let o = run(&["self", "update", "--check"]);
    assert_exit(&o, 0);
    assert_eq!(envelope(&o)["result"]["status"], "up-to-date");
    let o = run(&["self", "update"]);
    assert_exit(&o, 0);
    assert_eq!(envelope(&o)["result"]["status"], "up-to-date");

    // A tampered digest: the installed binary is unchanged.
    let sums = rel.join("latest/download/SHA256SUMS");
    let honest = fs::read_to_string(&sums).unwrap();
    fs::write(&sums, honest.replacen(&sha256(&good), &"2".repeat(64), 1)).unwrap();
    let before = fs::read(&installed).unwrap();
    let o = run(&["self", "update", "--force"]);
    assert_exit(&o, 57);
    assert_eq!(fs::read(&installed).unwrap(), before);
    fs::write(&sums, honest).unwrap();

    // Forced reinstall of the verified release, by rename (on Windows the running
    // executable is moved aside first).
    let o = run(&["self", "update", "--force"]);
    assert_exit(&o, 0);
    assert_eq!(envelope(&o)["result"]["status"], "updated");
    let o = Command::new(&installed).arg("--version").output().unwrap();
    assert_exit(&o, 0);
    assert_eq!(stdout(&o).trim(), format!("kb {ENGINE_VERSION}"));
    // A release that does not exist.
    assert_exit(&run(&["self", "update", "--version", "99.0.0"]), 66);
    let names: Vec<String> = fs::read_dir(&bin)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| !n.starts_with(".kb") || !cfg!(windows))
        .collect();
    assert_eq!(
        names,
        vec![platform::EXE.to_string()],
        "no staging files are left"
    );
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn executable(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A stand-in engine binary: answers the smoke test with `version` and `fp`, prints its
    /// arguments and KB_ROOT for anything else and exits 7 on `fail7`.
    fn fake_engine(label: &str, version: &str, fp: &str) -> String {
        format!(
            "#!/bin/sh\ncase \" $* \" in\n*' version '*) printf '{{\"ok\": true, \"result\": {{\"engine_version\": \"{version}\", \"build_fingerprint\": \"{fp}\"}}}}\\n'; exit 0 ;;\n*' fail7 '*) exit 7 ;;\nesac\necho \"{label} args=$* root=$KB_ROOT selected=${{KB_SELECTED_FINGERPRINT:-}}\"\n"
        )
    }

    fn path_with(dir: &Path) -> std::ffi::OsString {
        std::env::join_paths(
            std::iter::once(dir.to_path_buf())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap()
    }

    #[test]
    fn the_selected_runtime_runs_in_place_and_kbw_era_runtimes_are_reused() {
        let sb = Sandbox::new();
        let host = sb.path().join("host");
        let root = host.join(".kb");
        checkout(&root, Some("another engine"));
        let fp = engine_fingerprint(&sb, &root);
        // A runtime that the retired kbw launcher built inside the checkout.
        executable(
            &root.join(".cache/runtime").join(&fp).join("kb"),
            &fake_engine("legacy", ENGINE_VERSION, &fp),
        );
        let o = kb_at(&sb, &host, &["context", "--task", "two words"], &[]);
        assert_exit(&o, 0);
        assert_eq!(
            stdout(&o).trim(),
            format!(
                "legacy args=context --task two words root={} selected={fp}",
                root.display()
            )
        );
        // The store wins over the legacy location; exit statuses pass through.
        executable(
            &store(&sb).join(&fp).join("kb"),
            &fake_engine("store", ENGINE_VERSION, &fp),
        );
        let o = kb_at(&sb, &host, &["x"], &[]);
        assert!(stdout(&o).starts_with("store args=x"), "{}", show(&o));
        assert_exit(&kb_at(&sb, &host, &["fail7"], &[]), 7);
    }

    /// Toolchain stand-ins: rustc reports `release`; cargo writes a fake engine reporting
    /// the fingerprint it was asked to compile in, or fails / edits a source when told so
    /// through the marker files next to it.
    fn toolchain(sb: &Sandbox, release: &str) -> PathBuf {
        let bin = sb.path().join("toolchain");
        executable(
            &bin.join("rustc"),
            &format!(
                "#!/bin/sh\nprintf 'rustc {release} (fake)\\nrelease: {release}\\nhost: x-fake\\n'\n"
            ),
        );
        let engine = fake_engine("built", ENGINE_VERSION, "$KBW_BUILD_FINGERPRINT")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace("\\$KBW_BUILD_FINGERPRINT", "$KBW_BUILD_FINGERPRINT");
        executable(
            &bin.join("cargo"),
            &format!(
                "#!/bin/sh\nd=$(dirname \"$0\")\necho run >> \"$d/runs\"\n\
                 if [ -f \"$d/fail\" ]; then echo 'fake build failure' >&2; exit 101; fi\n\
                 if [ -f \"$d/edit\" ]; then echo '// edited' >> core/cli/src/lib.rs; fi\n\
                 mkdir -p \"$CARGO_TARGET_DIR/release\"\n\
                 printf '%s' \"{engine}\" > \"$CARGO_TARGET_DIR/release/kb\"\n\
                 chmod 755 \"$CARGO_TARGET_DIR/release/kb\"\n"
            ),
        );
        bin
    }

    fn runs(bin: &Path) -> usize {
        fs::read_to_string(bin.join("runs")).map_or(0, |t| t.lines().count())
    }

    #[test]
    fn bootstrap_builds_with_the_pinned_toolchain_and_never_damages_existing_runtimes() {
        let sb = Sandbox::new();
        let root = sb.path().join("kb");
        checkout(&root, Some("another engine"));
        let pinned = kb::versions::ReleaseManifest::parse(
            &fs::read_to_string(root.join("core/release.toml")).unwrap(),
        )
        .unwrap()
        .rust_toolchain;
        let bin = toolchain(&sb, "0.0.1");
        let path = path_with(&bin);
        let run = |args: &[&str], extra: &[(&str, &str)]| {
            let mut c = Command::new(kb_bin());
            c.current_dir(&root).args(args);
            isolated_env(&mut c, &sb.home());
            c.env("PATH", &path);
            for (k, v) in extra {
                c.env(k, v);
            }
            c.output().unwrap()
        };

        // rustc is not the pinned toolchain: refused unless explicitly allowed.
        let o = run(&["--json", "engine", "bootstrap"], &[]);
        assert_exit(&o, 56);
        assert_eq!(envelope(&o)["error"]["code"], "TOOLCHAIN_MISMATCH");
        assert_eq!(runs(&bin), 0);
        let bin = toolchain(&sb, &pinned);

        // The build, the smoke test and the activation.
        let fp = engine_fingerprint(&sb, &root);
        let o = run(&["engine", "bootstrap"], &[]);
        assert_exit(&o, 0);
        let runtime = store(&sb).join(&fp);
        assert_eq!(stdout(&o).trim(), runtime.to_str().unwrap());
        let info = fs::read_to_string(runtime.join("BUILD-INFO")).unwrap();
        assert!(info.contains(&format!("\nfingerprint={fp}\n")), "{info}");
        assert!(info.contains("\nsource=source-build\n"), "{info}");
        assert!(
            info.contains(&format!("\nrustc=rustc {pinned} (fake)\n")),
            "{info}"
        );
        // Selected from now on; a second bootstrap builds nothing.
        let o = run(&["anything"], &[]);
        assert!(
            stdout(&o).starts_with("built args=anything"),
            "{}",
            show(&o)
        );
        assert_exit(&run(&["engine", "bootstrap"], &[]), 0);
        assert_eq!(runs(&bin), 1);

        // An engine edit needs a new runtime; a failed build leaves the old one untouched.
        let lib = root.join("core/cli/src/lib.rs");
        fs::write(&lib, fs::read_to_string(&lib).unwrap() + "// second edit\n").unwrap();
        let before = fs::read(runtime.join("kb")).unwrap();
        assert_exit(&run(&["version"], &[]), 54);
        fs::write(bin.join("fail"), "").unwrap();
        let o = run(&["--json", "engine", "bootstrap"], &[]);
        assert_exit(&o, 55);
        assert!(stderr(&o).contains("fake build failure"), "{}", show(&o));
        // KB_AUTO_BOOTSTRAP=1 builds implicitly, here into the same failure.
        let o = run(&["version"], &[("KB_AUTO_BOOTSTRAP", "1")]);
        assert_exit(&o, 55);
        assert_eq!(fs::read(runtime.join("kb")).unwrap(), before);
        fs::remove_file(bin.join("fail")).unwrap();
        // Inputs edited while cargo runs: the binary is labelled with neither fingerprint.
        fs::write(bin.join("edit"), "").unwrap();
        let o = run(&["--json", "engine", "bootstrap"], &[]);
        assert_exit(&o, 55);
        assert!(
            stderr(&o).contains("changed during the build"),
            "{}",
            show(&o)
        );
        fs::remove_file(bin.join("edit")).unwrap();
        let o = run(&["probe"], &[("KB_AUTO_BOOTSTRAP", "1")]);
        assert_exit(&o, 0);
        assert!(stdout(&o).contains("built args=probe"), "{}", show(&o));
    }

    #[test]
    fn kbw_forwards_to_kb_for_its_checkout() {
        let sb = Sandbox::new();
        let root = sb.path().join("kb");
        fs::create_dir_all(&root).unwrap();
        fs::copy(repo_root().join("kbw"), root.join("kbw")).unwrap();
        let bin = sb.path().join("bin");
        let log = sb.path().join("kb.log");
        executable(
            &bin.join("kb"),
            &format!(
                "#!/bin/sh\necho \"args=$* root=$KB_ROOT auto=${{KB_AUTO_BOOTSTRAP:-}}\" >> '{}'\n",
                log.display()
            ),
        );
        let kbw = |args: &[&str], envs: &[(&str, &str)], path: &Path| {
            let mut c = Command::new("sh");
            c.current_dir(sb.path()).arg(root.join("kbw")).args(args);
            isolated_env(&mut c, &sb.home());
            c.env("PATH", path_with(path));
            for (k, v) in envs {
                c.env(k, v);
            }
            c.output().unwrap()
        };
        let canonical = kb::util::canonicalize(&root).unwrap();
        assert_exit(&kbw(&["--kbw-bootstrap"], &[], &bin), 0);
        assert_exit(
            &kbw(
                &["validate", "--strict"],
                &[("KBW_AUTO_BOOTSTRAP", "1")],
                &bin,
            ),
            0,
        );
        assert_exit(
            &kbw(
                &["--kbw-install-artifact", "a.tar.gz", "--sha256", "x"],
                &[],
                &bin,
            ),
            0,
        );
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            format!(
                "args=engine bootstrap root={0} auto=\nargs=validate --strict root={0} auto=1\n\
                 args=engine install a.tar.gz --sha256 x root={0} auto=\n",
                canonical.display()
            )
        );
        assert_exit(&kbw(&["--kbw-bogus"], &[], &bin), 2);
        // A kb on PATH that runs kbw again (a wrapper) is a loop, not a hang.
        let looping = sb.path().join("looping");
        executable(
            &looping.join("kb"),
            &format!(
                "#!/bin/sh\nexec sh '{}' \"$@\"\n",
                root.join("kbw").display()
            ),
        );
        let o = kbw(&["version"], &[], &looping);
        assert_exit(&o, 2);
        assert!(stderr(&o).contains("KBW_LOOP"), "{}", show(&o));
        // Without kb on PATH it says how to get it.
        let mut c = Command::new("/bin/sh");
        c.current_dir(sb.path())
            .arg(root.join("kbw"))
            .arg("version");
        isolated_env(&mut c, &sb.home());
        let o = c.env("PATH", "/usr/bin:/bin").output().unwrap();
        assert_exit(&o, 50);
        assert!(stderr(&o).contains("KB_NOT_INSTALLED"), "{}", show(&o));
    }

    #[test]
    fn install_sh_installs_the_release_and_never_replaces_a_foreign_kb_unasked() {
        let sb = Sandbox::new();
        let target = host_target();
        let good = release_archive(
            &sb.path().join("dist"),
            ENGINE_VERSION,
            target,
            BUILD_FINGERPRINT,
        );
        let rel = releases(&sb.path().join("releases"), ENGINE_VERSION, &good);
        let dir = sb.path().join("bin");
        let install = |args: &[&str]| {
            let mut c = Command::new("sh");
            c.current_dir(sb.path())
                .arg(repo_root().join("install.sh"))
                .args(["--dir", dir.to_str().unwrap()])
                .args(args);
            isolated_env(&mut c, &sb.home());
            c.env("KB_RELEASES_URL", &rel).output().unwrap()
        };
        let o = install(&[]);
        assert_exit(&o, 0);
        let o = Command::new(dir.join("kb"))
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(stdout(&o).trim(), format!("kb {ENGINE_VERSION}"));
        assert_exit(&install(&["--version", ENGINE_VERSION]), 0);
        // A wrapper script named kb is not replaced without --force.
        executable(&dir.join("kb"), "#!/bin/sh\necho kb 9.9.9\n");
        let o = install(&[]);
        assert_exit(&o, 1);
        assert!(stderr(&o).contains("not a kb binary"), "{}", show(&o));
        assert_exit(&install(&["--force"]), 0);
        assert_eq!(
            fs::read(dir.join("kb")).unwrap(),
            fs::read(kb_bin()).unwrap()
        );
        // A tampered checksum is refused.
        let sums = rel.join("latest/download/SHA256SUMS");
        let text = fs::read_to_string(&sums).unwrap();
        fs::write(&sums, text.replacen(&sha256(&good), &"3".repeat(64), 1)).unwrap();
        let o = install(&[]);
        assert_exit(&o, 1);
        assert!(stderr(&o).contains("sha256 mismatch"), "{}", show(&o));
    }
}
