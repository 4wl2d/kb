//! Under the engine, deadlines end everything the provider started and remove its frozen
//! checkout, whether the engine's deadline or the adapter's own one fires. A standalone
//! provider never ends a process group the engine did not create for it. Synthetic host
//! and native tool; no network.
#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use kb::model::CodeRequest;

fn git(root: &Path, home: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Synthetic")
        .env("GIT_COMMITTER_NAME", "Synthetic")
        .env("GIT_AUTHOR_EMAIL", "synthetic@example.invalid")
        .env("GIT_COMMITTER_EMAIL", "synthetic@example.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}

/// A killed process that awaits reaping is a zombie; it no longer runs.
fn running(pid: &str) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    out.status.success() && !stat.trim().is_empty() && !stat.trim_start().starts_with('Z')
}

/// Synthetic host and native tool. The tool answers the version check; otherwise it runs
/// `work`, which writes a pid and its working directory (the frozen checkout) to `$RECORD`.
struct Setup {
    _temp: tempfile::TempDir,
    request: CodeRequest,
    tool: PathBuf,
    record: PathBuf,
}

impl Setup {
    fn new(work: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let host = temp.path().join("host");
        let home = temp.path().join("home");
        fs::create_dir_all(&host).unwrap();
        fs::create_dir_all(&home).unwrap();
        git(&host, &home, &["init", "-q", "-b", "main"]);
        fs::write(host.join("source.rs"), "pub fn save() {}\n").unwrap();
        git(&host, &home, &["add", "--all"]);
        git(&host, &home, &["commit", "-qm", "synthetic base"]);
        let commit = git(&host, &home, &["rev-parse", "HEAD"]);
        let record = temp.path().join("native.txt");
        let tool = temp.path().join("ast-index");
        fs::write(
            &tool,
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = --version ]; then echo 'ast-index 3.54.0'; exit 0; fi\n\
                 RECORD='{}'\n\
                 {work}",
                record.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        // The first exec of a new script can be slow (for example while the OS assesses it);
        // take it outside the adapter's short per-command deadline.
        let version = Command::new(&tool).arg("--version").output().unwrap();
        assert!(version.status.success());
        let mut request: CodeRequest =
            serde_json::from_str(include_str!("fixtures/request.json")).unwrap();
        request.root = host.to_str().unwrap().into();
        request.commit = commit;
        Self {
            _temp: temp,
            request,
            tool,
            record,
        }
    }

    fn provider(&self, adapter_timeout: &str, engine_timeout: u64) -> kb::code::Provider {
        kb::code::Provider {
            program: Some(env!("CARGO_BIN_EXE_kb-code-provider").into()),
            args: vec![
                "--backend".into(),
                "ast-index".into(),
                "--tool".into(),
                self.tool.to_str().unwrap().into(),
                "--timeout".into(),
                adapter_timeout.into(),
            ],
            files: Vec::new(),
            timeout_seconds: engine_timeout,
        }
    }

    /// The recorded process no longer runs and the frozen checkout is gone.
    fn assert_cleaned_up(&self) {
        let recorded =
            fs::read_to_string(&self.record).expect("native tool started before the deadline");
        let (pid, checkout) = recorded.trim_end().split_once('\n').unwrap();
        // SIGKILL is delivered asynchronously; poll with a generous bound, never a timing claim.
        let mut stopped = false;
        for _ in 0..500 {
            if !running(pid) {
                stopped = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !stopped {
            let _ = Command::new("kill").args(["-9", pid]).status();
        }
        assert!(stopped, "process {pid} outlived the provider deadline");
        assert!(
            !Path::new(checkout).exists(),
            "frozen provider checkout {checkout} was left behind"
        );
    }
}

#[test]
fn engine_deadline_ends_native_tool_and_removes_frozen_checkout() {
    // The native tool itself outlives the engine deadline.
    let setup = Setup::new(
        "printf '%s\\n%s\\n' \"$$\" \"$(pwd -P)\" > \"$RECORD.tmp\"\n\
         mv \"$RECORD.tmp\" \"$RECORD\"\n\
         exec sleep 60\n",
    );
    let error = setup.provider("120", 2).load(&setup.request).unwrap_err();
    assert_eq!(error.details["kind"], "timeout", "{error}");
    setup.assert_cleaned_up();
}

#[test]
fn adapter_deadline_ends_native_descendants_and_removes_frozen_checkout() {
    // The native tool starts a detached helper and waits. The adapter's own deadline, far
    // shorter than the engine's, kills the tool itself but not its helper directly.
    let setup = Setup::new(
        "sleep 60 </dev/null >/dev/null 2>&1 &\n\
         printf '%s\\n%s\\n' \"$!\" \"$(pwd -P)\" > \"$RECORD.tmp\"\n\
         mv \"$RECORD.tmp\" \"$RECORD\"\n\
         wait\n",
    );
    let error = setup.provider("1", 120).load(&setup.request).unwrap_err();
    assert!(
        error.details["kind"] != "timeout"
            && error
                .message
                .contains("subprocess exceeded its configured deadline"),
        "{error}"
    );
    setup.assert_cleaned_up();
}

#[test]
fn standalone_group_leader_without_engine_marker_keeps_its_group() {
    // Run directly as the leader of its own process group, as the first command of an
    // interactive shell pipeline is. That group may hold the caller's pipeline peers, so
    // without the engine's marker the adapter deadline ends only the native tool: the
    // helper it started survives, and the provider reports its error and exits 1.
    let setup = Setup::new(
        "sleep 60 </dev/null >/dev/null 2>&1 &\n\
         printf '%s\\n%s\\n' \"$!\" \"$(pwd -P)\" > \"$RECORD.tmp\"\n\
         mv \"$RECORD.tmp\" \"$RECORD\"\n\
         wait\n",
    );
    let tmp = setup.record.with_file_name("tmp");
    fs::create_dir_all(&tmp).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kb-code-provider"))
        .args(["--backend", "ast-index", "--tool"])
        .arg(&setup.tool)
        .args(["--timeout", "1"])
        .current_dir(&setup.request.root)
        .env("TMPDIR", &tmp)
        .env_remove(kb::code::PROVIDER_OWNS_GROUP_ENV)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&setup.request).unwrap())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let recorded = fs::read_to_string(&setup.record)
        .unwrap_or_else(|e| panic!("native tool did not start ({e}): {stderr}"));
    let (pid, checkout) = recorded.trim_end().split_once('\n').unwrap();
    let survived = running(pid);
    if survived {
        let _ = Command::new("kill").args(["-9", pid]).status();
    }
    assert_eq!(out.status.code(), Some(1), "{:?}: {stderr}", out.status);
    assert!(
        stderr.contains("subprocess exceeded its configured deadline"),
        "{stderr}"
    );
    assert!(out.stdout.is_empty());
    assert!(
        survived,
        "the provider killed a process group it does not own"
    );
    assert!(
        !Path::new(checkout).exists(),
        "frozen provider checkout {checkout} was left behind"
    );
}
