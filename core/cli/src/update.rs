//! Coordinated upstream updates (`kb update`): compatibility check, reviewable update
//! branches in isolated worktrees, engine divergence and cleanup (docs/architecture.md §1, §8).
//!
//! Safety:
//! * `check` and `divergence` never write to the KB repository; upstream objects go to an
//!   isolated bare cache `<cache>/upstream/<hash>.git` fetched under the project's transport
//!   policy, and conflicts are predicted with `git merge-tree` without any work tree.
//! * `prepare` writes only a namespaced ref `refs/kb/upstream/<ref>` (plus objects), a new
//!   branch `kb-update/<ref>` and a linked worktree under `<cache>/update/`. The main checkout
//!   (HEAD, index, work tree, other branches, runtime cache) is never modified, conflicts are
//!   left for manual resolution, repository hooks are disabled and nothing is pushed.
//! * `abandon` only removes `kb-update/*` branches whose worktree lives under `<cache>/update/`;
//!   it is a dry-run unless `--apply`, and refuses a worktree with uncommitted, untracked or
//!   conflicted files unless `--force`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::Serialize;
use serde_json::json;
use toml_edit::DocumentMut;

use crate::env::Env;
use crate::error::{ErrorCode, KbError, Result};
use crate::git::{Git, check_revision_arg, git_failure, redact};
use crate::migrate;
use crate::model::{ProfileConfig, ProfileLocation, UPSTREAM_FILE, UpstreamConfig};
use crate::util::{atomic_write, check_rel_path, sha256_hex};
use crate::versions::{MANIFEST_PATH, ReleaseManifest};

/// Prefix of branches created by `update prepare`.
pub const BRANCH_PREFIX: &str = "kb-update/";

/// Runs the *target* engine inside an update worktree.
pub trait EngineRunner {
    /// Run the engine of the checkout at `worktree` with `args`; a non-zero exit status is
    /// returned as output, not as an error.
    fn run(&self, worktree: &Path, args: &[&str]) -> Result<Output>;

    /// Explicitly bootstrap the target engine of `worktree` before any other step. The
    /// launcher never builds implicitly, so `update prepare` performs this documented action
    /// itself. Returns `None` when the runner needs no bootstrap.
    fn bootstrap(&self, worktree: &Path) -> Result<Option<Output>> {
        let _ = worktree;
        Ok(None)
    }
}

/// Production runner: `<worktree>/kbw <args>` with the worktree as KB root and its own
/// cache, sharing only the Cargo target directory to reuse builds.
pub struct KbwRunner {
    pub cargo_target_dir: PathBuf,
}

impl KbwRunner {
    /// The Cargo target directory is the one the user configured for the launcher
    /// (`KBW_CARGO_TARGET_DIR`), else the main checkout's default, so the target engine's
    /// build reuses compiled dependencies instead of starting from scratch.
    pub fn new(kb_root: &Path) -> KbwRunner {
        let cargo_target_dir = std::env::var_os("KBW_CARGO_TARGET_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            // The engine runs in the worktree: anchor a relative value at our cwd.
            .map(|p| match std::env::current_dir() {
                Ok(cwd) if p.is_relative() => cwd.join(p),
                _ => p,
            })
            .unwrap_or_else(|| kb_root.join(".cache/cargo-target"));
        KbwRunner { cargo_target_dir }
    }

    fn launch(&self, worktree: &Path, args: &[&str]) -> Result<Output> {
        let launcher = worktree.join("kbw");
        if !launcher.is_file() {
            return Err(KbError::new(
                ErrorCode::UpdateFailed,
                format!(
                    "the target engine has no launcher at {}",
                    launcher.display()
                ),
            ));
        }
        Command::new(&launcher)
            .args(args)
            .current_dir(worktree)
            .env("KB_ROOT", worktree)
            .env("KB_CACHE_DIR", worktree.join(".cache"))
            .env("KBW_CARGO_TARGET_DIR", &self.cargo_target_dir)
            .env_remove("KBW_AUTO_BOOTSTRAP")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| {
                KbError::new(
                    ErrorCode::UpdateFailed,
                    format!("cannot run {}: {e}", launcher.display()),
                )
            })
    }
}

impl EngineRunner for KbwRunner {
    fn run(&self, worktree: &Path, args: &[&str]) -> Result<Output> {
        self.launch(worktree, args)
    }

    fn bootstrap(&self, worktree: &Path) -> Result<Option<Output>> {
        self.launch(worktree, &["--kbw-bootstrap"]).map(Some)
    }
}

/// `--upstream <url|path> --ref <tag|commit>`.
#[derive(Debug, Clone)]
pub struct UpstreamSource {
    pub upstream: String,
    pub reference: String,
}

// ---------------------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct UpstreamInfo {
    /// Redacted URL or absolute local path.
    pub url: String,
    pub reference: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct VersionChange {
    pub current: String,
    pub target: String,
    pub changed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SchemaSupport {
    /// Distinct schema versions declared by the project files.
    pub project_schemas: Vec<u32>,
    pub target: u32,
    pub migrates_from: Vec<u32>,
    pub migration_required: bool,
    /// Project schemas the target engine can neither read nor migrate.
    pub unsupported: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergePrediction {
    pub merge_base: Option<String>,
    /// The KB `HEAD` already contains the upstream commit.
    pub up_to_date: bool,
    pub clean: bool,
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct StatusEntry {
    pub status: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub upstream: UpstreamInfo,
    pub head: String,
    pub versions: BTreeMap<String, VersionChange>,
    pub schema: SchemaSupport,
    pub merge: MergePrediction,
    /// Engine divergence against `project/upstream.toml` (absent when it cannot be computed;
    /// see `notes`).
    pub divergence: Option<DivergenceReport>,
    /// Uncommitted changes in the KB checkout; they are not part of an update.
    pub uncommitted: Vec<StatusEntry>,
    pub notes: Vec<String>,
}

impl CheckReport {
    /// Non-success status carried with the report.
    pub fn failure(&self) -> Option<KbError> {
        if !self.schema.unsupported.is_empty() {
            return Some(unsupported_schema(&self.upstream, &self.schema));
        }
        if !self.merge.conflicts.is_empty() {
            return Some(
                KbError::new(
                    ErrorCode::UpdateConflict,
                    format!(
                        "merging upstream {} would conflict in {} file(s)",
                        self.upstream.reference,
                        self.merge.conflicts.len()
                    ),
                )
                .with_details(json!({"conflicts": self.merge.conflicts}))
                .with_hint(
                    "`kbw update prepare` creates an update worktree with these conflicts for \
                     manual resolution; the main checkout is never modified",
                ),
            );
        }
        None
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitInfo {
    pub id: String,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangeSummary {
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub other: usize,
    /// Changes under project-owned paths (all of them).
    pub project: Vec<StatusEntry>,
    /// Number of changed engine (non-project) files.
    pub engine: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepReport {
    pub name: String,
    pub command: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrepareReport {
    pub branch: String,
    pub worktree: String,
    /// KB `HEAD` the branch starts from.
    pub base: String,
    pub upstream: UpstreamInfo,
    pub versions: BTreeMap<String, VersionChange>,
    pub schema: SchemaSupport,
    pub steps: Vec<StepReport>,
    /// Commits created on the branch (first-parent order, oldest first).
    pub commits: Vec<CommitInfo>,
    pub changes: ChangeSummary,
    /// Uncommitted changes in the main checkout that are not part of the update.
    pub uncommitted: Vec<StatusEntry>,
    pub notes: Vec<String>,
    pub next_steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PatchedEntry {
    pub status: String,
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DivergenceReport {
    /// Upstream base revision from `project/upstream.toml`.
    pub revision: String,
    pub reference: Option<String>,
    pub url: Option<String>,
    pub engine_paths: Vec<String>,
    /// Engine-owned paths that differ from the base (committed or not) and are not declared.
    pub diverged: Vec<StatusEntry>,
    /// Differences covered by declared `engine_patches`.
    pub patched: Vec<PatchedEntry>,
    /// Declared patches that match no difference.
    pub unused_patches: Vec<String>,
}

impl DivergenceReport {
    pub fn failure(&self) -> Option<KbError> {
        if self.diverged.is_empty() {
            return None;
        }
        Some(
            KbError::new(
                ErrorCode::EngineDiverged,
                format!(
                    "{} engine-owned path(s) diverge from upstream {}",
                    self.diverged.len(),
                    short(&self.revision)
                ),
            )
            .with_details(json!({"diverged": self.diverged}))
            .with_hint(
                "move engine changes upstream, or declare intentional patches as \
                 `[[engine_patches]] path, reason` in project/upstream.toml",
            ),
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AbandonReport {
    /// `dry-run` (nothing removed) or `apply`.
    pub mode: &'static str,
    pub branch: String,
    /// Commit the (deleted) branch points to (recover with `git branch <name> <commit>`).
    pub commit: String,
    pub worktree: String,
    /// `git status --porcelain` entries (uncommitted, untracked or conflicted files) of the
    /// worktree; `None` when its directory no longer exists.
    pub dirty: Option<usize>,
    /// Commits on the branch that the KB `HEAD` does not contain.
    pub commits_not_in_head: u64,
}

// ---------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------

/// Fetch the upstream ref into the isolated cache and report compatibility, predicted
/// conflicts, engine divergence and uncommitted changes. Never writes to the KB repository.
pub fn check(env: &Env, loc: &ProfileLocation, src: &UpstreamSource) -> Result<CheckReport> {
    require_git(env)?;
    let repo = KbRepo::open(env)?;
    let cfg = migrate::effective_config(&env.kb_root, loc)?;
    let up = fetch_upstream(env, &cfg, src)?;
    let head = up.fetch_local_head(&repo)?;
    let merge = predict_merge(&up.cache, &head, &up.info.commit)?;
    let schema = schema_support(&migrate::project_schemas(&env.kb_root, loc)?, &up.manifest);
    let versions = version_changes(&env.manifest, &up.manifest);
    let uncommitted = repo.status()?;
    let mut notes = version_notes(&versions, &schema);
    let divergence = match divergence(env) {
        Ok(d) => Some(d),
        Err(e) => {
            notes.push(format!("engine divergence not computed: {}", e.message));
            None
        }
    };
    if merge.up_to_date {
        notes.push(format!(
            "the KB HEAD already contains upstream {}",
            src.reference
        ));
    }
    if !uncommitted.is_empty() {
        notes.push(format!(
            "{} uncommitted change(s) in the KB checkout are not part of an update",
            uncommitted.len()
        ));
    }
    Ok(CheckReport {
        upstream: up.info,
        head,
        versions,
        schema,
        merge,
        divergence,
        uncommitted,
        notes,
    })
}

/// Prepare a reviewable update branch: linked worktree from KB `HEAD`, merge of the
/// upstream commit, target-engine migration/integration/validation, recorded upstream base
/// and a commit. Conflicts fail with `UPDATE_CONFLICT`, engine or validation failures with
/// `UPDATE_FAILED`; in both cases the worktree is kept and the main checkout is untouched.
/// The branch (default `kb-update/<ref>`) must start with `kb-update/`, so that
/// [`abandon`] can always remove it.
pub fn prepare(
    env: &Env,
    loc: &ProfileLocation,
    src: &UpstreamSource,
    branch: Option<&str>,
    runner: &dyn EngineRunner,
) -> Result<PrepareReport> {
    require_git(env)?;
    let repo = KbRepo::open(env)?;
    let cfg = migrate::effective_config(&env.kb_root, loc)?;
    let slug = ref_slug(&src.reference);
    let branch = branch.map_or_else(|| format!("{BRANCH_PREFIX}{slug}"), str::to_string);
    // `abandon` only removes `kb-update/*` branches, so every branch `prepare` creates (and
    // tells the user to abandon on failure) must carry the prefix.
    if !branch.starts_with(BRANCH_PREFIX) {
        return Err(KbError::invalid_input(format!(
            "invalid update branch `{branch}`: update branches must start with `{BRANCH_PREFIX}`"
        ))
        .with_hint(format!(
            "pass e.g. --branch {BRANCH_PREFIX}{}, or omit --branch for `{BRANCH_PREFIX}{slug}`",
            sanitize(&branch)
        )));
    }
    check_branch_name(&repo.git, &branch)?;
    if repo.branch_commit(&branch)?.is_some() {
        return Err(KbError::invalid_input(format!("branch `{branch}` already exists"))
            .with_hint(format!(
                "pass --branch {BRANCH_PREFIX}<name>, or discard the previous attempt with `kbw update abandon {branch} --apply`"
            )));
    }
    let worktree = env.cache_dir.join("update").join(sanitize(&branch));
    if worktree.exists() {
        return Err(KbError::invalid_input(format!(
            "update worktree {} already exists",
            worktree.display()
        ))
        .with_hint(
            "remove it with `kbw update abandon <branch> --apply` or `git worktree remove`",
        ));
    }
    let up = fetch_upstream(env, &cfg, src)?;
    let schema = schema_support(&migrate::project_schemas(&env.kb_root, loc)?, &up.manifest);
    if !schema.unsupported.is_empty() {
        return Err(unsupported_schema(&up.info, &schema));
    }
    let local_head = up.fetch_local_head(&repo)?;
    if local_head != repo.head {
        return Err(KbError::new(
            ErrorCode::GitError,
            "the KB HEAD moved while preparing the update",
        )
        .with_hint("re-run `kbw update prepare`"));
    }
    if up.cache.is_ancestor(&up.info.commit, &repo.head)? {
        return Err(KbError::invalid_input(format!(
            "the KB HEAD already contains upstream {} ({})",
            src.reference,
            short(&up.info.commit)
        )));
    }
    let versions = version_changes(&env.manifest, &up.manifest);
    let uncommitted = repo.status()?;

    // Writes to the KB repository start here: a namespaced ref, a branch and a worktree.
    let upstream_ref = format!("refs/kb/upstream/{slug}");
    let resolved_ref = format!("refs/kb/resolved/{slug}");
    up.cache
        .run(&["update-ref", &resolved_ref, &up.info.commit])?;
    env.progress(format!(
        "fetching upstream {} into {upstream_ref}",
        src.reference
    ));
    let cache_path = path_str(&up.cache_dir)?;
    let refspec = format!("+{resolved_ref}:{upstream_ref}");
    // The cache is a local repository created by kb itself.
    fetch(&repo.git, cache_path, &[&refspec], &["file".to_string()])?;
    let parent = worktree.parent().unwrap_or(&env.cache_dir);
    std::fs::create_dir_all(parent).map_err(|e| KbError::io(parent.display(), e))?;
    let wt_str = path_str(&worktree)?;
    let o = repo.git.output_with(
        &[
            "worktree", "add", "--quiet", "-b", &branch, wt_str, &repo.head,
        ],
        &no_hooks(),
    )?;
    if !o.ok() {
        return Err(git_failure(&["worktree"], &o));
    }

    // From here on failures keep the worktree for inspection.
    let fail = FailureContext {
        branch: branch.clone(),
        worktree: worktree.clone(),
    };
    let wgit = Git::new(&worktree);
    env.progress(format!("merging upstream {} into {branch}", src.reference));
    let message = format!(
        "kb: merge upstream {} ({})",
        src.reference,
        short(&up.info.commit)
    );
    let merge = wgit.output_with(
        &[
            "merge",
            "--no-ff",
            "--no-edit",
            "--no-verify",
            "-m",
            &message,
            &upstream_ref,
        ],
        &no_hooks(),
    )?;
    if !merge.ok() {
        let conflicts = unmerged_paths(&wgit)?;
        if conflicts.is_empty() {
            return Err(fail.error("merge", "git merge", merge.code, &merge.stderr));
        }
        return Err(conflict_error(&fail, &up.info, &repo.head, conflicts));
    }

    let mut steps = Vec::new();
    // The launcher never builds implicitly: bootstrapping the target engine inside the
    // worktree (its own .cache) is an explicit step of this command.
    if let Some(boot) = runner
        .bootstrap(&worktree)
        .map_err(|e| fail.error("bootstrap", "kbw --kbw-bootstrap", 1, &e.message))?
    {
        if !boot.status.success() {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&boot.stdout),
                String::from_utf8_lossy(&boot.stderr)
            );
            return Err(fail.error(
                "bootstrap",
                "kbw --kbw-bootstrap",
                boot.status.code().unwrap_or(-1),
                &text,
            ));
        }
        steps.push(StepReport {
            name: "bootstrap".into(),
            command: "kbw --kbw-bootstrap".into(),
            status: "ok".into(),
            note: None,
        });
    }
    steps.push(run_step(runner, &fail, "migrate", &["migrate", "--apply"])?);
    let skill_config = format!("{}/skill.toml", loc.skill_config_dir());
    if worktree.join(&skill_config).is_file() {
        steps.push(run_step(
            runner,
            &fail,
            "integrate",
            &["integrate", "--generate", "--apply"],
        )?);
    } else {
        steps.push(StepReport {
            name: "integrate".into(),
            command: "kbw integrate --generate --apply".into(),
            status: "skipped".into(),
            note: Some(format!("no {skill_config}; nothing to generate")),
        });
    }
    steps.push(run_step(runner, &fail, "validate", &["validate"])?);

    write_upstream_file(&worktree, &up.info)
        .map_err(|e| fail.error("upstream-file", UPSTREAM_FILE, 1, &e.message))?;
    let mut paths: BTreeSet<&str> = BTreeSet::from(["project"]);
    paths.insert(loc.dir.as_str());
    let mut add = vec!["add", "-A", "--"];
    add.extend(paths.iter().copied());
    wgit.run(&add)?;
    let staged = wgit.output(&["diff", "--cached", "--quiet"])?;
    if !staged.ok() {
        let subject = format!("kb: migrate knowledge for upstream {}", src.reference);
        let o = wgit.output_with(
            &["commit", "--no-verify", "--quiet", "-m", &subject],
            &no_hooks(),
        )?;
        if !o.ok() {
            return Err(fail.error("commit", "git commit", o.code, &o.stderr));
        }
    }

    let commits = first_parent_commits(&wgit, &repo.head)?;
    let changes = change_summary(&wgit, &repo.head, &loc.dir)?;
    let mut notes = version_notes(&versions, &schema);
    let leftovers = worktree_status(&wgit)?;
    if !leftovers.is_empty() {
        notes.push(format!(
            "{} file(s) outside the profile were changed by the target engine and not committed; inspect the worktree",
            leftovers.len()
        ));
    }
    if !uncommitted.is_empty() {
        notes.push(format!(
            "{} uncommitted change(s) in the main checkout are not part of this update",
            uncommitted.len()
        ));
    }
    let next_steps = vec![
        format!("review: git diff HEAD...{branch}"),
        format!("push the branch yourself (kb never pushes): git push <remote> {branch}"),
        "open a merge request and merge it after review; then remove the worktree with `git worktree remove`".into(),
        format!("to discard instead: kbw update abandon {branch} --apply"),
    ];
    Ok(PrepareReport {
        branch,
        worktree: worktree.display().to_string(),
        base: repo.head.clone(),
        upstream: up.info,
        versions,
        schema,
        steps,
        commits,
        changes,
        uncommitted,
        notes,
        next_steps,
    })
}

/// Engine-owned paths that differ between the recorded upstream base and the work tree
/// (committed, staged, unstaged and untracked), minus declared `engine_patches`.
pub fn divergence(env: &Env) -> Result<DivergenceReport> {
    let repo = KbRepo::open(env)?;
    let cfg = read_upstream_config(&env.kb_root)?;
    let revision = cfg.revision.clone().ok_or_else(|| {
        KbError::new(
            ErrorCode::ConfigInvalid,
            format!("{UPSTREAM_FILE} has no `revision`"),
        )
        .with_hint("set `revision` to the upstream commit this downstream was last updated from (kbw update prepare records it)")
    })?;
    check_revision_arg(&revision)?;
    let commit = repo.git.resolve_commit(&revision)?.ok_or_else(|| {
        KbError::new(
            ErrorCode::GitError,
            format!("upstream revision {revision} is not present in the KB repository"),
        )
        .with_hint("fetch it from the upstream (e.g. `git fetch <upstream-url> <tag>`) or run `kbw update check`")
    })?;
    for p in &cfg.engine_patches {
        check_rel_path(p.path.trim_end_matches('/')).map_err(|e| {
            KbError::new(
                ErrorCode::ConfigInvalid,
                format!("{UPSTREAM_FILE}: engine_patches: {e}"),
            )
        })?;
    }
    let specs: Vec<String> = env
        .manifest
        .engine_paths
        .iter()
        .map(|p| format!(":(literal){p}"))
        .collect();
    let mut args = vec!["diff", "--name-status", "-z", "--no-renames", &commit, "--"];
    args.extend(specs.iter().map(String::as_str));
    let mut entries = parse_name_status(&repo.git.run_bytes(&args)?);
    let mut args = vec!["ls-files", "-z", "--others", "--exclude-standard", "--"];
    args.extend(specs.iter().map(String::as_str));
    entries.extend(
        split_z(&repo.git.run_bytes(&args)?)
            .into_iter()
            .map(|path| StatusEntry {
                status: "??".into(),
                path,
            }),
    );
    entries.sort();
    entries.dedup();
    let mut used = BTreeSet::new();
    let mut diverged = Vec::new();
    let mut patched = Vec::new();
    for e in entries {
        let patch = cfg.engine_patches.iter().find(|p| {
            let p = p.path.trim_end_matches('/');
            e.path == p || e.path.strip_prefix(p).is_some_and(|r| r.starts_with('/'))
        });
        match patch {
            Some(p) => {
                used.insert(p.path.clone());
                patched.push(PatchedEntry {
                    status: e.status,
                    path: e.path,
                    reason: p.reason.clone(),
                });
            }
            None => diverged.push(e),
        }
    }
    let unused_patches = cfg
        .engine_patches
        .iter()
        .filter(|p| !used.contains(&p.path))
        .map(|p| p.path.clone())
        .collect();
    Ok(DivergenceReport {
        revision: commit,
        reference: cfg.r#ref.clone(),
        url: cfg.url.as_deref().map(redact),
        engine_paths: env.manifest.engine_paths.clone(),
        diverged,
        patched,
        unused_patches,
    })
}

/// Remove a `kb-update/*` branch and its worktree under `<cache>/update/`. Anything else
/// is refused. Without `apply` nothing is removed (dry-run report); a worktree with
/// uncommitted, untracked or conflicted files is removed only with `force`.
pub fn abandon(env: &Env, branch: &str, apply: bool, force: bool) -> Result<AbandonReport> {
    if !branch.starts_with(BRANCH_PREFIX) {
        return Err(KbError::invalid_input(format!(
            "refusing to abandon `{branch}`: only `{BRANCH_PREFIX}*` branches created by `kb update prepare` can be abandoned"
        )));
    }
    let repo = KbRepo::open(env)?;
    check_branch_name(&repo.git, branch)?;
    let full = format!("refs/heads/{branch}");
    let commit = repo.branch_commit(branch)?;
    let wt = list_worktrees(&repo.git)?
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(full.as_str()));
    let (Some(commit), Some(wt)) = (commit, wt) else {
        return Err(KbError::new(
            ErrorCode::NotFound,
            format!("no update worktree for branch `{branch}`"),
        )
        .with_hint("only branches with a worktree created by `kbw update prepare` are abandoned; delete other branches with git after review"));
    };
    let update_dir = canonical(&env.cache_dir.join("update"));
    if !canonical(&wt.path).starts_with(&update_dir) {
        return Err(KbError::invalid_input(format!(
            "refusing to abandon `{branch}`: its worktree {} is not under {}",
            wt.path.display(),
            update_dir.display()
        )));
    }
    if wt.locked {
        return Err(KbError::invalid_input(format!(
            "refusing to abandon `{branch}`: its worktree is locked"
        ))
        .with_hint("unlock it with `git worktree unlock` first"));
    }
    let dirty = if wt.path.exists() {
        Some(worktree_status(&Git::new(&wt.path))?.len())
    } else {
        None
    };
    let commits_not_in_head = repo
        .git
        .run(&["rev-list", "--count", &format!("{}..{commit}", repo.head)])?
        .trim()
        .parse()
        .unwrap_or(0);
    let report = AbandonReport {
        mode: if apply { "apply" } else { "dry-run" },
        branch: branch.to_string(),
        commit,
        worktree: wt.path.display().to_string(),
        dirty,
        commits_not_in_head,
    };
    if !apply {
        return Ok(report);
    }
    if let Some(n) = dirty.filter(|n| *n > 0 && !force) {
        return Err(KbError::new(
            ErrorCode::Conflict,
            format!(
                "refusing to abandon `{branch}`: its worktree {} has {n} uncommitted, untracked or conflicted file(s); nothing was removed",
                report.worktree
            ),
        )
        .with_details(json!({ "branch": branch, "worktree": report.worktree, "dirty": n }))
        .with_hint(format!(
            "commit or copy what you want to keep, then discard the rest explicitly: kbw update abandon {branch} --apply --force"
        )));
    }
    if wt.path.exists() {
        repo.git
            .run(&["worktree", "remove", "--force", path_str(&wt.path)?])?;
    } else {
        repo.git.run(&["worktree", "prune"])?;
    }
    repo.git.run(&["branch", "-D", branch])?;
    Ok(report)
}

// ---------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------

pub fn render_check(r: &CheckReport, human: bool) -> String {
    let mut s = format!(
        "update check: upstream {} = {} (from {})\n",
        r.upstream.reference,
        short(&r.upstream.commit),
        r.upstream.url
    );
    s.push_str(&render_versions(&r.versions, human));
    s.push_str(&format!(
        "  schema: project {:?} -> target {} (migrates from {:?}){}\n",
        r.schema.project_schemas,
        r.schema.target,
        r.schema.migrates_from,
        if r.schema.unsupported.is_empty() {
            ""
        } else {
            " UNSUPPORTED"
        }
    ));
    let merge = if r.merge.up_to_date {
        "already up to date".to_string()
    } else if r.merge.clean {
        "clean".to_string()
    } else {
        format!("{} conflict(s)", r.merge.conflicts.len())
    };
    s.push_str(&format!("  merge with HEAD {}: {merge}\n", short(&r.head)));
    for c in &r.merge.conflicts {
        s.push_str(&format!("    conflict {c}\n"));
    }
    if let Some(d) = &r.divergence {
        s.push_str(&format!(
            "  engine divergence: {} path(s), {} declared patch(es)\n",
            d.diverged.len(),
            d.patched.len()
        ));
        for e in &d.diverged {
            s.push_str(&format!("    {} {}\n", e.status, e.path));
        }
    }
    if human {
        for e in &r.uncommitted {
            s.push_str(&format!("  uncommitted {} {}\n", e.status, e.path));
        }
    }
    for n in &r.notes {
        s.push_str(&format!("  note: {n}\n"));
    }
    s
}

pub fn render_prepare(r: &PrepareReport, human: bool) -> String {
    let mut s = format!(
        "update prepared on branch {} (worktree {})\n  upstream {} = {} merged into {}\n",
        r.branch,
        r.worktree,
        r.upstream.reference,
        short(&r.upstream.commit),
        short(&r.base)
    );
    s.push_str(&render_versions(&r.versions, human));
    for step in &r.steps {
        s.push_str(&format!(
            "  {} {}: {}{}\n",
            step.status,
            step.name,
            step.command,
            step.note
                .as_ref()
                .map(|n| format!(" ({n})"))
                .unwrap_or_default()
        ));
    }
    for c in &r.commits {
        s.push_str(&format!("  commit {} {}\n", short(&c.id), c.subject));
    }
    let ch = &r.changes;
    s.push_str(&format!(
        "  changes: {} added, {} modified, {} deleted, {} other ({} engine, {} project)\n",
        ch.added,
        ch.modified,
        ch.deleted,
        ch.other,
        ch.engine,
        ch.project.len()
    ));
    if human {
        for e in &ch.project {
            s.push_str(&format!("    {} {}\n", e.status, e.path));
        }
    }
    for n in &r.notes {
        s.push_str(&format!("  note: {n}\n"));
    }
    s.push_str("next steps:\n");
    for n in &r.next_steps {
        s.push_str(&format!("  - {n}\n"));
    }
    s
}

pub fn render_divergence(r: &DivergenceReport, human: bool) -> String {
    let mut s = format!(
        "engine divergence against upstream {}{}: {} diverged, {} patched\n",
        short(&r.revision),
        r.reference
            .as_ref()
            .map(|x| format!(" ({x})"))
            .unwrap_or_default(),
        r.diverged.len(),
        r.patched.len()
    );
    for e in &r.diverged {
        s.push_str(&format!("  {} {}\n", e.status, e.path));
    }
    for p in &r.patched {
        s.push_str(&format!(
            "  patched {} {}: {}\n",
            p.status, p.path, p.reason
        ));
    }
    for p in &r.unused_patches {
        s.push_str(&format!("  unused patch declaration: {p}\n"));
    }
    if human && r.diverged.is_empty() {
        s.push_str("no undeclared engine changes\n");
    }
    s
}

pub fn render_abandon(r: &AbandonReport) -> String {
    let recover = format!(
        "{} commit(s) not in HEAD; recover with `git branch {} {}`",
        r.commits_not_in_head, r.branch, r.commit
    );
    if r.mode == "apply" {
        return format!(
            "abandoned {} (was {}); removed worktree {}\n  {recover}\n",
            r.branch,
            short(&r.commit),
            r.worktree
        );
    }
    let (state, flags) = match r.dirty {
        None => (
            "missing (only its registration is pruned)".to_string(),
            "--apply",
        ),
        Some(0) => ("clean".to_string(), "--apply"),
        Some(n) => (
            format!("{n} uncommitted, untracked or conflicted file(s) would be discarded"),
            "--apply --force",
        ),
    };
    format!(
        "dry-run: would abandon {branch} (at {at})\n  worktree {wt}: {state}\n  branch {branch}: {recover}\n\
         nothing was removed; run `kbw update abandon {branch} {flags}` to remove both\n",
        branch = r.branch,
        at = short(&r.commit),
        wt = r.worktree,
    )
}

fn render_versions(v: &BTreeMap<String, VersionChange>, human: bool) -> String {
    let mut s = String::new();
    for (name, c) in v {
        if c.changed {
            s.push_str(&format!("  {name}: {} -> {}\n", c.current, c.target));
        } else if human {
            s.push_str(&format!("  {name}: {} (unchanged)\n", c.current));
        }
    }
    if !human && v.values().all(|c| !c.changed) {
        s.push_str("  versions: unchanged\n");
    }
    s
}

// ---------------------------------------------------------------------------------------
// Git plumbing
// ---------------------------------------------------------------------------------------

/// The KB checkout as a Git work tree.
struct KbRepo {
    git: Git,
    head: String,
}

impl KbRepo {
    fn open(env: &Env) -> Result<KbRepo> {
        let not_repo = || {
            KbError::new(
                ErrorCode::GitError,
                format!(
                    "the KB root {} is not the top of a Git work tree",
                    env.kb_root.display()
                ),
            )
            .with_hint("kb update works on a KB checkout that keeps the upstream Git history")
        };
        let top = crate::git::toplevel(&env.kb_root).ok_or_else(not_repo)?;
        if canonical(&top) != canonical(&env.kb_root) {
            return Err(not_repo());
        }
        let git = Git::new(&env.kb_root);
        let head = git
            .resolve_commit("HEAD")?
            .ok_or_else(|| KbError::new(ErrorCode::GitError, "the KB checkout has no commits"))?;
        Ok(KbRepo { git, head })
    }

    fn branch_commit(&self, branch: &str) -> Result<Option<String>> {
        self.git.resolve_commit(&format!("refs/heads/{branch}"))
    }

    /// Uncommitted changes (staged, unstaged, untracked) of the main checkout.
    fn status(&self) -> Result<Vec<StatusEntry>> {
        worktree_status(&self.git)
    }
}

/// Upstream fetched into the isolated cache.
struct FetchedUpstream {
    info: UpstreamInfo,
    cache: Git,
    cache_dir: PathBuf,
    manifest: ReleaseManifest,
}

impl FetchedUpstream {
    /// Fetch the KB `HEAD` into the cache (for merge prediction) and return its commit.
    fn fetch_local_head(&self, repo: &KbRepo) -> Result<String> {
        let kb = path_str(&repo.git.dir)?;
        // The KB checkout itself is a local repository.
        fetch(
            &self.cache,
            kb,
            &["+HEAD:refs/kb/local/head"],
            &["file".to_string()],
        )?;
        self.cache
            .resolve_commit("refs/kb/local/head")?
            .ok_or_else(|| KbError::new(ErrorCode::GitError, "cannot read the fetched KB HEAD"))
    }
}

fn fetch_upstream(env: &Env, cfg: &ProfileConfig, src: &UpstreamSource) -> Result<FetchedUpstream> {
    check_revision_arg(&src.reference)?;
    let (url, protocol) = resolve_url(env, &src.upstream)?;
    if !cfg.source.allowed_protocols.contains(&protocol) {
        return Err(KbError::new(
            ErrorCode::ConfigInvalid,
            format!(
                "upstream `{}` uses protocol `{protocol}`, which is not in source.allowed_protocols {:?}",
                redact(&url),
                cfg.source.allowed_protocols
            ),
        )
        .with_hint(if protocol == "file" {
            "local upstream paths need \"file\" in [source] allowed_protocols of the profile config"
        } else {
            "add the protocol to [source] allowed_protocols of the profile config if it is trusted"
        }));
    }
    let cache_dir = env
        .cache_dir
        .join("upstream")
        .join(format!("{}.git", &sha256_hex(url.as_bytes())[..16]));
    if !cache_dir.join("HEAD").is_file() {
        std::fs::create_dir_all(&cache_dir).map_err(|e| KbError::io(cache_dir.display(), e))?;
        Git::new(&cache_dir).run(&["init", "--bare", "--quiet", "."])?;
    }
    let cache = Git::bare(&cache_dir);
    env.progress(format!("fetching upstream {}", redact(&url)));
    fetch(
        &cache,
        &url,
        &[
            "+refs/heads/*:refs/upstream/heads/*",
            "+refs/tags/*:refs/upstream/tags/*",
        ],
        &cfg.source.allowed_protocols,
    )?;
    let commit = resolve_upstream_ref(&cache, &src.reference)?.ok_or_else(|| {
        KbError::new(
            ErrorCode::NotFound,
            format!("upstream ref `{}` not found", src.reference),
        )
        .with_hint("use an upstream tag, branch, or a commit reachable from one of them")
    })?;
    let spec = format!("{commit}:{MANIFEST_PATH}");
    let o = cache.output(&["cat-file", "blob", &spec])?;
    if !o.ok() {
        return Err(KbError::invalid_input(format!(
            "upstream {} has no {MANIFEST_PATH}; it is not a kb upstream revision",
            src.reference
        )));
    }
    let text = String::from_utf8(o.stdout)
        .map_err(|_| KbError::invalid_input(format!("upstream {MANIFEST_PATH} is not UTF-8")))?;
    let manifest = ReleaseManifest::parse(&text)?;
    Ok(FetchedUpstream {
        info: UpstreamInfo {
            url: redact(&url),
            reference: src.reference.clone(),
            commit,
        },
        cache,
        cache_dir,
        manifest,
    })
}

/// Resolve `--ref` among fetched upstream tags and branches, or as a commit contained in them.
fn resolve_upstream_ref(cache: &Git, reference: &str) -> Result<Option<String>> {
    let mut candidates = Vec::new();
    if let Some(t) = reference.strip_prefix("refs/tags/") {
        candidates.push(format!("refs/upstream/tags/{t}"));
    } else if let Some(h) = reference.strip_prefix("refs/heads/") {
        candidates.push(format!("refs/upstream/heads/{h}"));
    } else {
        candidates.push(format!("refs/upstream/tags/{reference}"));
        candidates.push(format!("refs/upstream/heads/{reference}"));
    }
    for c in candidates {
        if let Some(id) = cache.resolve_commit(&c)? {
            return Ok(Some(id));
        }
    }
    let hex = reference.len() >= 7 && reference.chars().all(|c| c.is_ascii_hexdigit());
    if !hex {
        return Ok(None);
    }
    let Some(id) = cache.resolve_commit(reference)? else {
        return Ok(None);
    };
    // The cache also holds the local KB HEAD; only accept commits reachable from upstream.
    let contained = cache.run(&[
        "for-each-ref",
        "--count=1",
        "--format=%(refname)",
        "--contains",
        &id,
        "refs/upstream/",
    ])?;
    Ok((!contained.is_empty()).then_some(id))
}

/// Normalize the upstream argument: local paths become absolute. Returns (url, protocol).
fn resolve_url(env: &Env, raw: &str) -> Result<(String, String)> {
    if raw.is_empty() || raw.starts_with('-') || raw.chars().any(char::is_control) {
        return Err(KbError::invalid_input("invalid --upstream value"));
    }
    let protocol = url_protocol(raw);
    if protocol != "file" || raw.contains("://") {
        return Ok((raw.to_string(), protocol));
    }
    let path = env.cwd.join(raw);
    let path = path.canonicalize().map_err(|e| {
        KbError::invalid_input(format!("upstream path `{raw}` is not accessible: {e}"))
    })?;
    Ok((path_str(&path)?.to_string(), protocol))
}

/// Transport Git uses for a repository argument (`file` for local paths).
pub fn url_protocol(url: &str) -> String {
    let scheme = |s: &str| match s.to_ascii_lowercase().as_str() {
        "git+ssh" | "ssh+git" => "ssh".to_string(),
        other => other.to_string(),
    };
    if let Some(i) = url.find("://") {
        return scheme(&url[..i]);
    }
    if let Some(i) = url.find("::") {
        return scheme(&url[..i]);
    }
    // scp-like `[user@]host:path`: a colon before the first slash (not a drive letter).
    if let Some(i) = url.find(':')
        && i > 1
        && !url[..i].contains('/')
    {
        return "ssh".into();
    }
    "file".into()
}

fn predict_merge(cache: &Git, head: &str, upstream: &str) -> Result<MergePrediction> {
    let base = cache.output(&["merge-base", head, upstream])?;
    let merge_base = base.ok().then(|| base.stdout_str().trim().to_string());
    if cache.is_ancestor(upstream, head)? {
        return Ok(MergePrediction {
            merge_base,
            up_to_date: true,
            clean: true,
            conflicts: Vec::new(),
        });
    }
    let o = cache.output(&[
        "merge-tree",
        "--write-tree",
        "--name-only",
        "--no-messages",
        "-z",
        head,
        upstream,
    ])?;
    match o.code {
        0 | 1 => {
            let mut names = split_z(&o.stdout);
            // The first entry is the merged tree id.
            if !names.is_empty() {
                names.remove(0);
            }
            names.sort();
            names.dedup();
            Ok(MergePrediction {
                merge_base,
                up_to_date: false,
                clean: o.code == 0,
                conflicts: names,
            })
        }
        _ => Err(git_failure(&["merge-tree"], &o).with_hint(
            "the downstream must keep the upstream Git history (a fork or a copy with history)",
        )),
    }
}

/// `git fetch` of explicit refspecs without tags, submodules, `FETCH_HEAD` or automatic
/// maintenance, under an explicit transport policy. Stale destination refs are pruned.
fn fetch(git: &Git, from: &str, refspecs: &[&str], protocols: &[String]) -> Result<()> {
    let mut args = vec![
        "fetch",
        "--quiet",
        "--prune",
        "--no-tags",
        "--no-recurse-submodules",
        "--no-write-fetch-head",
        "--no-auto-maintenance",
        "--end-of-options",
        from,
    ];
    args.extend_from_slice(refspecs);
    let o = git.run_network(&args, protocols)?;
    if !o.ok() {
        return Err(KbError::new(
            ErrorCode::GitError,
            format!("cannot fetch from `{}`: {}", redact(from), o.stderr.trim()),
        ));
    }
    Ok(())
}

fn unmerged_paths(git: &Git) -> Result<Vec<String>> {
    let mut v = split_z(&git.run_bytes(&["diff", "--name-only", "-z", "--diff-filter=U"])?);
    v.sort();
    v.dedup();
    Ok(v)
}

fn worktree_status(git: &Git) -> Result<Vec<StatusEntry>> {
    let out = git.run_bytes(&[
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--no-renames",
    ])?;
    let mut v: Vec<StatusEntry> = split_z(&out)
        .into_iter()
        .filter(|e| e.len() > 3)
        .map(|e| StatusEntry {
            status: e[..2].trim().to_string(),
            path: e[3..].to_string(),
        })
        .collect();
    v.sort();
    Ok(v)
}

fn first_parent_commits(git: &Git, base: &str) -> Result<Vec<CommitInfo>> {
    let range = format!("{base}..HEAD");
    let out = git.run(&[
        "log",
        "--first-parent",
        "--reverse",
        "--format=%H%x09%s",
        &range,
    ])?;
    Ok(out
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(id, subject)| CommitInfo {
            id: id.to_string(),
            subject: subject.to_string(),
        })
        .collect())
}

fn change_summary(git: &Git, base: &str, profile_dir: &str) -> Result<ChangeSummary> {
    let entries = parse_name_status(&git.run_bytes(&[
        "diff",
        "--name-status",
        "-z",
        "--no-renames",
        base,
        "HEAD",
    ])?);
    let count = |s: &str| entries.iter().filter(|e| e.status == s).count();
    let in_profile = |p: &str| {
        ["project", profile_dir]
            .iter()
            .any(|d| p.strip_prefix(d).is_some_and(|r| r.starts_with('/')))
    };
    let (added, modified, deleted) = (count("A"), count("M"), count("D"));
    let project: Vec<StatusEntry> = entries
        .iter()
        .filter(|e| in_profile(&e.path))
        .cloned()
        .collect();
    Ok(ChangeSummary {
        added,
        modified,
        deleted,
        other: entries.len() - added - modified - deleted,
        engine: entries.len() - project.len(),
        project,
    })
}

struct Worktree {
    path: PathBuf,
    branch: Option<String>,
    locked: bool,
}

fn list_worktrees(git: &Git) -> Result<Vec<Worktree>> {
    let out = git.run_bytes(&["worktree", "list", "--porcelain", "-z"])?;
    let mut list: Vec<Worktree> = Vec::new();
    for field in out.split(|b| *b == 0) {
        let field = String::from_utf8_lossy(field);
        if let Some(p) = field.strip_prefix("worktree ") {
            list.push(Worktree {
                path: PathBuf::from(p),
                branch: None,
                locked: false,
            });
        } else if let Some(last) = list.last_mut() {
            if let Some(b) = field.strip_prefix("branch ") {
                last.branch = Some(b.to_string());
            } else if field == "locked" || field.starts_with("locked ") {
                last.locked = true;
            }
        }
    }
    Ok(list)
}

fn check_branch_name(git: &Git, name: &str) -> Result<()> {
    let invalid = || KbError::invalid_input(format!("invalid branch name `{name}`"));
    if name.is_empty() || name.len() > 200 || name.starts_with('-') || name.contains("@{") {
        return Err(invalid());
    }
    let o = git.output(&["check-ref-format", "--branch", name])?;
    if !o.ok() || o.stdout_str().trim() != name {
        return Err(invalid());
    }
    Ok(())
}

fn parse_name_status(out: &[u8]) -> Vec<StatusEntry> {
    let fields = split_z(out);
    fields
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| StatusEntry {
            status: c[0].clone(),
            path: c[1].clone(),
        })
        .collect()
}

fn split_z(out: &[u8]) -> Vec<String> {
    out.split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect()
}

/// Disable repository hooks for commands that would run them (worktree add, merge, commit).
fn no_hooks() -> Vec<String> {
    vec!["core.hooksPath=/dev/null".to_string()]
}

fn require_git(env: &Env) -> Result<()> {
    let out = Git::new(&env.kb_root).run(&["--version"])?;
    let found = parse_version(out.split_whitespace().nth(2).unwrap_or(""));
    let min = parse_version(&env.manifest.min_git);
    if found < min {
        return Err(KbError::new(
            ErrorCode::GitError,
            format!(
                "`kb update` needs git >= {} (found `{out}`)",
                env.manifest.min_git
            ),
        ));
    }
    Ok(())
}

fn parse_version(v: &str) -> (u32, u32, u32) {
    let mut parts = v.split('.').map(|p| {
        p.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap_or(0)
    });
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

// ---------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------

struct FailureContext {
    branch: String,
    worktree: PathBuf,
}

impl FailureContext {
    fn error(&self, step: &str, command: &str, exit_code: i32, output: &str) -> KbError {
        KbError::new(
            ErrorCode::UpdateFailed,
            format!("update step `{step}` failed on branch {}", self.branch),
        )
        .with_details(json!({
            "branch": self.branch,
            "worktree": self.worktree.display().to_string(),
            "step": step,
            "command": command,
            "exit_code": exit_code,
            "output": tail(output, 40),
        }))
        .with_hint(format!(
            "the worktree {} is kept for inspection and the main checkout was not modified; \
             fix and commit there, or discard with `kbw update abandon {} --apply` \
             (add --force for uncommitted changes)",
            self.worktree.display(),
            self.branch
        ))
    }
}

fn run_step(
    runner: &dyn EngineRunner,
    fail: &FailureContext,
    name: &str,
    args: &[&str],
) -> Result<StepReport> {
    let command = format!("kbw {}", args.join(" "));
    let out = runner
        .run(&fail.worktree, args)
        .map_err(|e| fail.error(name, &command, 1, &e.message))?;
    if !out.status.success() {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return Err(fail.error(name, &command, out.status.code().unwrap_or(-1), &text));
    }
    Ok(StepReport {
        name: name.to_string(),
        command,
        status: "ok".into(),
        note: None,
    })
}

fn conflict_error(
    fail: &FailureContext,
    up: &UpstreamInfo,
    base: &str,
    conflicts: Vec<String>,
) -> KbError {
    let wt = fail.worktree.display().to_string();
    let next = vec![
        format!("cd {wt} and resolve the conflicts; kb never resolves semantic conflicts"),
        "git add <files> && git commit --no-edit".to_string(),
        "run ./kbw migrate --apply, ./kbw integrate --generate --apply (if configured) and ./kbw validate in the worktree".to_string(),
        format!("set revision = \"{}\" and ref = \"{}\" in {UPSTREAM_FILE}, commit", up.commit, up.reference),
        format!(
            "or discard: kbw update abandon {} --apply (add --force to discard the conflicted files)",
            fail.branch
        ),
    ];
    KbError::new(
        ErrorCode::UpdateConflict,
        format!(
            "merging upstream {} into {} produced conflicts in {} file(s)",
            up.reference,
            fail.branch,
            conflicts.len()
        ),
    )
    .with_details(json!({
        "branch": fail.branch,
        "worktree": wt,
        "base": base,
        "upstream": up,
        "conflicts": conflicts,
        "next_steps": next,
    }))
    .with_hint(format!(
        "the conflicted worktree {wt} is left for manual resolution; the main checkout was not modified"
    ))
}

fn unsupported_schema(up: &UpstreamInfo, schema: &SchemaSupport) -> KbError {
    KbError::new(
        ErrorCode::UnsupportedSchemaVersion,
        format!(
            "upstream {} (document schema {}) cannot migrate project schema(s) {:?}",
            up.reference, schema.target, schema.unsupported
        ),
    )
    .with_details(json!({"schema": schema}))
    .with_hint("update through an intermediate upstream release whose `migrates_from` covers the project schema")
}

fn schema_support(project: &BTreeSet<u32>, target: &ReleaseManifest) -> SchemaSupport {
    let unsupported = project
        .iter()
        .copied()
        .filter(|s| *s != target.document_schema && !target.migrates_from.contains(s))
        .collect();
    SchemaSupport {
        project_schemas: project.iter().copied().collect(),
        target: target.document_schema,
        migrates_from: target.migrates_from.clone(),
        migration_required: project.iter().any(|s| *s != target.document_schema),
        unsupported,
    }
}

/// Reads one manifest field as a display string.
type ManifestField = fn(&ReleaseManifest) -> String;

fn version_changes(
    cur: &ReleaseManifest,
    tgt: &ReleaseManifest,
) -> BTreeMap<String, VersionChange> {
    let fields: [(&str, ManifestField); 6] = [
        ("manifest", |m| m.manifest.to_string()),
        ("engine_version", |m| m.engine_version.clone()),
        ("document_schema", |m| m.document_schema.to_string()),
        ("protocol", |m| m.protocol.to_string()),
        ("index_schema", |m| m.index_schema.to_string()),
        ("skill_protocol", |m| m.skill_protocol.to_string()),
    ];
    fields
        .into_iter()
        .map(|(name, get)| {
            let (current, target) = (get(cur), get(tgt));
            let changed = current != target;
            let change = VersionChange {
                current,
                target,
                changed,
            };
            (name.to_string(), change)
        })
        .collect()
}

fn version_notes(v: &BTreeMap<String, VersionChange>, schema: &SchemaSupport) -> Vec<String> {
    let changed = |k: &str| v.get(k).is_some_and(|c| c.changed);
    let mut notes = Vec::new();
    if schema.migration_required {
        notes.push("knowledge migration required: `update prepare` runs `kbw migrate --apply` on the update branch".into());
    }
    if changed("protocol") {
        notes.push("the CLI JSON protocol changes: update scripts that parse kb output".into());
    }
    if changed("index_schema") {
        notes.push("the index schema changes: derived indexes are rebuilt on first use".into());
    }
    if changed("skill_protocol") {
        notes.push("the skill protocol changes: after merging, re-run `kbw integrate --apply` in host repositories and start new agent sessions so the updated skill is loaded".into());
    }
    notes
}

fn read_upstream_config(kb_root: &Path) -> Result<UpstreamConfig> {
    let path = kb_root.join(UPSTREAM_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(KbError::new(
                ErrorCode::ConfigInvalid,
                format!("{UPSTREAM_FILE} is missing: the upstream base is unknown"),
            )
            .with_hint("`kbw update prepare` records it; or create it with `schema = 1` and `revision = \"<upstream commit>\"`"));
        }
        Err(e) => return Err(KbError::io(UPSTREAM_FILE, e)),
    };
    let cfg: UpstreamConfig = toml::from_str(&text)
        .map_err(|e| KbError::new(ErrorCode::ConfigInvalid, format!("{UPSTREAM_FILE}: {e}")))?;
    if cfg.schema != 1 {
        return Err(KbError::new(
            ErrorCode::ConfigInvalid,
            format!("{UPSTREAM_FILE}: unsupported schema {}", cfg.schema),
        ));
    }
    Ok(cfg)
}

/// Record the upstream base in the worktree, keeping other keys (e.g. `engine_patches`).
fn write_upstream_file(worktree: &Path, up: &UpstreamInfo) -> Result<()> {
    let path = worktree.join(UPSTREAM_FILE);
    let mut doc = match std::fs::read_to_string(&path) {
        Ok(text) => text
            .parse::<DocumentMut>()
            .map_err(|e| KbError::new(ErrorCode::ConfigInvalid, format!("{UPSTREAM_FILE}: {e}")))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DocumentMut::new(),
        Err(e) => return Err(KbError::io(UPSTREAM_FILE, e)),
    };
    if !doc.contains_key("schema") {
        doc["schema"] = toml_edit::value(1);
    }
    doc["url"] = toml_edit::value(up.url.clone());
    doc["revision"] = toml_edit::value(up.commit.clone());
    doc["ref"] = toml_edit::value(up.reference.clone());
    let text = doc.to_string();
    toml::from_str::<UpstreamConfig>(&text)
        .map_err(|e| KbError::new(ErrorCode::ConfigInvalid, format!("{UPSTREAM_FILE}: {e}")))?;
    atomic_write(&path, text.as_bytes())
}

/// Branch/directory-safe slug of a ref: `refs/tags/v1.2` → `v1.2`, other characters → `-`.
pub fn ref_slug(reference: &str) -> String {
    let r = reference
        .strip_prefix("refs/tags/")
        .or_else(|| reference.strip_prefix("refs/heads/"))
        .unwrap_or(reference);
    sanitize(r)
}

fn sanitize(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' {
            c
        } else {
            '-'
        };
        if matches!(c, '-' | '.') && (out.is_empty() || out.ends_with(['-', '.'])) {
            continue;
        }
        out.push(c);
        if out.len() >= 64 {
            break;
        }
    }
    let mut out = out.trim_end_matches(['-', '.']).to_string();
    if let Some(stem) = out.strip_suffix(".lock") {
        out = format!("{stem}-lock");
    }
    if out.is_empty() { "update".into() } else { out }
}

fn short(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    redact(&all[all.len().saturating_sub(lines)..].join("\n"))
}

fn canonical(p: &Path) -> PathBuf {
    if let Ok(c) = p.canonicalize() {
        return c;
    }
    match (
        p.parent().and_then(|d| d.canonicalize().ok()),
        p.file_name(),
    ) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => p.to_path_buf(),
    }
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str()
        .ok_or_else(|| KbError::invalid_input(format!("path {} is not UTF-8", p.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocols_of_repository_arguments() {
        assert_eq!(url_protocol("https://example.invalid/kb.git"), "https");
        assert_eq!(url_protocol("git+ssh://host/kb"), "ssh");
        assert_eq!(url_protocol("git@example.invalid:org/kb.git"), "ssh");
        assert_eq!(url_protocol("file:///srv/kb.git"), "file");
        assert_eq!(url_protocol("/srv/kb.git"), "file");
        assert_eq!(url_protocol("../kb"), "file");
        assert_eq!(url_protocol("ext::sh -c evil"), "ext");
    }

    #[test]
    fn slugs_are_safe_branch_and_directory_names() {
        assert_eq!(ref_slug("v0.2.0"), "v0.2.0");
        assert_eq!(ref_slug("refs/tags/v1"), "v1");
        assert_eq!(ref_slug("release/2026 q3"), "release-2026-q3");
        assert_eq!(ref_slug("..x..lock"), "x-lock");
        assert_eq!(ref_slug("///"), "update");
        assert_eq!(sanitize("kb-update/v0.2.0"), "kb-update-v0.2.0");
        assert_eq!(ref_slug(&"a".repeat(100)).len(), 64);
    }

    #[test]
    fn schema_support_uses_migrates_from() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../release.toml"))
            .unwrap();
        let m = ReleaseManifest::parse(&text).unwrap();
        let s = schema_support(&BTreeSet::from([0, 1]), &m);
        assert!(s.unsupported.is_empty() && s.migration_required);
        let s = schema_support(&BTreeSet::from([7]), &m);
        assert_eq!(s.unsupported, vec![7]);
        assert!(!schema_support(&BTreeSet::from([1]), &m).migration_required);
    }

    #[test]
    fn git_version_parsing() {
        assert!(parse_version("2.39.3") >= parse_version("2.38.0"));
        assert!(parse_version("2.37.9") < parse_version("2.38.0"));
        assert_eq!(parse_version("2.55.0.windows.1"), (2, 55, 0));
    }
}
