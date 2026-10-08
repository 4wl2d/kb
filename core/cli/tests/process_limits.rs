//! Exercise the real subprocess adapter without a shell, network or external helper.
use std::io::{Read, Write};
use std::process::Command;
use std::time::Duration;

use kb::process::{Limits, capture};

fn child(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "process_fixture_child", "--nocapture"])
        .env("KB_PROCESS_FIXTURE", mode);
    command
}

#[test]
fn process_fixture_child() {
    let Ok(mode) = std::env::var("KB_PROCESS_FIXTURE") else {
        return;
    };
    match mode.as_str() {
        "echo" => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input).unwrap();
            println!("echo:{input}");
            eprintln!("synthetic diagnostic");
            std::process::exit(7);
        }
        "large" => {
            for _ in 0..64 {
                if std::io::stdout().write_all(&[b'x'; 16384]).is_err() {
                    break;
                }
            }
        }
        "sleep" => std::thread::sleep(Duration::from_secs(60)),
        "descendant" => {
            let _child = child("sleep").spawn().unwrap();
            std::process::exit(0);
        }
        _ => panic!("unknown fixture mode"),
    }
}

fn limits() -> Limits {
    Limits {
        timeout: Duration::from_secs(5),
        stdout: 8192,
        stderr: 8192,
    }
}

#[test]
fn preserves_exit_stdout_and_stderr_with_stdin_eof() {
    let result = capture(&mut child("echo"), b"request", limits()).unwrap();
    assert_eq!(result.exit_code, 7);
    assert!(String::from_utf8_lossy(&result.stdout).contains("echo:request"));
    assert!(String::from_utf8_lossy(&result.stderr).contains("synthetic diagnostic"));
}

#[test]
fn refuses_excess_output() {
    let error = capture(&mut child("large"), b"", limits()).err().unwrap();
    assert!(error.message.contains("stdout exceeds"), "{error}");
}

#[test]
fn terminates_at_deadline_including_inherited_output_pipes() {
    for mode in ["sleep", "descendant"] {
        let mut limits = limits();
        limits.timeout = Duration::from_millis(300);
        let error = capture(&mut child(mode), b"", limits).err().unwrap();
        assert_eq!(error.details["kind"], "timeout");
    }
}
