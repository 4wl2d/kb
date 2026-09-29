//! Measurements: in-process indexing and retrieval, and process-level CLI timings.
//!
//! Timings are reported, never asserted. Sanity checks (zero validation errors, incremental
//! builds parse only the changed file, deterministic output) fail the run.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use kb::context::{self, ContextRequest, TaskEnv};
use kb::index::Index;
use kb::knowledge::{Freshness, Selection, SnapshotInfo};
use kb::model::{Intent, Profile, ProfileLocation};
use kb::output::Format;
use kb::source::WorkingTreeSource;
use serde::Serialize;

use crate::corpus::{self, REPOS};

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub samples: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
}

pub fn stats(mut v: Vec<Duration>) -> Stats {
    v.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let pick = |q: f64| {
        let i = ((v.len() as f64 - 1.0) * q).round() as usize;
        ms(v[i.min(v.len() - 1)])
    };
    Stats {
        samples: v.len(),
        p50_ms: pick(0.50),
        p95_ms: pick(0.95),
        min_ms: ms(v[0]),
        max_ms: ms(v[v.len() - 1]),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryReport {
    pub records: usize,
    pub full_index_ms: f64,
    pub warm_ensure_ms: f64,
    pub incremental_index_ms: f64,
    pub incremental_parsed: usize,
    pub db_bytes: u64,
    /// Open index + view + assemble + render (compact), per query.
    pub warm_query: Stats,
    /// assemble() alone, per query.
    pub assemble_only: Stats,
    pub response_bytes_p50: usize,
    pub response_bytes_max: usize,
    pub mandatory_avg: f64,
    pub statuses: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CliReport {
    pub records: usize,
    pub startup: Stats,
    pub sync_first_ms: f64,
    pub index_first_ms: f64,
    /// `kb --offline context` on a warm index (process spawn + git reads + retrieval).
    pub warm_offline_context: Stats,
    /// `kb context` with the freshness fetch from a local file remote.
    pub online_context_local_remote: Stats,
    pub response_bytes: usize,
    pub peak_rss_bytes: Option<u64>,
}

fn working_tree_snapshot(key: &str) -> SnapshotInfo {
    SnapshotInfo {
        profile: "project".into(),
        remote: "origin".into(),
        source: "bench (synthetic)".into(),
        approved_ref: "refs/heads/main".into(),
        selection: Selection::WorkingTree,
        freshness: Freshness::Unverified,
        revision: None,
        latest_approved: None,
        approved: Some(false),
        pin: None,
        overlay: None,
        engine_version: kb::versions::ENGINE_VERSION.into(),
        key: key.into(),
    }
}

/// Deterministic query set over the generated registry.
pub fn queries(records: usize, n: usize) -> Vec<ContextRequest> {
    let mpr = corpus::modules_per_repo(records);
    const TASKS: [&str; 6] = [
        "fix token refresh race",
        "add payment retry with timeout",
        "refactor cache invalidation",
        "review audit logging of consent",
        "исправить кэш профиля",
        "debug search queue worker",
    ];
    (0..n)
        .map(|q| {
            let repo = REPOS[q % REPOS.len()];
            let m = (q * 7) % mpr;
            let mut r = ContextRequest::new(Intent::Implement);
            r.repos = vec![repo.to_string()];
            r.paths = vec![format!("src/m{m:02}/file{q}.rs")];
            r.task = Some(TASKS[q % TASKS.len()].to_string());
            r.budget = Some(16_000);
            r
        })
        .collect()
}

fn dir_size(p: &Path) -> u64 {
    fs::read_dir(p)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// In-process measurement over a generated corpus in `dir` (already generated).
pub fn library(dir: &Path, records: usize, nq: usize) -> Result<LibraryReport, String> {
    let loc = ProfileLocation::for_profile(Profile::Project);
    let cache = dir.join(".cache-bench");
    // Production working-tree path: content ids are reused through the stat cache. Files
    // younger than the racy window are always re-hashed, so let the generated corpus age.
    std::thread::sleep(Duration::from_millis(2100));
    let src = WorkingTreeSource::with_stat_cache(dir, cache.join("wt-stat.json"));
    let e = |e: kb::error::KbError| e.to_string();

    let t = Instant::now();
    let mut ix = Index::open(&cache, Profile::Project).map_err(e)?;
    let st = ix.ensure("bench-v1", &src, &loc, None).map_err(e)?;
    let full = t.elapsed();
    if st.errors != 0 {
        return Err(format!(
            "generated corpus has {} validation errors",
            st.errors
        ));
    }
    let t = Instant::now();
    let warm = ix.ensure("bench-v1", &src, &loc, None).map_err(e)?;
    let warm_ensure = t.elapsed();
    if !warm.reused {
        return Err("warm ensure did not reuse the snapshot".into());
    }

    // Incremental: change one record body; only that file must be parsed.
    let victim = find_record(dir).ok_or("no record found")?;
    let mut text = fs::read_to_string(&victim).map_err(|e| e.to_string())?;
    text.push_str("\nIncremental benchmark edit.\n");
    fs::write(&victim, text).map_err(|e| e.to_string())?;
    let t = Instant::now();
    let inc = ix.ensure("bench-v2", &src, &loc, None).map_err(e)?;
    let incremental = t.elapsed();
    if inc.parsed != 1 {
        return Err(format!(
            "incremental build parsed {} files, expected 1",
            inc.parsed
        ));
    }
    drop(ix);

    let env = TaskEnv::new(working_tree_snapshot("bench-v2"));
    let mut total = Vec::new();
    let mut only = Vec::new();
    let mut sizes = Vec::new();
    let mut mandatory = 0usize;
    let mut statuses: std::collections::BTreeMap<String, usize> = Default::default();
    for (i, req) in queries(records, nq).iter().enumerate() {
        let t = Instant::now();
        let ix = Index::open(&cache, Profile::Project).map_err(e)?;
        let view = ix.view("bench-v2").map_err(e)?;
        let t2 = Instant::now();
        let r = context::assemble(req, &env, &view, Format::Compact).map_err(e)?;
        only.push(t2.elapsed());
        let out = context::render(&r, Format::Compact, false);
        total.push(t.elapsed());
        if i == 0 {
            // Determinism: the same request renders byte-identically.
            let again = context::render(
                &context::assemble(req, &env, &view, Format::Compact).map_err(e)?,
                Format::Compact,
                false,
            );
            if again != out {
                return Err("context output is not deterministic".into());
            }
        }
        sizes.push(out.len());
        mandatory += r.mandatory_ids().len();
        *statuses.entry(r.status().as_str().to_string()).or_default() += 1;
    }
    sizes.sort();
    Ok(LibraryReport {
        records,
        full_index_ms: full.as_secs_f64() * 1000.0,
        warm_ensure_ms: warm_ensure.as_secs_f64() * 1000.0,
        incremental_index_ms: incremental.as_secs_f64() * 1000.0,
        incremental_parsed: inc.parsed,
        db_bytes: dir_size(&cache.join("index")),
        warm_query: stats(total),
        assemble_only: stats(only),
        response_bytes_p50: sizes[sizes.len() / 2],
        response_bytes_max: *sizes.last().unwrap_or(&0),
        mandatory_avg: mandatory as f64 / nq.max(1) as f64,
        statuses: statuses.into_iter().collect(),
    })
}

fn find_record(dir: &Path) -> Option<PathBuf> {
    let base = dir.join("project/knowledge/mobile/policy");
    let mut v: Vec<PathBuf> = fs::read_dir(base)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    v.sort();
    v.into_iter().nth(3)
}

/// Git with a reproducible, isolated configuration for benchmark repositories.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "protocol.file.allow=always",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "kb-bench")
        .env("GIT_AUTHOR_EMAIL", "kb-bench@example.invalid")
        .env("GIT_COMMITTER_NAME", "kb-bench")
        .env("GIT_COMMITTER_EMAIL", "kb-bench@example.invalid")
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn kb_cmd(kb: &Path, root: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(kb);
    c.current_dir(root)
        .arg("--root")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("KB_ROOT")
        .env_remove("KB_CACHE_DIR")
        .stdin(Stdio::null());
    c
}

fn time_run(mut c: Command) -> Result<(Duration, Vec<u8>), String> {
    let t = Instant::now();
    let out = c.output().map_err(|e| e.to_string())?;
    let d = t.elapsed();
    // Exit 30 (CONTEXT_INCOMPLETE) still carries a full result.
    match out.status.code() {
        Some(0) | Some(30) => Ok((d, out.stdout)),
        other => Err(format!(
            "command failed ({other:?}): {}",
            String::from_utf8_lossy(&out.stderr)
        )),
    }
}

fn context_args(q: &ContextRequest) -> Vec<String> {
    let mut a = vec!["context".to_string(), "--intent".into(), "implement".into()];
    if let Some(b) = q.budget {
        // Same budget as the in-process measurement.
        a.push("--budget".into());
        a.push(b.to_string());
    }
    for r in &q.repos {
        a.push("--repo".into());
        a.push(r.clone());
    }
    for p in &q.paths {
        a.push("--path".into());
        a.push(p.clone());
    }
    if let Some(t) = &q.task {
        a.push("--task".into());
        a.push(t.clone());
    }
    a
}

/// Peak RSS of one command through /usr/bin/time (macOS `-l` bytes, GNU `-v` kbytes).
fn peak_rss(kb: &Path, root: &Path, args: &[String]) -> Option<u64> {
    let time = Path::new("/usr/bin/time");
    if !time.exists() {
        return None;
    }
    let flag = if cfg!(target_os = "macos") {
        "-l"
    } else {
        "-v"
    };
    let out = Command::new(time)
        .arg(flag)
        .arg(kb)
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let err = String::from_utf8_lossy(&out.stderr);
    for line in err.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_suffix("maximum resident set size") {
            return v.trim().parse().ok();
        }
        if let Some(v) = l.strip_prefix("Maximum resident set size (kbytes):") {
            return v.trim().parse::<u64>().ok().map(|k| k * 1024);
        }
    }
    None
}

/// Process-level measurement with a real Git snapshot and a local file remote.
pub fn cli(kb: &Path, dir: &Path, records: usize, nq: usize) -> Result<CliReport, String> {
    git(dir, &["init", "-q", "-b", "main"])?;
    fs::write(dir.join(".gitignore"), "/.cache/\n/.cache-bench/\n").map_err(|e| e.to_string())?;
    git(dir, &["add", "-A"])?;
    git(dir, &["commit", "-q", "-m", "synthetic benchmark corpus"])?;
    let origin = dir.with_extension("origin.git");
    fs::create_dir_all(&origin).map_err(|e| e.to_string())?;
    git(&origin, &["init", "-q", "--bare", "-b", "main"])?;
    git(
        dir,
        &["remote", "add", "origin", origin.to_str().ok_or("path")?],
    )?;
    git(dir, &["push", "-q", "origin", "main"])?;
    git(dir, &["fetch", "-q", "origin"])?;

    let mut startup = Vec::new();
    for _ in 0..20 {
        startup.push(time_run(kb_cmd(kb, dir, &["version"]))?.0);
    }
    let (sync_first, _) = time_run(kb_cmd(kb, dir, &["--quiet", "sync"]))?;
    let (index_first, _) = time_run(kb_cmd(
        kb,
        dir,
        &["--quiet", "index", "--snapshot", "latest"],
    ))?;
    let qs = queries(records, nq);
    let mut warm = Vec::new();
    let mut bytes = 0;
    for q in &qs {
        let mut args = vec!["--quiet".to_string(), "--offline".into()];
        args.extend(context_args(q));
        let (d, out) = time_run(kb_cmd(
            kb,
            dir,
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        ))?;
        warm.push(d);
        bytes = out.len();
    }
    let mut online = Vec::new();
    for q in qs.iter().take((nq / 5).max(3)) {
        let mut args = vec!["--quiet".to_string()];
        args.extend(context_args(q));
        online.push(
            time_run(kb_cmd(
                kb,
                dir,
                &args.iter().map(String::as_str).collect::<Vec<_>>(),
            ))?
            .0,
        );
    }
    let mut rss_args = vec!["--quiet".to_string(), "--offline".into()];
    rss_args.extend(context_args(&qs[0]));
    Ok(CliReport {
        records,
        startup: stats(startup),
        sync_first_ms: sync_first.as_secs_f64() * 1000.0,
        index_first_ms: index_first.as_secs_f64() * 1000.0,
        warm_offline_context: stats(warm),
        online_context_local_remote: stats(online),
        response_bytes: bytes,
        peak_rss_bytes: peak_rss(kb, dir, &rss_args),
    })
}

/// Environment description for the report.
pub fn environment() -> Vec<(String, String)> {
    let run = |cmd: &str, args: &[&str]| {
        Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".into())
    };
    let cpu = if cfg!(target_os = "macos") {
        run("sysctl", &["-n", "machdep.cpu.brand_string"])
    } else {
        fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("model name"))
                    .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
            })
            .unwrap_or_else(|| "unknown".into())
    };
    vec![
        ("os".into(), run("uname", &["-srm"])),
        ("cpu".into(), cpu),
        ("rustc".into(), run("rustc", &["--version"])),
        ("git".into(), run("git", &["--version"])),
        (
            "kb-bench build".into(),
            if cfg!(debug_assertions) {
                "debug".into()
            } else {
                "release".into()
            },
        ),
        ("engine".into(), kb::versions::ENGINE_VERSION.into()),
    ]
}
