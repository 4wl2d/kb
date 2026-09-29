//! Impact analysis: which knowledge does a host-repository change touch?
//!
//! * [`host_diff`] (Git adapter) lists the files changed between `merge-base(base, head)` and
//!   `head`, or the work tree including untracked files, and separates the KB submodule
//!   pointer from ordinary files.
//! * [`analyze`] (pure) links every changed path (old and new path of a rename, deleted paths
//!   too) to records through path selectors, source/test/doc anchors, module and feature
//!   scope, contract parties and feature records. A changed file without a link is *unknown
//!   coverage*: nothing describes it, which is not a claim that nothing needs describing.
//!   Anchors on deleted or renamed paths are reported as stale.
//! * [`parse_statement`] reads the reviewable `kb-impact` acknowledgement from an MR
//!   description and [`check`] verifies its presence and structure. kb never evaluates the
//!   engineering reasoning behind a statement.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{ErrorCode, KbError, Result};
use crate::git::{Git, check_revision_arg, git_failure};
use crate::glob::{RepoGlob, dir_prefixes};
use crate::knowledge::{MetaEntry, Origin};
use crate::model::{AnchorKind, Kind, Registry, Status};
use crate::output::Format;
use crate::scope::implied_repos;
use crate::util::check_rel_path;

/// Marker of the acknowledgement block in an MR description: `<!-- kb-impact:v1 ... -->`.
pub const STATEMENT_TAG: &str = "kb-impact:v1";

/// Printed with every verdict: the check is structural only.
pub const REASONING_NOTE: &str = "kb checks only that an acknowledgement is present and \
well-formed; it does not evaluate the engineering reasoning, which stays with reviewers.";

/// Printed with unknown coverage.
pub const UNKNOWN_COVERAGE_NOTE: &str = "unknown coverage: no knowledge record is linked to \
these files; this does not mean that no documentation is needed.";

/// Git file mode of a gitlink (submodule commit) entry.
const GITLINK_MODE: &str = "160000";

// ---------------------------------------------------------------------------------------
// Host diff (Git adapter)
// ---------------------------------------------------------------------------------------

/// How a file changed between the merge-base and the head (or work tree).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

impl ChangeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeStatus::Added => "added",
            ChangeStatus::Modified => "modified",
            ChangeStatus::Deleted => "deleted",
            ChangeStatus::Renamed => "renamed",
            ChangeStatus::Copied => "copied",
            ChangeStatus::TypeChanged => "type-changed",
        }
    }

    /// One-letter code as printed by `git diff --name-status`.
    pub fn letter(self) -> char {
        match self {
            ChangeStatus::Added => 'A',
            ChangeStatus::Modified => 'M',
            ChangeStatus::Deleted => 'D',
            ChangeStatus::Renamed => 'R',
            ChangeStatus::Copied => 'C',
            ChangeStatus::TypeChanged => 'T',
        }
    }

    /// Map a raw diff status letter. Unmerged work-tree paths (`U`) count as modified.
    fn from_git(letter: char) -> Option<ChangeStatus> {
        Some(match letter {
            'A' => ChangeStatus::Added,
            'M' | 'U' => ChangeStatus::Modified,
            'D' => ChangeStatus::Deleted,
            'R' => ChangeStatus::Renamed,
            'C' => ChangeStatus::Copied,
            'T' => ChangeStatus::TypeChanged,
            _ => return None,
        })
    }
}

/// One changed host file (repo-relative, `/`-separated, validated paths).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ChangedFile {
    /// New path (the removed path for deletions).
    pub path: String,
    /// Source path of a rename or copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: ChangeStatus,
}

/// Change of the KB submodule gitlink. `None` = no gitlink on that side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KbPointerChange {
    pub old: Option<String>,
    pub new: Option<String>,
}

/// Files changed in the host repository relative to `merge-base(base, head)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostDiff {
    /// Resolved base commit.
    pub base: String,
    pub merge_base: String,
    /// Resolved head commit; `None` = the work tree (staged, unstaged and untracked).
    pub head: Option<String>,
    /// Host-relative path of the KB submodule the diff was checked for, if any.
    pub kb_submodule: Option<String>,
    /// Sorted by path; the KB submodule pointer is never listed here.
    pub files: Vec<ChangedFile>,
    pub kb_pointer: Option<KbPointerChange>,
}

/// Diff the host repository at `host_root`.
///
/// Compares `merge-base(base, head or HEAD)` with `head`, or with the work tree when
/// `working_tree` is set (untracked, non-ignored files are added; `head` must then be
/// `None`). Renames are detected (`-M`). The gitlink at `kb_submodule_path` becomes
/// [`HostDiff::kb_pointer`]; other gitlinks are reported as files. Only reads the
/// repository: no checkout, index or ref is modified.
pub fn host_diff(
    host_root: &Path,
    base: &str,
    head: Option<&str>,
    working_tree: bool,
    kb_submodule_path: Option<&str>,
) -> Result<HostDiff> {
    check_revision_arg(base)?;
    if let Some(h) = head {
        check_revision_arg(h)?;
    }
    if working_tree && head.is_some() {
        return Err(KbError::invalid_input(
            "working-tree mode compares against the local work tree and cannot take a head revision",
        ));
    }
    let kb_path = match kb_submodule_path {
        Some(p) => {
            let p = p.trim_end_matches('/');
            check_rel_path(p).map_err(KbError::unsafe_path)?;
            Some(p.to_string())
        }
        None => None,
    };
    let top = crate::git::toplevel(host_root).ok_or_else(|| {
        KbError::new(
            ErrorCode::GitError,
            format!("`{}` is not inside a Git work tree", host_root.display()),
        )
    })?;
    let git = Git::new(&top);
    let base_oid = resolve(&git, base, "base")?;
    let head_oid = resolve(&git, head.unwrap_or("HEAD"), "head")?;
    let merge_base = merge_base(&git, &base_oid, &head_oid)?;

    let mut args = vec![
        "diff",
        "--raw",
        "-z",
        "-M",
        "--no-abbrev",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        // Report gitlink commit changes even when `.gitmodules` says `ignore = all`, but not
        // uncommitted content inside submodules.
        "--ignore-submodules=dirty",
        merge_base.as_str(),
    ];
    if !working_tree {
        args.push(head_oid.as_str());
    }
    args.push("--");
    let raw = git.run_bytes(&args)?;

    let mut files: BTreeMap<String, ChangedFile> = BTreeMap::new();
    let mut kb_pointer = None;
    for entry in parse_raw_diff(&raw)? {
        let is_gitlink = entry.src_mode == GITLINK_MODE || entry.dst_mode == GITLINK_MODE;
        if is_gitlink && kb_path.as_deref() == Some(entry.final_path()) {
            kb_pointer = Some(pointer_change(&top, &entry)?);
            continue;
        }
        let file = entry.into_changed_file();
        files.insert(file.path.clone(), file);
    }
    if working_tree {
        for path in untracked_files(&git)? {
            match files.get_mut(&path) {
                // Removed from the index but still present in the work tree.
                Some(f) if f.status == ChangeStatus::Deleted => f.status = ChangeStatus::Modified,
                Some(_) => {}
                None => {
                    files.insert(
                        path.clone(),
                        ChangedFile {
                            path,
                            old_path: None,
                            status: ChangeStatus::Added,
                        },
                    );
                }
            }
        }
    }
    Ok(HostDiff {
        base: base_oid,
        merge_base,
        head: (!working_tree).then_some(head_oid),
        kb_submodule: kb_path,
        files: files.into_values().collect(),
        kb_pointer,
    })
}

fn resolve(git: &Git, rev: &str, what: &str) -> Result<String> {
    git.resolve_commit(rev)?.ok_or_else(|| {
        KbError::invalid_input(format!(
            "{what} revision `{rev}` does not name a commit in the host repository"
        ))
    })
}

fn merge_base(git: &Git, base: &str, head: &str) -> Result<String> {
    let args = ["merge-base", base, head];
    let out = git.output(&args)?;
    match out.code {
        0 => Ok(out.stdout_str().trim().to_string()),
        1 => Err(KbError::invalid_input(format!(
            "base {base} and head {head} have no common ancestor"
        ))),
        _ => Err(git_failure(&args, &out)),
    }
}

/// One record of `git diff --raw -z` output.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawEntry {
    src_mode: String,
    dst_mode: String,
    src_oid: String,
    dst_oid: String,
    status: ChangeStatus,
    path: String,
    /// Destination of a rename or copy.
    dst_path: Option<String>,
}

impl RawEntry {
    fn final_path(&self) -> &str {
        self.dst_path.as_deref().unwrap_or(&self.path)
    }

    fn into_changed_file(self) -> ChangedFile {
        match self.dst_path {
            Some(dst) => ChangedFile {
                path: dst,
                old_path: Some(self.path),
                status: self.status,
            },
            None => ChangedFile {
                path: self.path,
                old_path: None,
                status: self.status,
            },
        }
    }
}

fn bad_diff(msg: impl std::fmt::Display) -> KbError {
    KbError::new(
        ErrorCode::GitError,
        format!("unexpected `git diff --raw` output: {msg}"),
    )
}

/// Parse `git diff --raw -z`: `:<mode> <mode> <oid> <oid> <status>\0<path>\0[<dst>\0]`.
fn parse_raw_diff(bytes: &[u8]) -> Result<Vec<RawEntry>> {
    let mut fields = bytes.split(|b| *b == 0).filter(|f| !f.is_empty());
    let mut out = Vec::new();
    while let Some(header) = fields.next() {
        let header = std::str::from_utf8(header)
            .ok()
            .and_then(|h| h.strip_prefix(':'))
            .ok_or_else(|| bad_diff("record does not start with `:`"))?;
        let parts: Vec<&str> = header.split(' ').collect();
        let [src_mode, dst_mode, src_oid, dst_oid, status] = parts.as_slice() else {
            return Err(bad_diff(format!("malformed record header {header:?}")));
        };
        let status = status
            .chars()
            .next()
            .and_then(ChangeStatus::from_git)
            .ok_or_else(|| bad_diff(format!("unsupported change status {status:?}")))?;
        let path = checked_path(fields.next())?;
        let dst_path = match status {
            ChangeStatus::Renamed | ChangeStatus::Copied => Some(checked_path(fields.next())?),
            _ => None,
        };
        out.push(RawEntry {
            src_mode: src_mode.to_string(),
            dst_mode: dst_mode.to_string(),
            src_oid: src_oid.to_string(),
            dst_oid: dst_oid.to_string(),
            status,
            path,
            dst_path,
        });
    }
    Ok(out)
}

/// Validate a path reported by Git (UTF-8, repo-relative, no traversal or control chars).
fn checked_path(field: Option<&[u8]>) -> Result<String> {
    let bytes = field.ok_or_else(|| bad_diff("truncated record"))?;
    let path = std::str::from_utf8(bytes).map_err(|_| {
        KbError::unsafe_path(format!(
            "changed path {:?} is not valid UTF-8",
            String::from_utf8_lossy(bytes)
        ))
    })?;
    check_rel_path(path)
        .map_err(|e| KbError::unsafe_path(format!("changed path {path:?} is not safe: {e}")))?;
    Ok(path.to_string())
}

fn is_null_oid(oid: &str) -> bool {
    oid.bytes().all(|b| b == b'0')
}

fn pointer_change(top: &Path, entry: &RawEntry) -> Result<KbPointerChange> {
    let old = (entry.src_mode == GITLINK_MODE && !is_null_oid(&entry.src_oid))
        .then(|| entry.src_oid.clone());
    let new = if entry.dst_mode != GITLINK_MODE {
        None
    } else if is_null_oid(&entry.dst_oid) {
        // Work-tree side of a gitlink: Git reports a null id, so read the checkout's HEAD.
        Some(submodule_head(top, entry.final_path())?)
    } else {
        Some(entry.dst_oid.clone())
    };
    Ok(KbPointerChange { old, new })
}

/// HEAD of the submodule checked out at `rel`. Refuses a directory that is not its own
/// work tree (Git would otherwise silently answer for the superproject).
fn submodule_head(top: &Path, rel: &str) -> Result<String> {
    let dir = crate::util::safe_join(top, rel)?;
    let own_top = crate::git::toplevel(&dir).and_then(|p| p.canonicalize().ok());
    if own_top.is_none() || own_top != dir.canonicalize().ok() {
        return Err(KbError::new(
            ErrorCode::GitError,
            format!("the KB submodule at `{rel}` is not checked out"),
        ));
    }
    Git::new(&dir).resolve_commit("HEAD")?.ok_or_else(|| {
        KbError::new(
            ErrorCode::GitError,
            format!("the KB submodule at `{rel}` has no HEAD commit"),
        )
    })
}

/// Untracked, non-ignored files (an untracked nested repository is listed as its directory).
fn untracked_files(git: &Git) -> Result<Vec<String>> {
    let raw = git.run_bytes(&[
        "ls-files",
        "--others",
        "--exclude-standard",
        "--full-name",
        "-z",
    ])?;
    raw.split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| checked_path(Some(f.strip_suffix(b"/").unwrap_or(f))))
        .collect()
}

// ---------------------------------------------------------------------------------------
// Analysis (pure)
// ---------------------------------------------------------------------------------------

/// Why a record is linked to a changed file. The string forms are stable protocol values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Relation {
    /// A `selectors.paths` glob matches the path.
    PathSelector,
    /// A `source` anchor names the file or a directory containing it.
    SourceAnchor,
    TestAnchor,
    DocAnchor,
    /// The record's scope names a module of the file (and all its dimensions apply).
    ModuleScope,
    /// The record's scope names a feature of the file (and all its dimensions apply).
    FeatureScope,
    /// A contract party lists a module of the file.
    ContractParty,
    /// A contract party covers the file's whole repo (no modules listed). Reported as
    /// affected knowledge, but not counted as coverage of the changed area.
    ContractRepoParty,
    /// A feature record documents a feature of the file.
    FeatureRecord,
}

impl Relation {
    pub fn as_str(self) -> &'static str {
        match self {
            Relation::PathSelector => "path-selector",
            Relation::SourceAnchor => "source-anchor",
            Relation::TestAnchor => "test-anchor",
            Relation::DocAnchor => "doc-anchor",
            Relation::ModuleScope => "module-scope",
            Relation::FeatureScope => "feature-scope",
            Relation::ContractParty => "contract-party",
            Relation::ContractRepoParty => "contract-repo-party",
            Relation::FeatureRecord => "feature-record",
        }
    }

    /// Does this relation describe the changed area (rather than the whole repository)?
    pub fn is_coverage(self) -> bool {
        self != Relation::ContractRepoParty
    }

    fn of_anchor(kind: AnchorKind) -> Option<Relation> {
        match kind {
            AnchorKind::Source => Some(Relation::SourceAnchor),
            AnchorKind::Test => Some(Relation::TestAnchor),
            AnchorKind::Doc => Some(Relation::DocAnchor),
            AnchorKind::Change => None,
        }
    }
}

/// One link between a changed file and a record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct FileLink {
    pub record: String,
    pub relation: Relation,
    /// What matched: the glob, anchor path, module, feature or party id.
    pub via: String,
    /// The changed path that matched (path-selector and anchor relations).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileImpact {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    /// Registry modules containing the path (both paths of a rename).
    pub modules: Vec<String>,
    /// Features of those modules plus features whose globs match the path.
    pub features: Vec<String>,
    pub links: Vec<FileLink>,
    /// At least one link describes the changed area (see [`Relation::is_coverage`]).
    pub covered: bool,
}

/// A record linked to at least one changed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AffectedRecord {
    pub id: String,
    pub kind: Kind,
    pub status: Status,
    pub title: String,
    pub origin: Origin,
    /// KB-root-relative path of the record file.
    pub record_path: String,
    pub relations: Vec<Relation>,
    /// Changed files (their current path) linked to the record.
    pub files: Vec<String>,
}

/// A changed file that no record describes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnknownCoverage {
    pub path: String,
    pub status: ChangeStatus,
    /// Declared modules the file belongs to (it may be in a module and still be undescribed).
    pub modules: Vec<String>,
    pub features: Vec<String>,
}

/// An anchor whose path was deleted or renamed by the diff.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct StaleAnchor {
    pub record: String,
    pub kind: AnchorKind,
    pub repo: String,
    /// The anchored (removed) path.
    pub path: String,
    pub change: ChangeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renamed_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImpactReport {
    pub base: String,
    pub merge_base: String,
    /// `None` = work tree.
    pub head: Option<String>,
    /// Registry repo of the host; `None` = not identified (repo-bound links not evaluated).
    pub repo: Option<String>,
    pub kb_submodule: Option<String>,
    /// The diff changes the KB submodule pointer.
    pub kb_change: bool,
    pub kb_pointer: Option<KbPointerChange>,
    pub files: Vec<FileImpact>,
    pub affected: Vec<AffectedRecord>,
    pub unknown_coverage: Vec<UnknownCoverage>,
    pub stale_anchors: Vec<StaleAnchor>,
}

/// Link every changed file to knowledge.
///
/// `repo` is the host's registry repo id; without it only repo-independent relations
/// (unqualified path selectors of records not bound to specific repos) can match.
/// Superseded records are ignored (their successors carry the knowledge). Output is
/// sorted and independent of the order of `metas`.
pub fn analyze(
    diff: &HostDiff,
    repo: Option<&str>,
    registry: &Registry,
    metas: &[MetaEntry],
) -> ImpactReport {
    let index = LinkIndex::new(metas, registry);
    let mut files = Vec::with_capacity(diff.files.len());
    let mut affected: BTreeMap<usize, (BTreeSet<Relation>, BTreeSet<String>)> = BTreeMap::new();
    let mut unknown_coverage = Vec::new();
    let mut stale_anchors = Vec::new();

    for file in &diff.files {
        let mut paths = vec![file.path.as_str()];
        if file.status == ChangeStatus::Renamed
            && let Some(old) = &file.old_path
        {
            paths.push(old);
        }
        let (modules, features) = file_area(registry, repo, &paths);
        let mut links = Vec::new();
        for (i, relation, via, path) in index.links(repo, &paths, &modules, &features) {
            let entry = affected.entry(i).or_default();
            entry.0.insert(relation);
            entry.1.insert(file.path.clone());
            links.push(FileLink {
                record: index.metas[i].meta.id.clone(),
                relation,
                via,
                path,
            });
        }
        links.sort();
        links.dedup();
        let covered = links.iter().any(|l| l.relation.is_coverage());
        let modules: Vec<String> = modules.into_iter().collect();
        let features: Vec<String> = features.into_iter().collect();
        if !covered {
            unknown_coverage.push(UnknownCoverage {
                path: file.path.clone(),
                status: file.status,
                modules: modules.clone(),
                features: features.clone(),
            });
        }

        let removed = match file.status {
            ChangeStatus::Deleted => Some((file.path.as_str(), None)),
            ChangeStatus::Renamed => file.old_path.as_deref().map(|o| (o, Some(&file.path))),
            _ => None,
        };
        if let (Some((old, renamed_to)), Some(repo)) = (removed, repo) {
            for &(i, kind) in index.anchors_at(repo, old) {
                stale_anchors.push(StaleAnchor {
                    record: index.metas[i].meta.id.clone(),
                    kind,
                    repo: repo.to_string(),
                    path: old.to_string(),
                    change: file.status,
                    renamed_to: renamed_to.cloned(),
                });
            }
        }

        files.push(FileImpact {
            path: file.path.clone(),
            old_path: file.old_path.clone(),
            status: file.status,
            modules,
            features,
            links,
            covered,
        });
    }

    let mut affected: Vec<AffectedRecord> = affected
        .into_iter()
        .map(|(i, (relations, paths))| {
            let e = index.metas[i];
            AffectedRecord {
                id: e.meta.id.clone(),
                kind: e.meta.kind,
                status: e.meta.status,
                title: e.meta.title.clone(),
                origin: e.origin,
                record_path: e.path.clone(),
                relations: relations.into_iter().collect(),
                files: paths.into_iter().collect(),
            }
        })
        .collect();
    affected
        .sort_by(|a, b| (&a.id, a.origin, &a.record_path).cmp(&(&b.id, b.origin, &b.record_path)));
    files.sort_by(|a, b| (&a.path, &a.old_path).cmp(&(&b.path, &b.old_path)));
    unknown_coverage.sort_by(|a, b| a.path.cmp(&b.path));
    stale_anchors.sort();
    stale_anchors.dedup();

    ImpactReport {
        base: diff.base.clone(),
        merge_base: diff.merge_base.clone(),
        head: diff.head.clone(),
        repo: repo.map(str::to_string),
        kb_submodule: diff.kb_submodule.clone(),
        kb_change: diff.kb_pointer.is_some(),
        kb_pointer: diff.kb_pointer.clone(),
        files,
        affected,
        unknown_coverage,
        stale_anchors,
    }
}

/// Modules and features of the given paths in `repo` (empty when the repo is unknown).
fn file_area(
    registry: &Registry,
    repo: Option<&str>,
    paths: &[&str],
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut modules = BTreeSet::new();
    let mut features = BTreeSet::new();
    let Some(repo) = repo else {
        return (modules, features);
    };
    for path in paths {
        for m in registry.modules_for_path(repo, path) {
            modules.insert(m.id.clone());
            features.extend(m.features.iter().cloned());
        }
        for f in registry.features_for_path(repo, path) {
            features.insert(f.id.clone());
        }
    }
    (modules, features)
}

/// (meta index, relation, via, matched path)
type RawLink = (usize, Relation, String, Option<String>);

/// Anchored path (no trailing `/`) → (meta index, anchor kind).
type AnchorsByPath<'a> = BTreeMap<&'a str, Vec<(usize, AnchorKind)>>;

/// Lookup structures built once per analysis so that cost grows with the number of changed
/// files and matching records, not files × corpus.
struct LinkIndex<'a> {
    metas: Vec<&'a MetaEntry>,
    /// Repos each record's scope implies (`None` = unconstrained).
    implied: Vec<Option<BTreeSet<String>>>,
    /// Path selector globs keyed by their literal directory prefix.
    globs: BTreeMap<String, Vec<(usize, &'a str, RepoGlob)>>,
    /// Source/test/doc anchors by repo.
    anchors: BTreeMap<&'a str, AnchorsByPath<'a>>,
    scope_modules: BTreeMap<&'a str, Vec<usize>>,
    scope_features: BTreeMap<&'a str, Vec<usize>>,
    /// Contract parties listing modules: module → (meta index, party id).
    party_modules: BTreeMap<&'a str, Vec<(usize, &'a str)>>,
    /// Contract parties without modules: repo → (meta index, party id).
    party_repos: BTreeMap<&'a str, Vec<(usize, &'a str)>>,
    feature_records: BTreeMap<&'a str, Vec<usize>>,
}

impl<'a> LinkIndex<'a> {
    fn new(metas: &'a [MetaEntry], registry: &Registry) -> LinkIndex<'a> {
        let mut ix = LinkIndex {
            metas: metas
                .iter()
                .filter(|m| m.meta.status != Status::Superseded)
                .collect(),
            implied: Vec::new(),
            globs: BTreeMap::new(),
            anchors: BTreeMap::new(),
            scope_modules: BTreeMap::new(),
            scope_features: BTreeMap::new(),
            party_modules: BTreeMap::new(),
            party_repos: BTreeMap::new(),
            feature_records: BTreeMap::new(),
        };
        for (i, entry) in ix.metas.iter().enumerate() {
            let meta = &*entry.meta;
            ix.implied.push(implied_repos(&meta.scope, registry));
            for spec in &meta.selectors.paths {
                // Invalid globs are reported by validation; they cannot link anything.
                if let Ok(glob) = RepoGlob::parse(spec) {
                    ix.globs
                        .entry(glob.literal_dir_prefix())
                        .or_default()
                        .push((i, spec.as_str(), glob));
                }
            }
            for anchor in &meta.anchors {
                let (Some(repo), Some(path)) = (&anchor.repo, &anchor.path) else {
                    continue;
                };
                let path = path.trim_end_matches('/');
                if Relation::of_anchor(anchor.kind).is_some() && check_rel_path(path).is_ok() {
                    ix.anchors
                        .entry(repo.as_str())
                        .or_default()
                        .entry(path)
                        .or_default()
                        .push((i, anchor.kind));
                }
            }
            for m in &meta.scope.modules {
                ix.scope_modules.entry(m.as_str()).or_default().push(i);
            }
            for f in &meta.scope.features {
                ix.scope_features.entry(f.as_str()).or_default().push(i);
            }
            for party in &meta.parties {
                if party.modules.is_empty() {
                    ix.party_repos
                        .entry(party.repo.as_str())
                        .or_default()
                        .push((i, party.id.as_str()));
                }
                for m in &party.modules {
                    ix.party_modules
                        .entry(m.as_str())
                        .or_default()
                        .push((i, party.id.as_str()));
                }
            }
            if meta.kind == Kind::Feature
                && let Some(f) = &meta.feature
            {
                ix.feature_records.entry(f.as_str()).or_default().push(i);
            }
        }
        ix
    }

    /// Anchors (of any linked kind) whose path is exactly `path` in `repo`.
    fn anchors_at(&self, repo: &str, path: &str) -> &[(usize, AnchorKind)] {
        self.anchors
            .get(repo)
            .and_then(|m| m.get(path))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// May an unqualified selector of record `i` match files of `repo`?
    fn repo_allowed(&self, i: usize, repo: Option<&str>) -> bool {
        match &self.implied[i] {
            None => true,
            Some(repos) => repo.is_some_and(|r| repos.contains(r)),
        }
    }

    fn links(
        &self,
        repo: Option<&str>,
        paths: &[&str],
        modules: &BTreeSet<String>,
        features: &BTreeSet<String>,
    ) -> Vec<RawLink> {
        let mut out = Vec::new();
        for &path in paths {
            let prefixes = dir_prefixes(path);
            for prefix in &prefixes {
                for (i, spec, glob) in self.globs.get(prefix).into_iter().flatten() {
                    if glob.matches(repo, path)
                        && (glob.repo.is_some() || self.repo_allowed(*i, repo))
                    {
                        out.push((
                            *i,
                            Relation::PathSelector,
                            spec.to_string(),
                            Some(path.to_string()),
                        ));
                    }
                }
            }
            if let Some(repo) = repo {
                let dirs = prefixes.iter().map(|p| p.trim_end_matches('/'));
                for anchored in dirs.filter(|d| !d.is_empty()).chain([path]) {
                    for &(i, kind) in self.anchors_at(repo, anchored) {
                        if let Some(relation) = Relation::of_anchor(kind) {
                            out.push((i, relation, anchored.to_string(), Some(path.to_string())));
                        }
                    }
                }
            }
        }
        self.scope_links(repo, modules, features, &mut out);
        for m in modules {
            for &(i, party) in self.party_modules.get(m.as_str()).into_iter().flatten() {
                out.push((i, Relation::ContractParty, party.to_string(), None));
            }
        }
        if let Some(repo) = repo {
            for &(i, party) in self.party_repos.get(repo).into_iter().flatten() {
                out.push((i, Relation::ContractRepoParty, party.to_string(), None));
            }
        }
        for f in features {
            for &i in self.feature_records.get(f.as_str()).into_iter().flatten() {
                out.push((i, Relation::FeatureRecord, f.clone(), None));
            }
        }
        out
    }

    /// Records scoped to the file's modules or features. The record's scope must apply to
    /// the file in every dimension it constrains (AND across dimensions, OR within one).
    fn scope_links(
        &self,
        repo: Option<&str>,
        modules: &BTreeSet<String>,
        features: &BTreeSet<String>,
        out: &mut Vec<RawLink>,
    ) {
        let mut candidates: BTreeSet<usize> = BTreeSet::new();
        for m in modules {
            candidates.extend(self.scope_modules.get(m.as_str()).into_iter().flatten());
        }
        for f in features {
            candidates.extend(self.scope_features.get(f.as_str()).into_iter().flatten());
        }
        for i in candidates {
            let scope = &self.metas[i].meta.scope;
            let repo_ok =
                scope.repos.is_empty() || repo.is_some_and(|r| scope.repos.iter().any(|x| x == r));
            let hit_modules: Vec<&String> = scope
                .modules
                .iter()
                .filter(|m| modules.contains(*m))
                .collect();
            let hit_features: Vec<&String> = scope
                .features
                .iter()
                .filter(|f| features.contains(*f))
                .collect();
            let applies = repo_ok
                && (scope.modules.is_empty() || !hit_modules.is_empty())
                && (scope.features.is_empty() || !hit_features.is_empty());
            if !applies {
                continue;
            }
            for m in hit_modules {
                out.push((i, Relation::ModuleScope, m.clone(), None));
            }
            for f in hit_features {
                out.push((i, Relation::FeatureScope, f.clone(), None));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// MR statement and check (pure)
// ---------------------------------------------------------------------------------------

/// What the MR says about knowledge changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KbChangeKind {
    /// No KB change is needed; `reason` explains why.
    None,
    /// A separate KB change (`kb_revision` and/or `change_id`) carries the knowledge.
    Linked,
    /// The KB change is part of this MR (e.g. a KB pointer update).
    Included,
}

impl KbChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            KbChangeKind::None => "none",
            KbChangeKind::Linked => "linked",
            KbChangeKind::Included => "included",
        }
    }
}

/// The reviewable acknowledgement block of an MR description:
///
/// ```text
/// <!-- kb-impact:v1
/// kb_change = "none"            # none | linked | included
/// reason = "Pure refactoring; no behavior or contract changes."
/// # kb_revision = "<KB commit>" # linked: kb_revision and/or change_id
/// # change_id = "PROJ-123"
/// -->
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpactStatement {
    pub kb_change: KbChangeKind,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kb_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,
}

/// Find and parse the single `<!-- kb-impact:v1 ... -->` block in an MR description.
///
/// `Ok(None)` when there is no block. Errors: more than one block, an unterminated block, an
/// unsupported version, invalid or non-strict TOML (unknown fields, duplicate keys), an
/// empty `reason`, `linked` without `kb_revision`/`change_id`, malformed ids. The TOML must
/// not contain `-->` (it would end the HTML comment).
pub fn parse_statement(text: &str) -> std::result::Result<Option<ImpactStatement>, String> {
    let blocks = statement_blocks(text)?;
    match blocks.as_slice() {
        [] => Ok(None),
        [body] => parse_statement_body(body).map(Some),
        many => Err(format!(
            "found {} `{STATEMENT_TAG}` blocks; exactly one is allowed",
            many.len()
        )),
    }
}

/// Bodies of all `<!-- kb-impact... -->` comments.
fn statement_blocks(text: &str) -> std::result::Result<Vec<&str>, String> {
    const TAG: &str = "kb-impact";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        let after = rest[start + 4..].trim_start_matches([' ', '\t']);
        let Some(tagged) = after.strip_prefix(TAG) else {
            rest = &rest[start + 4..];
            continue;
        };
        let end = tagged
            .find("-->")
            .ok_or_else(|| format!("unterminated `{STATEMENT_TAG}` block (missing `-->`)"))?;
        let inner = &tagged[..end];
        let version_end = inner.find(char::is_whitespace).unwrap_or(inner.len());
        let version = &inner[..version_end];
        if format!("{TAG}{version}") != STATEMENT_TAG {
            return Err(format!(
                "unsupported block marker `{TAG}{}`; expected `{STATEMENT_TAG}`",
                version.escape_debug()
            ));
        }
        out.push(&inner[version_end..]);
        rest = &tagged[end + 3..];
    }
    Ok(out)
}

fn parse_statement_body(body: &str) -> std::result::Result<ImpactStatement, String> {
    let mut s: ImpactStatement = toml::from_str(body)
        .map_err(|e| format!("invalid `{STATEMENT_TAG}` block: {}", e.message().trim()))?;
    s.reason = s.reason.trim().to_string();
    if s.reason.is_empty() {
        return Err("`reason` must not be empty".into());
    }
    if let Some(rev) = &s.kb_revision
        && (rev.len() < 7 || rev.len() > 64 || !rev.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return Err("`kb_revision` must be a KB commit id of 7..=64 hex characters".into());
    }
    if let Some(id) = &s.change_id
        && (id.trim().is_empty() || id.chars().count() > 128 || id.chars().any(char::is_control))
    {
        return Err("`change_id` must be a single line of 1..=128 characters".into());
    }
    match s.kb_change {
        KbChangeKind::Linked if s.kb_revision.is_none() && s.change_id.is_none() => {
            Err("`kb_change = \"linked\"` requires `kb_revision` or `change_id`".into())
        }
        KbChangeKind::None if s.kb_revision.is_some() => {
            Err("`kb_revision` contradicts `kb_change = \"none\"`".into())
        }
        _ => Ok(s),
    }
}

/// Outcome of the acknowledgement check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    pub ok: bool,
    /// Affected knowledge or unknown coverage exists and the KB pointer is unchanged.
    pub acknowledgement_required: bool,
    /// Human-readable, deterministic explanation (first entry is the decisive one).
    pub reasons: Vec<String>,
    pub statement: Option<ImpactStatement>,
}

/// Check that the change acknowledges its knowledge impact.
///
/// An acknowledgement is required when affected knowledge or unknown coverage exists and the
/// diff does not change the KB pointer; it is a statement with `kb_change = none` (plus a
/// reason), `linked` or `included`. Statements contradicting the diff (`none` with a pointer
/// change, `included` without one while a KB submodule is known) fail. Only presence and
/// structure are checked, never the reasoning.
pub fn check(report: &ImpactReport, statement: Option<&ImpactStatement>) -> Verdict {
    let required =
        !report.kb_change && (!report.affected.is_empty() || !report.unknown_coverage.is_empty());
    let mut ok = true;
    let reason = match statement {
        None if required => {
            ok = false;
            format!(
                "{} affected record(s) and {} file(s) with unknown coverage are not acknowledged: \
                 update the KB pointer or add a `<!-- {STATEMENT_TAG} ... -->` block \
                 (kb_change = none|linked|included, reason) to the MR description",
                report.affected.len(),
                report.unknown_coverage.len()
            )
        }
        None if report.kb_change => "the diff updates the KB pointer".to_string(),
        None => "no affected knowledge and no unknown coverage".to_string(),
        Some(s) => match s.kb_change {
            KbChangeKind::None if report.kb_change => {
                ok = false;
                "the statement declares `kb_change = \"none\"`, but the diff updates the KB pointer"
                    .to_string()
            }
            KbChangeKind::None => "acknowledged: `kb_change = \"none\"` with a reason".to_string(),
            KbChangeKind::Linked => {
                let mut refs = Vec::new();
                if let Some(r) = &s.kb_revision {
                    refs.push(format!("kb_revision {r}"));
                }
                if let Some(c) = &s.change_id {
                    refs.push(format!("change_id {c}"));
                }
                format!(
                    "acknowledged: linked KB change ({}); the link itself is not verified",
                    refs.join(", ")
                )
            }
            KbChangeKind::Included if report.kb_change => {
                "acknowledged: the KB change is included (KB pointer updated)".to_string()
            }
            KbChangeKind::Included => match &report.kb_submodule {
                Some(path) => {
                    ok = false;
                    format!(
                        "the statement declares `kb_change = \"included\"`, but the diff does not \
                         update the KB pointer at `{path}`"
                    )
                }
                None => "acknowledged: `kb_change = \"included\"` (no KB submodule is known, so \
                         this is not verified against the diff)"
                    .to_string(),
            },
        },
    };
    Verdict {
        ok,
        acknowledgement_required: required,
        reasons: vec![reason],
        statement: statement.cloned(),
    }
}

// ---------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------

/// Deterministic JSON result (the CLI `result` object).
pub fn to_json(report: &ImpactReport, verdict: Option<&Verdict>) -> Value {
    let covered = report.files.iter().filter(|f| f.covered).count();
    json!({
        "diff": {
            "base": report.base,
            "merge_base": report.merge_base,
            "head": report.head,
            "working_tree": report.head.is_none(),
        },
        "repo": report.repo,
        "kb_submodule": report.kb_submodule,
        "kb_change": report.kb_change,
        "kb_pointer": report.kb_pointer,
        "summary": {
            "files": report.files.len(),
            "covered": covered,
            "unknown_coverage": report.unknown_coverage.len(),
            "affected": report.affected.len(),
            "stale_anchors": report.stale_anchors.len(),
        },
        "files": report.files,
        "affected": report.affected,
        "unknown_coverage": report.unknown_coverage,
        "stale_anchors": report.stale_anchors,
        "notes": [UNKNOWN_COVERAGE_NOTE],
        "check": verdict.map(|v| json!({
            "ok": v.ok,
            "acknowledgement_required": v.acknowledgement_required,
            "reasons": v.reasons,
            "statement": v.statement,
            "reasoning_evaluated": false,
            "note": REASONING_NOTE,
        })),
    })
}

/// Render the report (and verdict) in the requested format.
pub fn render(report: &ImpactReport, verdict: Option<&Verdict>, format: Format) -> String {
    match format {
        Format::Json => {
            let mut s = serde_json::to_string_pretty(&to_json(report, verdict)).unwrap_or_default();
            s.push('\n');
            s
        }
        Format::Compact => render_compact(report, verdict),
        Format::Human => render_human(report, verdict),
    }
}

fn short(oid: &str) -> &str {
    oid.get(..12).unwrap_or(oid)
}

fn pointer_text(report: &ImpactReport) -> String {
    match &report.kb_pointer {
        None => "unchanged".into(),
        Some(p) => format!(
            "{} -> {}",
            p.old.as_deref().map(short).unwrap_or("(none)"),
            p.new.as_deref().map(short).unwrap_or("(none)")
        ),
    }
}

fn list_or_dash(v: &[String]) -> String {
    if v.is_empty() {
        "-".into()
    } else {
        v.join(",")
    }
}

const REPO_UNKNOWN_NOTE: &str = "host repo not identified: module, feature, anchor and \
repo-scoped links were not evaluated";

fn render_compact(r: &ImpactReport, verdict: Option<&Verdict>) -> String {
    let mut s = format!(
        "impact base {} merge-base {} head {} repo {} kb-pointer {}\n",
        short(&r.base),
        short(&r.merge_base),
        r.head.as_deref().map(short).unwrap_or("working-tree"),
        r.repo.as_deref().unwrap_or("unknown"),
        pointer_text(r)
    );
    s.push_str(&format!(
        "summary files={} covered={} unknown={} affected={} stale-anchors={}\n",
        r.files.len(),
        r.files.iter().filter(|f| f.covered).count(),
        r.unknown_coverage.len(),
        r.affected.len(),
        r.stale_anchors.len()
    ));
    if r.repo.is_none() {
        s.push_str(&format!("warning {REPO_UNKNOWN_NOTE}\n"));
    }
    for f in &r.files {
        let from = f
            .old_path
            .as_deref()
            .map(|o| format!(" <- {o}"))
            .unwrap_or_default();
        let links = if f.links.is_empty() {
            "unknown-coverage".to_string()
        } else {
            let mut t = f
                .links
                .iter()
                .map(|l| format!("{}:{}({})", l.record, l.relation.as_str(), l.via))
                .collect::<Vec<_>>()
                .join(" ");
            if !f.covered {
                t.push_str(" unknown-coverage");
            }
            t
        };
        s.push_str(&format!(
            "file {} {}{} modules={} -> {}\n",
            f.status.letter(),
            f.path,
            from,
            list_or_dash(&f.modules),
            links
        ));
    }
    for a in &r.affected {
        s.push_str(&format!(
            "affected {} {} {} files={} relations={} {:?}\n",
            a.id,
            a.kind.as_str(),
            a.status.as_str(),
            a.files.len(),
            a.relations
                .iter()
                .map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(","),
            a.title
        ));
    }
    for u in &r.unknown_coverage {
        s.push_str(&format!(
            "unknown {} {} modules={}\n",
            u.status.letter(),
            u.path,
            list_or_dash(&u.modules)
        ));
    }
    for a in &r.stale_anchors {
        let to = a
            .renamed_to
            .as_deref()
            .map(|t| format!(" -> {t}"))
            .unwrap_or_default();
        s.push_str(&format!(
            "stale {} {}-anchor {}:{} {}{}\n",
            a.record,
            anchor_kind_str(a.kind),
            a.repo,
            a.path,
            a.change.as_str(),
            to
        ));
    }
    if !r.unknown_coverage.is_empty() {
        s.push_str(&format!("note {UNKNOWN_COVERAGE_NOTE}\n"));
    }
    if let Some(v) = verdict {
        s.push_str(&format!(
            "check {} acknowledgement-required={}: {}\n",
            if v.ok { "ok" } else { "failed" },
            v.acknowledgement_required,
            v.reasons.join("; ")
        ));
        s.push_str(&format!("note {REASONING_NOTE}\n"));
    }
    s
}

fn render_human(r: &ImpactReport, verdict: Option<&Verdict>) -> String {
    let mut s = String::from("Impact analysis\n");
    s.push_str(&format!("  base        {}\n", short(&r.base)));
    s.push_str(&format!("  merge-base  {}\n", short(&r.merge_base)));
    s.push_str(&format!(
        "  head        {}\n",
        r.head
            .as_deref()
            .map(short)
            .unwrap_or("work tree (staged, unstaged and untracked changes)")
    ));
    match &r.repo {
        Some(repo) => s.push_str(&format!("  repo        {repo}\n")),
        None => s.push_str(&format!("  repo        unknown ({REPO_UNKNOWN_NOTE})\n")),
    }
    let kb_at = r
        .kb_submodule
        .as_deref()
        .map(|p| format!(" (submodule `{p}`)"))
        .unwrap_or_default();
    s.push_str(&format!("  KB pointer  {}{kb_at}\n", pointer_text(r)));

    s.push_str(&format!("\nChanged files ({})\n", r.files.len()));
    for f in &r.files {
        let from = f
            .old_path
            .as_deref()
            .map(|o| format!(" (from {o})"))
            .unwrap_or_default();
        s.push_str(&format!(
            "  {} {}{}  [modules: {}; features: {}]\n",
            f.status.letter(),
            f.path,
            from,
            list_or_dash(&f.modules),
            list_or_dash(&f.features)
        ));
        for l in &f.links {
            s.push_str(&format!(
                "      {}  {}  {}\n",
                l.record,
                l.relation.as_str(),
                l.via
            ));
        }
        if !f.covered {
            s.push_str("      unknown coverage\n");
        }
    }

    s.push_str(&format!("\nAffected knowledge ({})\n", r.affected.len()));
    for a in &r.affected {
        s.push_str(&format!(
            "  {}  {}, {} - {}\n      via {}; files: {}\n",
            a.id,
            a.kind.as_str(),
            a.status.as_str(),
            a.title,
            a.relations
                .iter()
                .map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            a.files.join(", ")
        ));
    }

    s.push_str(&format!(
        "\nUnknown coverage ({})\n",
        r.unknown_coverage.len()
    ));
    if !r.unknown_coverage.is_empty() {
        s.push_str(&format!("  {UNKNOWN_COVERAGE_NOTE}\n"));
    }
    for u in &r.unknown_coverage {
        s.push_str(&format!(
            "  {} {}  [modules: {}]\n",
            u.status.letter(),
            u.path,
            list_or_dash(&u.modules)
        ));
    }

    s.push_str(&format!("\nStale anchors ({})\n", r.stale_anchors.len()));
    for a in &r.stale_anchors {
        let what = match &a.renamed_to {
            Some(t) => format!("renamed to {t}"),
            None => a.change.as_str().to_string(),
        };
        s.push_str(&format!(
            "  {}  {} anchor {}:{} - {}\n",
            a.record,
            anchor_kind_str(a.kind),
            a.repo,
            a.path,
            what
        ));
    }

    if let Some(v) = verdict {
        s.push_str(&format!(
            "\nAcknowledgement: {}{}\n",
            if v.ok { "OK" } else { "FAILED" },
            if v.acknowledgement_required {
                " (required)"
            } else {
                ""
            }
        ));
        for reason in &v.reasons {
            s.push_str(&format!("  - {reason}\n"));
        }
        if let Some(st) = &v.statement {
            s.push_str(&format!(
                "  statement: kb_change = {}; reason: {}\n",
                st.kb_change.as_str(),
                st.reason
            ));
        }
        s.push_str(&format!("  Note: {REASONING_NOTE}\n"));
    }
    s
}

fn anchor_kind_str(kind: AnchorKind) -> &'static str {
    match kind {
        AnchorKind::Source => "source",
        AnchorKind::Test => "test",
        AnchorKind::Change => "change",
        AnchorKind::Doc => "doc",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_diff_parsing() {
        let raw = b":100644 100644 aaa bbb M\0src/a.rs\0:100644 100644 ccc ccc R100\0old.rs\0new.rs\0:160000 160000 d0 e0 M\0.kb\0";
        let entries = parse_raw_diff(raw).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].status, ChangeStatus::Modified);
        assert_eq!(entries[1].status, ChangeStatus::Renamed);
        assert_eq!(entries[1].final_path(), "new.rs");
        let f = entries[1].clone().into_changed_file();
        assert_eq!(
            (f.path.as_str(), f.old_path.as_deref()),
            ("new.rs", Some("old.rs"))
        );
        assert_eq!(entries[2].dst_mode, GITLINK_MODE);
        assert!(parse_raw_diff(b"").unwrap().is_empty());
    }

    #[test]
    fn raw_diff_rejects_unsafe_or_malformed_records() {
        let cases: [&[u8]; 8] = [
            b":100644 100644 a b M\0../etc/passwd\0",
            b":100644 100644 a b M\0/abs\0",
            b":100644 100644 a b M\0a\\b\0",
            b":100644 100644 a b M\0bad\xff\0",
            b":100644 100644 a b R100\0only-one\0",
            b":100644 a b M\0x\0",
            b":100644 100644 a b X\0x\0",
            b"garbage\0",
        ];
        for bad in cases {
            assert!(
                parse_raw_diff(bad).is_err(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
        let e = parse_raw_diff(b":100644 100644 a b M\0../x\0").unwrap_err();
        assert_eq!(e.code, ErrorCode::UnsafePath);
    }

    #[test]
    fn statement_block_scanning() {
        let text = "Intro <!-- other comment -->\n<!--kb-impact:v1\nkb_change = \"none\"\nreason = \"x\"\n-->\n";
        assert_eq!(statement_blocks(text).unwrap().len(), 1);
        assert!(statement_blocks("<!-- kb-impact:v2\n-->").is_err());
        assert!(statement_blocks("<!-- kb-impact:v1\nkb_change = \"none\"").is_err());
        assert!(statement_blocks("no blocks here").unwrap().is_empty());
    }
}
