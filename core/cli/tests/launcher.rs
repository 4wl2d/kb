//! End-to-end tests of the `kbw` launcher (docs/architecture.md, section 9).
//!
//! Every test runs the real `kbw` script inside a temporary copy of the engine files with
//! an isolated HOME. Most scenarios place a small shell-script runtime in the cache so the
//! launcher logic (fingerprint, warm path, invalidation, packaging, artifact verification)
//! is exercised quickly and deterministically; `cargo`/`rustc` are replaced by fakes that
//! record any invocation. One test performs a real source bootstrap with the pinned
//! toolchain, sharing compiled dependencies through a directory under Cargo's test tmpdir.
mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use common::{Sandbox, stderr, stdout};
use kb::util::sha256_hex;
use kb::versions::ReleaseManifest;

/// Exit status of every launcher runtime/bootstrap/artifact error.
const RUNTIME_EXIT: i32 = 50;

fn manifest() -> ReleaseManifest {
    let text = fs::read_to_string(common::repo_root().join("core/release.toml")).unwrap();
    ReleaseManifest::parse(&text).unwrap()
}

fn engine_version() -> String {
    manifest().engine_version
}

/// Cargo target directory shared by source builds across test runs.
fn shared_target_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("kbw-cargo-target")
}

fn real_home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is set"))
}

/// The registry cache of the real user (tests isolate HOME but must build offline).
fn cargo_home() -> PathBuf {
    std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| real_home().join(".cargo"))
}

fn rustup_home() -> Option<PathBuf> {
    std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(real_home().join(".rustup")).filter(|p| p.is_dir()))
}

/// Directory holding the cargo (and rustc) that builds these tests.
fn real_toolchain_dir() -> PathBuf {
    Path::new(env!("CARGO")).parent().unwrap().to_path_buf()
}

fn write_executable(path: &Path, text: &str) {
    common::write(path, text);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    let mut entries: Vec<_> = fs::read_dir(src).unwrap().map(|e| e.unwrap()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let ty = e.file_type().unwrap();
        let name = e.file_name();
        if ty.is_dir() && name != "target" && name != ".cache" {
            copy_tree(&e.path(), &dst.join(&name));
        } else if ty.is_file() {
            fs::copy(e.path(), dst.join(&name)).unwrap();
        }
    }
}

/// The pretty-printed `kb --json version` envelope of an engine built for `fp`.
fn version_json(version: &str, fp: &str) -> String {
    format!(
        "{{\n  \"command\": \"version\",\n  \"ok\": true,\n  \"result\": {{\n    \
         \"build_fingerprint\": \"{fp}\",\n    \"engine_version\": \"{version}\"\n  }}\n}}\n"
    )
}

/// A shell-script runtime built for `fp`: `version` prints the engine version and
/// `--json version` also the build fingerprint; `fail7` exits 7; anything else reports its
/// label, the exported launcher variables and its arguments.
fn fake_kb(label: &str, fp: &str) -> String {
    format!(
        "#!/bin/sh\n\
         if [ \"${{1-}}\" = version ]; then echo \"kb {v} ({label})\"; exit 0; fi\n\
         if [ \"${{1-}}\" = --json ] && [ \"${{2-}}\" = version ]; then cat <<'EOF'\n\
         {json}EOF\n\
         exit 0; fi\n\
         if [ \"${{1-}}\" = fail7 ]; then exit 7; fi\n\
         echo \"label={label}\"\n\
         echo \"KB_ROOT=${{KB_ROOT-}}\"\n\
         echo \"KBW_FINGERPRINT=${{KBW_FINGERPRINT-}}\"\n\
         for a in \"$@\"; do echo \"arg=$a\"; done\n",
        v = engine_version(),
        json = version_json(&engine_version(), fp)
    )
}

fn build_info(version: &str, fp: &str, target: &str, source: &str) -> String {
    format!(
        "engine_version={version}\nfingerprint={fp}\ntarget={target}\nsource={source}\n\
         rustc=rustc test\nbuilt_from=unknown\n"
    )
}

/// Map of `.cache/runtime/<fp>` file names to their sha256 (to prove a runtime is unchanged).
fn dir_digest(dir: &Path) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().unwrap().is_file())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (name, sha256_hex(&fs::read(e.path()).unwrap()))
        })
        .collect();
    out.sort();
    out
}

fn assert_ok(o: &Output) {
    assert!(
        o.status.success(),
        "kbw failed ({:?})\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        stdout(o),
        stderr(o)
    );
}

fn assert_kbw_error(o: &Output, code: &str, needle: &str) {
    assert_eq!(
        o.status.code(),
        Some(RUNTIME_EXIT),
        "expected exit {RUNTIME_EXIT}\nstdout:\n{}\nstderr:\n{}",
        stdout(o),
        stderr(o)
    );
    let err = stderr(o);
    assert!(
        err.contains(&format!("kbw: error[{code}]")) && err.contains(needle),
        "expected error[{code}] mentioning {needle:?}, got:\n{err}"
    );
}

/// Value of `key=` in `key=value` lines.
fn kv<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

/// A temporary KB checkout containing a copy of the engine files, with fake `cargo`
/// (records a marker, exits 99) and fake `rustc` (reports the pinned version).
struct Kb {
    sb: Sandbox,
    root: PathBuf,
    fake_bin: PathBuf,
    marker: PathBuf,
}

impl Kb {
    fn new() -> Kb {
        let sb = Sandbox::new();
        let root = sb.path().join("kb");
        let src = common::repo_root();
        for f in ["kbw", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml"] {
            let dst = root.join(f);
            fs::create_dir_all(dst.parent().unwrap()).unwrap();
            fs::copy(src.join(f), dst).unwrap();
        }
        copy_tree(&src.join("core"), &root.join("core"));
        let fake_bin = sb.path().join("fake-bin");
        let marker = sb.path().join("cargo-invoked");
        write_executable(
            &fake_bin.join("cargo"),
            &format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\nexit 99\n",
                marker.display()
            ),
        );
        Self::set_fake_rustc(&fake_bin, &manifest().rust_toolchain);
        Kb {
            sb,
            root,
            fake_bin,
            marker,
        }
    }

    fn set_fake_rustc(fake_bin: &Path, version: &str) {
        write_executable(
            &fake_bin.join("rustc"),
            &format!("#!/bin/sh\necho 'rustc {version} (fake)'\n"),
        );
    }

    /// Replace the fake cargo with one that "builds" a script runtime labelled `label` into
    /// `$CARGO_TARGET_DIR/release/kb` after running the shell command `during`. The binary
    /// reports the fingerprint kbw passed in KBW_BUILD_FINGERPRINT, or `reported` (a binary
    /// left over from another build). Callers point KBW_CARGO_TARGET_DIR at
    /// `private_target()` so the shared real target directory is never touched.
    fn use_building_cargo(&self, label: &str, reported: Option<&str>, during: &str) {
        let template = self.sb.path().join("kb.template");
        common::write(&template, &fake_kb(label, "@FP@"));
        let fp = reported.unwrap_or("${KBW_BUILD_FINGERPRINT:-unknown}");
        write_executable(
            &self.fake_bin.join("cargo"),
            &format!(
                "#!/bin/sh\n\
                 echo \"$* KBW_BUILD_FINGERPRINT=${{KBW_BUILD_FINGERPRINT-}}\" >> '{marker}'\n\
                 {during}\n\
                 mkdir -p \"$CARGO_TARGET_DIR/release\" || exit 1\n\
                 sed \"s/@FP@/{fp}/g\" '{template}' > \"$CARGO_TARGET_DIR/release/kb\" || exit 1\n\
                 chmod 755 \"$CARGO_TARGET_DIR/release/kb\"\n",
                marker = self.marker.display(),
                template = template.display()
            ),
        );
    }

    fn private_target(&self) -> String {
        self.sb.path().join("cargo-target").display().to_string()
    }

    /// A `kbw` command whose PATH starts with `tools` (fake or real toolchain).
    fn command(&self, shell: Option<&str>, tools: &Path) -> Command {
        let kbw = self.root.join("kbw");
        let mut c = match shell {
            Some(sh) => {
                let mut c = Command::new(sh);
                c.arg(&kbw);
                c
            }
            None => Command::new(&kbw),
        };
        self.configure(&mut c, tools);
        c
    }

    /// Isolated environment for a launcher process whose PATH starts with `tools`.
    fn configure(&self, c: &mut Command, tools: &Path) {
        common::isolated_env(c, &self.sb.home());
        let mut path = std::ffi::OsString::from(tools);
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        c.current_dir(self.sb.path())
            .env("PATH", path)
            .env("CARGO_HOME", cargo_home())
            .env("CARGO_NET_OFFLINE", "true")
            .env("KBW_CARGO_TARGET_DIR", shared_target_dir())
            .env_remove("KBW_FINGERPRINT")
            .env_remove("KBW_ALLOW_TOOLCHAIN_MISMATCH")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET");
        if let Some(r) = rustup_home() {
            c.env("RUSTUP_HOME", r);
        }
    }

    fn kbw(&self, args: &[&str]) -> Output {
        self.kbw_env(args, &[])
    }

    fn kbw_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut c = self.command(None, &self.fake_bin);
        c.args(args);
        for (k, v) in envs {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    fn fingerprint(&self) -> String {
        let o = self.kbw(&["--kbw-fingerprint"]);
        assert_ok(&o);
        stdout(&o).trim().to_string()
    }

    fn target(&self) -> String {
        let o = self.kbw(&["--kbw-runtime-info"]);
        assert_ok(&o);
        let t = kv(&stdout(&o), "target").unwrap().to_string();
        assert_ne!(t, "unknown", "tests need a supported host platform");
        t
    }

    fn runtime_dir(&self, fp: &str) -> PathBuf {
        self.root.join(".cache/runtime").join(fp)
    }

    /// Place a script runtime for `fp` as if it had been built from source.
    fn install_fake_runtime(&self, fp: &str, label: &str) -> PathBuf {
        let dir = self.runtime_dir(fp);
        write_executable(&dir.join("kb"), &fake_kb(label, fp));
        common::write(
            &dir.join("BUILD-INFO"),
            &build_info(&engine_version(), fp, &self.target(), "source-build"),
        );
        dir
    }

    fn cargo_invoked(&self) -> bool {
        self.marker.exists()
    }

    fn reset_marker(&self) {
        if self.marker.exists() {
            fs::remove_file(&self.marker).unwrap();
        }
    }

    /// Names of launcher temporary entries left in the runtime cache.
    fn leftovers(&self) -> Vec<String> {
        let dir = self.root.join(".cache/runtime");
        if !dir.exists() {
            return Vec::new();
        }
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".tmp"))
            .collect()
    }

    fn stamp(&self) -> PathBuf {
        self.root.join(".cache/runtime/stamp")
    }
}

/// Modify a file after the filesystem clock has visibly advanced past the last stamp.
fn edit(path: &Path, text: &str) {
    std::thread::sleep(Duration::from_millis(50));
    fs::write(path, text).unwrap();
}

fn set_mtime(path: &Path, t: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

// --- archives ---------------------------------------------------------------------------

enum Entry {
    Dir(String),
    File(String, Vec<u8>, u32),
    Symlink(String, String),
    Hardlink(String, String),
}

fn octal(field: &mut [u8], value: u64) {
    let width = field.len() - 1;
    let digits = format!("{value:0width$o}");
    field[..width].copy_from_slice(digits.as_bytes());
    field[width] = 0;
}

/// Write a gzip-compressed ustar archive with exactly the given entries (no sanitizing,
/// so tests can build unsafe archives).
fn write_archive(path: &Path, entries: &[Entry]) {
    let mut tar = Vec::new();
    for e in entries {
        let (name, flag, data, mode, link): (&str, u8, &[u8], u32, &str) = match e {
            Entry::Dir(n) => (n, b'5', &[], 0o755, ""),
            Entry::File(n, d, m) => (n, b'0', d, *m, ""),
            Entry::Symlink(n, t) => (n, b'2', &[], 0o777, t),
            Entry::Hardlink(n, t) => (n, b'1', &[], 0o644, t),
        };
        assert!(name.len() <= 100 && link.len() <= 100);
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        octal(&mut h[100..108], u64::from(mode));
        octal(&mut h[108..116], 0);
        octal(&mut h[116..124], 0);
        octal(&mut h[124..136], data.len() as u64);
        octal(&mut h[136..148], 1_767_225_600);
        h[148..156].fill(b' ');
        h[156] = flag;
        h[157..157 + link.len()].copy_from_slice(link.as_bytes());
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u64 = h.iter().map(|&b| u64::from(b)).sum();
        octal(&mut h[148..155], sum);
        h[155] = b' ';
        tar.extend_from_slice(&h);
        tar.extend_from_slice(data);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }
    tar.resize(tar.len() + 1024, 0);
    let raw = path.with_extension("rawtar");
    fs::write(&raw, &tar).unwrap();
    let out = Command::new("gzip")
        .arg("-n")
        .arg("-f")
        .arg(&raw)
        .output()
        .unwrap();
    assert!(out.status.success(), "gzip failed: {}", stderr(&out));
    fs::rename(raw.with_extension("rawtar.gz"), path).unwrap();
}

fn file_sha(path: &Path) -> String {
    sha256_hex(&fs::read(path).unwrap())
}

/// The standard layout: `<top>/`, `<top>/kb`, `<top>/BUILD-INFO`.
fn standard_entries(top: &str, kb: &str, kb_mode: u32, info: &str) -> Vec<Entry> {
    vec![
        Entry::Dir(format!("{top}/")),
        Entry::File(format!("{top}/kb"), kb.as_bytes().to_vec(), kb_mode),
        Entry::File(format!("{top}/BUILD-INFO"), info.as_bytes().to_vec(), 0o644),
    ]
}

// --- tests ------------------------------------------------------------------------------

#[test]
fn kbw_is_shellcheck_clean() {
    let shellcheck = [
        "shellcheck",
        "/opt/homebrew/bin/shellcheck",
        "/usr/bin/shellcheck",
    ]
    .into_iter()
    .find(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    });
    let Some(shellcheck) = shellcheck else {
        eprintln!("skipping: shellcheck is not installed (CI runs it on ubuntu-latest)");
        return;
    };
    let o = Command::new(shellcheck)
        .arg(common::repo_root().join("kbw"))
        .output()
        .unwrap();
    assert!(o.status.success(), "shellcheck kbw:\n{}", stdout(&o));
}

#[test]
fn fingerprint_matches_the_documented_definition() {
    let kb = Kb::new();
    // Hidden files and editor backups are not build inputs.
    common::write(&kb.root.join("core/cli/src/.DS_Store"), "junk");
    common::write(&kb.root.join("core/cli/src/lib.rs~"), "backup");

    let mut files: Vec<String> = [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "core/release.toml",
        "core/cli/Cargo.toml",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    fn walk(root: &Path, rel: &str, out: &mut Vec<String>) {
        for e in fs::read_dir(root.join(rel)).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            let child = format!("{rel}/{name}");
            if e.file_type().unwrap().is_dir() {
                walk(root, &child, out);
            } else if e.file_type().unwrap().is_file()
                && !name.starts_with('.')
                && !name.ends_with('~')
            {
                out.push(child);
            }
        }
    }
    walk(&kb.root, "core/cli/src", &mut files);
    let mut lines: Vec<String> = files
        .iter()
        .map(|f| format!("{}  {f}\n", file_sha(&kb.root.join(f))))
        .collect();
    lines.sort();
    let expected = sha256_hex(lines.concat().as_bytes());

    assert_eq!(kb.fingerprint(), expected);
    // Independent of the invoking shell (dash is /bin/sh on Debian/Ubuntu).
    if Path::new("/bin/dash").exists() {
        let o = kb
            .command(Some("/bin/dash"), &kb.fake_bin)
            .arg("--kbw-fingerprint")
            .output()
            .unwrap();
        assert_ok(&o);
        assert_eq!(stdout(&o).trim(), expected);
    }
}

#[test]
fn only_engine_build_inputs_change_the_fingerprint() {
    let kb = Kb::new();
    let fp = kb.fingerprint();

    // Project knowledge, docs and caches are not build inputs.
    common::write_min_project(&kb.root);
    common::write(&kb.root.join("project/knowledge/extra.md"), "+++\n+++\n");
    common::write(&kb.root.join("docs/guide.md"), "# guide\n");
    common::write(&kb.root.join(".cache/index/project.sqlite"), "x");
    assert_eq!(
        kb.fingerprint(),
        fp,
        "project/docs/cache edits must not change the fingerprint"
    );

    for input in [
        "core/cli/src/lib.rs",
        "Cargo.lock",
        "core/release.toml",
        "core/cli/Cargo.toml",
    ] {
        let path = kb.root.join(input);
        let original = fs::read_to_string(&path).unwrap();
        edit(&path, &format!("{original}\n# local edit\n"));
        assert_ne!(
            kb.fingerprint(),
            fp,
            "editing {input} must change the fingerprint"
        );
        edit(&path, &original);
        assert_eq!(
            kb.fingerprint(),
            fp,
            "reverting {input} restores the fingerprint"
        );
    }
}

#[test]
fn warm_runs_exec_the_cached_runtime_without_cargo() {
    let kb = Kb::new();
    let fp = kb.fingerprint();
    kb.install_fake_runtime(&fp, "cached");
    assert!(!kb.stamp().exists());

    for _ in 0..2 {
        let o = kb.kbw(&["context", "--task", "two words", ""]);
        assert_ok(&o);
        let out = stdout(&o);
        assert_eq!(kv(&out, "label"), Some("cached"));
        assert_eq!(kv(&out, "KB_ROOT"), Some(kb.root.to_str().unwrap()));
        assert_eq!(kv(&out, "KBW_FINGERPRINT"), Some(fp.as_str()));
        let args: Vec<&str> = out.lines().filter_map(|l| l.strip_prefix("arg=")).collect();
        assert_eq!(args, ["context", "--task", "two words", ""]);
        assert!(
            stderr(&o).is_empty(),
            "warm runs are silent: {}",
            stderr(&o)
        );
    }
    assert!(!kb.cargo_invoked(), "a warm run must not invoke cargo");
    assert_eq!(fs::read_to_string(kb.stamp()).unwrap().trim(), fp);

    // The runtime's exit status is passed through.
    assert_eq!(kb.kbw(&["fail7"]).status.code(), Some(7));

    // A symlinked launcher (relative link in another directory) resolves the real root.
    let bin = kb.sb.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    symlink("../kb/kbw", bin.join("kbw-link")).unwrap();
    symlink(bin.join("kbw-link"), bin.join("kbw-link2")).unwrap();
    let mut c = Command::new(bin.join("kbw-link2"));
    kb.configure(&mut c, &kb.fake_bin);
    let o = c.current_dir(kb.sb.home()).arg("status").output().unwrap();
    assert_ok(&o);
    assert_eq!(kv(&stdout(&o), "KB_ROOT"), Some(kb.root.to_str().unwrap()));
    assert!(!kb.cargo_invoked());
}

#[test]
fn the_stamp_fast_path_skips_hashing_until_inputs_change() {
    let kb = Kb::new();
    let fp = kb.fingerprint();
    kb.install_fake_runtime(&fp, "real");
    assert_ok(&kb.kbw(&["x"]));

    // Point the stamp at another existing runtime, dated after every input: the warm path
    // trusts it without hashing.
    let other = "f".repeat(64);
    kb.install_fake_runtime(&other, "other");
    fs::write(kb.stamp(), format!("{other}\n")).unwrap();
    set_mtime(&kb.stamp(), SystemTime::now() + Duration::from_secs(3600));
    let o = kb.kbw(&["x"]);
    assert_ok(&o);
    assert_eq!(kv(&stdout(&o), "label"), Some("other"));

    // Once any input is newer than the stamp, the fingerprint is recomputed.
    set_mtime(
        &kb.stamp(),
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
    );
    let o = kb.kbw(&["x"]);
    assert_ok(&o);
    assert_eq!(kv(&stdout(&o), "label"), Some("real"));
    assert_eq!(fs::read_to_string(kb.stamp()).unwrap().trim(), fp);
    assert!(!kb.cargo_invoked());
}

#[test]
fn engine_edits_invalidate_the_runtime_and_failed_rebuilds_keep_the_old_one() {
    let kb = Kb::new();
    let fp = kb.fingerprint();
    let runtime = kb.install_fake_runtime(&fp, "v1");
    assert_ok(&kb.kbw(&["x"]));
    let before = dir_digest(&runtime);

    let lib = kb.root.join("core/cli/src/lib.rs");
    let original = fs::read_to_string(&lib).unwrap();
    let extra = kb.root.join("core/cli/src/extra_module.rs");
    let scenarios: [(&str, &dyn Fn()); 3] = [
        ("content edit", &|| {
            edit(&lib, &format!("{original}\n// local engine edit\n"))
        }),
        ("new source file", &|| edit(&extra, "// new file\n")),
        // Copies that preserve old mtimes (rsync -a, cp -p, tar) are caught by the ctime.
        ("edit with an old mtime", &|| {
            edit(&lib, &format!("{original}\n// copied engine edit\n"));
            set_mtime(
                &lib,
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
            );
        }),
    ];
    for (what, apply) in scenarios {
        apply();
        // Normal commands never build implicitly: the missing runtime is an error.
        let o = kb.kbw(&["version"]);
        assert_kbw_error(&o, "KBW_RUNTIME_NOT_BOOTSTRAPPED", "--kbw-bootstrap");
        assert!(
            !kb.cargo_invoked(),
            "{what}: a normal command must not invoke cargo"
        );
        // The explicit bootstrap (or the CI opt-in) attempts the rebuild.
        let o = kb.kbw_env(&["version"], &[("KBW_AUTO_BOOTSTRAP", "1")]);
        assert_kbw_error(&o, "KBW_BOOTSTRAP_FAILED", "cargo build");
        kb.reset_marker();
        let o = kb.kbw(&["--kbw-bootstrap"]);
        assert_kbw_error(&o, "KBW_BOOTSTRAP_FAILED", "cargo build");
        assert!(kb.cargo_invoked(), "{what}: the rebuild must invoke cargo");
        let marker = fs::read_to_string(&kb.marker).unwrap();
        assert!(
            marker.contains("build --release --locked -p kb"),
            "{marker}"
        );
        assert_ne!(kb.fingerprint(), fp, "{what}");
        assert_eq!(
            dir_digest(&runtime),
            before,
            "{what}: the old runtime must stay intact"
        );
        assert!(kb.leftovers().is_empty(), "{:?}", kb.leftovers());

        // Reverting makes the warm path work again without cargo.
        if extra.exists() {
            std::thread::sleep(Duration::from_millis(50));
            fs::remove_file(&extra).unwrap();
        }
        edit(&lib, &original);
        kb.reset_marker();
        let o = kb.kbw(&["x"]);
        assert_ok(&o);
        assert_eq!(kv(&stdout(&o), "label"), Some("v1"), "{what}");
        assert!(
            !kb.cargo_invoked(),
            "{what}: reverted engine must not rebuild"
        );
    }
}

#[test]
fn toolchain_mismatch_is_reported_unless_explicitly_allowed() {
    let kb = Kb::new();
    Kb::set_fake_rustc(&kb.fake_bin, "1.0.0");
    let o = kb.kbw(&["--kbw-bootstrap"]);
    assert_kbw_error(&o, "KBW_TOOLCHAIN_MISMATCH", "1.0.0");
    assert!(
        !kb.cargo_invoked(),
        "the build must not start with the wrong toolchain"
    );

    let o = kb.kbw_env(
        &["--kbw-bootstrap"],
        &[("KBW_ALLOW_TOOLCHAIN_MISMATCH", "1")],
    );
    assert!(
        stderr(&o).contains("warning[KBW_TOOLCHAIN_MISMATCH]"),
        "{}",
        stderr(&o)
    );
    assert_kbw_error(&o, "KBW_BOOTSTRAP_FAILED", "cargo build");
    assert!(kb.cargo_invoked());
    assert!(kb.leftovers().is_empty());
}

#[test]
fn source_builds_are_labelled_only_with_the_fingerprint_compiled_into_them() {
    let kb = Kb::new();
    let target_dir = kb.private_target();
    let build_env = [("KBW_CARGO_TARGET_DIR", target_dir.as_str())];
    let fp = kb.fingerprint();

    // kbw hands the fingerprint to the build and the binary must report it back.
    kb.use_building_cargo("built", None, ":");
    let o = kb.kbw_env(&["--kbw-bootstrap"], &build_env);
    assert_ok(&o);
    let marker = fs::read_to_string(&kb.marker).unwrap();
    assert!(
        marker.contains(&format!("KBW_BUILD_FINGERPRINT={fp}")),
        "cargo must see the fingerprint: {marker}"
    );
    let o = kb.kbw(&["--json", "version"]);
    assert_ok(&o);
    assert_eq!(common::json(&o)["result"]["build_fingerprint"], fp.as_str());
    let first = dir_digest(&kb.runtime_dir(&fp));

    // Cargo judges freshness by mtimes, kbw by content: when cargo keeps the binary of an
    // earlier build (an mtime-preserving copy, a target dir shared between checkouts), that
    // binary never becomes the runtime of the new fingerprint.
    let lib = kb.root.join("core/cli/src/lib.rs");
    let original = fs::read_to_string(&lib).unwrap();
    edit(&lib, &format!("{original}\n// engine edit\n"));
    let edited = kb.fingerprint();
    assert_ne!(edited, fp);
    kb.use_building_cargo("stale", Some(&fp), ":");
    let o = kb.kbw_env(&["--kbw-bootstrap"], &build_env);
    assert_kbw_error(&o, "KBW_BOOTSTRAP_FAILED", "smoke test");
    assert!(
        stderr(&o).contains(&format!("build fingerprint {fp}")),
        "{}",
        stderr(&o)
    );
    assert!(!kb.runtime_dir(&edited).exists());
    assert!(kb.leftovers().is_empty(), "{:?}", kb.leftovers());

    // An input edited while cargo runs may or may not be compiled in: the result is labelled
    // with neither fingerprint.
    let during = format!(
        "printf '// edited during the build\\n' >> '{}'",
        lib.display()
    );
    kb.use_building_cargo("racy", None, &during);
    let o = kb.kbw_env(&["--kbw-bootstrap"], &build_env);
    assert_kbw_error(&o, "KBW_BOOTSTRAP_FAILED", "changed during the build");
    let after = kb.fingerprint();
    assert_ne!(after, edited);
    assert!(!kb.runtime_dir(&edited).exists());
    assert!(!kb.runtime_dir(&after).exists());
    assert!(kb.leftovers().is_empty(), "{:?}", kb.leftovers());

    // Once the inputs are stable the build succeeds; the first runtime was never touched.
    kb.use_building_cargo("final", None, ":");
    let o = kb.kbw_env(&["--kbw-bootstrap"], &build_env);
    assert_ok(&o);
    let o = kb.kbw(&["--json", "version"]);
    assert_eq!(
        common::json(&o)["result"]["build_fingerprint"],
        after.as_str()
    );
    assert_eq!(dir_digest(&kb.runtime_dir(&fp)), first);
}

#[test]
fn a_relative_cargo_target_dir_is_resolved_against_the_callers_directory() {
    // Cargo runs in the KB root; the caller is neither the root nor its parent. Like
    // `kb update prepare` (which runs the target engine with an absolute value), kbw
    // resolves a relative value against the caller's directory, so cargo and the launcher
    // look for the binary at the same place.
    let mut shells = vec![None];
    if Path::new("/bin/dash").exists() {
        shells.push(Some("/bin/dash"));
    }
    for shell in shells {
        let kb = Kb::new();
        let fp = kb.fingerprint();
        kb.use_building_cargo("relative", None, ":");
        let caller = kb.sb.path().join("caller");
        fs::create_dir_all(&caller).unwrap();
        let o = kb
            .command(shell, &kb.fake_bin)
            .current_dir(&caller)
            .env("KBW_CARGO_TARGET_DIR", "rel-target")
            .arg("--kbw-bootstrap")
            .output()
            .unwrap();
        assert_ok(&o);
        assert_eq!(stdout(&o).trim(), kb.runtime_dir(&fp).display().to_string());
        assert!(
            caller.join("rel-target/release/kb").is_file(),
            "{shell:?}: cargo builds below the caller's directory"
        );
        assert!(
            !kb.root.join("rel-target").exists(),
            "{shell:?}: nothing is built in the KB root"
        );
        assert!(kb.leftovers().is_empty(), "{:?}", kb.leftovers());
        let o = kb.kbw(&["--json", "version"]);
        assert_ok(&o);
        assert_eq!(common::json(&o)["result"]["build_fingerprint"], fp.as_str());
        // The rule is part of the launcher's documented interface.
        let help = stdout(&kb.kbw(&["--kbw-help"]));
        let words = help.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            words.contains(
                "KBW_CARGO_TARGET_DIR cargo target directory for source builds \
                 (default: <root>/.cache/cargo-target); a relative value is resolved \
                 against the directory kbw is run from, not against <root>"
            ),
            "{help}"
        );
    }
}

#[test]
fn package_rebuilds_a_runtime_that_does_not_report_its_fingerprint() {
    let kb = Kb::new();
    let fp = kb.fingerprint();
    // A source-built runtime whose binary does not prove it was built from these inputs
    // (e.g. one made by an older launcher) would be rejected by every installer.
    let dir = kb.install_fake_runtime(&fp, "unproven");
    write_executable(&dir.join("kb"), &fake_kb("unproven", "unknown"));
    kb.use_building_cargo("rebuilt", None, ":");
    let dist = kb.sb.path().join("dist");
    let o = kb.kbw_env(
        &["--kbw-package", dist.to_str().unwrap()],
        &[("KBW_CARGO_TARGET_DIR", kb.private_target().as_str())],
    );
    assert_ok(&o);
    assert!(kb.cargo_invoked(), "the package must come from a new build");

    let other = Kb::new();
    let archive = dist.join(format!("kb-{}-{}.tar.gz", engine_version(), kb.target()));
    let sums = dist.join(format!(
        "{}.sha256",
        archive.file_name().unwrap().to_string_lossy()
    ));
    assert_ok(&other.kbw(&[
        "--kbw-install-artifact",
        archive.to_str().unwrap(),
        "--sha256-file",
        sums.to_str().unwrap(),
    ]));
    assert_eq!(kv(&stdout(&other.kbw(&["x"])), "label"), Some("rebuilt"));
}

#[test]
fn package_writes_a_verified_archive_that_installs_elsewhere() {
    let kb = Kb::new();
    let fp = kb.fingerprint();
    let target = kb.target();
    let v = engine_version();
    kb.install_fake_runtime(&fp, "packaged");
    common::write(&kb.root.join("LICENSE"), "MIT (test)\n");
    common::write(&kb.root.join("README.md"), "# kb (test)\n");

    let dist = kb.sb.path().join("dist");
    let o = kb.kbw(&["--kbw-package", dist.to_str().unwrap()]);
    assert_ok(&o);
    assert!(
        !kb.cargo_invoked(),
        "an existing source-built runtime is packaged as is"
    );
    let name = format!("kb-{v}-{target}.tar.gz");
    let archive = dist.join(&name);
    let sums = dist.join(format!("{name}.sha256"));
    let printed: Vec<String> = stdout(&o).lines().map(str::to_string).collect();
    assert_eq!(
        printed,
        [archive.display().to_string(), sums.display().to_string()]
    );
    assert_eq!(
        fs::read_to_string(&sums).unwrap(),
        format!("{}  {name}\n", file_sha(&archive))
    );
    let listing = Command::new("tar")
        .arg("-tzf")
        .arg(&archive)
        .output()
        .unwrap();
    let mut names: Vec<String> = stdout(&listing).lines().map(str::to_string).collect();
    names.sort();
    let top = format!("kb-{v}-{target}");
    assert_eq!(
        names,
        [
            format!("{top}/"),
            format!("{top}/BUILD-INFO"),
            format!("{top}/LICENSE"),
            format!("{top}/README.md"),
            format!("{top}/kb"),
        ]
    );
    assert!(
        fs::read_dir(&dist).unwrap().count() == 2,
        "no temporary files remain"
    );

    // A fresh checkout of the same engine installs it without any toolchain.
    let other = Kb::new();
    assert_eq!(other.fingerprint(), fp);
    let o = other.kbw(&[
        "--kbw-install-artifact",
        archive.to_str().unwrap(),
        "--sha256-file",
        sums.to_str().unwrap(),
    ]);
    assert_ok(&o);
    let o = other.kbw(&["x"]);
    assert_ok(&o);
    assert_eq!(kv(&stdout(&o), "label"), Some("packaged"));
    assert!(!other.cargo_invoked());
}

#[test]
fn package_never_repackages_an_installed_artifact() {
    let kb = Kb::new();
    let fp = kb.fingerprint();
    let dir = kb.install_fake_runtime(&fp, "artifact");
    let info = fs::read_to_string(dir.join("BUILD-INFO"))
        .unwrap()
        .replace("source=source-build", "source=artifact");
    fs::write(dir.join("BUILD-INFO"), info).unwrap();
    let dist = kb.sb.path().join("dist");
    let o = kb.kbw(&["--kbw-package", dist.to_str().unwrap()]);
    assert_kbw_error(&o, "KBW_BOOTSTRAP_FAILED", "cargo build");
    assert!(kb.cargo_invoked(), "packaging requires a source build");
    assert_eq!(fs::read_dir(&dist).map(|d| d.count()).unwrap_or(0), 0);
}

#[test]
fn install_activates_a_verified_artifact() {
    let v = engine_version();
    // Fresh checkout (no runtime yet) and a checkout with an active runtime to replace.
    for with_previous in [false, true] {
        let kb = Kb::new();
        let fp = kb.fingerprint();
        let target = kb.target();
        if with_previous {
            kb.install_fake_runtime(&fp, "previous");
            assert_ok(&kb.kbw(&["x"]));
        }
        let top = format!("kb-{v}-{target}");
        let dir = kb.sb.path().join("downloads");
        fs::create_dir_all(&dir).unwrap();
        let archive = dir.join(format!("{top}.tar.gz"));
        write_archive(
            &archive,
            &standard_entries(
                &top,
                &fake_kb("artifact", &fp),
                0o755,
                &build_info(&v, &fp, &target, "source-build"),
            ),
        );
        let digest = file_sha(&archive);
        let o = if with_previous {
            kb.kbw(&[
                "--kbw-install-artifact",
                archive.to_str().unwrap(),
                "--sha256",
                &digest.to_uppercase(),
            ])
        } else {
            // A multi-platform SHA256SUMS with CRLF line endings and binary-mode markers.
            let sums = dir.join("SHA256SUMS");
            let other = "0".repeat(64);
            fs::write(
                &sums,
                format!("{other}  kb-{v}-other.tar.gz\r\n{digest} *{top}.tar.gz\r\n"),
            )
            .unwrap();
            kb.kbw(&[
                "--kbw-install-artifact",
                archive.to_str().unwrap(),
                "--sha256-file",
                sums.to_str().unwrap(),
            ])
        };
        assert_ok(&o);
        let runtime = kb.runtime_dir(&fp);
        assert_eq!(stdout(&o).trim(), runtime.display().to_string());
        let info = fs::read_to_string(runtime.join("BUILD-INFO")).unwrap();
        assert_eq!(kv(&info, "source"), Some("artifact"));
        assert_eq!(kv(&info, "artifact_sha256"), Some(digest.as_str()));
        assert_eq!(kv(&info, "fingerprint"), Some(fp.as_str()));
        assert!(kb.leftovers().is_empty(), "{:?}", kb.leftovers());

        let o = kb.kbw(&["x"]);
        assert_ok(&o);
        assert_eq!(kv(&stdout(&o), "label"), Some("artifact"));
        let info = stdout(&kb.kbw(&["--kbw-runtime-info"]));
        assert_eq!(kv(&info, "status"), Some("active"));
        assert_eq!(kv(&info, "source"), Some("artifact"));
        assert!(!kb.cargo_invoked());
    }
}

/// The real `mv` (the launcher tests wrap it to observe and interrupt activation).
fn real_mv() -> &'static str {
    ["/bin/mv", "/usr/bin/mv"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .expect("mv is installed")
}

#[test]
fn reinstalling_a_runtime_keeps_it_resolvable_at_every_step() {
    let kb = Kb::new();
    let fp = kb.fingerprint();
    let target = kb.target();
    let v = engine_version();
    let runtime = kb.runtime_dir(&fp);
    let top = format!("kb-{v}-{target}");
    let archive = kb.sb.path().join(format!("{top}.tar.gz"));
    write_archive(
        &archive,
        &standard_entries(
            &top,
            &fake_kb("artifact", &fp),
            0o755,
            &build_info(&v, &fp, &target, "source-build"),
        ),
    );
    let digest = file_sha(&archive);

    // After every `mv` the launcher runs, record whether the runtime still resolves (what a
    // concurrent `./kbw ...` would see) and, at step KILL_AT, SIGKILL the launcher.
    let log = kb.sb.path().join("mv.log");
    let count = kb.sb.path().join("mv.count");
    write_executable(
        &kb.fake_bin.join("mv"),
        &format!(
            "#!/bin/sh\n\
             {mv} \"$@\"\n\
             st=$?\n\
             if [ -n \"${{WATCH-}}\" ]; then\n\
               [ -f \"$WATCH/kb\" ] && [ -x \"$WATCH/kb\" ] || echo \"missing after: mv $*\" >> '{log}'\n\
               n=$(( $(cat '{count}' 2>/dev/null || echo 0) + 1 ))\n\
               echo \"$n\" > '{count}'\n\
               if [ \"$n\" = \"${{KILL_AT-0}}\" ]; then kill -9 \"$PPID\"; fi\n\
             fi\n\
             exit $st\n",
            mv = real_mv(),
            log = log.display(),
            count = count.display()
        ),
    );
    let watch = runtime.display().to_string();
    let reset = || {
        kb.install_fake_runtime(&fp, "previous");
        assert_eq!(kv(&stdout(&kb.kbw(&["x"])), "label"), Some("previous"));
        let _ = fs::remove_file(&count);
    };
    let install = |kill_at: &str| {
        kb.kbw_env(
            &[
                "--kbw-install-artifact",
                archive.to_str().unwrap(),
                "--sha256",
                &digest,
            ],
            &[("WATCH", watch.as_str()), ("KILL_AT", kill_at)],
        )
    };

    reset();
    assert_ok(&install("0"));
    assert!(
        !log.exists(),
        "the runtime was unresolvable during activation:\n{}",
        fs::read_to_string(&log).unwrap_or_default()
    );
    assert!(kb.leftovers().is_empty(), "{:?}", kb.leftovers());
    assert_eq!(kv(&stdout(&kb.kbw(&["x"])), "label"), Some("artifact"));
    let steps: u32 = fs::read_to_string(&count).unwrap().trim().parse().unwrap();

    // A launcher killed after any step leaves a working runtime (the old or the new one).
    for step in 1..=steps {
        reset();
        let o = install(&step.to_string());
        assert_eq!(o.status.code(), None, "step {step}: expected SIGKILL");
        let o = kb.kbw(&["x"]);
        assert_ok(&o);
        let label = kv(&stdout(&o), "label").unwrap().to_string();
        assert!(
            label == "previous" || label == "artifact",
            "step {step}: {label}"
        );
        let info = fs::read_to_string(runtime.join("BUILD-INFO")).unwrap();
        assert_eq!(kv(&info, "fingerprint"), Some(fp.as_str()), "step {step}");
    }
}

#[test]
fn install_rejects_invalid_artifacts_and_keeps_the_active_runtime() {
    let kb = Kb::new();
    let fp = kb.fingerprint();
    let target = kb.target();
    let v = engine_version();
    let runtime = kb.install_fake_runtime(&fp, "active");
    assert_ok(&kb.kbw(&["x"]));
    let before = dir_digest(&runtime);
    let dir = kb.sb.path().join("archives");
    fs::create_dir_all(&dir).unwrap();

    let top = format!("kb-{v}-{target}");
    let info = build_info(&v, &fp, &target, "source-build");
    let script = fake_kb("candidate", &fp);
    let zeros = "0".repeat(64);
    let file = |name: &str, text: &str, mode: u32| {
        Entry::File(name.to_string(), text.as_bytes().to_vec(), mode)
    };
    let layout = |kb_text: &str, kb_mode: u32, info_text: &str, extra: Vec<Entry>| {
        let mut entries = standard_entries(&top, kb_text, kb_mode, info_text);
        entries.extend(extra);
        entries
    };
    let with_extra = |extra: Entry| layout(&script, 0o755, &info, vec![extra]);
    let with_info = |info_text: &str| layout(&script, 0o755, info_text, vec![]);
    let with_kb = |kb_text: &str, mode: u32| layout(kb_text, mode, &info, vec![]);
    let build = |name: &str, entries: Vec<Entry>| {
        let p = dir.join(format!("{name}.tar.gz"));
        write_archive(&p, &entries);
        p.to_str().unwrap().to_string()
    };
    let rejected = |case: &str, args: [&str; 3], needle: &str| {
        let o = kb.kbw(&["--kbw-install-artifact", args[0], args[1], args[2]]);
        assert_kbw_error(&o, "KBW_ARTIFACT_INVALID", needle);
        assert!(
            stderr(&o).contains("the active runtime is unchanged"),
            "{case}"
        );
        assert_eq!(
            dir_digest(&runtime),
            before,
            "{case}: active runtime changed"
        );
        assert!(kb.leftovers().is_empty(), "{case}: {:?}", kb.leftovers());
        assert!(!kb.root.join(".cache/runtime/evil").exists(), "{case}");
        assert_eq!(
            kv(&stdout(&kb.kbw(&["x"])), "label"),
            Some("active"),
            "{case}"
        );
    };
    // Invalid content behind its own (matching) digest.
    let rejected_archive = |name: &str, entries: Vec<Entry>, needle: &str| {
        let archive = build(name, entries);
        rejected(
            name,
            [&archive, "--sha256", &file_sha(Path::new(&archive))],
            needle,
        );
    };

    let good = build("good", layout(&script, 0o755, &info, vec![]));
    let good_sha = file_sha(Path::new(&good));

    // The digest is compared before anything is listed or extracted: an unsafe archive
    // with a wrong digest is reported as a mismatch.
    let unsafe_archive = build("unsafe", with_extra(file("../evil", "x", 0o644)));
    rejected(
        "wrong digest",
        [&unsafe_archive, "--sha256", &good_sha],
        "sha256 mismatch",
    );
    let tampered = build("tampered", with_kb(&format!("{script}# tampered\n"), 0o755));
    rejected(
        "tampered",
        [&tampered, "--sha256", &good_sha],
        "sha256 mismatch",
    );

    let unsafe_entry = "unsafe archive entry";
    rejected_archive(
        "parent",
        with_extra(file("../evil", "x", 0o644)),
        unsafe_entry,
    );
    let nested = format!("{top}/../evil");
    rejected_archive(
        "nested-parent",
        with_extra(file(&nested, "x", 0o644)),
        unsafe_entry,
    );
    let absolute = format!("/{top}-evil/kb");
    rejected_archive(
        "absolute",
        with_extra(file(&absolute, "x", 0o644)),
        unsafe_entry,
    );
    let outside = with_extra(file("other/kb", "x", 0o644));
    rejected_archive("outside", outside, "unsafe archive entry 'other/kb'");

    let not_regular = "not a regular file or directory";
    let hardlink = Entry::Hardlink(format!("{top}/kb2"), format!("{top}/kb"));
    rejected_archive("hardlink", with_extra(hardlink), not_regular);
    let symlinked = vec![
        Entry::Dir(format!("{top}/")),
        Entry::Symlink(format!("{top}/kb"), "/bin/sh".into()),
        file(&format!("{top}/BUILD-INFO"), &info, 0o644),
    ];
    rejected_archive("symlink", symlinked, not_regular);

    let other_target = if target == "x86_64-unknown-linux-gnu" {
        "aarch64-apple-darwin"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let wrong_fp = with_info(&build_info(&v, &zeros, &target, "x"));
    rejected_archive("wrong-fingerprint", wrong_fp, "BUILD-INFO fingerprint");
    let wrong_target = with_info(&build_info(&v, &fp, other_target, "x"));
    rejected_archive("wrong-target", wrong_target, "BUILD-INFO target");
    let wrong_version = with_info(&build_info("9.9.9", &fp, &target, "x"));
    rejected_archive("wrong-version", wrong_version, "BUILD-INFO engine_version");
    let duplicate = with_info(&format!("{info}fingerprint={zeros}\n"));
    rejected_archive(
        "duplicate-fingerprint",
        duplicate,
        "exactly one fingerprint",
    );
    let no_info = vec![
        Entry::Dir(format!("{top}/")),
        file(&format!("{top}/kb"), &script, 0o755),
    ];
    rejected_archive("missing-build-info", no_info, "BUILD-INFO is missing");
    rejected_archive(
        "not-executable",
        with_kb(&script, 0o644),
        "not an executable",
    );
    rejected_archive(
        "smoke-exit-1",
        with_kb("#!/bin/sh\nexit 1\n", 0o755),
        "smoke test",
    );
    let wrong_output = with_kb("#!/bin/sh\necho 'kb 9.9.9'\n", 0o755);
    rejected_archive("smoke-wrong-version", wrong_output, "smoke test");
    let json_version = |version: &str, build: &str| {
        let text = format!(
            "#!/bin/sh\ncat <<'EOF'\n{}EOF\n",
            version_json(version, build)
        );
        with_kb(&text, 0o755)
    };
    rejected_archive(
        "smoke-json-wrong-version",
        json_version("9.9.9", &fp),
        "smoke test",
    );
    // A binary built for other engine inputs, or outside the launcher, does not become
    // the runtime of this fingerprint even though its BUILD-INFO claims so.
    rejected_archive(
        "smoke-other-build",
        with_kb(&fake_kb("candidate", &zeros), 0o755),
        "smoke test",
    );
    rejected_archive(
        "smoke-unknown-build",
        json_version(&v, "unknown"),
        "smoke test",
    );

    let not_gzip = dir.join("not-gzip.tar.gz");
    fs::write(&not_gzip, "definitely not an archive").unwrap();
    let not_gzip_sha = file_sha(&not_gzip);
    rejected(
        "not gzip",
        [not_gzip.to_str().unwrap(), "--sha256", &not_gzip_sha],
        "cannot list",
    );
    rejected(
        "malformed digest",
        [&good, "--sha256", "abc"],
        "64 hexadecimal",
    );
    let sums = dir.join("SHA256SUMS");
    fs::write(&sums, format!("{good_sha}  another-name.tar.gz\n")).unwrap();
    rejected(
        "no sums entry",
        [&good, "--sha256-file", sums.to_str().unwrap()],
        "exactly one entry",
    );
    let missing = dir.join("missing.tar.gz");
    rejected(
        "missing file",
        [missing.to_str().unwrap(), "--sha256", &good_sha],
        "not found",
    );
    let http = "http://example.invalid/kb.tar.gz";
    rejected("plain http", [http, "--sha256", &good_sha], "only https://");
    assert!(!kb.cargo_invoked());

    // The good archive is accepted, which shows the rejections above were specific.
    assert_ok(&kb.kbw(&["--kbw-install-artifact", &good, "--sha256", &good_sha]));
    assert_eq!(kv(&stdout(&kb.kbw(&["x"])), "label"), Some("candidate"));
}

#[test]
fn launcher_usage_errors_exit_2() {
    let kb = Kb::new();
    for args in [
        vec!["--kbw-unknown"],
        vec!["--kbw-fingerprint", "extra"],
        vec!["--kbw-install-artifact", "a.tar.gz"],
        vec!["--kbw-install-artifact", "a.tar.gz", "--md5", "x"],
        vec!["--kbw-package"],
    ] {
        let o = kb.kbw(&args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", stderr(&o));
        assert!(stderr(&o).contains("kbw: error[KBW_USAGE]"), "{args:?}");
    }
    let o = kb.kbw(&["--kbw-help"]);
    assert_ok(&o);
    assert!(stdout(&o).contains("--kbw-install-artifact"));
    // Launcher flags are recognized only as the first argument.
    kb.install_fake_runtime(&kb.fingerprint(), "rt");
    let o = kb.kbw(&["show", "--kbw-help"]);
    assert_ok(&o);
    assert!(stdout(&o).contains("arg=--kbw-help"));
}

#[test]
fn launcher_works_under_dash() {
    if !Path::new("/bin/dash").exists() {
        eprintln!("skipping: /bin/dash is not installed");
        return;
    }
    let kb = Kb::new();
    let fp = kb.fingerprint();
    kb.install_fake_runtime(&fp, "dash");
    let run = |args: &[&str]| {
        kb.command(Some("/bin/dash"), &kb.fake_bin)
            .args(args)
            .output()
            .unwrap()
    };
    let o = run(&["a b"]);
    assert_ok(&o);
    assert_eq!(kv(&stdout(&o), "KBW_FINGERPRINT"), Some(fp.as_str()));
    assert_eq!(kv(&stdout(&o), "arg"), Some("a b"));
    let o = run(&["--kbw-runtime-info"]);
    assert_eq!(kv(&stdout(&o), "source"), Some("source-build"));
    let o = run(&[
        "--kbw-install-artifact",
        "/nonexistent.tar.gz",
        "--sha256",
        &"a".repeat(64),
    ]);
    assert_kbw_error(&o, "KBW_ARTIFACT_INVALID", "not found");
    assert!(!kb.cargo_invoked());
}

/// The only test that compiles the engine: source bootstrap with the real toolchain, then
/// the warm path, packaging and installation of the real binary into a toolchain-less copy.
#[test]
fn source_bootstrap_builds_packages_and_installs_the_real_engine() {
    let v = engine_version();
    let a = Kb::new();
    let fp = a.fingerprint();
    // Without an explicit bootstrap a normal command refuses to build.
    let o = a
        .command(None, &real_toolchain_dir())
        .arg("version")
        .output()
        .unwrap();
    assert_kbw_error(&o, "KBW_RUNTIME_NOT_BOOTSTRAPPED", "--kbw-bootstrap");
    assert!(!a.runtime_dir(&fp).exists());
    let o = a
        .command(None, &real_toolchain_dir())
        .arg("--kbw-bootstrap")
        .output()
        .unwrap();
    assert_ok(&o);
    assert_eq!(stdout(&o).trim(), a.runtime_dir(&fp).display().to_string());
    assert!(
        stderr(&o).contains("building the kb runtime from source"),
        "{}",
        stderr(&o)
    );
    let runtime = a.runtime_dir(&fp);
    let info = fs::read_to_string(runtime.join("BUILD-INFO")).unwrap();
    assert_eq!(kv(&info, "engine_version"), Some(v.as_str()));
    assert_eq!(kv(&info, "fingerprint"), Some(fp.as_str()));
    assert_eq!(kv(&info, "source"), Some("source-build"));
    assert_eq!(kv(&info, "target"), Some(a.target().as_str()));
    assert!(
        kv(&info, "rustc").is_some_and(|r| r.starts_with("rustc ")),
        "{info}"
    );
    assert!(a.leftovers().is_empty(), "{:?}", a.leftovers());

    // Warm: fake cargo in PATH is never invoked.
    let o = a.kbw(&["--kbw-bootstrap"]);
    assert_ok(&o);
    assert_eq!(stdout(&o).trim(), runtime.display().to_string());
    let o = a.kbw(&["version"]);
    assert_ok(&o);
    assert!(stdout(&o).starts_with(&format!("kb {v} ")));
    assert!(!a.cargo_invoked(), "the warm run invoked cargo");

    // Package the real runtime and install it into a copy without a usable toolchain.
    let dist = a.sb.path().join("dist");
    assert_ok(&a.kbw(&["--kbw-package", dist.to_str().unwrap()]));
    assert!(!a.cargo_invoked());
    let target = a.target();
    let archive = dist.join(format!("kb-{v}-{target}.tar.gz"));
    let sums = dist.join(format!("kb-{v}-{target}.tar.gz.sha256"));
    let c = Kb::new();
    assert_eq!(
        c.fingerprint(),
        fp,
        "identical engine files give identical fingerprints"
    );
    let o = c.kbw(&[
        "--kbw-install-artifact",
        archive.to_str().unwrap(),
        "--sha256-file",
        sums.to_str().unwrap(),
    ]);
    assert_ok(&o);
    let o = c.kbw(&["version"]);
    assert_ok(&o);
    assert!(stdout(&o).starts_with(&format!("kb {v} ")));
    assert_eq!(
        file_sha(&c.runtime_dir(&fp).join("kb")),
        file_sha(&runtime.join("kb")),
        "the installed binary is the packaged one"
    );
    assert!(!c.cargo_invoked());
    let o = c.kbw(&["--json", "version"]);
    assert_ok(&o);
    assert_eq!(common::json(&o)["result"]["build_fingerprint"], fp.as_str());

    // An engine edit carrying an old mtime (cp -p, rsync -a, tar x) looks fresh to cargo's
    // mtime check; the rebuilt runtime must still contain it and report its own fingerprint.
    let versions = a.root.join("core/cli/src/versions.rs");
    let text = fs::read_to_string(&versions).unwrap();
    let decl = "pub const PARSER_VERSION: u32 = ";
    let line = text.lines().find(|l| l.starts_with(decl)).unwrap();
    let edited = text.replace(line, &format!("{decl}4242;"));
    assert_ne!(edited, text);
    fs::write(&versions, edited).unwrap();
    set_mtime(
        &versions,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
    );
    let fp2 = a.fingerprint();
    assert_ne!(fp2, fp);
    let o = a
        .command(None, &real_toolchain_dir())
        .arg("--kbw-bootstrap")
        .output()
        .unwrap();
    assert_ok(&o);
    assert_eq!(stdout(&o).trim(), a.runtime_dir(&fp2).display().to_string());
    let o = a.kbw(&["--json", "version"]);
    assert_ok(&o);
    let result = &common::json(&o)["result"];
    assert_eq!(result["parser_version"], 4242, "stale binary: {result}");
    assert_eq!(result["build_fingerprint"], fp2.as_str());
    assert!(!a.cargo_invoked());
}
