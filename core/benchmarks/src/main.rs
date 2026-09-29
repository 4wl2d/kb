//! kb-bench: deterministic corpus generation and measurements.
//!
//! Usage:
//!   kb-bench gen --records N [--seed S] --out DIR
//!   kb-bench verify --dir DIR
//!   kb-bench run [--sizes 1000,10000] [--queries 50] [--workdir DIR] [--kb PATH]
//!                [--json FILE] [--skip-cli]
//!   kb-bench smoke            (small corpus, sanity checks only; used by CI)
//!
//! Timings are reported, never asserted. All corpora are synthetic.

mod corpus;
mod measure;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage:\n  kb-bench gen --records N [--seed S] --out DIR\n  kb-bench verify --dir DIR\n  kb-bench run [--sizes 1000,10000] [--queries 50] [--workdir DIR] [--kb PATH] [--json FILE] [--skip-cli]\n  kb-bench smoke"
    );
    ExitCode::from(2)
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn manifest_text() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../release.toml"))
        .expect("core/release.toml is readable")
}

fn is_empty_dir(p: &Path) -> bool {
    !p.exists()
        || std::fs::read_dir(p)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
}

fn fresh_workdir(args: &[String]) -> Result<PathBuf, String> {
    let dir = match arg_value(args, "--workdir") {
        Some(d) => PathBuf::from(d),
        None => std::env::temp_dir().join(format!("kb-bench-{}", std::process::id())),
    };
    if !is_empty_dir(&dir) {
        return Err(format!("work directory {} is not empty", dir.display()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn generate(dir: &Path, records: usize) -> Result<(), String> {
    corpus::generate(dir, corpus::Params { records, seed: 42 }, &manifest_text())
        .map_err(|e| e.to_string())
}

fn smoke() -> Result<(), String> {
    let work = std::env::temp_dir().join(format!("kb-bench-smoke-{}", std::process::id()));
    let dir = work.join("corpus-300");
    generate(&dir, 300)?;
    let r = measure::library(&dir, 300, 5)?;
    if r.mandatory_avg <= 0.0 {
        return Err("queries returned no mandatory knowledge".into());
    }
    println!(
        "smoke ok: 300 records, full index {:.1} ms, incremental {:.1} ms (parsed {}), warm query p50 {:.2} ms over {} queries, statuses {:?}",
        r.full_index_ms,
        r.incremental_index_ms,
        r.incremental_parsed,
        r.warm_query.p50_ms,
        r.warm_query.samples,
        r.statuses
    );
    println!("(work directory: {})", work.display());
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    let sizes: Vec<usize> = arg_value(args, "--sizes")
        .unwrap_or_else(|| "1000,10000".into())
        .split(',')
        .map(|s| {
            s.trim()
                .parse::<usize>()
                .map_err(|_| format!("bad size `{s}`"))
        })
        .collect::<Result<_, _>>()?;
    let nq: usize = arg_value(args, "--queries")
        .map(|s| s.parse().map_err(|_| "bad --queries".to_string()))
        .transpose()?
        .unwrap_or(50);
    let skip_cli = args.iter().any(|a| a == "--skip-cli");
    let kb = arg_value(args, "--kb")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/release/kb"
            ))
        });
    if !skip_cli && !kb.is_file() {
        return Err(format!(
            "kb executable not found at {} (build it with `cargo build --release -p kb` or pass --kb)",
            kb.display()
        ));
    }
    let work = fresh_workdir(args)?;
    let mut libs = Vec::new();
    let mut clis = Vec::new();
    for &n in &sizes {
        eprintln!("kb-bench: generating {n} records");
        let dir = work.join(format!("corpus-{n}"));
        generate(&dir, n)?;
        eprintln!("kb-bench: in-process measurements ({n} records, {nq} queries)");
        libs.push(measure::library(&dir, n, nq)?);
        if !skip_cli {
            eprintln!("kb-bench: CLI measurements ({n} records)");
            clis.push(measure::cli(&kb, &dir, n, nq)?);
        }
    }
    let env = measure::environment();
    println!("## Environment\n");
    for (k, v) in &env {
        println!("- {k}: {v}");
    }
    println!("- kb executable: {}", kb.display());
    println!("- queries per size: {nq} (deterministic set; timings are wall-clock)\n");
    println!("## In-process (library)\n");
    println!(
        "| records | full index ms | warm ensure ms | incremental ms (parsed) | db MiB | warm query p50/p95 ms | assemble p50/p95 ms | response bytes p50/max | mandatory avg | statuses |"
    );
    println!("|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|");
    for r in &libs {
        println!(
            "| {} | {:.1} | {:.2} | {:.1} ({}) | {:.1} | {:.2} / {:.2} | {:.2} / {:.2} | {} / {} | {:.1} | {:?} |",
            r.records,
            r.full_index_ms,
            r.warm_ensure_ms,
            r.incremental_index_ms,
            r.incremental_parsed,
            r.db_bytes as f64 / 1048576.0,
            r.warm_query.p50_ms,
            r.warm_query.p95_ms,
            r.assemble_only.p50_ms,
            r.assemble_only.p95_ms,
            r.response_bytes_p50,
            r.response_bytes_max,
            r.mandatory_avg,
            r.statuses
        );
    }
    if !clis.is_empty() {
        println!("\n## Process level (real `kb` executable)\n");
        println!(
            "| records | startup p50/p95 ms | first sync ms | first index ms | warm offline context p50/p95 ms | online context (local file remote) p50/p95 ms (n) | response bytes | peak RSS MiB |"
        );
        println!("|---:|---:|---:|---:|---:|---:|---:|---:|");
        for c in &clis {
            println!(
                "| {} | {:.2} / {:.2} | {:.1} | {:.1} | {:.2} / {:.2} | {:.2} / {:.2} ({}) | {} | {} |",
                c.records,
                c.startup.p50_ms,
                c.startup.p95_ms,
                c.sync_first_ms,
                c.index_first_ms,
                c.warm_offline_context.p50_ms,
                c.warm_offline_context.p95_ms,
                c.online_context_local_remote.p50_ms,
                c.online_context_local_remote.p95_ms,
                c.online_context_local_remote.samples,
                c.response_bytes,
                c.peak_rss_bytes
                    .map(|b| format!("{:.1}", b as f64 / 1048576.0))
                    .unwrap_or_else(|| "n/a".into())
            );
        }
    }
    if let Some(path) = arg_value(args, "--json") {
        let v =
            serde_json::json!({ "environment": env, "library": libs, "cli": clis, "queries": nq });
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n",
        )
        .map_err(|e| e.to_string())?;
    }
    eprintln!("kb-bench: work directory {}", work.display());
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("gen") => {
            let (Some(n), Some(out)) = (arg_value(&args, "--records"), arg_value(&args, "--out"))
            else {
                return usage();
            };
            let Ok(records) = n.parse::<usize>() else {
                return usage();
            };
            let seed = arg_value(&args, "--seed")
                .and_then(|s| s.parse().ok())
                .unwrap_or(42);
            let out = PathBuf::from(out);
            if !is_empty_dir(&out) {
                eprintln!("kb-bench: output directory {} is not empty", out.display());
                return ExitCode::from(2);
            }
            corpus::generate(&out, corpus::Params { records, seed }, &manifest_text())
                .map(|_| {
                    println!(
                        "generated {records} records (seed {seed}) in {}",
                        out.display()
                    )
                })
                .map_err(|e| e.to_string())
        }
        Some("verify") => {
            let Some(dir) = arg_value(&args, "--dir") else {
                return usage();
            };
            verify(Path::new(&dir))
        }
        Some("run") => run(&args),
        Some("smoke") => smoke(),
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kb-bench: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Parse and validate a corpus directory; fails on any error diagnostic.
fn verify(dir: &Path) -> Result<(), String> {
    let loc = kb::model::ProfileLocation::for_profile(kb::model::Profile::Project);
    let c = kb::corpus::load_corpus(&kb::source::WorkingTreeSource::new(dir), &loc)
        .map_err(|e| e.to_string())?;
    let report = kb::validate::validate_corpus(&c);
    println!(
        "records: {}, errors: {}, warnings: {}",
        report.records, report.errors, report.warnings
    );
    for d in report.diagnostics.iter().take(10) {
        eprintln!("{:?} {} {:?}: {}", d.severity, d.code, d.path, d.message);
    }
    if report.errors == 0 {
        Ok(())
    } else {
        Err("corpus has validation errors".into())
    }
}
