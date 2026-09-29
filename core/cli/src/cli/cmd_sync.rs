//! `kb sync` handler: fetch the approved ref into the isolated mirror and report the approved
//! tip, the local KB checkout and the host pin. Never modifies the checkout or the host.

use std::fmt::Write as _;

use super::Ctx;
use super::args::SyncArgs;
use crate::error::Result;
use crate::host;
use crate::knowledge::{Freshness, PinSource};
use crate::output::CommandOutput;
use crate::snapshot::{self, PinStatus, SyncReport};

pub fn run(ctx: &Ctx, _args: &SyncArgs) -> Result<CommandOutput> {
    let loc = ctx.location()?;
    let host = host::detect(&ctx.env, ctx.global.host.as_deref())?;
    let report = snapshot::sync_with(&ctx.env, &loc, host.as_ref(), ctx.global.offline)?;
    let text = render(&report);
    Ok(CommandOutput::new(serde_json::to_value(&report)?, text))
}

fn short(rev: &str) -> String {
    rev.chars().take(12).collect()
}

fn render(r: &SyncReport) -> String {
    let mut s = String::new();
    let source = if r.source.is_empty() {
        String::new()
    } else {
        format!(" ({})", r.source)
    };
    let _ = writeln!(s, "kb sync: {} on {}{source}", r.approved_ref, r.remote);
    let freshness = match r.freshness {
        Freshness::Verified => "verified (fetched now)",
        Freshness::Unverified => "unverified (offline, last known state)",
    };
    let _ = writeln!(s, "  freshness:      {freshness}");
    let tip = match (&r.latest_approved, &r.previous_approved) {
        (None, _) => "unknown".to_string(),
        (Some(t), Some(p)) if t == p => format!("{} (unchanged)", short(t)),
        (Some(t), Some(p)) => format!("{} (was {})", short(t), short(p)),
        (Some(t), None) => format!("{} (first fetch)", short(t)),
    };
    let _ = writeln!(s, "  approved tip:   {tip}");

    let l = &r.local;
    if !l.git {
        let _ = writeln!(s, "  local KB:       not a Git checkout");
    } else {
        let head = l
            .head
            .as_deref()
            .map(short)
            .unwrap_or_else(|| "no commits".into());
        let on = l
            .branch
            .as_deref()
            .map(|b| format!(" on {b}"))
            .unwrap_or_else(|| " (detached)".into());
        let rel = match (l.ahead, l.behind) {
            (Some(0), Some(0)) => ": same as approved".to_string(),
            (Some(a), Some(b)) => format!(": {a} ahead, {b} behind approved"),
            _ => String::new(),
        };
        let _ = writeln!(s, "  local KB HEAD:  {head}{on}{rel}");
        let tree = if l.dirty {
            format!("dirty ({} changes)", l.changes)
        } else {
            "clean".into()
        };
        let _ = writeln!(s, "  working tree:   {tree}");
    }

    match &r.host {
        None => {
            let _ = writeln!(s, "  host:           none detected");
        }
        Some(h) => {
            let wt = if h.linked_worktree {
                " (linked worktree)"
            } else {
                ""
            };
            let _ = writeln!(s, "  host:           {}{wt}", h.root);
            let pin = match &h.pin {
                None => "none".to_string(),
                Some(p) => {
                    let from = match (p.source, &p.path) {
                        (PinSource::Submodule, Some(path)) => format!("submodule {path}"),
                        (PinSource::Submodule, None) => "submodule".into(),
                        (PinSource::BindingFile, _) => ".kbw.toml".into(),
                    };
                    let status = match h.status {
                        PinStatus::NoPin => String::new(),
                        PinStatus::Current => ": current".into(),
                        PinStatus::Behind => {
                            format!(": {} commits behind approved", h.behind.unwrap_or(0))
                        }
                        PinStatus::Ahead => format!(
                            ": {} commits ahead of approved (not approved)",
                            h.ahead.unwrap_or(0)
                        ),
                        PinStatus::Diverged => format!(
                            ": diverged ({} ahead, {} behind approved)",
                            h.ahead.unwrap_or(0),
                            h.behind.unwrap_or(0)
                        ),
                        PinStatus::Unknown => ": relation unknown".into(),
                    };
                    format!("{} ({from}){status}", short(&p.revision))
                }
            };
            let _ = writeln!(s, "  host pin:       {pin}");
        }
    }
    let unchanged = if r.checkout_unchanged {
        "nothing modified: kb writes only to its isolated mirror cache; the KB checkout \
         (HEAD, branches, index, working tree) and host pins were left as they were"
    } else {
        "warning: the KB checkout changed during this call (another process?); kb itself \
         wrote only to the isolated mirror"
    };
    let _ = writeln!(s, "  {unchanged}");
    s
}
