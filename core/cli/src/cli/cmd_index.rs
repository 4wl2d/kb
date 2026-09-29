//! `kb index` handler: build or reuse the derived index for the selected snapshot.
//!
//! `kb index` never contacts the remote: the selection (`--snapshot`, default `auto`) is
//! resolved against the last known approved revision, and the output says that freshness
//! was not verified. `--rebuild` drops the cache first; `--gc` removes cached snapshots
//! older than the selected one plus documents no snapshot references.

use std::fmt::Write as _;

use serde_json::json;

use super::Ctx;
use super::args::IndexArgs;
use super::session::{self, Options, Session};
use crate::error::Result;
use crate::index::{BuildStats, IndexStats};
use crate::output::{CommandOutput, Format};

/// Stated in every `kb index` result.
const FRESHNESS_NOTE: &str = "kb index does not contact the remote, so freshness is not \
     verified; reading commands (context, search, show, impact) check the approved ref on \
     every call unless --offline";

pub fn run(ctx: &Ctx, args: &IndexArgs) -> Result<CommandOutput> {
    let host = session::detect_host(ctx)?;
    let opts = Options {
        include_proposals: false,
        offline: true,
        rebuild: args.rebuild,
    };
    let mut s = Session::open(ctx, host, opts)?;
    let gc = if args.gc {
        let stats = s.index.stats()?;
        // Keep the selected snapshot and every snapshot built after it (other checkouts may
        // be using those); remove the older ones.
        let keep = stats
            .snapshots
            .iter()
            .position(|x| x.key == s.info.key)
            .map_or(1, |i| i + 1);
        let removed = s.index.gc(keep)?;
        ctx.env.progress(format!(
            "index: gc removed {removed} snapshot(s), kept {keep}"
        ));
        Some(json!({ "keep": keep, "removed": removed }))
    } else {
        None
    };
    let stats = s.index.stats()?;
    // The cache location and its size in bytes depend on the machine and on SQLite page
    // reuse: they are metadata (`meta.cache`), never part of the deterministic result.
    let mut index = serde_json::to_value(&stats)?;
    if let Some(m) = index.as_object_mut() {
        m.remove("path");
        m.remove("bytes");
    }
    let result = json!({
        "snapshot": s.info,
        "build": s.build,
        "gc": gc,
        "index": index,
        "freshness_verified": false,
        "note": FRESHNESS_NOTE,
    });
    let text = render(ctx.format, &s, gc.as_ref(), &stats);
    let mut out = CommandOutput::new(result, text);
    out.meta.insert(
        "cache".into(),
        json!({ "path": stats.path, "bytes": stats.bytes }),
    );
    Ok(s.finish(out))
}

fn render(
    format: Format,
    s: &Session,
    gc: Option<&serde_json::Value>,
    stats: &IndexStats,
) -> String {
    let b: &BuildStats = &s.build;
    let mut o = String::new();
    let _ = writeln!(o, "kb index: snapshot {}", s.info.label());
    let action = if b.reused {
        "reused (already indexed; nothing listed or parsed)"
    } else {
        "built"
    };
    let _ = writeln!(o, "  snapshot key: {}", b.snapshot_key);
    let _ = writeln!(o, "  build:        {action}");
    let _ = writeln!(
        o,
        "  records:      {} files, {} parsed now, {} reused, {} proposals",
        b.files, b.parsed, b.reused_docs, b.proposals
    );
    let _ = writeln!(
        o,
        "  diagnostics:  {} errors, {} warnings (run `kbw validate` for details)",
        b.errors, b.warnings
    );
    if b.collected > 0 {
        let _ = writeln!(o, "  retention:    removed {} old snapshot(s)", b.collected);
    }
    if let Some(gc) = gc {
        let _ = writeln!(
            o,
            "  gc:           removed {} snapshot(s), kept {}",
            gc["removed"], gc["keep"]
        );
    }
    let _ = writeln!(
        o,
        "  cache:        {} ({} snapshots, {} documents, {} bytes)",
        stats.path.display(),
        stats.snapshots.len(),
        stats.docs,
        stats.bytes
    );
    if let Some(r) = &b.recovery {
        let _ = writeln!(o, "  recovery:     {}", r.diagnostic().message);
    }
    if format == Format::Human {
        for x in &stats.snapshots {
            let mark = if x.key == s.info.key { "*" } else { " " };
            let _ = writeln!(
                o,
                "   {mark} {} build #{} files {} proposals {} errors {} warnings {}",
                &x.key[..x.key.len().min(16)],
                x.created,
                x.files,
                x.proposals,
                x.errors,
                x.warnings
            );
        }
    }
    let _ = writeln!(o, "  note: {FRESHNESS_NOTE}");
    o
}
