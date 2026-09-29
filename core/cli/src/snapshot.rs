//! Freshness (remote check per call) and snapshot selection; isolated Git mirror cache;
//! immutable Git-tree sources. See docs/architecture.md §6.
//!
//! Trust and safety rules implemented here:
//! * The source settings (remote, approved ref, transport policy) come from the *local*
//!   working-tree profile config; a snapshot can never redirect where kb fetches from.
//! * Fetches go into an isolated bare mirror under the cache directory. The KB checkout and
//!   the host are only read: no pull/merge/rebase/reset/checkout/stash/submodule update, and
//!   no ref of the KB repository is ever written.
//! * The engine manifest of a snapshot is checked before any of its knowledge is read.

pub(crate) mod mirror;
mod tree;

pub use tree::GitTreeSource;

use std::path::Path;

use serde::Serialize;
use serde_json::json;

use crate::corpus;
use crate::env::Env;
use crate::error::{ErrorCode, KbError, Result};
use crate::git::{self, Git, git_failure};
use crate::host::HostContext;
use crate::knowledge::{Freshness, Overlay, PinInfo, Selection, SnapshotInfo};
use crate::model::registry::normalize_remote_url;
use crate::model::{ProfileConfig, ProfileLocation, SkillSnapshot};
use crate::source::{FrozenWorkingTree, SourceTree, WorkingTreeSource};
use crate::util::FieldHasher;
use crate::versions::{
    ENGINE_VERSION, INDEX_SCHEMA, MANIFEST_PATH, PARSER_VERSION, ReleaseManifest,
    check_snapshot_compat,
};
use mirror::{LOCAL_HEAD_REF, Mirror, RemoteFetch, ahead_behind, is_full_oid};

/// Requested snapshot selection (`--snapshot`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionRequest {
    Auto,
    Latest,
    Pinned,
    WorkingTree,
    Revision(String),
}

impl SelectionRequest {
    pub fn parse(s: Option<&str>) -> Result<SelectionRequest> {
        Ok(match s {
            None | Some("auto") => SelectionRequest::Auto,
            Some("latest") => SelectionRequest::Latest,
            Some("pinned") => SelectionRequest::Pinned,
            Some("working-tree") => SelectionRequest::WorkingTree,
            Some(rev) => {
                crate::git::check_revision_arg(rev)?;
                SelectionRequest::Revision(rev.to_string())
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotRequest {
    pub selection: SelectionRequest,
    pub offline: bool,
    pub include_proposals: bool,
}

/// A resolved, immutable snapshot ready for indexing.
pub struct ResolvedSnapshot {
    pub info: SnapshotInfo,
    /// Content of the selected revision (or working tree).
    pub source: Box<dyn SourceTree>,
    /// Profile config read from the selected snapshot.
    pub config: ProfileConfig,
    pub overlay: Option<Overlay>,
}

/// Resolve freshness + selection + engine compatibility + overlay.
///
/// Errors: `PROJECT_NOT_INITIALIZED` (no local profile config), `FRESHNESS_UNVERIFIED`
/// (remote check failed and `offline` is false), `UPDATE_REQUIRED` (host pin differs from
/// the approved tip under `auto`, or the snapshot needs another engine),
/// `SNAPSHOT_NOT_FOUND` (nothing to select).
///
/// Proposals (`include_proposals`) are computed for Git snapshots only: a working-tree
/// snapshot already contains every local change.
pub fn resolve(
    env: &Env,
    loc: &ProfileLocation,
    host: Option<&HostContext>,
    req: &SnapshotRequest,
) -> Result<ResolvedSnapshot> {
    let local_cfg = corpus::load_config(&WorkingTreeSource::new(&env.kb_root), loc)?;
    let kb = kb_git(env);
    let src = ApprovedSource::configured(env, kb.as_ref(), &local_cfg)?;
    let mirror = src.mirror(env)?;
    let state = refresh(env, kb.as_ref(), &src, mirror.as_ref(), req.offline)?;
    let tip = state.tip.as_deref();
    let pin = host
        .and_then(|h| h.pin.as_ref())
        .map(|p| resolve_pin(p, kb.as_ref(), mirror.as_ref()))
        .transpose()?;

    let (selection, target) = select(req, host, pin.as_ref(), tip, kb.as_ref(), mirror.as_ref())
        .map_err(|e| add_source_details(e, &src, &state))?;

    // `content` identifies what is read: the commit, or a digest of working-tree files.
    let (source, revision, approved, content): (Box<dyn SourceTree>, _, _, _) = match target {
        Target::WorkingTree => {
            // The stat cache avoids re-hashing unchanged files on every working-tree call.
            let stat_cache = env
                .cache_dir
                .join(format!("wt-stat-{}.json", loc.profile.as_str()));
            let wt = WorkingTreeSource::with_stat_cache(&env.kb_root, stat_cache);
            // Key and build use one listing: the source serves exactly the digested content
            // (or fails with a retryable error), so a key never names other content.
            let mut prefixes = loc.content_prefixes(&local_cfg);
            prefixes.push(MANIFEST_PATH.to_string());
            let frozen = FrozenWorkingTree::freeze(wt, &prefixes)?;
            let digest = working_tree_digest(&frozen);
            (
                Box::new(frozen),
                None,
                Some(false),
                format!("working-tree:{digest}"),
            )
        }
        Target::Commit(rev) => {
            let m = mirror.as_ref().ok_or_else(|| {
                KbError::new(
                    ErrorCode::SnapshotNotFound,
                    format!(
                        "revision {rev} cannot be read: remote `{}` is not configured in the KB checkout",
                        src.remote
                    ),
                )
            })?;
            let keep_as = format!("refs/kb/selected/{rev}");
            let remote = (!req.offline).then(|| src.remote_fetch()).flatten();
            if !m.ensure_commit(
                &rev,
                &keep_as,
                kb.as_ref().map(|_| env.kb_root.as_path()),
                remote.as_ref(),
            )? {
                return Err(KbError::new(
                    ErrorCode::SnapshotNotFound,
                    format!("KB revision {rev} is not available locally or on the remote"),
                )
                .with_hint("fetch it in the KB checkout, or select another snapshot"));
            }
            let approved = match tip {
                Some(t) => Some(rev == t || m.git().is_ancestor(&rev, t)?),
                None => None,
            };
            let tree = GitTreeSource::new(m.git().clone(), rev.clone())?;
            // The engine manifest and the profile config are read next: fetch both at once.
            tree.prefetch(&[MANIFEST_PATH, loc.config.as_str()])?;
            (Box::new(tree), Some(rev.clone()), approved, rev)
        }
    };

    check_engine(
        source.as_ref(),
        revision.as_deref().unwrap_or("working-tree"),
    )?;
    let config = corpus::load_config(source.as_ref(), loc)?;
    let overlay = if req.include_proposals && revision.is_some() {
        Some(crate::overlay::compute_with(
            env,
            loc,
            &local_cfg,
            tip,
            kb.as_ref(),
            mirror.as_ref(),
        )?)
    } else {
        None
    };

    let mut h = FieldHasher::new();
    h.field("kb-snapshot/1")
        .field(loc.profile.as_str())
        .field(&loc.config)
        .field(src.identity())
        .field(&src.approved_ref)
        .field(&content)
        .field(ENGINE_VERSION)
        .field(INDEX_SCHEMA.to_string())
        .field(PARSER_VERSION.to_string())
        .field(overlay.as_ref().map(|o| o.digest.as_str()).unwrap_or(""));

    let info = SnapshotInfo {
        profile: loc.profile.as_str().to_string(),
        remote: src.remote.clone(),
        source: src.display(),
        approved_ref: src.approved_ref.clone(),
        selection,
        freshness: state.freshness,
        revision,
        latest_approved: state.tip.clone(),
        approved,
        pin,
        overlay: overlay.as_ref().map(Overlay::info),
        engine_version: ENGINE_VERSION.to_string(),
        key: h.finish_hex(),
    };
    Ok(ResolvedSnapshot {
        info,
        source,
        config,
        overlay,
    })
}

/// The KB checkout's own Git repository, when `env.kb_root` is a Git work-tree top level
/// (a plain directory inside some other repository is not a KB checkout).
pub fn kb_git(env: &Env) -> Option<Git> {
    kb_git_at(&env.kb_root)
}

/// [`kb_git`] for an explicit KB root.
pub fn kb_git_at(kb_root: &Path) -> Option<Git> {
    let top = git::toplevel(kb_root)?.canonicalize().ok()?;
    (top == kb_root.canonicalize().ok()?).then(|| Git::new(kb_root))
}

// ---------------------------------------------------------------------------------------
// Approved source and freshness
// ---------------------------------------------------------------------------------------

/// The trusted approved source of a profile, taken from the local working-tree config.
#[derive(Debug, Clone)]
pub(crate) struct ApprovedSource {
    pub remote: String,
    pub approved_ref: String,
    pub allowed_protocols: Vec<String>,
    /// Fetch URL (relative local paths resolved against the KB root); `None` when the remote
    /// is not configured in the KB checkout (or the KB root is not a Git checkout).
    pub url: Option<String>,
}

impl ApprovedSource {
    pub fn configured(env: &Env, kb: Option<&Git>, cfg: &ProfileConfig) -> Result<ApprovedSource> {
        check_approved_ref(&cfg.source.approved_ref)?;
        let url = match kb {
            Some(kb) => remote_url(kb, &env.kb_root, &cfg.source.remote)?,
            None => None,
        };
        Ok(ApprovedSource {
            remote: cfg.source.remote.clone(),
            approved_ref: cfg.source.approved_ref.clone(),
            allowed_protocols: cfg.source.allowed_protocols.clone(),
            url,
        })
    }

    /// The URL with credentials removed (safe to print).
    pub fn display(&self) -> String {
        self.url.as_deref().map(git::redact).unwrap_or_default()
    }

    /// Normalized source identity (scheme, credentials and `.git` suffix removed).
    pub fn identity(&self) -> String {
        self.url
            .as_deref()
            .map(normalize_remote_url)
            .unwrap_or_default()
    }

    /// The isolated mirror for this source (`None` without a remote URL).
    pub fn mirror(&self, env: &Env) -> Result<Option<Mirror>> {
        if self.url.is_none() {
            return Ok(None);
        }
        let mut h = FieldHasher::new();
        h.field(self.identity()).field(&self.approved_ref);
        let id = h.finish_hex();
        Mirror::open(&env.cache_dir, &id[..16]).map(Some)
    }

    fn remote_fetch(&self) -> Option<RemoteFetch<'_>> {
        self.url.as_deref().map(|url| RemoteFetch {
            url,
            allowed_protocols: &self.allowed_protocols,
        })
    }
}

/// What one call knows about the approved tip.
struct ApprovedState {
    freshness: Freshness,
    tip: Option<String>,
    /// Tip the mirror held before this call.
    previous: Option<String>,
}

/// Verify freshness (fetch the approved ref) or, offline, find the last known approved tip:
/// the mirror's `refs/kb/approved`, else the KB checkout's remote-tracking ref.
fn refresh(
    env: &Env,
    kb: Option<&Git>,
    src: &ApprovedSource,
    mirror: Option<&Mirror>,
    offline: bool,
) -> Result<ApprovedState> {
    let previous = match mirror {
        Some(m) => m.approved_tip()?,
        None => None,
    };
    if !offline {
        let (Some(m), Some(url)) = (mirror, src.url.as_deref()) else {
            let why = if kb.is_none() {
                "the KB root is not a Git checkout".to_string()
            } else {
                format!(
                    "remote `{}` is not configured in the KB checkout",
                    src.remote
                )
            };
            return Err(freshness_error(src, &why).with_hint(format!(
                "add the approved remote to the KB checkout (`git remote add {} <url>`), or pass \
                 --offline to read without a freshness check",
                src.remote
            )));
        };
        env.progress(format!(
            "checking {} on {} ({})",
            src.approved_ref,
            src.remote,
            src.display()
        ));
        let out = m.fetch_approved(url, &src.approved_ref, &src.allowed_protocols)?;
        if !out.ok() {
            return Err(freshness_error(src, out.stderr.trim()));
        }
        let tip = m
            .approved_tip()?
            .ok_or_else(|| freshness_error(src, "the fetch did not record the approved ref"))?;
        return Ok(ApprovedState {
            freshness: Freshness::Verified,
            tip: Some(tip),
            previous,
        });
    }
    if previous.is_some() {
        return Ok(ApprovedState {
            freshness: Freshness::Unverified,
            tip: previous.clone(),
            previous,
        });
    }
    let mut tip = None;
    if let (Some(kb), Some(m), Some(branch)) =
        (kb, mirror, src.approved_ref.strip_prefix("refs/heads/"))
        && let Some(c) = kb.resolve_commit(&format!("refs/remotes/{}/{branch}", src.remote))?
        && m.ensure_commit(&c, "refs/kb/local/tracking", Some(&env.kb_root), None)?
    {
        tip = Some(c);
    }
    Ok(ApprovedState {
        freshness: Freshness::Unverified,
        tip,
        previous: None,
    })
}

fn freshness_error(src: &ApprovedSource, reason: &str) -> KbError {
    let shown = src.display();
    let at = if shown.is_empty() {
        String::new()
    } else {
        format!(" ({shown})")
    };
    KbError::new(
        ErrorCode::FreshnessUnverified,
        git::redact(&format!(
            "cannot verify `{}` on remote `{}`{at}: {reason}",
            src.approved_ref, src.remote
        )),
    )
    .with_details(json!({
        "remote": src.remote,
        "source": shown,
        "approved_ref": src.approved_ref,
    }))
    .with_hint(
        "check network access, credentials and `source.allowed_protocols`; or pass --offline \
         to use the last fetched approved revision (reported as freshness=unverified)",
    )
}

/// Attach source/freshness facts to selection errors that do not carry details yet.
fn add_source_details(e: KbError, src: &ApprovedSource, state: &ApprovedState) -> KbError {
    if !e.details.is_null() {
        return e;
    }
    let details = json!({
        "remote": src.remote,
        "source": src.display(),
        "approved_ref": src.approved_ref,
        "latest_approved": state.tip,
        "freshness": state.freshness,
    });
    e.with_details(details)
}

/// Reject approved refs that could not be used safely in a refspec.
fn check_approved_ref(r: &str) -> Result<()> {
    let bad_char = |c: char| {
        c.is_control() || c.is_whitespace() || matches!(c, ':' | '*' | '?' | '[' | '\\' | '^' | '~')
    };
    if !r.starts_with("refs/")
        || r.ends_with('/')
        || r.ends_with(".lock")
        || r.contains("..")
        || r.contains("//")
        || r.contains("@{")
        || r.chars().any(bad_char)
    {
        return Err(KbError::new(
            ErrorCode::ConfigInvalid,
            format!("source.approved_ref `{r}` is not a valid fully qualified ref name"),
        ));
    }
    Ok(())
}

/// `remote.<name>.url` of the KB checkout, with relative local paths resolved.
fn remote_url(kb: &Git, kb_root: &Path, remote: &str) -> Result<Option<String>> {
    let key = format!("remote.{remote}.url");
    let o = kb.output(&["config", "--get", &key])?;
    if o.code == 1 {
        return Ok(None);
    }
    if !o.ok() {
        return Err(git_failure(&["config"], &o));
    }
    let url = o.stdout_str().trim().to_string();
    if url.is_empty() {
        return Ok(None);
    }
    if url.starts_with('-') || url.chars().any(char::is_control) {
        return Err(KbError::new(
            ErrorCode::ConfigInvalid,
            format!(
                "remote `{remote}` has an unsafe URL `{}`",
                git::redact(&url)
            ),
        ));
    }
    Ok(Some(resolve_local_url(kb_root, &url)))
}

/// Git treats a URL without `://` whose first `:` (if any) comes after a `/` as a local
/// path; relative ones are resolved against the KB root.
fn resolve_local_url(kb_root: &Path, url: &str) -> String {
    let scp_like = url.find(':').is_some_and(|c| !url[..c].contains('/'));
    if url.contains("://") || scp_like || Path::new(url).is_absolute() {
        return url.to_string();
    }
    let joined = kb_root.join(url);
    joined
        .canonicalize()
        .unwrap_or(joined)
        .to_string_lossy()
        .into_owned()
}

// ---------------------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------------------

enum Target {
    Commit(String),
    WorkingTree,
}

enum Mode {
    Latest,
    Pinned,
    Revision(String),
    WorkingTree,
}

fn select(
    req: &SnapshotRequest,
    host: Option<&HostContext>,
    pin: Option<&PinInfo>,
    tip: Option<&str>,
    kb: Option<&Git>,
    mirror: Option<&Mirror>,
) -> Result<(Selection, Target)> {
    let mode = match &req.selection {
        SelectionRequest::Auto => {
            match host
                .and_then(|h| h.binding.as_ref())
                .and_then(|b| b.selection)
            {
                Some(SkillSnapshot::Latest) => Mode::Latest,
                Some(SkillSnapshot::Pinned) => Mode::Pinned,
                Some(SkillSnapshot::Auto) | None => match (pin, tip) {
                    (Some(p), Some(t)) if p.revision != t => {
                        return Err(pin_differs(p, t));
                    }
                    _ => Mode::Latest,
                },
            }
        }
        SelectionRequest::Latest => Mode::Latest,
        SelectionRequest::Pinned => Mode::Pinned,
        SelectionRequest::WorkingTree => Mode::WorkingTree,
        SelectionRequest::Revision(r) => Mode::Revision(r.clone()),
    };
    match mode {
        Mode::Latest => match tip {
            Some(t) => Ok((Selection::Latest, Target::Commit(t.to_string()))),
            None => Err(KbError::new(
                ErrorCode::SnapshotNotFound,
                "no approved revision is known (offline, and nothing was fetched before)",
            )
            .with_hint(
                "run once without --offline (e.g. `kbw sync`), or select --snapshot pinned or working-tree",
            )),
        },
        Mode::Pinned => match pin {
            Some(p) => Ok((Selection::Pinned, Target::Commit(p.revision.clone()))),
            None => Err(KbError::new(
                ErrorCode::SnapshotNotFound,
                if host.is_some() {
                    "the host has no KB pin (no KB submodule gitlink in HEAD and no `.kbw.toml` pin)"
                } else {
                    "no host repository was detected, so there is no KB pin"
                },
            )
            .with_hint("run from the host repository or pass --host; or select --snapshot latest")),
        },
        Mode::Revision(r) => {
            let found = match kb.map(|k| k.resolve_commit(&r)).transpose()?.flatten() {
                Some(c) => Some(c),
                None => mirror.map(|m| m.commit(&r)).transpose()?.flatten(),
            };
            match found {
                Some(c) => Ok((Selection::Revision, Target::Commit(c))),
                None => Err(KbError::new(
                    ErrorCode::SnapshotNotFound,
                    format!("revision `{r}` does not resolve to a commit in the KB checkout or mirror"),
                )),
            }
        }
        Mode::WorkingTree => Ok((Selection::WorkingTree, Target::WorkingTree)),
    }
}

fn pin_differs(pin: &PinInfo, tip: &str) -> KbError {
    let short = |s: &str| s.chars().take(12).collect::<String>();
    KbError::new(
        ErrorCode::UpdateRequired,
        format!(
            "the host pins KB revision {} but the latest approved revision is {}",
            short(&pin.revision),
            short(tip)
        ),
    )
    .with_details(json!({ "pin": pin, "latest_approved": tip }))
    .with_hint(
        "select --snapshot pinned to use the pinned knowledge (both revisions are reported), \
         --snapshot latest to use the approved tip, or update the host pin through review",
    )
}

/// Expand an abbreviated pin to a full commit id when the mirror or checkout knows it.
fn resolve_pin(pin: &PinInfo, kb: Option<&Git>, mirror: Option<&Mirror>) -> Result<PinInfo> {
    if is_full_oid(&pin.revision) {
        return Ok(pin.clone());
    }
    let mut full = match mirror {
        Some(m) => m.commit(&pin.revision)?,
        None => None,
    };
    if full.is_none()
        && let Some(kb) = kb
    {
        full = kb.resolve_commit(&pin.revision)?;
    }
    Ok(PinInfo {
        revision: full.unwrap_or_else(|| pin.revision.clone()),
        ..pin.clone()
    })
}

/// Read `core/release.toml` of a snapshot and require this engine to serve it.
fn check_engine(source: &dyn SourceTree, label: &str) -> Result<()> {
    let incompatible = |why: String| {
        KbError::new(
            ErrorCode::UpdateRequired,
            format!("snapshot {label} cannot be interpreted by this engine: {why}"),
        )
        .with_details(json!({ "revision": label }))
        .with_hint("update the KB checkout to that revision through review and re-run kbw")
    };
    let bytes = source
        .read_path(MANIFEST_PATH)?
        .ok_or_else(|| incompatible(format!("it has no {MANIFEST_PATH}")))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| incompatible(format!("{MANIFEST_PATH} is not UTF-8")))?;
    let manifest = ReleaseManifest::parse(text).map_err(|e| incompatible(e.message))?;
    check_snapshot_compat(&manifest, label)
}

/// Content digest of the working-tree snapshot (profile content + engine manifest).
fn working_tree_digest(wt: &FrozenWorkingTree) -> String {
    let mut h = FieldHasher::new();
    h.field("kb-working-tree/1");
    for e in wt.entries() {
        h.field(&e.path).field(&e.content_id);
    }
    for i in wt.issues() {
        h.field("issue").field(&i.path).field(i.code);
    }
    h.finish_hex()
}

// ---------------------------------------------------------------------------------------
// sync
// ---------------------------------------------------------------------------------------

/// Result of `kb sync`: approved tip, local checkout relation and host pin status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncReport {
    pub profile: String,
    pub remote: String,
    /// Redacted remote URL.
    pub source: String,
    pub approved_ref: String,
    pub freshness: Freshness,
    /// Approved tip after this call (fetched, or last known when offline).
    pub latest_approved: Option<String>,
    /// Approved tip the mirror held before this call.
    pub previous_approved: Option<String>,
    pub local: LocalStatus,
    pub host: Option<HostPinStatus>,
    /// HEAD, branch, refs and `git status` of the KB checkout are identical before and
    /// after the call (sync writes only to the isolated mirror).
    pub checkout_unchanged: bool,
}

/// The local KB checkout relative to the approved tip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalStatus {
    /// False when the KB root is not its own Git checkout.
    pub git: bool,
    pub head: Option<String>,
    /// Current branch (`None` when detached).
    pub branch: Option<String>,
    /// Local commits not in the approved tip.
    pub ahead: Option<u64>,
    /// Approved commits not in the local `HEAD`.
    pub behind: Option<u64>,
    /// Uncommitted or untracked changes exist.
    pub dirty: bool,
    /// Number of `git status --porcelain` entries.
    pub changes: usize,
}

/// The host pin relative to the approved tip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostPinStatus {
    pub root: String,
    pub linked_worktree: bool,
    pub pin: Option<PinInfo>,
    pub status: PinStatus,
    /// Pinned commits not in the approved tip.
    pub ahead: Option<u64>,
    /// Approved commits not in the pin.
    pub behind: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PinStatus {
    /// The host has no KB pin.
    NoPin,
    /// The pin is the approved tip.
    Current,
    /// The pin is an older approved revision.
    Behind,
    /// The pin contains commits that are not approved.
    Ahead,
    /// Pin and approved tip have diverged.
    Diverged,
    /// The pin or the approved tip is not available locally.
    Unknown,
}

/// `kb sync`: fetch the approved ref into the mirror and report, without modifying the KB
/// checkout or the host.
pub fn sync(env: &Env, loc: &ProfileLocation, host: Option<&HostContext>) -> Result<SyncReport> {
    sync_with(env, loc, host, false)
}

/// [`sync`] with an explicit offline flag (offline: report the last known state only).
pub fn sync_with(
    env: &Env,
    loc: &ProfileLocation,
    host: Option<&HostContext>,
    offline: bool,
) -> Result<SyncReport> {
    let local_cfg = corpus::load_config(&WorkingTreeSource::new(&env.kb_root), loc)?;
    let kb = kb_git(env);
    let before = kb.as_ref().map(CheckoutState::capture).transpose()?;
    let src = ApprovedSource::configured(env, kb.as_ref(), &local_cfg)?;
    let mirror = src.mirror(env)?;
    let state = refresh(env, kb.as_ref(), &src, mirror.as_ref(), offline)?;
    let tip = state.tip.as_deref();
    let remote = (!offline).then(|| src.remote_fetch()).flatten();
    let local_root = kb.as_ref().map(|_| env.kb_root.as_path());

    let local = match (&kb, &before) {
        (Some(_), Some(st)) => {
            let (ahead, behind) = match (&st.head, tip, &mirror) {
                (Some(h), Some(t), Some(m))
                    if m.ensure_commit(h, LOCAL_HEAD_REF, local_root, None)? =>
                {
                    let (a, b) = ahead_behind(m.git(), h, t)?;
                    (Some(a), Some(b))
                }
                _ => (None, None),
            };
            let changes = count_status_entries(&st.status);
            LocalStatus {
                git: true,
                head: st.head.clone(),
                branch: st.branch.clone(),
                ahead,
                behind,
                dirty: changes > 0,
                changes,
            }
        }
        _ => LocalStatus {
            git: false,
            head: None,
            branch: None,
            ahead: None,
            behind: None,
            dirty: false,
            changes: 0,
        },
    };

    let host = match host {
        None => None,
        Some(h) => {
            let pin = h
                .pin
                .as_ref()
                .map(|p| resolve_pin(p, kb.as_ref(), mirror.as_ref()))
                .transpose()?;
            let (status, ahead, behind) = match (&pin, tip, &mirror) {
                (None, _, _) => (PinStatus::NoPin, None, None),
                (Some(p), Some(t), Some(m))
                    if m.ensure_commit(
                        &p.revision,
                        &format!("refs/kb/selected/{}", p.revision),
                        local_root,
                        remote.as_ref(),
                    )? =>
                {
                    let (a, b) = ahead_behind(m.git(), &p.revision, t)?;
                    let status = match (a, b) {
                        (0, 0) => PinStatus::Current,
                        (0, _) => PinStatus::Behind,
                        (_, 0) => PinStatus::Ahead,
                        _ => PinStatus::Diverged,
                    };
                    (status, Some(a), Some(b))
                }
                _ => (PinStatus::Unknown, None, None),
            };
            Some(HostPinStatus {
                root: h.root.display().to_string(),
                linked_worktree: h.linked_worktree,
                pin,
                status,
                ahead,
                behind,
            })
        }
    };

    let after = kb.as_ref().map(CheckoutState::capture).transpose()?;
    Ok(SyncReport {
        profile: loc.profile.as_str().to_string(),
        remote: src.remote.clone(),
        source: src.display(),
        approved_ref: src.approved_ref.clone(),
        freshness: state.freshness,
        latest_approved: state.tip.clone(),
        previous_approved: state.previous.clone(),
        local,
        host,
        checkout_unchanged: before == after,
    })
}

/// Observable state of the KB checkout (read with `GIT_OPTIONAL_LOCKS=0`).
#[derive(Debug, PartialEq, Eq)]
struct CheckoutState {
    head: Option<String>,
    branch: Option<String>,
    refs: Vec<u8>,
    status: Vec<u8>,
}

impl CheckoutState {
    fn capture(kb: &Git) -> Result<CheckoutState> {
        let branch = kb.output(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        Ok(CheckoutState {
            head: kb.resolve_commit("HEAD")?,
            branch: branch.ok().then(|| branch.stdout_str().trim().to_string()),
            refs: kb.run_bytes(&["for-each-ref", "--format=%(refname) %(objectname)"])?,
            status: kb.run_bytes(&[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=normal",
                "--ignore-submodules=none",
            ])?,
        })
    }
}

/// Count entries of `git status --porcelain=v1 -z` output (renames carry two paths).
fn count_status_entries(out: &[u8]) -> usize {
    let mut n = 0;
    let mut fields = out.split(|b| *b == 0).filter(|f| !f.is_empty());
    while let Some(entry) = fields.next() {
        n += 1;
        if matches!(entry.first(), Some(b'R' | b'C')) || matches!(entry.get(1), Some(b'R' | b'C')) {
            fields.next();
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_ref_must_be_refspec_safe() {
        assert!(check_approved_ref("refs/heads/main").is_ok());
        assert!(check_approved_ref("refs/heads/release/1.x").is_ok());
        for bad in [
            "main",
            "refs/heads/a:refs/heads/b",
            "refs/heads/*",
            "refs/heads/a b",
            "refs/heads/x.lock",
            "refs/heads/",
            "refs/heads/a..b",
        ] {
            assert!(check_approved_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn local_urls_resolve_against_kb_root() {
        let root = Path::new("/nonexistent/kb");
        assert_eq!(
            resolve_local_url(root, "../origin.git"),
            "/nonexistent/kb/../origin.git"
        );
        assert_eq!(resolve_local_url(root, "/srv/kb.git"), "/srv/kb.git");
        assert_eq!(
            resolve_local_url(root, "git@example.invalid:acme/kb.git"),
            "git@example.invalid:acme/kb.git"
        );
        assert_eq!(
            resolve_local_url(root, "https://example.invalid/kb.git"),
            "https://example.invalid/kb.git"
        );
        assert_eq!(resolve_local_url(root, "./a:b"), "/nonexistent/kb/./a:b");
    }

    #[test]
    fn status_entries_count_renames_once() {
        assert_eq!(count_status_entries(b""), 0);
        assert_eq!(count_status_entries(b" M a.md\0?? b.md\0"), 2);
        assert_eq!(count_status_entries(b"R  new.md\0old.md\0 D c.md\0"), 2);
    }

    #[test]
    fn selection_parse() {
        assert_eq!(
            SelectionRequest::parse(None).unwrap(),
            SelectionRequest::Auto
        );
        assert_eq!(
            SelectionRequest::parse(Some("abc123")).unwrap(),
            SelectionRequest::Revision("abc123".into())
        );
        assert!(SelectionRequest::parse(Some("--upload-pack=x")).is_err());
    }
}
