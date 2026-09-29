//! `kb doctor`: environment, configuration, runtime, Git binding, index and integration
//! checks, in a fixed order.
//!
//! Every check reports `ok`, `warn`, `fail` or `skip` with a message. Doctor never modifies
//! the KB checkout or the host: it only reads them, plus the caches kb owns (the isolated Git
//! mirror and the index, which may be created or recovered like on any reading command).
//! Only `--online` contacts the remote.

use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

use crate::corpus::{load_config, load_corpus};
use crate::diag::Severity;
use crate::env::Env;
use crate::error::KbError;
use crate::git::Git;
use crate::host::{self, HostContext};
use crate::index::Index;
use crate::integrate::generate::{plan_generate, skill_config_path};
use crate::integrate::{Action, SkillState, skill_status};
use crate::knowledge::{Freshness, PinSource};
use crate::model::{
    Profile, ProfileConfig, ProfileLocation, Registry, SkillSnapshot, UPSTREAM_FILE, UpstreamConfig,
};
use crate::output::Format;
use crate::snapshot::{self, ApprovedSource, GitTreeSource, PinStatus, SyncReport};
use crate::source::{SourceTree, WorkingTreeSource};
use crate::util::read_file_limited;
use crate::validate::validate_corpus;
use crate::versions::{
    CompiledVersions, ENGINE_VERSION, MANIFEST_PATH, ReleaseManifest, check_snapshot_compat,
};

/// Outcome of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Skip,
}

impl CheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
            CheckStatus::Skip => "skip",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub status: CheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

impl Check {
    fn new(id: &'static str, status: CheckStatus, message: impl Into<String>) -> Check {
        Check {
            id,
            status,
            message: message.into(),
            hint: None,
            details: Value::Null,
        }
    }
    fn ok(id: &'static str, m: impl Into<String>) -> Check {
        Check::new(id, CheckStatus::Ok, m)
    }
    fn warn(id: &'static str, m: impl Into<String>) -> Check {
        Check::new(id, CheckStatus::Warn, m)
    }
    fn fail(id: &'static str, m: impl Into<String>) -> Check {
        Check::new(id, CheckStatus::Fail, m)
    }
    fn skip(id: &'static str, m: impl Into<String>) -> Check {
        Check::new(id, CheckStatus::Skip, m)
    }
    fn hint(mut self, h: impl Into<String>) -> Check {
        self.hint = Some(h.into());
        self
    }
    fn details(mut self, d: Value) -> Check {
        self.details = d;
        self
    }
    fn from_error(id: &'static str, status: CheckStatus, e: &KbError) -> Check {
        let mut c = Check::new(id, status, format!("{}: {}", e.code, e.message));
        c.hint = e.hint.clone();
        c
    }
}

/// All checks, in execution order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DoctorReport {
    pub profile: String,
    pub online: bool,
    pub summary: Summary,
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub ok: usize,
    pub warn: usize,
    pub fail: usize,
    pub skip: usize,
}

impl DoctorReport {
    /// Ids of failed checks.
    pub fn failed(&self) -> Vec<&'static str> {
        self.checks
            .iter()
            .filter(|c| c.status == CheckStatus::Fail)
            .map(|c| c.id)
            .collect()
    }
}

/// Inputs of a doctor run.
#[derive(Debug, Clone, Default)]
pub struct DoctorOptions<'a> {
    /// `--host`.
    pub host: Option<&'a Path>,
    /// `--online`: also fetch the approved ref.
    pub online: bool,
    /// `KBW_FINGERPRINT` as exported by the launcher.
    pub fingerprint: Option<String>,
}

/// Run every check. Never fails: problems become `fail` checks.
pub fn run(env: &Env, loc: &ProfileLocation, opts: &DoctorOptions<'_>) -> DoctorReport {
    let mut checks = vec![check_git(env), check_kb_root(env), check_runtime(env, opts)];

    let wt = WorkingTreeSource::new(&env.kb_root);
    let config = load_config(&wt, loc);
    checks.push(match &config {
        Ok(cfg) => Check::ok(
            "profile",
            format!(
                "profile `{}` ({}): namespace `{}`, approved ref {} on remote `{}`",
                loc.profile.as_str(),
                loc.config,
                cfg.project.namespace,
                cfg.source.approved_ref,
                cfg.source.remote
            ),
        ),
        Err(e) => Check::from_error("profile", CheckStatus::Fail, e),
    });

    let mut registry: Option<Registry> = None;
    checks.push(match &config {
        Err(_) => Check::skip(
            "knowledge",
            "not checked: the profile config is unavailable",
        ),
        Ok(_) => match load_corpus(&wt, loc) {
            Err(e) => Check::from_error("knowledge", CheckStatus::Fail, &e),
            Ok(corpus) => {
                registry = Some(corpus.registry.clone());
                let r = validate_corpus(&corpus);
                let shown: Vec<_> = r
                    .diagnostics
                    .iter()
                    .filter(|d| d.severity != Severity::Info)
                    .take(20)
                    .collect();
                let msg = format!(
                    "working tree: {} records in {} files, {} errors, {} warnings",
                    r.records, r.files, r.errors, r.warnings
                );
                let c = if r.errors > 0 {
                    Check::fail("knowledge", msg)
                } else if r.warnings > 0 {
                    Check::warn("knowledge", msg)
                } else {
                    Check::ok("knowledge", msg)
                };
                let c = c.details(json!({ "diagnostics": shown }));
                if r.errors > 0 || r.warnings > 0 {
                    c.hint("run `kbw validate` for the full report")
                } else {
                    c
                }
            }
        },
    });

    let host = host::detect(env, opts.host);
    let host_ctx = host.as_ref().ok().and_then(Option::as_ref);

    // Source and freshness: offline facts always, a fetch only with --online.
    let kb_git = snapshot::kb_git(env).is_some();
    let offline_sync = match (&config, kb_git) {
        (Ok(_), true) => Some(snapshot::sync_with(env, loc, host_ctx, true)),
        _ => None,
    };
    let online_sync = match (&config, kb_git, opts.online) {
        (Ok(_), true, true) => Some(snapshot::sync_with(env, loc, host_ctx, false)),
        _ => None,
    };
    // Source and pin relations use the freshest tip available: the fetched one when
    // --online succeeded, else the last known one.
    let freshest = match &online_sync {
        Some(Ok(_)) => online_sync.as_ref(),
        _ => offline_sync.as_ref(),
    };
    checks.push(check_source(config.is_ok(), kb_git, freshest));
    checks.push(check_freshness(
        opts.online,
        config.is_ok(),
        kb_git,
        online_sync.as_ref(),
    ));
    let sync = freshest.and_then(|r| r.as_ref().ok());

    checks.push(match &host {
        Err(e) => Check::from_error("host", CheckStatus::Fail, e),
        Ok(None) => Check::skip(
            "host",
            "no host repository detected (run from the host, or pass --host); the KB is used as a standalone checkout",
        ),
        Ok(Some(h)) => check_host(h),
    });
    checks.push(check_host_repo(host_ctx, registry.as_ref()));
    let compat = match (&config, kb_git, sync) {
        (Ok(cfg), true, Some(r)) => Some(snapshot_compat(env, cfg, r, host_ctx)),
        _ => None,
    };
    checks.push(check_host_pin(host_ctx, sync, compat.as_ref()));
    checks.push(check_snapshot_engine(
        config.is_ok(),
        kb_git,
        compat.as_ref(),
    ));
    checks.push(check_index(env, loc.profile));
    let (skill, bundle) = check_integration(env, loc, host_ctx);
    checks.push(skill);
    checks.push(bundle);
    checks.push(check_engine(env));

    let mut summary = Summary::default();
    for c in &checks {
        match c.status {
            CheckStatus::Ok => summary.ok += 1,
            CheckStatus::Warn => summary.warn += 1,
            CheckStatus::Fail => summary.fail += 1,
            CheckStatus::Skip => summary.skip += 1,
        }
    }
    DoctorReport {
        profile: loc.profile.as_str().to_string(),
        online: opts.online,
        summary,
        checks,
    }
}

fn check_git(env: &Env) -> Check {
    let out = match Git::new(&env.kb_root).output(&["--version"]) {
        Ok(o) if o.ok() => o.stdout_str().trim().to_string(),
        Ok(o) => {
            return Check::fail(
                "git",
                format!("`git --version` failed: {}", o.stderr.trim()),
            );
        }
        Err(e) => return Check::from_error("git", CheckStatus::Fail, &e),
    };
    let found = out.split_whitespace().nth(2).unwrap_or("");
    let min = &env.manifest.min_git;
    if parse_version(found) >= parse_version(min) {
        Check::ok("git", format!("{out} (>= {min})"))
    } else {
        Check::fail("git", format!("{out} is older than the required git {min}")).hint(
            "install a newer git; kb needs `merge-tree --write-tree` and modern fetch options",
        )
    }
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

fn check_kb_root(env: &Env) -> Check {
    let v = CompiledVersions::current();
    Check::ok(
        "kb-root",
        format!(
            "{} ({MANIFEST_PATH}: engine {}, document schema {}, protocol {}, index schema {}, skill protocol {})",
            env.kb_root.display(),
            env.manifest.engine_version,
            env.manifest.document_schema,
            env.manifest.protocol,
            env.manifest.index_schema,
            env.manifest.skill_protocol
        ),
    )
    .details(json!({
        "kb_root": env.kb_root,
        "cache_dir": env.cache_dir,
        "engine": v,
        "min_git": env.manifest.min_git,
    }))
}

/// The runtime binary: it already matches the manifest (checked before any command runs);
/// here we report how it was provided (`BUILD-INFO` written by kbw).
fn check_runtime(env: &Env, opts: &DoctorOptions<'_>) -> Check {
    let Some(fp) = opts.fingerprint.as_deref().filter(|f| !f.is_empty()) else {
        return Check::ok(
            "runtime",
            format!(
                "kb {ENGINE_VERSION} matches the engine manifest; not started through kbw \
                 (KBW_FINGERPRINT is unset), so the runtime source is unknown"
            ),
        )
        .hint(
            "run the project launcher (`kbw`) so the runtime is selected by the engine fingerprint",
        );
    };
    if fp.len() != 64 || !fp.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Check::warn(
            "runtime",
            "KBW_FINGERPRINT is not a 64-hex engine fingerprint",
        );
    }
    let rel = format!(".cache/runtime/{fp}/BUILD-INFO");
    let path = env.kb_root.join(&rel);
    let text = match read_file_limited(&path, 64 * 1024) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => {
            return Check::warn(
                "runtime",
                format!("runtime {} has no readable {rel}: {}", &fp[..12], e.message),
            );
        }
    };
    let get = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
            .map(str::trim)
    };
    let source = get("source").unwrap_or("unknown");
    let target = get("target").unwrap_or("unknown");
    let details = json!({
        "fingerprint": fp,
        "build_info": rel,
        "source": source,
        "target": target,
        "engine_version": get("engine_version"),
        "built_from": get("built_from"),
    });
    if get("fingerprint") != Some(fp) {
        return Check::warn(
            "runtime",
            format!("{rel} does not record fingerprint {}", &fp[..12]),
        )
        .details(details);
    }
    if get("engine_version") != Some(ENGINE_VERSION) {
        return Check::warn(
            "runtime",
            format!("{rel} records another engine version than {ENGINE_VERSION}"),
        )
        .details(details);
    }
    Check::ok(
        "runtime",
        format!(
            "runtime {} ({source}, {target}), engine {ENGINE_VERSION}",
            &fp[..12]
        ),
    )
    .details(details)
}

fn check_source(
    config_ok: bool,
    kb_git: bool,
    sync: Option<&crate::error::Result<SyncReport>>,
) -> Check {
    if !config_ok {
        return Check::skip("source", "not checked: the profile config is unavailable");
    }
    if !kb_git {
        return Check::fail(
            "source",
            "the KB root is not its own Git checkout, so the approved ref cannot be fetched or verified",
        )
        .hint("use a clone or submodule checkout of the KB; only `--offline --snapshot working-tree` works without one");
    }
    let r = match sync {
        Some(Ok(r)) => r,
        Some(Err(e)) => return Check::from_error("source", CheckStatus::Fail, e),
        None => return Check::skip("source", "not checked"),
    };
    if r.source.is_empty() {
        return Check::fail(
            "source",
            format!("remote `{}` is not configured in the KB checkout", r.remote),
        )
        .hint(format!(
            "add it (`git remote add {} <url>`); every reading command fetches {} from it",
            r.remote, r.approved_ref
        ));
    }
    let local = match (&r.local.head, r.local.ahead, r.local.behind) {
        (Some(h), Some(a), Some(b)) => {
            format!("; local HEAD {} is {a} ahead, {b} behind", short(h))
        }
        (Some(h), _, _) => format!("; local HEAD {}", short(h)),
        (None, _, _) => "; the KB checkout has no commits".into(),
    };
    let dirty = if r.local.dirty {
        format!(" ({} local changes)", r.local.changes)
    } else {
        String::new()
    };
    let details = json!({
        "remote": r.remote,
        "source": r.source,
        "approved_ref": r.approved_ref,
        "latest_approved": r.latest_approved,
        "local": r.local,
    });
    match &r.latest_approved {
        Some(tip) => Check::ok(
            "source",
            format!(
                "{} on `{}` ({}): {} {}{local}{dirty}",
                r.approved_ref,
                r.remote,
                r.source,
                tip_word(r),
                short(tip)
            ),
        )
        .details(details),
        None => Check::warn(
            "source",
            format!(
                "{} on `{}` ({}) has never been fetched and no remote-tracking ref is known{local}",
                r.approved_ref, r.remote, r.source
            ),
        )
        .hint("run `kbw sync` (or `kbw doctor --online`)")
        .details(details),
    }
}

fn check_freshness(
    online: bool,
    config_ok: bool,
    kb_git: bool,
    sync: Option<&crate::error::Result<SyncReport>>,
) -> Check {
    if !online {
        return Check::skip(
            "freshness",
            "not checked (pass --online to fetch the approved ref); reading commands check it on every call",
        );
    }
    if !config_ok || !kb_git {
        return Check::skip("freshness", "not checked: no usable approved source");
    }
    match sync {
        Some(Ok(r)) if r.freshness == Freshness::Verified => {
            let tip = r.latest_approved.as_deref().map(short).unwrap_or_default();
            let was = match (&r.previous_approved, &r.latest_approved) {
                (Some(p), Some(t)) if p == t => " (unchanged)".to_string(),
                (Some(p), Some(_)) => format!(" (was {})", short(p)),
                _ => " (first fetch)".to_string(),
            };
            Check::ok(
                "freshness",
                format!("verified: fetched {} → {tip}{was}", r.approved_ref),
            )
        }
        Some(Ok(_)) => Check::fail("freshness", "the approved ref was not verified"),
        Some(Err(e)) => Check::from_error("freshness", CheckStatus::Fail, e),
        None => Check::skip("freshness", "not checked"),
    }
}

fn check_host(h: &HostContext) -> Check {
    let wt = if h.linked_worktree {
        " (linked worktree)"
    } else {
        ""
    };
    let kb = match &h.kb_submodule_path {
        Some(p) => format!("; KB submodule at `{p}`"),
        None => "; the KB is not a submodule of this host".into(),
    };
    let binding = if h.binding.is_some() {
        "; `.kbw.toml` present"
    } else {
        ""
    };
    let head = h.head.as_deref().map(short).unwrap_or("no commits");
    Check::ok(
        "host",
        format!("{}{wt} at {head}{kb}{binding}", h.root.display()),
    )
    .details(json!({
        "root": h.root,
        "linked_worktree": h.linked_worktree,
        "head": h.head,
        "kb_submodule_path": h.kb_submodule_path,
        "binding": h.binding,
    }))
}

fn check_host_repo(host: Option<&HostContext>, registry: Option<&Registry>) -> Check {
    let Some(h) = host else {
        return Check::skip("host-repo", "no host repository");
    };
    let Some(reg) = registry else {
        return Check::skip("host-repo", "not checked: the registry is unavailable");
    };
    let unknown = host::unknown_binding_repo(h, reg);
    match (host::identify_repo(h, reg), unknown) {
        (Some((repo, source)), None) => {
            let how = serde_json::to_value(source)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            Check::ok("host-repo", format!("identified as registry repo `{repo}` ({how})"))
        }
        (found, Some(bad)) => {
            let fallback = match found {
                Some((repo, _)) => format!("; identified as `{repo}` by its remotes instead"),
                None => "; the repo scope of context requests is unknown".into(),
            };
            Check::warn(
                "host-repo",
                format!("`.kbw.toml` names repo `{bad}`, which is not in the registry{fallback}"),
            )
            .hint("fix `repo` in .kbw.toml or add the repo to registry/repos.toml")
        }
        (None, None) => Check::warn(
            "host-repo",
            "the host is not identified: no remote matches registry/repos.toml and `.kbw.toml` has no `repo`; context requests will have an unknown repo scope (partial)",
        )
        .hint("add the host remote to registry/repos.toml `remotes`, or `repo = \"<id>\"` to .kbw.toml"),
    }
}

/// "approved tip" when this run fetched it, else "last known approved tip".
fn tip_word(r: &SyncReport) -> &'static str {
    match r.freshness {
        Freshness::Verified => "approved tip",
        Freshness::Unverified => "last known approved tip",
    }
}

/// The `.kbw.toml selection` that `--snapshot auto` follows (`None` = no explicit choice).
fn binding_selection(h: &HostContext) -> Option<SkillSnapshot> {
    h.binding
        .as_ref()
        .and_then(|b| b.selection)
        .filter(|s| *s != SkillSnapshot::Auto)
}

fn check_host_pin(
    host: Option<&HostContext>,
    sync: Option<&SyncReport>,
    compat: Option<&SnapshotCompat>,
) -> Check {
    let Some(h) = host else {
        return Check::skip("host-pin", "no host repository");
    };
    let Some(pin) = &h.pin else {
        return Check::ok(
            "host-pin",
            "the host does not pin a KB revision; `--snapshot auto` uses the latest approved revision",
        );
    };
    let from = match (pin.source, &pin.path) {
        (PinSource::Submodule, Some(p)) => format!("submodule `{p}`"),
        (PinSource::Submodule, None) => "submodule".into(),
        (PinSource::BindingFile, _) => "`.kbw.toml` pin".into(),
    };
    let rev = short(&pin.revision);
    let (Some(r), Some(status)) = (sync, sync.and_then(|r| r.host.as_ref())) else {
        return Check::warn(
            "host-pin",
            format!("pin {rev} ({from}): relation to the approved tip is unknown"),
        )
        .hint("run `kbw sync`");
    };
    let tip = tip_word(r);
    let details = json!({ "pin": pin, "status": status.status, "ahead": status.ahead, "behind": status.behind });
    // What `--snapshot auto` does when the pin is not the approved tip (snapshot::select).
    let selection = binding_selection(h);
    let auto = match selection {
        Some(SkillSnapshot::Pinned) => {
            "; `.kbw.toml` selection = \"pinned\": `--snapshot auto` reads the pin".to_string()
        }
        Some(_) => {
            format!("; `.kbw.toml` selection = \"latest\": `--snapshot auto` reads the {tip}")
        }
        None => "; `--snapshot auto` returns UPDATE_REQUIRED".to_string(),
    };
    // Suggest `--snapshot latest` only when this engine can read the approved tip.
    let latest_readable = compat.is_none_or(|c| !c.tip_incompatible());
    let explicit = if latest_readable {
        "pass --snapshot pinned / --snapshot latest explicitly"
    } else {
        "pass --snapshot pinned explicitly (the approved tip needs another engine, see snapshot-engine)"
    };
    let pin_hint = match selection {
        None => format!(
            "update the host pin through review, {explicit}, or set `selection` in .kbw.toml"
        ),
        Some(_) => "update the host pin through review".to_string(),
    };
    match status.status {
        PinStatus::Current | PinStatus::NoPin => {
            Check::ok("host-pin", format!("pin {rev} ({from}) is the {tip}"))
        }
        PinStatus::Behind => Check::warn(
            "host-pin",
            format!(
                "pin {rev} ({from}) is {} commit(s) behind the {tip}{auto}",
                status.behind.unwrap_or(0)
            ),
        )
        .hint(pin_hint),
        PinStatus::Ahead => Check::warn(
            "host-pin",
            format!(
                "pin {rev} ({from}) has {} commit(s) that are not approved{auto}",
                status.ahead.unwrap_or(0)
            ),
        )
        .hint("merge the KB changes into the approved ref through review"),
        PinStatus::Diverged => Check::warn(
            "host-pin",
            format!(
                "pin {rev} ({from}) diverged from the {tip} ({} ahead, {} behind){auto}",
                status.ahead.unwrap_or(0),
                status.behind.unwrap_or(0)
            ),
        )
        .hint(pin_hint),
        PinStatus::Unknown => Check::warn(
            "host-pin",
            format!("pin {rev} ({from}): the pin or the approved tip is not available locally"),
        )
        .hint("run `kbw sync`"),
    }
    .details(details)
}

/// Engine compatibility of one snapshot revision.
#[derive(Debug)]
enum Compat {
    /// This engine serves the revision.
    Ok,
    /// Reading it fails with this `UPDATE_REQUIRED` error.
    Incompatible(KbError),
    /// Not checked (for example the commit is not available locally).
    Unknown(String),
}

/// Compatibility of the snapshots `--snapshot auto`, `latest` and `pinned` would read.
#[derive(Debug)]
struct SnapshotCompat {
    /// Revision `--snapshot auto` selects; `None` when auto fails with UPDATE_REQUIRED
    /// (a pin that differs from the approved tip without `.kbw.toml selection`) or when
    /// nothing is known.
    auto: Option<String>,
    /// `(role, revision, compatibility)` for the approved tip and a differing pin.
    revisions: Vec<(&'static str, String, Compat)>,
}

impl SnapshotCompat {
    fn tip_incompatible(&self) -> bool {
        self.revisions
            .iter()
            .any(|(role, _, c)| *role == "latest" && matches!(c, Compat::Incompatible(_)))
    }
}

/// Read `core/release.toml` of the approved tip and of the host pin from the isolated
/// mirror (never the KB checkout) and check them against this engine, as every reading
/// command does before it reads a snapshot.
fn snapshot_compat(
    env: &Env,
    cfg: &ProfileConfig,
    r: &SyncReport,
    host: Option<&HostContext>,
) -> SnapshotCompat {
    let tip = r.latest_approved.clone();
    let pin = r
        .host
        .as_ref()
        .and_then(|h| h.pin.as_ref())
        .map(|p| p.revision.clone());
    // `--snapshot auto`, as snapshot::select resolves it.
    let auto = match host.and_then(binding_selection) {
        Some(SkillSnapshot::Pinned) => pin.clone(),
        Some(_) => tip.clone(),
        None => match (&pin, &tip) {
            (Some(p), Some(t)) if p != t => None,
            _ => tip.clone(),
        },
    };
    let mut revisions = Vec::new();
    if let Some(t) = &tip {
        revisions.push(("latest", t.clone()));
    }
    if let Some(p) = pin.as_ref().filter(|p| Some(*p) != tip.as_ref()) {
        revisions.push(("pinned", p.clone()));
    }
    let kb = snapshot::kb_git(env);
    let mirror = ApprovedSource::configured(env, kb.as_ref(), cfg).and_then(|s| s.mirror(env));
    let revisions = revisions
        .into_iter()
        .map(|(role, rev)| {
            let c = match &mirror {
                Ok(Some(m)) => revision_compat(m.git(), &rev),
                Ok(None) => Compat::Unknown("the approved remote is not configured".into()),
                Err(e) => Compat::Unknown(e.message.clone()),
            };
            (role, rev, c)
        })
        .collect();
    SnapshotCompat { auto, revisions }
}

/// Can this engine read the snapshots the reading commands select? An incompatible
/// snapshot makes every read of it fail with UPDATE_REQUIRED: `fail` for the snapshot
/// `--snapshot auto` selects, `warn` for the other one.
fn check_snapshot_engine(config_ok: bool, kb_git: bool, compat: Option<&SnapshotCompat>) -> Check {
    const ID: &str = "snapshot-engine";
    if !config_ok || !kb_git {
        return Check::skip(ID, "not checked: no usable approved source");
    }
    let Some(c) = compat.filter(|c| !c.revisions.is_empty()) else {
        return Check::skip(
            ID,
            "not checked: no approved tip or host pin is known locally",
        )
        .hint("run `kbw sync` (or `kbw doctor --online`)");
    };
    let role_word = |role: &str| {
        if role == "latest" {
            "approved tip"
        } else {
            "host pin"
        }
    };
    let why = |e: &KbError| {
        let mm: Vec<String> = e.details["mismatches"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|m| {
                        format!(
                            "{} {} vs runtime {}",
                            m["field"].as_str().unwrap_or("?"),
                            m["snapshot"].as_str().unwrap_or("?"),
                            m["runtime"].as_str().unwrap_or("?")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if mm.is_empty() {
            e.message.clone()
        } else {
            mm.join(", ")
        }
    };
    let details = json!({
        "engine_version": ENGINE_VERSION,
        "auto": c.auto,
        "revisions": c.revisions.iter().map(|(role, rev, compat)| match compat {
            Compat::Ok => json!({ "role": role, "revision": rev, "status": "ok" }),
            Compat::Incompatible(e) => json!({
                "role": role, "revision": rev, "status": "incompatible",
                "message": e.message, "mismatches": e.details.get("mismatches"),
            }),
            Compat::Unknown(m) => json!({
                "role": role, "revision": rev, "status": "unknown", "message": m,
            }),
        }).collect::<Vec<_>>(),
    });
    let auto = c
        .revisions
        .iter()
        .find(|(_, rev, _)| c.auto.as_deref() == Some(rev.as_str()));
    let auto_text = match auto {
        Some((role, rev, _)) => format!(
            "`--snapshot auto` reads the {} {}",
            role_word(role),
            short(rev)
        ),
        None => "`--snapshot auto` returns UPDATE_REQUIRED (the pin differs from the approved tip)"
            .to_string(),
    };
    let update_hint = "update the KB checkout to that revision through review, then bootstrap its \
                       engine explicitly (`./kbw --kbw-bootstrap` or `./kbw --kbw-install-artifact`)";
    if let Some((role, rev, Compat::Incompatible(e))) = auto {
        return Check::fail(
            ID,
            format!(
                "`--snapshot auto` reads the {} {}, which needs another engine than kb {ENGINE_VERSION} ({}): \
                 every reading command returns UPDATE_REQUIRED",
                role_word(role),
                short(rev),
                why(e)
            ),
        )
        .hint(e.hint.clone().unwrap_or_else(|| update_hint.to_string()))
        .details(details);
    }
    let incompatible: Vec<String> = c
        .revisions
        .iter()
        .filter_map(|(role, rev, compat)| match compat {
            Compat::Incompatible(e) => Some(format!(
                "the {} {} needs another engine ({}), so `--snapshot {role}` returns UPDATE_REQUIRED",
                role_word(role),
                short(rev),
                why(e)
            )),
            _ => None,
        })
        .collect();
    if !incompatible.is_empty() {
        return Check::warn(ID, format!("{}; {auto_text}", incompatible.join("; ")))
            .hint(update_hint)
            .details(details);
    }
    let served: Vec<String> = c
        .revisions
        .iter()
        .filter(|(_, _, compat)| matches!(compat, Compat::Ok))
        .map(|(role, rev, _)| format!("the {} {}", role_word(role), short(rev)))
        .collect();
    let unchecked = c
        .revisions
        .iter()
        .find_map(|(role, rev, compat)| match compat {
            Compat::Unknown(m) => Some(format!(
                "the engine of the {} {} was not checked: {m}",
                role_word(role),
                short(rev)
            )),
            _ => None,
        });
    if let (Some(u), true) = (
        unchecked,
        served.is_empty() || matches!(auto, Some((_, _, Compat::Unknown(_)))),
    ) {
        return Check::warn(ID, u).hint("run `kbw sync`").details(details);
    }
    Check::ok(
        ID,
        format!(
            "kb {ENGINE_VERSION} serves {}; {auto_text}",
            served.join(" and ")
        ),
    )
    .details(details)
}

fn revision_compat(git: &Git, rev: &str) -> Compat {
    let incompatible = |why: String| {
        Compat::Incompatible(KbError::new(
            crate::error::ErrorCode::UpdateRequired,
            format!("snapshot {rev} cannot be interpreted by this engine: {why}"),
        ))
    };
    match git.resolve_commit(rev) {
        Ok(Some(_)) => {}
        Ok(None) => return Compat::Unknown(format!("{} is not available locally", short(rev))),
        Err(e) => return Compat::Unknown(e.message),
    }
    let bytes = match GitTreeSource::new(git.clone(), rev).and_then(|t| t.read_path(MANIFEST_PATH))
    {
        Ok(Some(b)) => b,
        Ok(None) => return incompatible(format!("it has no {MANIFEST_PATH}")),
        Err(e) => return Compat::Unknown(e.message),
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return incompatible(format!("{MANIFEST_PATH} is not UTF-8"));
    };
    match ReleaseManifest::parse(text).and_then(|m| check_snapshot_compat(&m, rev)) {
        Ok(()) => Compat::Ok,
        Err(e) if e.code == crate::error::ErrorCode::UpdateRequired => Compat::Incompatible(e),
        Err(e) => incompatible(e.message),
    }
}

fn check_index(env: &Env, profile: Profile) -> Check {
    let path = env
        .cache_dir
        .join("index")
        .join(format!("{}.sqlite", profile.as_str()));
    if !path.exists() {
        return Check::ok(
            "index",
            format!(
                "not built yet ({}); it is built on the first read",
                path.display()
            ),
        );
    }
    let index = match Index::open(&env.cache_dir, profile) {
        Ok(i) => i,
        Err(e) => return Check::from_error("index", CheckStatus::Fail, &e),
    };
    let stats = match index.stats() {
        Ok(s) => s,
        Err(e) => return Check::from_error("index", CheckStatus::Fail, &e),
    };
    drop(index);
    let integrity = quick_check(&path);
    let summary = format!(
        "{}: {} snapshot(s), {} document(s), {} bytes",
        path.display(),
        stats.snapshots.len(),
        stats.docs,
        stats.bytes
    );
    let details = json!({ "stats": stats, "quick_check": integrity });
    if let Some(r) = &stats.recovery {
        let d = r.diagnostic();
        return Check::warn("index", format!("{summary}; {}: {}", d.code, d.message))
            .details(details);
    }
    match integrity {
        Ok(()) => Check::ok("index", format!("{summary}; integrity ok")).details(details),
        Err(problem) => Check::fail(
            "index",
            format!("{summary}; integrity check failed: {problem}"),
        )
        .hint("the index is a disposable cache: run `kbw index --rebuild`")
        .details(details),
    }
}

/// `PRAGMA quick_check` on a separate read-only connection.
fn quick_check(path: &Path) -> std::result::Result<(), String> {
    use rusqlite::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let rows: Vec<String> = conn
        .prepare("PRAGMA quick_check")
        .and_then(|mut s| {
            s.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|e| e.to_string())?;
    if rows.len() == 1 && rows[0] == "ok" {
        Ok(())
    } else {
        Err(rows.join("; "))
    }
}

fn check_integration(
    env: &Env,
    loc: &ProfileLocation,
    host: Option<&HostContext>,
) -> (Check, Check) {
    if loc.profile != Profile::Project {
        let why = "harness integrations belong to the project profile";
        return (Check::skip("skill", why), Check::skip("bundle", why));
    }
    let cfg_path = skill_config_path(loc);
    let has_config = WorkingTreeSource::new(&env.kb_root)
        .read_path(&cfg_path)
        .ok()
        .flatten()
        .is_some();
    if !has_config {
        let why = format!("no `{cfg_path}`: integrations are not configured");
        return (
            Check::skip("skill", why.clone()),
            Check::skip("bundle", why),
        );
    }
    let skill = match host {
        None => Check::skip("skill", "no host repository"),
        Some(h) => match skill_status(&env.kb_root, &h.root) {
            Err(e) => Check::from_error("skill", CheckStatus::Fail, &e),
            Ok(st) => {
                let details = serde_json::to_value(&st).unwrap_or(Value::Null);
                match st.state {
                    SkillState::Current => Check::ok("skill", st.message),
                    SkillState::NotInstalled => Check::warn("skill", st.message),
                    SkillState::Outdated => Check::warn("skill", st.message).hint(
                        "after re-integrating, agents must re-read the kb skill or start a new session: \
                         a changed skill file does not update instructions already loaded",
                    ),
                    SkillState::LockInvalid => Check::fail(
                        "skill",
                        format!("the host integration lock is invalid: {}", st.message),
                    )
                    .hint("inspect .kbw/integration.lock in the host, then run `kbw integrate --apply`"),
                }
                .details(details)
            }
        },
    };
    let bundle = match plan_generate(&env.kb_root, loc) {
        Err(e) => Check::from_error("bundle", CheckStatus::Warn, &e),
        Ok(plan) if plan.is_clean() => Check::ok(
            "bundle",
            format!(
                "generated skill bundle is up to date (skill protocol {})",
                plan.bundle.manifest.skill_protocol
            ),
        ),
        Ok(plan) => {
            let changed: Vec<String> = plan
                .changes
                .iter()
                .filter(|c| c.action != Action::Unchanged)
                .map(|c| c.line())
                .collect();
            Check::warn(
                "bundle",
                format!(
                    "{} generated file(s) differ from the canonical skill sources",
                    changed.len()
                ),
            )
            .hint("run `kbw integrate --generate --apply` and commit the result")
            .details(json!({ "changes": changed }))
        }
    };
    (skill, bundle)
}

fn check_engine(env: &Env) -> Check {
    let bytes = match WorkingTreeSource::new(&env.kb_root).read_path(UPSTREAM_FILE) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return Check::skip(
                "engine",
                format!("no `{UPSTREAM_FILE}`: no upstream base is recorded"),
            );
        }
        Err(e) => return Check::from_error("engine", CheckStatus::Warn, &e),
    };
    let cfg: UpstreamConfig = match std::str::from_utf8(&bytes)
        .map_err(|e| e.to_string())
        .and_then(|t| toml::from_str(t).map_err(|e| e.to_string()))
    {
        Ok(c) => c,
        Err(e) => return Check::warn("engine", format!("`{UPSTREAM_FILE}` is invalid: {e}")),
    };
    if cfg.revision.is_none() {
        return Check::skip(
            "engine",
            format!("`{UPSTREAM_FILE}` records no upstream revision"),
        );
    }
    match crate::update::divergence(env) {
        Err(e) => Check::from_error("engine", CheckStatus::Warn, &e),
        Ok(r) => {
            let details = json!({
                "revision": r.revision,
                "diverged": r.diverged,
                "patched": r.patched.len(),
                "unused_patches": r.unused_patches,
            });
            match r.failure() {
                Some(f) => Check::warn("engine", f.message)
                    .hint("engine changes belong upstream; declare intended local patches in project/upstream.toml `engine_patches` (see `kbw update divergence`)")
                    .details(details),
                None => Check::ok(
                    "engine",
                    format!(
                        "engine-owned paths match upstream {} ({} declared patch(es))",
                        short(&r.revision),
                        r.patched.len()
                    ),
                )
                .details(details),
            }
        }
    }
}

fn short(rev: &str) -> &str {
    rev.get(..12).unwrap_or(rev)
}

/// Text rendering (compact and human share one layout; human adds hints).
pub fn render(report: &DoctorReport, format: Format) -> String {
    let s = &report.summary;
    let mut out = format!(
        "kb doctor ({} profile{}): {} ok, {} warn, {} fail, {} skip\n",
        report.profile,
        if report.online { ", online" } else { "" },
        s.ok,
        s.warn,
        s.fail,
        s.skip
    );
    for c in &report.checks {
        out.push_str(&format!(
            "{:<4} {}: {}\n",
            c.status.as_str(),
            c.id,
            c.message
        ));
        if let Some(h) = &c.hint
            && (format == Format::Human
                || matches!(c.status, CheckStatus::Fail | CheckStatus::Warn))
        {
            out.push_str(&format!("     hint: {h}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_versions_compare_numerically() {
        assert!(parse_version("2.39.3") >= parse_version("2.38.0"));
        assert!(parse_version("2.9.0") < parse_version("2.38.0"));
        assert!(parse_version("2.45.1.windows.1") >= parse_version("2.38.0"));
        assert_eq!(parse_version("garbage"), (0, 0, 0));
    }
}
