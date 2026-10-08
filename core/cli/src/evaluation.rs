//! Tier A history adapter. This evaluates routing and evidence, never agent task quality.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::{self, ContextRequest, TaskEnv};
use crate::error::{KbError, Result};
use crate::git::Git;
use crate::host::HostContext;
use crate::impact;
use crate::knowledge::{KnowledgeView, Origin, SnapshotInfo};
use crate::model::{Intent, Kind, Status};
use crate::output::Format;
use crate::provenance::HostRoots;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LabelExport {
    protocol: String,
    changes: Vec<Label>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Label {
    commit: String,
    /// Complete MR description containing an optional kb-impact:v1 block.
    description: String,
}

pub fn load_labels(path: &Path) -> Result<BTreeMap<String, String>> {
    let bytes = crate::util::read_file_limited(path, 16 * 1024 * 1024)?;
    let export: LabelExport = serde_json::from_slice(&bytes)
        .map_err(|e| KbError::invalid_input(format!("history labels: {e}")))?;
    if export.protocol != "kb.history-labels.v1" || export.changes.len() > 1000 {
        return Err(KbError::invalid_input(
            "expected kb.history-labels.v1 with at most 1000 changes",
        ));
    }
    let mut labels = BTreeMap::new();
    for label in export.changes {
        if !matches!(label.commit.len(), 40 | 64)
            || !label.commit.bytes().all(|c| c.is_ascii_hexdigit())
            || label.description.len() > 1024 * 1024
        {
            return Err(KbError::invalid_input(
                "label commit must be a full SHA; description is limited to 1 MiB",
            ));
        }
        impact::parse_statement(&label.description).map_err(KbError::invalid_input)?;
        if labels
            .insert(label.commit.to_ascii_lowercase(), label.description)
            .is_some()
        {
            return Err(KbError::invalid_input("duplicate history label commit"));
        }
    }
    Ok(labels)
}

#[derive(Debug)]
pub struct Change {
    pub commit: String,
    pub parent: String,
}

pub fn changes(root: &Path, range: &str, limit: usize) -> Result<Vec<Change>> {
    if !(1..=1000).contains(&limit) {
        return Err(KbError::invalid_input("max-changes must be 1..1000"));
    }
    let (base, end) = range
        .split_once("..")
        .filter(|(a, b)| !a.is_empty() && !b.is_empty() && !b.contains(".."))
        .ok_or_else(|| KbError::invalid_input("range must be BASE..HEAD"))?;
    let git = Git::new(root);
    let resolve = |rev| {
        git.resolve_commit(rev)?
            .ok_or_else(|| KbError::invalid_input(format!("unknown range revision {rev}")))
    };
    let base = resolve(base)?;
    let end = resolve(end)?;
    if !git.is_ancestor(&base, &end)? {
        return Err(KbError::invalid_input(
            "history base must be an ancestor of the end",
        ));
    }
    let range = format!("{base}..{end}");
    let max = format!("--max-count={}", limit + 1);
    let bytes = git.run_bytes_limited(
        &[
            "rev-list",
            "--first-parent",
            "--reverse",
            "--parents",
            &max,
            &range,
            "--",
        ],
        256 * 1024,
    )?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| KbError::invalid_input("invalid Git history output"))?;
    let mut changes = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let commit = words.next().unwrap_or_default();
        let parent = words.next().ok_or_else(|| {
            KbError::invalid_input("missing first parent; fetch complete host history")
        })?;
        changes.push(Change {
            commit: commit.into(),
            parent: parent.into(),
        });
    }
    if changes.len() > limit {
        return Err(KbError::invalid_input(
            "range exceeds max-changes; narrow the range or raise the explicit limit",
        ));
    }
    if changes.first().is_some_and(|c| c.parent != base) {
        return Err(KbError::invalid_input(
            "range base must lie on the end revision's first-parent chain",
        ));
    }
    Ok(changes)
}

pub struct HistoryOptions<'a> {
    pub kb_root: &'a Path,
    pub host: &'a HostContext,
    pub roots: &'a HostRoots,
    pub repo: &'a str,
    pub as_of: bool,
}

pub fn history(
    view: &dyn KnowledgeView,
    snapshot: &SnapshotInfo,
    changes: &[Change],
    labels: &BTreeMap<String, String>,
    options: &HistoryOptions<'_>,
) -> Result<Value> {
    let mut rows = Vec::new();
    let mut incomplete = 0;
    let mut tokens_total = 0;
    // Commit bounds of records scoped to other repositories are not resolved in this host.
    let mut records = view.all_records(Origin::Accepted)?;
    if view.registry().repo(options.repo).is_some() {
        records.retain(|r| {
            context::temporal::scope_reaches_repo(
                r.parsed.record.common().scope,
                view.registry(),
                options.repo,
            )
        });
    }
    let selected: BTreeSet<_> = changes.iter().map(|c| &c.commit).collect();
    let unused: Vec<_> = labels.keys().filter(|id| !selected.contains(id)).collect();
    if !unused.is_empty() {
        return Err(KbError::invalid_input(format!(
            "label commits are outside the requested first-parent range: {}",
            unused
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    for change in changes {
        let point = options
            .as_of
            .then(|| {
                crate::host::facts::resolve_as_of(
                    Some(&options.host.root),
                    &change.parent,
                    &records,
                )
            })
            .transpose()?;
        let temporal = point
            .as_ref()
            .map(|p| context::temporal::TemporalView::new(view, p))
            .transpose()?;
        let scoped: &dyn KnowledgeView = temporal.as_ref().map_or(view, |v| v);
        let diff = impact::host_diff(
            &options.host.root,
            &change.parent,
            Some(&change.commit),
            false,
            options.host.kb_submodule_path.as_deref(),
        )?;
        let metas = scoped.metas_by_kind(&Kind::ALL, Origin::Accepted)?;
        let report = impact::analyze(&diff, Some(options.repo), scoped.registry(), &metas);
        let description = match labels.get(&change.commit) {
            Some(text) => text.clone(),
            None => String::from_utf8(Git::new(&options.host.root).run_bytes_limited(
                &["show", "--no-patch", "--format=%B", &change.commit, "--"],
                1024 * 1024,
            )?)
            .map_err(|_| KbError::invalid_input("commit message is not UTF-8"))?,
        };
        let statement = impact::parse_statement(&description)
            .map_err(|e| KbError::invalid_input(format!("{}: {e}", change.commit)))?;
        let mut request = ContextRequest::new(Intent::Review);
        request.repos = vec![options.repo.into()];
        request.changed_paths = diff
            .files
            .iter()
            .flat_map(|f| std::iter::once(f.path.clone()).chain(f.old_path.clone()))
            .collect();
        let mut env = TaskEnv::new(snapshot.clone());
        env.host_repo = Some(options.repo.into());
        env.changed_scope = true;
        env.set_changed_text(crate::host::facts::diff_text(&options.host.root, &diff)?);
        env.known_files
            .extend(request.changed_paths.iter().cloned());
        request.as_of = point;
        env.host_head = Some(change.parent.clone());
        env.reference_date =
            crate::host::facts::reference_date(&options.host.root, &change.parent, None)?;
        if let Some(path) = scoped
            .registry()
            .repo(options.repo)
            .and_then(|r| r.version_file.as_deref())
            && let Some(bytes) =
                crate::host::facts::blob_at(&options.host.root, &change.parent, path, 64 * 1024)?
            && let Ok(text) = std::str::from_utf8(&bytes)
            && let Some(line) = text.lines().next()
            && let Ok(version) = semver::Version::parse(line.trim().trim_start_matches('v'))
        {
            env.host_versions.insert(options.repo.into(), version);
        }
        let context = context::assemble(&request, &env, view, Format::Compact);
        let selected_records: Vec<_> = scoped
            .all_records(Origin::Accepted)?
            .into_iter()
            .filter(|r| {
                r.parsed.record.status() == Status::Accepted
                    && (report.affected.iter().any(|a| a.id == r.parsed.record.id())
                        || context.as_ref().is_ok_and(|result| {
                            result
                                .units
                                .iter()
                                .any(|u| u.record_id == r.parsed.record.id())
                        }))
            })
            .collect();
        let drift = crate::trust::drift::report(
            options.kb_root,
            options.roots,
            &selected_records,
            "verified",
            &change.commit,
        );
        let mut row = json!({"commit":change.commit, "base":change.parent,"statement":statement,
            "coverage":{"changed_files":report.files.len(),"unknown_files":report.unknown_coverage,"affected_records":report.affected},
            "drift":{"changed_records":drift.changed_records,"unverifiable_records":drift.unverifiable_records},
            "as_of":request.as_of});
        match context {
            Ok(context) => {
                let mandatory: BTreeSet<_> = context.mandatory_ids().into_iter().collect();
                let included: BTreeSet<_> =
                    context.units.iter().map(|u| u.record_id.as_str()).collect();
                let labeled = statement
                    .as_ref()
                    .filter(|s| s.applicability_reviewed == Some(true));
                let false_positive: Vec<_> = statement
                    .as_ref()
                    .map(|s| {
                        s.not_applicable
                            .iter()
                            .filter(|id| mandatory.contains(id.as_str()))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                let precision = labeled.map(|_| {
                    context::routing::Ratio {
                        matched: mandatory.len() - false_positive.len(),
                        total: mandatory.len(),
                    }
                    .value()
                });
                let recall = statement
                    .as_ref()
                    .filter(|s| !s.applicable.is_empty())
                    .map(|s| {
                        context::routing::Ratio {
                            matched: s
                                .applicable
                                .iter()
                                .filter(|id| included.contains(id.as_str()))
                                .count(),
                            total: s.applicable.len(),
                        }
                        .value()
                    });
                let available: BTreeSet<_> = metas
                    .iter()
                    .filter(|e| e.meta.status == Status::Accepted)
                    .map(|e| e.meta.id.as_str())
                    .collect();
                let missing: Vec<_> = statement
                    .as_ref()
                    .map(|s| {
                        s.applicable
                            .iter()
                            .chain(&s.not_applicable)
                            .filter(|id| !available.contains(id.as_str()))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                let tokens =
                    context::estimate_tokens(&context::render(&context, Format::Compact, false));
                tokens_total += tokens;
                if context.knowledge_status() != context::Completeness::Complete
                    || !missing.is_empty()
                {
                    incomplete += 1;
                }
                row["context"] = json!({"status":context.knowledge_status().as_str(),"mandatory":mandatory,"included":included,"tokens_est":tokens,"receipt":context.receipt_id(),"mandatory_precision":precision,"mandatory_not_applicable":false_positive,"labeled_recall":recall,"unavailable_label_ids":missing});
            }
            Err(error) => {
                incomplete += 1;
                row["context_error"] = json!({"code":error.code.as_str(),"message":error.message});
            }
        }
        rows.push(row);
    }
    Ok(
        json!({"mode":if options.as_of {"parent-validity-filter"} else {"current-snapshot-retrospective"},"changes":rows,"incomplete_changes":incomplete,"tokens_est_total":tokens_total,
        "note":"First-parent committed scope, including squash commits. Labels are human claims, not engine ground truth. Validity filtering does not reconstruct historical record revisions or registries; freeze the KB snapshot separately for leak-free replay. Drift is source support, not semantic correctness."}),
    )
}
