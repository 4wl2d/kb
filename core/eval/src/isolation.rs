//! Fail-closed OS isolation. Agent, tests and judges use different writable directories.
mod proxy;
use crate::{
    Result, ensure, files,
    model::{Isolation, Program},
};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Area {
    pub root: PathBuf,
    pub work: PathBuf,
    pub home: PathBuf,
    pub temp: PathBuf,
    pub private: PathBuf,
}
impl Area {
    pub fn create(root: &Path) -> Result<Self> {
        fs::create_dir(root)?;
        let root = root.canonicalize()?;
        let area = Self {
            work: root.join("work"),
            home: root.join("home"),
            temp: root.join("tmp"),
            private: root.join("private"),
            root,
        };
        for dir in [&area.home, &area.temp, &area.private] {
            fs::create_dir(dir)?;
        }
        // `run` points CODEX_HOME here; Codex refuses to load its configuration without it.
        fs::create_dir(area.home.join(".codex"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&area.root, fs::Permissions::from_mode(0o700))?;
        }
        Ok(area)
    }
}

pub fn validate(policy: &Isolation, protected: &[PathBuf]) -> Result<()> {
    for root in &policy.read_roots {
        ensure(root.is_absolute(), "runtime roots must be absolute")?;
        let root = root.canonicalize()?;
        ensure(
            root.parent().is_some() && root.components().count() > 2,
            "runtime root is too broad",
        )?;
        for p in protected {
            let p = p.canonicalize()?;
            ensure(
                !p.starts_with(&root) && !root.starts_with(&p),
                format!(
                    "runtime root overlaps private input/output: {}",
                    root.display()
                ),
            )?;
        }
        if let Some(home) = std::env::var_os("HOME") {
            ensure(
                !PathBuf::from(home).starts_with(&root),
                "runtime root exposes real HOME",
            )?;
        }
    }
    for (key, value) in &policy.environment {
        ensure(
            !key.is_empty()
                && key
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                && !value.contains('\0'),
            "invalid environment entry",
        )?;
        ensure(
            ![
                "HOME", "PATH", "TMPDIR", "TMP", "TEMP", "ENV", "BASH_ENV", "ZDOTDIR", "CDPATH",
                "SHELL",
            ]
            .contains(&key.as_str())
                && ![
                    "GIT_", "XDG_", "CODEX_", "CLAUDE_", "CURSOR_", "GROK_", "LD_", "DYLD_",
                ]
                .iter()
                .any(|p| key.starts_with(p))
                && !key.contains("KEY")
                && !key.contains("TOKEN")
                && !key.contains("SECRET")
                && !key.contains("PROXY"),
            "reserved or secret environment setting",
        )?;
    }
    for host in &policy.api_hosts {
        proxy::host(host)?;
    }
    ensure(policy.api_hosts.len() <= 16, "too many API hosts")
}

fn quote(path: &Path) -> Result<String> {
    Ok(serde_json::to_string(
        path.to_str().ok_or("non-UTF-8 sandbox path")?,
    )?)
}

pub fn seatbelt(
    area: &Area,
    policy: &Isolation,
    executables: &[PathBuf],
    proxy_port: Option<u16>,
) -> Result<String> {
    let mut text = String::from(
        "(version 1)\n(deny default)\n(import \"dyld-support.sb\")\n(allow process-exec process-fork sysctl-read)\n(allow mach-lookup (global-name \"com.apple.trustd\") (global-name \"com.apple.system.logger\"))\n(allow file-read-metadata)\n",
    );
    for dir in [
        "/System",
        "/usr",
        "/bin",
        "/sbin",
        "/Library/Apple",
        "/private/etc/ssl",
        "/private/etc/hosts",
        "/private/etc/localtime",
    ] {
        text.push_str(&format!(
            "(allow file-read* file-map-executable (subpath {}))\n",
            quote(Path::new(dir))?
        ));
    }
    for dir in &policy.read_roots {
        text.push_str(&format!(
            "(allow file-read* file-map-executable (subpath {}))\n",
            quote(&dir.canonicalize()?)?
        ));
    }
    for exe in executables {
        text.push_str(&format!(
            "(allow file-read* file-map-executable (literal {}))\n",
            quote(&exe.canonicalize()?)?
        ));
    }
    for dir in [&area.work, &area.home, &area.temp] {
        text.push_str(&format!(
            "(allow file-read* file-write* file-map-executable (subpath {}))\n",
            quote(dir)?
        ));
    }
    text.push_str("(allow file-read* file-write* (literal \"/dev/null\"))\n(allow file-read* (literal \"/dev/urandom\") (literal \"/dev/random\"))\n");
    if let Some(port) = proxy_port {
        // Seatbelt accepts only `*` or `localhost` as the host. `localhost` admits the port on
        // every local address (IPv4/IPv6 loopback and the host's interface addresses), not
        // only on the proxy's 127.0.0.1 listener; the proxy also holds the port on `[::1]`.
        text.push_str(&format!(
            "(allow network-outbound (remote ip \"localhost:{port}\"))\n"
        ));
    }
    Ok(text)
}

fn wrap(
    area: &Area,
    policy: &Isolation,
    program: &Program,
    name: &str,
    egress: Option<&proxy::Proxy>,
) -> Result<Command> {
    let executable = if program.program.is_absolute() {
        program.program.canonicalize()?
    } else {
        area.work.join(&program.program).canonicalize()?
    };
    let current = std::env::current_exe()?.canonicalize()?;
    #[cfg(target_os = "macos")]
    {
        ensure(
            Path::new("/usr/bin/sandbox-exec").is_file(),
            "Seatbelt launcher unavailable; no unsandboxed fallback",
        )?;
        let profile = area.private.join(format!("{name}.sb"));
        let mut policy_text = seatbelt(
            area,
            policy,
            &[executable.clone(), current],
            egress.map(|p| p.port),
        )?;
        for file in [format!("{name}.stdout.jsonl"), format!("{name}.stderr.log")] {
            policy_text.push_str(&format!(
                "(allow file-write-data (literal {}))\n",
                quote(&area.private.join(file))?
            ));
        }
        files::write_new(&profile, policy_text.as_bytes())?;
        let mut c = Command::new("/usr/bin/sandbox-exec");
        c.arg("-f").arg(profile).arg(executable).args(&program.args);
        Ok(c)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = name;
        let mut c = Command::new("bwrap");
        c.args([
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
        ]);
        for path in [
            "/usr",
            "/bin",
            "/sbin",
            "/lib",
            "/lib64",
            "/etc/ssl",
            "/etc/pki",
            "/etc/ld.so.cache",
        ] {
            if Path::new(path).exists() {
                c.args(["--ro-bind", path, path]);
            }
        }
        for path in &policy.read_roots {
            let path = path.canonicalize()?;
            c.arg("--ro-bind").arg(&path).arg(&path);
        }
        for path in [&executable, &current] {
            c.arg("--ro-bind").arg(path).arg(path);
        }
        for path in [&area.work, &area.home, &area.temp] {
            c.arg("--bind").arg(path).arg(path);
        }
        c.arg("--chdir").arg(&area.work);
        if let Some(proxy) = egress {
            c.arg("--ro-bind")
                .arg(&proxy.socket)
                .arg(&proxy.socket)
                .arg("--")
                .arg(current)
                .arg("_proxy-exec")
                .arg(&proxy.socket)
                .arg("--")
                .arg(executable)
                .args(&program.args);
        } else {
            c.arg("--").arg(executable).args(&program.args);
        }
        Ok(c)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (area, policy, program, name, egress, current, executable);
        Err("replay supports Seatbelt or bubblewrap only".into())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub timed_out: bool,
    pub output_limited: bool,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub elapsed_ms: u64,
}
impl Outcome {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && !self.output_limited
    }
}

pub fn run(
    area: &Area,
    policy: &Isolation,
    program: &Program,
    name: &str,
    secret_env: &[String],
    network: bool,
) -> Result<Outcome> {
    program.validate()?;
    crate::model::id(name)?;
    let egress = if network && !policy.api_hosts.is_empty() {
        Some(proxy::Proxy::start(&policy.api_hosts)?)
    } else {
        None
    };
    let mut command = wrap(area, policy, program, name, egress.as_ref())?;
    files::isolated_env(&mut command, &area.home);
    command
        .current_dir(&area.work)
        .env("TMPDIR", &area.temp)
        .env("TMP", &area.temp)
        .env("TEMP", &area.temp)
        .env("CODEX_HOME", area.home.join(".codex"))
        .env("GROK_AGENT_DASHBOARD", "0")
        .env("NO_OPEN_BROWSER", "1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_AUTOUPDATER", "1")
        .env("NO_PROXY", "")
        .env("no_proxy", "");
    for (k, v) in &policy.environment {
        command.env(k, v);
    }
    for name in secret_env {
        let value = std::env::var_os(name)
            .ok_or_else(|| format!("required credential variable {name} is unset"))?;
        command.env(name, value);
    }
    if let Some(proxy) = &egress {
        let url = format!("http://127.0.0.1:{}", proxy.port);
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            command.env(name, &url);
        }
    }
    let out_path = area.private.join(format!("{name}.stdout.jsonl"));
    let err_path = area.private.join(format!("{name}.stderr.log"));
    command
        .stdin(Stdio::null())
        .stdout(
            fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&out_path)?,
        )
        .stderr(
            fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&err_path)?,
        );
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let start = Instant::now();
    let mut child = command.spawn()?;
    let (status, timed_out, output_limited) = loop {
        let timeout = start.elapsed() >= Duration::from_secs(program.timeout_seconds);
        let limited = fs::metadata(&out_path)?.len() > 128 * 1024 * 1024
            || fs::metadata(&err_path)?.len() > 32 * 1024 * 1024;
        if timeout || limited {
            #[cfg(unix)]
            if let Ok(pid) = i32::try_from(child.id()) {
                // SAFETY: owned, unreaped child leads this freshly created process group.
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
            let _ = child.kill();
            break (child.wait()?, timeout, limited);
        }
        if let Some(status) = child.try_wait()? {
            break (status, false, false);
        }
        std::thread::park_timeout(Duration::from_millis(20));
    };
    drop(egress);
    let elapsed_ms = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    // Logs remain on disk on failure/timeouts. The limit is not a reason to discard evidence.
    let out = files::read(&out_path, 256 * 1024 * 1024)?;
    let err = files::read(&err_path, 64 * 1024 * 1024)?;
    let result = Outcome {
        exit_code: status.code(),
        signal: {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                status.signal()
            }
            #[cfg(not(unix))]
            {
                None
            }
        },
        timed_out,
        output_limited,
        stdout_sha256: files::digest(&out),
        stderr_sha256: files::digest(&err),
        elapsed_ms,
    };
    files::save(&area.private.join(format!("{name}.outcome.json")), &result)?;
    Ok(result)
}

pub fn check(area: &Area, policy: &Isolation) -> Result<()> {
    let allowed = area.work.join(".kb-eval-canary");
    let forbidden = area.private.join("canary.txt");
    files::write_new(&allowed, b"synthetic allowed canary")?;
    files::write_new(&forbidden, b"synthetic forbidden canary")?;
    let p = Program {
        program: std::env::current_exe()?,
        args: vec![
            "_probe".into(),
            allowed.to_string_lossy().into_owned(),
            forbidden.to_string_lossy().into_owned(),
        ],
        timeout_seconds: 10,
    };
    let outcome = run(area, policy, &p, "isolation-probe", &[], false)?;
    ensure(
        outcome.success(),
        format!(
            "isolation canary failed; no agent was started. Probe stderr ({}): {}",
            area.private.display(),
            String::from_utf8_lossy(&files::read(
                &area.private.join("isolation-probe.stderr.log"),
                1024 * 1024
            )?)
        ),
    )
}
pub fn probe(allowed: &Path, forbidden: &Path) -> Result<()> {
    ensure(
        fs::read(allowed)?.starts_with(b"synthetic"),
        "allowed file unreadable",
    )?;
    ensure(
        fs::read(forbidden).is_err(),
        "sandbox can read private files",
    )?;
    ensure(
        fs::OpenOptions::new().append(true).open(forbidden).is_err(),
        "sandbox can write private files",
    )?;
    Ok(())
}
pub fn proxy_exec(socket: &Path, program: &Path, args: &[String]) -> Result<i32> {
    proxy::bridge(socket, program, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_grants_only_designated_writes_and_proxy_endpoint() {
        let t = tempfile::tempdir().unwrap();
        let a = Area::create(&t.path().join("trial")).unwrap();
        fs::create_dir(&a.work).unwrap();
        let p = Isolation {
            read_roots: vec![],
            environment: Default::default(),
            api_hosts: vec![],
        };
        assert!(a.home.join(".codex").is_dir());
        let profile = seatbelt(&a, &p, &[], Some(3210)).unwrap();
        assert!(profile.starts_with("(version 1)\n(deny default)"));
        assert!(profile.contains("(allow network-outbound (remote ip \"localhost:3210\"))"));
        assert!(!profile.contains(&format!("(subpath {})", quote(&a.private).unwrap())));
        assert!(!profile.contains("(allow network*)"));
    }
    #[test]
    fn broad_runtime_mounts_and_environment_escape_are_refused() {
        let t = tempfile::tempdir().unwrap();
        let private = t.path().join("kit");
        fs::create_dir(&private).unwrap();
        let mut p = Isolation {
            read_roots: vec![t.path().to_path_buf()],
            environment: Default::default(),
            api_hosts: vec![],
        };
        assert!(validate(&p, std::slice::from_ref(&private)).is_err());
        p.read_roots.clear();
        p.environment
            .insert("BASH_ENV".into(), "/host/private.sh".into());
        assert!(validate(&p, &[private]).is_err());
    }
}
