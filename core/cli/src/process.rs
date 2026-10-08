//! Bounded, shell-free subprocess adapter. No output or duration enters deterministic data
//! unless the caller explicitly chooses it. Each child owns a separate Unix process group.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::error::{ErrorCode, KbError, Result};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub timeout: Duration,
    pub stdout: usize,
    pub stderr: usize,
}

pub struct Captured {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

enum Event {
    Out(std::io::Result<Vec<u8>>),
    Err(std::io::Result<Vec<u8>>),
    Input(std::io::Result<()>),
}

fn stop(child: &mut Child) {
    #[cfg(unix)]
    {
        // The child has not been reaped and was spawned with process_group(0), so its PID
        // cannot be reused and this negative PID names only our owned process group.
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: kill receives a valid signal and a verified, owned process-group id.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub fn capture(command: &mut Command, input: &[u8], limits: Limits) -> Result<Captured> {
    if limits.timeout.is_zero() || limits.stdout == 0 || limits.stderr == 0 {
        return Err(KbError::invalid_input("process limits must be positive"));
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| KbError::io("cannot start subprocess", e))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = mpsc::channel();
    let write_tx = tx.clone();
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        let result = stdin.write_all(&input);
        drop(stdin);
        let _ = write_tx.send(Event::Input(result));
    });
    let out_tx = tx.clone();
    let out_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(limits.stdout as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = out_tx.send(Event::Out(result));
    });
    let err_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stderr
            .take(limits.stderr as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(Event::Err(result));
    });
    let started = Instant::now();
    let mut out = None;
    let mut err = None;
    let mut input_done = false;
    let gathered = (|| -> Result<Captured> {
        while out.is_none() || err.is_none() || !input_done {
            let remaining = limits
                .timeout
                .checked_sub(started.elapsed())
                .ok_or_else(|| timeout(limits))?;
            let event = rx.recv_timeout(remaining).map_err(|_| timeout(limits))?;
            match event {
                Event::Out(result) => {
                    let bytes = result.map_err(|e| KbError::io("subprocess stdout", e))?;
                    if bytes.len() > limits.stdout {
                        return Err(KbError::invalid_input(
                            "subprocess stdout exceeds the byte limit",
                        ));
                    }
                    out = Some(bytes);
                }
                Event::Err(result) => {
                    let bytes = result.map_err(|e| KbError::io("subprocess stderr", e))?;
                    if bytes.len() > limits.stderr {
                        return Err(KbError::invalid_input(
                            "subprocess stderr exceeds the byte limit",
                        ));
                    }
                    err = Some(bytes);
                }
                Event::Input(result) => {
                    // A failing program may close stdin without consuming the request.
                    if let Err(e) = result
                        && e.kind() != std::io::ErrorKind::BrokenPipe
                    {
                        return Err(KbError::io("subprocess stdin", e));
                    }
                    input_done = true;
                }
            }
        }
        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|e| KbError::io("subprocess wait", e))?
            {
                return Ok(Captured {
                    exit_code: status.code().unwrap_or(-1),
                    stdout: out.take().unwrap_or_default(),
                    stderr: err.take().unwrap_or_default(),
                });
            }
            let remaining = limits
                .timeout
                .checked_sub(started.elapsed())
                .ok_or_else(|| timeout(limits))?;
            // Only programs that close both output pipes before exiting reach this branch.
            std::thread::park_timeout(remaining.min(Duration::from_millis(10)));
        }
    })();
    if gathered.is_err() {
        stop(&mut child);
    }
    // Never let a nonconforming daemon that escaped its process group hold up a timeout.
    for thread in [writer, out_reader, err_reader] {
        if thread.is_finished() {
            let _ = thread.join();
        }
    }
    gathered
}

fn timeout(limits: Limits) -> KbError {
    KbError::new(
        ErrorCode::InvalidInput,
        "subprocess exceeded its configured deadline",
    )
    .with_details(serde_json::json!({"kind":"timeout", "timeout_ms":limits.timeout.as_millis()}))
}
