//! The engine's provider deadline also ends the native tool the provider started and
//! removes the provider's frozen checkout. Synthetic host and native tool; no network.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
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

#[test]
fn engine_deadline_ends_native_tool_and_removes_frozen_checkout() {
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
    // Synthetic native tool: it answers the version check, then records its pid and working
    // directory (the frozen checkout) and outlives the engine deadline.
    let record = temp.path().join("native.txt");
    let tool = temp.path().join("ast-index");
    fs::write(
        &tool,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = --version ]; then echo 'ast-index 3.54.0'; exit 0; fi\n\
             printf '%s\\n%s\\n' \"$$\" \"$(pwd -P)\" > '{0}.tmp'\n\
             mv '{0}.tmp' '{0}'\n\
             exec sleep 60\n",
            record.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let mut request: CodeRequest =
        serde_json::from_str(include_str!("fixtures/request.json")).unwrap();
    request.root = host.to_str().unwrap().into();
    request.commit = commit;
    let provider = kb::code::Provider {
        program: Some(env!("CARGO_BIN_EXE_kb-code-provider").into()),
        args: vec![
            "--backend".into(),
            "ast-index".into(),
            "--tool".into(),
            tool.to_str().unwrap().into(),
        ],
        files: Vec::new(),
        timeout_seconds: 2,
    };
    let error = provider.load(&request).unwrap_err();
    assert_eq!(error.details["kind"], "timeout", "{error}");
    let recorded = fs::read_to_string(&record).expect("native tool started before the deadline");
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
    assert!(stopped, "native tool {pid} outlived the provider deadline");
    assert!(
        !Path::new(checkout).exists(),
        "frozen provider checkout {checkout} was left behind"
    );
}
