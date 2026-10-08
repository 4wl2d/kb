//! Read-only, Git-backed facts for context and knowledge-maintenance commands.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::context::temporal::AsOf;
use crate::error::{KbError, Result};
use crate::git::Git;
use crate::knowledge::RecordEntry;
use crate::model::{date_days, date_from_days};
use crate::util::check_rel_path;

#[derive(Debug, Clone, serde::Serialize)]
pub struct HistoryWindow {
    pub head: String,
    pub since: String,
    pub since_epoch_seconds: i64,
    pub complete_history: bool,
    pub file_changes: BTreeMap<String, u64>,
}

/// Calendar reference is explicit or commit-derived, never the process clock. CI passes
/// its UTC calendar date through --on when enforcing a calendar SLA.
pub fn reference_date(
    root: &Path,
    revision: &str,
    on: Option<&str>,
) -> Result<Option<crate::freshness::ReferenceDate>> {
    if let Some(date) = on {
        if date_days(date).is_none() {
            return Err(KbError::invalid_input("--on must be YYYY-MM-DD"));
        }
        return Ok(Some(crate::freshness::ReferenceDate {
            date: date.into(),
            source: "explicit-date".into(),
            commit: None,
        }));
    }
    let git = Git::new(root);
    let Some(commit) = git.resolve_commit(revision)? else {
        return Ok(None);
    };
    let timestamp = git
        .run(&["show", "--no-patch", "--format=%ct", &commit, "--"])?
        .parse::<i64>()
        .map_err(|_| KbError::invalid_input("invalid commit timestamp"))?;
    let date = date_from_days(timestamp.div_euclid(86_400))
        .ok_or_else(|| KbError::invalid_input("commit date is outside the supported calendar"))?;
    Ok(Some(crate::freshness::ReferenceDate {
        date,
        source: "commit-date".into(),
        commit: Some(commit),
    }))
}

/// Relative windows are anchored at HEAD's commit date, never the process clock.
pub fn history_window(root: &Path, since: &str) -> Result<HistoryWindow> {
    let git = Git::new(root);
    let head = git
        .resolve_commit("HEAD")?
        .ok_or_else(|| KbError::invalid_input("coverage needs a committed host HEAD"))?;
    let timestamp = git
        .run(&["show", "--no-patch", "--format=%ct", &head, "--"])?
        .parse::<i64>()
        .map_err(|_| KbError::invalid_input("invalid host commit timestamp"))?;
    let cutoff = if let Some(days) = since.strip_suffix('d') {
        let days = days
            .parse::<i64>()
            .ok()
            .filter(|d| (1..=365_000).contains(d))
            .ok_or_else(|| {
                KbError::invalid_input("--since must be 1..365000 days (e.g. 180d) or YYYY-MM-DD")
            })?;
        timestamp
            .checked_sub(days * 86_400)
            .ok_or_else(|| KbError::invalid_input("history window overflow"))?
    } else {
        date_days(since)
            .ok_or_else(|| KbError::invalid_input("--since must be a day count or ISO date"))?
            * 86_400
    };
    let bytes = git.run_bytes(&[
        "log",
        "--format=",
        "--name-only",
        "-z",
        "--no-renames",
        &format!("--since=@{cutoff}"),
        &head,
        "--",
    ])?;
    let mut file_changes = BTreeMap::new();
    for part in bytes.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = std::str::from_utf8(part)
            .map_err(|_| KbError::invalid_input("history path is not UTF-8"))?
            .trim_matches('\n');
        if path.is_empty() {
            continue;
        }
        check_rel_path(path).map_err(KbError::unsafe_path)?;
        *file_changes.entry(path.to_string()).or_insert(0u64) += 1;
    }
    let shallow = git.run(&["rev-parse", "--is-shallow-repository"])? == "true";
    Ok(HistoryWindow {
        head,
        since: since.into(),
        since_epoch_seconds: cutoff,
        complete_history: !shallow,
        file_changes,
    })
}

/// Tracked names only. Historical requests use an immutable tree; ordinary requests use
/// `ls-files` so staged additions are visible without reading source files from disk.
pub fn tracked_files(root: &Path, revision: Option<&str>) -> Result<Vec<String>> {
    let git = Git::new(root);
    let bytes = match revision {
        Some(rev) => {
            let commit = git
                .resolve_commit(rev)?
                .ok_or_else(|| KbError::invalid_input(format!("unknown host revision {rev}")))?;
            git.run_bytes(&["ls-tree", "-r", "--name-only", "-z", &commit, "--"])?
        }
        None => git.run_bytes(&["ls-files", "--cached", "-z", "--"])?,
    };
    let mut paths = BTreeSet::new();
    for bytes in bytes.split(|b| *b == 0).filter(|b| !b.is_empty()) {
        let path = std::str::from_utf8(bytes)
            .map_err(|_| KbError::invalid_input("tracked filename is not UTF-8"))?;
        check_rel_path(path).map_err(KbError::unsafe_path)?;
        paths.insert(path.to_string());
    }
    Ok(paths.into_iter().collect())
}

pub fn diff_text(root: &Path, diff: &crate::impact::HostDiff) -> Result<String> {
    let mut args = vec![
        "diff",
        "--unified=0",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        diff.merge_base.as_str(),
    ];
    if let Some(head) = &diff.head {
        args.push(head);
    }
    args.push("--");
    let bytes = Git::new(root).run_bytes(&args)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(KbError::invalid_input(
            "host diff exceeds the 8 MiB lexical-analysis limit; split the change or pass explicit paths and change types",
        ));
    }
    // Git's binary-file notices remain lexical text; no binary contents are read.
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn resolve_as_of(
    host: Option<&Path>,
    requested: &str,
    records: &[RecordEntry],
) -> Result<AsOf> {
    if requested.len() == 10
        && requested.as_bytes()[4] == b'-'
        && requested.as_bytes()[7] == b'-'
        && date_days(requested).is_none()
    {
        return Err(KbError::invalid_input(
            "--as-of contains an invalid calendar date",
        ));
    }
    let git = host.map(Git::new);
    let mut point = if date_days(requested).is_some() {
        let mut p = AsOf::on_date(requested)?;
        if let Some(git) = &git {
            let before = format!("--before={requested}T23:59:59Z");
            let revision = git.run(&["rev-list", "--first-parent", "-1", &before, "HEAD", "--"])?;
            p.host_revision = (!revision.is_empty()).then_some(revision);
        }
        p
    } else {
        let git = git
            .as_ref()
            .ok_or_else(|| KbError::invalid_input("--as-of revision needs --host"))?;
        let commit = git.resolve_commit(requested)?.ok_or_else(|| {
            KbError::invalid_input(format!("unknown --as-of revision {requested}"))
        })?;
        let timestamp = git
            .run(&["show", "--no-patch", "--format=%ct", &commit, "--"])?
            .parse::<i64>()
            .map_err(|_| KbError::invalid_input("host commit has an invalid timestamp"))?;
        let date = date_from_days(timestamp.div_euclid(86_400)).ok_or_else(|| {
            KbError::invalid_input("host commit date is outside the supported calendar")
        })?;
        AsOf {
            requested: requested.into(),
            date,
            host_revision: Some(commit),
            commit_ancestry: Default::default(),
        }
    };
    let bounds: BTreeSet<_> = records
        .iter()
        .flat_map(|r| {
            let c = r.parsed.record.common();
            [c.introduced, c.retired]
                .into_iter()
                .flatten()
                .filter(|s| date_days(s).is_none())
                .map(str::to_string)
        })
        .collect();
    for bound in bounds {
        let git = git.as_ref().ok_or_else(|| {
            KbError::invalid_input("commit-bounded knowledge needs --host for --as-of")
        })?;
        let commit = git.resolve_commit(&bound)?.ok_or_else(|| {
            KbError::invalid_input(format!(
                "temporal commit {bound} is missing or ambiguous in the host"
            ))
        })?;
        let reached = match &point.host_revision {
            Some(cutoff) => git.is_ancestor(&commit, cutoff)?,
            None => false,
        };
        point.commit_ancestry.insert(bound, reached);
    }
    Ok(point)
}

/// Read a bounded ordinary file from an immutable Git tree. Symlinks are not followed.
pub fn blob_at(root: &Path, revision: &str, path: &str, max_bytes: u64) -> Result<Option<Vec<u8>>> {
    check_rel_path(path).map_err(KbError::unsafe_path)?;
    let git = Git::new(root);
    let commit = git
        .resolve_commit(revision)?
        .ok_or_else(|| KbError::invalid_input(format!("unknown host revision {revision}")))?;
    let listing = git.run_bytes(&[
        "--literal-pathspecs",
        "ls-tree",
        "-l",
        "-z",
        &commit,
        "--",
        path,
    ])?;
    for entry in listing.split(|b| *b == 0).filter(|b| !b.is_empty()) {
        let text = std::str::from_utf8(entry)
            .map_err(|_| KbError::invalid_input("Git tree path is not UTF-8"))?;
        let Some((metadata, name)) = text.split_once('\t') else {
            continue;
        };
        if name != path {
            continue;
        }
        let parts: Vec<_> = metadata.split_whitespace().collect();
        if parts.len() != 4 || parts[1] != "blob" {
            return Ok(None);
        }
        if !matches!(parts[0], "100644" | "100755") {
            return Err(KbError::unsafe_path(format!(
                "{path} is not an ordinary Git file"
            )));
        }
        let size = parts[3]
            .parse::<u64>()
            .map_err(|_| KbError::invalid_input("invalid Git blob size"))?;
        if size > max_bytes {
            return Err(KbError::invalid_input(format!(
                "{path} exceeds the {max_bytes}-byte limit"
            )));
        }
        return git.run_bytes(&["cat-file", "blob", parts[2]]).map(Some);
    }
    Ok(None)
}

/// Ordinary stage-0 index blob, independent of unstaged changes. Unmerged files have no
/// single version and return None instead of borrowing a version from the work tree.
pub fn index_blob(root: &Path, path: &str, max_bytes: usize) -> Result<Option<Vec<u8>>> {
    check_rel_path(path).map_err(KbError::unsafe_path)?;
    let git = Git::new(root);
    let listing = git.run_bytes_limited(
        &[
            "--literal-pathspecs",
            "ls-files",
            "--stage",
            "-z",
            "--",
            path,
        ],
        64 * 1024,
    )?;
    for entry in listing.split(|b| *b == 0).filter(|b| !b.is_empty()) {
        let text = std::str::from_utf8(entry)
            .map_err(|_| KbError::invalid_input("index entry is not UTF-8"))?;
        let Some((meta, name)) = text.split_once('\t') else {
            continue;
        };
        let fields: Vec<_> = meta.split_whitespace().collect();
        if name != path || fields.len() != 3 || fields[2] != "0" {
            continue;
        }
        if !matches!(fields[0], "100644" | "100755") {
            return Err(KbError::unsafe_path("index entry is not an ordinary file"));
        }
        return git
            .run_bytes_limited(&["cat-file", "blob", fields[1]], max_bytes)
            .map(Some);
    }
    Ok(None)
}
