//! Shared plumbing of the knowledge-reading commands (`context`, `search`, `show`, `impact`,
//! `index`): host detection, snapshot resolution (freshness + selection, docs/architecture.md
//! §6), index build or reuse (§7) and a consistent read view.
//!
//! Everything noteworthy that is not part of a command's deterministic result (index
//! recovery, proposal overlay problems, an unknown `.kbw.toml` repo) is reported twice: as a
//! progress line on stderr and in the envelope `meta.diagnostics`. Recovery is never silent.

use serde_json::{Value, json};

use super::Ctx;
use crate::diag::{Diagnostic, Severity};
use crate::error::Result;
use crate::host::{self, HostContext};
use crate::index::{BuildStats, Index, IndexView};
use crate::knowledge::{KnowledgeView, Overlay, SnapshotInfo};
use crate::model::ProfileLocation;
use crate::output::{CommandOutput, Format};
use crate::snapshot::{self, SelectionRequest, SnapshotRequest};
use crate::source::SourceTree;

/// How a command wants its snapshot.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub include_proposals: bool,
    /// Skip the remote freshness check.
    pub offline: bool,
    /// Drop the index cache before building (`kb index --rebuild`).
    pub rebuild: bool,
}

impl Options {
    /// A reading command: freshness per call unless `--offline`.
    pub fn reading(ctx: &Ctx, include_proposals: bool) -> Options {
        Options {
            include_proposals,
            offline: ctx.global.offline,
            rebuild: false,
        }
    }
}

/// Host repository of this invocation (`--host` or detection from the current directory).
pub fn detect_host(ctx: &Ctx) -> Result<Option<HostContext>> {
    host::detect(&ctx.env, ctx.global.host.as_deref())
}

/// A resolved snapshot that is present in the index.
pub struct Session {
    pub loc: ProfileLocation,
    pub host: Option<HostContext>,
    pub info: SnapshotInfo,
    pub build: BuildStats,
    pub index: Index,
    source: Box<dyn SourceTree>,
    overlay: Option<Overlay>,
    include_proposals: bool,
    quiet: bool,
    notes: Vec<Diagnostic>,
    view_checked: bool,
}

impl Session {
    /// Resolve the snapshot selected by `--snapshot` and make sure the index holds it.
    pub fn open(ctx: &Ctx, host: Option<HostContext>, opts: Options) -> Result<Session> {
        let loc = ctx.location()?;
        let req = SnapshotRequest {
            selection: SelectionRequest::parse(ctx.global.snapshot.as_deref())?,
            offline: opts.offline,
            include_proposals: opts.include_proposals,
        };
        let snap = snapshot::resolve(&ctx.env, &loc, host.as_ref(), &req)?;
        ctx.env.progress(format!(
            "snapshot {} (key {})",
            snap.info.label(),
            short_key(&snap.info.key)
        ));
        let mut index = if opts.rebuild {
            Index::rebuild(&ctx.env.cache_dir, loc.profile)?
        } else {
            Index::open(&ctx.env.cache_dir, loc.profile)?
        };
        let build = index.ensure(
            &snap.info.key,
            snap.source.as_ref(),
            &loc,
            snap.overlay.as_ref(),
        )?;
        let mut session = Session {
            loc,
            host,
            info: snap.info,
            build,
            index,
            source: snap.source,
            overlay: snap.overlay,
            include_proposals: opts.include_proposals,
            quiet: ctx.global.quiet,
            notes: Vec::new(),
            view_checked: false,
        };
        session.progress(&build_line(&session.build));
        if let Some(r) = session.build.recovery.clone() {
            session.note(r.diagnostic());
        }
        Ok(session)
    }

    /// Run `f` on a consistent read view of the snapshot. When another process
    /// garbage-collected the snapshot, or the index turned out to be damaged at query time
    /// (it is then moved aside and reported as INDEX_RECOVERED), the snapshot is rebuilt and
    /// `f` runs once more.
    pub fn with_view<T>(&mut self, mut f: impl FnMut(&IndexView<'_>) -> Result<T>) -> Result<T> {
        let checked = self.view_checked;
        let include_proposals = self.include_proposals;
        let host = self.host.as_ref();
        // Index::with_view rebuilds the snapshot once when it was collected by another
        // process or when query-time damage forced a recovery; `f` may therefore run twice.
        let run = self.index.with_view(
            &self.info.key,
            self.source.as_ref(),
            &self.loc,
            self.overlay.as_ref(),
            |view| {
                let found = if checked {
                    Vec::new()
                } else {
                    view_notes(view, host, include_proposals)?
                };
                Ok((found, f(view)?))
            },
        )?;
        if let Some(stats) = run.rebuilt {
            self.progress(
                "the snapshot was rebuilt (collected by another process or recovered from a damaged index)",
            );
            if let Some(r) = &stats.recovery {
                self.note(r.diagnostic());
            }
            self.progress(&build_line(&stats));
            self.build = stats;
        }
        let (found, value) = run.value;
        self.view_checked = true;
        for d in found {
            self.note(d);
        }
        Ok(value)
    }

    /// Add a diagnostic for stderr and `meta.diagnostics`.
    pub fn note(&mut self, d: Diagnostic) {
        let sev = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };
        let at = d
            .path
            .as_deref()
            .map(|p| format!(" {p}"))
            .unwrap_or_default();
        let line = format!("kb: {sev}[{}]{at}: {}", d.code, d.message);
        // Warnings and errors are shown even with --quiet: they are not progress.
        if d.severity != Severity::Info || !self.quiet {
            eprintln!("{line}");
        }
        if !self.notes.contains(&d) {
            self.notes.push(d);
        }
    }

    fn progress(&self, msg: &str) {
        if !self.quiet {
            eprintln!("kb: {msg}");
        }
    }

    /// Attach the non-deterministic session facts to the envelope `meta`.
    pub fn finish(&self, mut out: CommandOutput) -> CommandOutput {
        out.meta.insert(
            "index".into(),
            serde_json::to_value(&self.build).unwrap_or(Value::Null),
        );
        if !self.notes.is_empty() {
            out.meta.insert("diagnostics".into(), json!(self.notes));
        }
        out
    }
}

/// Problems visible through the view that a command result does not carry: proposal
/// overlay diagnostics and an unknown `.kbw.toml` repo.
fn view_notes(
    view: &IndexView<'_>,
    host: Option<&HostContext>,
    include_proposals: bool,
) -> Result<Vec<Diagnostic>> {
    let mut found: Vec<Diagnostic> = view
        .diagnostics()
        .iter()
        .filter(|d| d.code.starts_with("PROPOSAL_"))
        .cloned()
        .collect();
    if include_proposals {
        for p in view.proposals()? {
            for d in p.diagnostics {
                found.push(if d.path.is_none() {
                    d.at_path(p.path.clone())
                } else {
                    d
                });
            }
        }
    }
    if let Some(h) = host
        && let Some(repo) = host::unknown_binding_repo(h, view.registry())
    {
        found.push(Diagnostic::warning(
            "HOST_BINDING_REPO_UNKNOWN",
            format!(
                "`.kbw.toml` names repo `{repo}`, which the registry does not define; the host \
                 repo is identified by its remotes instead, if any"
            ),
        ));
    }
    Ok(found)
}

/// Text of a knowledge-reading command (`show`, `search`, `impact`) headed by the snapshot
/// provenance line ([`SnapshotInfo::summary_line`]): revision, selection, freshness,
/// approval, approved tip and pin. JSON output carries `result.snapshot` instead.
pub fn with_snapshot_line(text: String, info: &SnapshotInfo, format: Format) -> String {
    if format == Format::Json {
        return text;
    }
    format!("{}\n{text}", info.summary_line())
}

/// Insert the snapshot provenance into a JSON object result.
pub fn with_snapshot(mut result: Value, info: &SnapshotInfo) -> Value {
    if let Some(m) = result.as_object_mut() {
        m.insert(
            "snapshot".into(),
            serde_json::to_value(info).unwrap_or(Value::Null),
        );
    }
    result
}

fn short_key(key: &str) -> &str {
    key.get(..16).unwrap_or(key)
}

fn build_line(b: &BuildStats) -> String {
    if b.reused {
        format!(
            "index: reused snapshot {} ({} record files)",
            short_key(&b.snapshot_key),
            b.files
        )
    } else {
        format!(
            "index: built snapshot {} ({} record files, {} parsed, {} reused, {} proposals, {} errors, {} warnings)",
            short_key(&b.snapshot_key),
            b.files,
            b.parsed,
            b.reused_docs,
            b.proposals,
            b.errors,
            b.warnings
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::cli::args::{GlobalOpts, ProfileArg};
    use crate::env::Env;
    use crate::error::ErrorCode;
    use crate::knowledge::{Origin, Selection};
    use crate::model::Kind;
    use crate::output::Format;
    use crate::versions::ReleaseManifest;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    /// A working-tree KB (no Git needed with `--offline --snapshot working-tree`).
    fn ctx(dir: &Path) -> Ctx {
        let manifest_text = include_str!("../../../release.toml");
        write(dir, "core/release.toml", manifest_text);
        write(
            dir,
            "project/project.toml",
            "schema = 1\n[project]\nname = \"T\"\nnamespace = \"t\"\n[source]\nremote = \"origin\"\n\
             approved_ref = \"refs/heads/main\"\nallowed_protocols = [\"file\"]\n",
        );
        write(
            dir,
            "project/registry/owners.toml",
            "schema = 1\n[[owner]]\nid = \"o\"\ntitle = \"O\"\nproduct = true\n",
        );
        write(
            dir,
            "project/knowledge/a.md",
            "+++\nschema = 1\nid = \"t.policy.a\"\nkind = \"policy\"\ntitle = \"A\"\n\
             status = \"accepted\"\nowner = \"o\"\n\n[scope]\nproduct = true\n\n[[rules]]\n\
             id = \"r\"\nlevel = \"must\"\ntext = \"Do A.\"\n+++\n",
        );
        let kb_root = dir.canonicalize().unwrap();
        Ctx {
            env: Env {
                cwd: kb_root.clone(),
                cache_dir: kb_root.join(".cache"),
                quiet: true,
                manifest: ReleaseManifest::parse(manifest_text).unwrap(),
                kb_root,
            },
            global: GlobalOpts {
                root: None,
                config: None,
                profile: ProfileArg::Project,
                format: None,
                json: false,
                offline: true,
                snapshot: Some("working-tree".into()),
                host: None,
                quiet: true,
                skill_protocol: None,
            },
            format: Format::Compact,
        }
    }

    #[test]
    fn view_rebuilds_a_snapshot_collected_by_another_process() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let mut s = Session::open(&ctx, None, Options::reading(&ctx, false)).unwrap();
        assert_eq!(s.info.selection, Selection::WorkingTree);
        assert!(!s.build.reused);
        // Simulate a concurrent `kb index --gc` in another process between ensure and view.
        assert_eq!(s.index.gc(0).unwrap(), 1);
        let ids = s
            .with_view(|v| {
                Ok(v.metas_by_kind(&Kind::ALL, Origin::Accepted)?
                    .into_iter()
                    .map(|e| e.meta.id.clone())
                    .collect::<Vec<_>>())
            })
            .unwrap();
        assert_eq!(ids, ["t.policy.a"]);
        assert!(!s.build.reused, "the snapshot was rebuilt");
        assert!(s.index.has_snapshot(&s.info.key).unwrap());
    }

    #[test]
    fn callback_errors_propagate_and_meta_carries_the_build() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let mut s = Session::open(&ctx, None, Options::reading(&ctx, false)).unwrap();
        let e = s
            .with_view(|_| -> Result<()> { Err(crate::error::KbError::invalid_input("boom")) })
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidInput);
        let out = s.finish(CommandOutput::new(Value::Null, String::new()));
        assert_eq!(out.meta["index"]["snapshot_key"], json!(s.info.key));
    }
}
