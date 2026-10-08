//! Host-level installation of the committed skill bundle (`kb integrate [--check|--apply]`).
//!
//! Targets per harness (see [`super::generate::layout`]): `.claude/skills/kb/` and a
//! `kb-instructions` block in `CLAUDE.md` for Claude Code; `.agents/skills/kb/` and a block in
//! `AGENTS.md` for Codex (and for Cursor when Claude Code is not enabled; Cursor also reads
//! `.claude/skills` and `AGENTS.md`).
//!
//! Installed hashes are recorded in `.kbw/integration.lock` in the host. A generated file or
//! block whose current bytes differ both from the lock and from the new content was edited
//! outside kb: it is a conflict and is never overwritten without `--force`. Nothing is written
//! when any conflict remains. Unmanaged bytes of instruction files are preserved exactly.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::blocks::{
    BLOCK_NAME, append_block, find_block, normalize_content, remove_block, replace_inner,
};
use super::generate::{
    AGENTS_SKILL_DIR, BundleManifest, CLAUDE_SKILL_DIR, GROK_SKILL_DIR, MANIFEST_FILE,
    PORTABLE_SKILL_DIR, SKILL_PREFIX, generated_dir, load_bundle, load_settings, render_bundle,
};
use super::{Action, Change, prune_empty_dirs};
use crate::diag::Diagnostic;
use crate::error::{ErrorCode, KbError, Result};
use crate::git::Git;
use crate::model::{Harness, Profile, ProfileLocation};
use crate::source::{SourceTree, WorkingTreeSource};
use crate::util::{atomic_write, read_file_limited, safe_join, sha256_hex};
use crate::versions::SKILL_PROTOCOL;

/// Host-relative lock file.
pub const LOCK_PATH: &str = ".kbw/integration.lock";
pub const LOCK_SCHEMA: u32 = 1;
/// Largest host file kb reads for integration.
const MAX_HOST_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// `.kbw/integration.lock`: what kb installed into the host, with content hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationLock {
    pub schema: u32,
    pub skill_protocol: u32,
    pub engine_version: String,
    pub harnesses: Vec<Harness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_receipt: Option<String>,
    #[serde(default, rename = "file")]
    pub files: Vec<LockedFile>,
    #[serde(default, rename = "block")]
    pub blocks: Vec<LockedBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedBlock {
    pub file: String,
    pub name: String,
    /// SHA-256 of the managed content between the marker lines.
    pub sha256: String,
}

impl IntegrationLock {
    pub fn to_toml(&self) -> Result<String> {
        let body = toml::to_string(self)
            .map_err(|e| KbError::internal(format!("cannot serialize the lock: {e}")))?;
        Ok(format!(
            "# Managed by `kbw integrate`; do not edit. Commit it so conflict detection works for everyone.\n{body}"
        ))
    }

    fn file_hash(&self, path: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.sha256.as_str())
    }

    fn block_hash(&self, file: &str, name: &str) -> Option<&str> {
        self.blocks
            .iter()
            .find(|b| b.file == file && b.name == name)
            .map(|b| b.sha256.as_str())
    }
}

/// Resolve the host repository root: `--host`, else the Git top-level of `cwd` when it is
/// not the KB checkout, else the superproject of the KB checkout (KB mounted as a submodule).
pub fn resolve_host_root(kb_root: &Path, cwd: &Path, host: Option<&Path>) -> Result<PathBuf> {
    if let Some(h) = host {
        let abs = if h.is_absolute() {
            h.to_path_buf()
        } else {
            cwd.join(h)
        };
        let c = abs
            .canonicalize()
            .map_err(|e| KbError::invalid_input(format!("--host `{}`: {e}", abs.display())))?;
        if !c.is_dir() {
            return Err(KbError::invalid_input(format!(
                "--host `{}` is not a directory",
                c.display()
            )));
        }
        return Ok(c);
    }
    let kb = kb_root
        .canonicalize()
        .unwrap_or_else(|_| kb_root.to_path_buf());
    if let Some(top) = crate::git::toplevel(cwd) {
        let top = top.canonicalize().unwrap_or(top);
        if top != kb {
            return Ok(top);
        }
    }
    let o = Git::new(&kb).output(&["rev-parse", "--show-superproject-working-tree"])?;
    let sup = o.stdout_str().trim().to_string();
    if o.ok() && !sup.is_empty() {
        let p = PathBuf::from(&sup);
        return Ok(p.canonicalize().unwrap_or(p));
    }
    Err(KbError::invalid_input(
        "cannot determine the host repository (the current directory is not in a Git work tree other than the KB, and the KB is not a submodule)",
    )
    .with_hint("run from inside the host repository or pass --host <dir>"))
}

/// Read a host file: `None` when absent; symlinks and non-regular files are refused.
fn read_host_file(host_root: &Path, rel: &str) -> Result<Option<Vec<u8>>> {
    let abs = safe_join(host_root, rel)?;
    match std::fs::symlink_metadata(&abs) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(KbError::io(abs.display(), e)),
        Ok(_) => read_file_limited(&abs, MAX_HOST_FILE_BYTES).map(Some),
    }
}

/// Read and strictly parse the host lock (`None` when the host has no lock yet).
pub fn read_lock(host_root: &Path) -> Result<Option<IntegrationLock>> {
    let Some(bytes) = read_host_file(host_root, LOCK_PATH)? else {
        return Ok(None);
    };
    parse_lock(&bytes).map(Some)
}

fn parse_lock(bytes: &[u8]) -> Result<IntegrationLock> {
    let invalid = |m: String| {
        KbError::new(ErrorCode::ConfigInvalid, format!("`{LOCK_PATH}`: {m}")).with_hint(
            "restore the lock from version control; without it kb cannot tell generated files from local edits",
        )
    };
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("not UTF-8".into()))?;
    let lock: IntegrationLock = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
    if lock.schema != LOCK_SCHEMA {
        return Err(invalid(format!(
            "lock schema {} is not supported (expected {LOCK_SCHEMA})",
            lock.schema
        )));
    }
    // The lock decides what kb may update or delete: it may only name kb-managed locations,
    // so a crafted lock cannot make kb remove arbitrary host files.
    for f in &lock.files {
        if !managed_file_path(&f.path) {
            return Err(invalid(format!(
                "`{}` is not a kb-managed skill file",
                f.path
            )));
        }
    }
    for b in &lock.blocks {
        if !MANAGED_BLOCK_FILES.contains(&b.file.as_str()) {
            return Err(invalid(format!(
                "`{}` is not a kb-managed instruction file",
                b.file
            )));
        }
    }
    Ok(lock)
}

/// Instruction files that may carry a kb block.
const MANAGED_BLOCK_FILES: [&str; 5] = [
    "AGENTS.md",
    "CLAUDE.md",
    "AGENTS.override.md",
    ".junie/AGENTS.md",
    ".github/copilot-instructions.md",
];

/// Is `path` inside one of the skill directories kb installs?
fn managed_file_path(path: &str) -> bool {
    crate::util::check_rel_path(path).is_ok()
        && (matches!(path, ".cursor/rules/kb.mdc" | ".claude/commands/kb.md")
            || [
                AGENTS_SKILL_DIR,
                CLAUDE_SKILL_DIR,
                GROK_SKILL_DIR,
                PORTABLE_SKILL_DIR,
            ]
            .iter()
            .any(|d| {
                path.strip_prefix(d)
                    .is_some_and(|r| r.len() > 1 && r.starts_with('/'))
            }))
}

/// Planned host installation.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub host_root: PathBuf,
    /// Host-relative changes, sorted by (path, block).
    pub changes: Vec<Change>,
    /// What happens to `.kbw/integration.lock`.
    pub lock_action: Action,
    /// Non-fatal findings (stale bundle, kb_path mismatch).
    pub warnings: Vec<Diagnostic>,
    /// Manifest of the bundle being installed.
    pub manifest: BundleManifest,
    writes: Vec<(String, Vec<u8>)>,
    removals: Vec<String>,
    lock_bytes: Vec<u8>,
}

impl InstallPlan {
    pub fn conflicts(&self) -> Vec<&Change> {
        self.changes
            .iter()
            .filter(|c| c.action == Action::Conflict)
            .collect()
    }

    /// True when the host already matches the bundle and the lock is current.
    pub fn is_clean(&self) -> bool {
        self.lock_action == Action::Unchanged
            && self.changes.iter().all(|c| c.action == Action::Unchanged)
    }
}

/// `CONFLICT` error for a plan with unresolved conflicts.
pub fn conflict_error(plan: &InstallPlan) -> KbError {
    let items: Vec<_> = plan
        .conflicts()
        .iter()
        .map(|c| json!({"path": c.path, "block": c.block, "reason": c.reason}))
        .collect();
    KbError::new(
        ErrorCode::Conflict,
        format!(
            "{} generated file(s) or block(s) were modified outside kb; nothing was written",
            items.len()
        ),
    )
    .with_details(json!({ "conflicts": items }))
    .with_hint("move local edits to project/skill-config/skill.toml `notes` in the KB, or re-run with --apply --force to overwrite them")
}

/// Decide what to do with one file or block given desired, current and locked content.
fn decide(
    desired: Option<&[u8]>,
    current: Option<&[u8]>,
    locked: Option<&str>,
    force: bool,
) -> Option<(Action, Option<&'static str>)> {
    let pristine = |c: &[u8]| locked == Some(sha256_hex(c).as_str());
    match (desired, current) {
        (Some(_), None) => Some((Action::Create, None)),
        (Some(d), Some(c)) if c == d => Some((Action::Unchanged, None)),
        (Some(_), Some(c)) if pristine(c) => Some((Action::Update, None)),
        (Some(_), Some(_)) if force => Some((
            Action::Update,
            Some("overwrites local modifications (--force)"),
        )),
        (Some(_), Some(_)) => Some((
            Action::Conflict,
            Some(if locked.is_some() {
                "modified since kb installed it"
            } else {
                "exists but was not installed by kb (no lock entry)"
            }),
        )),
        (None, Some(c)) if pristine(c) => {
            Some((Action::Remove, Some("no longer part of the integration")))
        }
        (None, Some(_)) if force => Some((
            Action::Remove,
            Some(
                "removes a locally modified file that is no longer part of the integration (--force)",
            ),
        )),
        (None, Some(_)) => Some((
            Action::Conflict,
            Some("modified since kb installed it and no longer part of the integration"),
        )),
        (None, None) => None,
    }
}

/// Plan installing the committed bundle of `loc` into `host_root`.
pub fn plan_install(
    kb_root: &Path,
    loc: &ProfileLocation,
    host_root: &Path,
    force: bool,
) -> Result<InstallPlan> {
    let bundle = load_bundle(kb_root, loc)?;
    let lock = read_lock(host_root)?;
    let layout = bundle.layout();
    let warnings = install_warnings(kb_root, loc, host_root, &bundle);

    // Desired skill files, one copy per skill directory.
    let mut desired: BTreeMap<String, &[u8]> = BTreeMap::new();
    for dir in &layout.skill_dirs {
        for (rel, bytes) in &bundle.files {
            if let Some(sub) = rel.strip_prefix(SKILL_PREFIX) {
                desired.insert(format!("{dir}/{sub}"), bytes);
            }
        }
    }
    for file in &layout.files {
        let bytes = bundle.files.get(file.bundle_file).ok_or_else(|| {
            KbError::invalid_input(format!("bundle is missing {}", file.bundle_file))
        })?;
        desired.insert(file.host_file.into(), bytes);
    }
    let mut paths: BTreeSet<String> = desired.keys().cloned().collect();
    if let Some(l) = &lock {
        paths.extend(l.files.iter().map(|f| f.path.clone()));
    }

    let mut changes = Vec::new();
    let mut writes = Vec::new();
    let mut removals = Vec::new();
    for path in &paths {
        let current = read_host_file(host_root, path)?;
        let want = desired.get(path).copied();
        let locked = lock.as_ref().and_then(|l| l.file_hash(path));
        let Some((action, reason)) = decide(want, current.as_deref(), locked, force) else {
            continue;
        };
        match (action, want) {
            (Action::Create | Action::Update, Some(w)) => writes.push((path.clone(), w.to_vec())),
            (Action::Remove, _) => removals.push(path.clone()),
            _ => {}
        }
        let mut c = Change::file(path.clone(), action);
        c.reason = reason.map(str::to_string);
        changes.push(c);
    }

    // Managed blocks.
    let mut block_targets: BTreeMap<String, Option<Vec<u8>>> = BTreeMap::new();
    for b in &layout.blocks {
        if b.only_if_exists && read_host_file(host_root, b.host_file)?.is_none() {
            continue;
        }
        let content = bundle.files.get(b.bundle_file).ok_or_else(|| {
            KbError::new(
                ErrorCode::DriftDetected,
                format!("the bundle has no `{}` for its harnesses", b.bundle_file),
            )
            .with_hint("regenerate the bundle with `./kbw integrate --generate --apply`")
        })?;
        block_targets.insert(b.host_file.to_string(), Some(normalize_content(content)));
    }
    if let Some(l) = &lock {
        for b in l.blocks.iter().filter(|b| b.name == BLOCK_NAME) {
            block_targets.entry(b.file.clone()).or_insert(None);
        }
    }
    let mut locked_blocks = Vec::new();
    for (file, want) in &block_targets {
        let current = read_host_file(host_root, file).map_err(|e| {
            if e.code == ErrorCode::UnsafePath {
                e.with_hint(
                    "kb never writes through symlinks; if CLAUDE.md links to AGENTS.md, enable only one of `claude` or `codex`/`cursor` in skill.toml, or replace the link with a file",
                )
            } else {
                e
            }
        })?;
        if want.is_some()
            && let Some(bytes) = &current
        {
            super::generate::check_size(file, bytes)?;
        }
        let span = match &current {
            Some(c) => find_block(c, BLOCK_NAME).map_err(|e| {
                KbError::invalid_input(format!("`{file}`: {e}")).with_hint(
                    "fix the `<!-- kb:begin/end ... -->` marker lines by hand; kb never guesses block boundaries",
                )
            })?,
            None => None,
        };
        let inner = match (&current, &span) {
            (Some(c), Some(s)) => Some(s.inner(c)),
            _ => None,
        };
        let locked = lock.as_ref().and_then(|l| l.block_hash(file, BLOCK_NAME));
        let Some((action, reason)) = decide(want.as_deref(), inner, locked, force) else {
            continue;
        };
        // `Create` appends a new block (creating the file when it is missing).
        let new_bytes = match (action, &current, &span, want) {
            (Action::Create, c, _, Some(w)) => Some(append_block(
                c.as_deref().unwrap_or_default(),
                BLOCK_NAME,
                w,
            )),
            (Action::Update, Some(c), Some(s), Some(w)) => Some(replace_inner(c, s, w)),
            (Action::Remove, Some(c), Some(s), _) => Some(remove_block(c, s)),
            _ => None,
        };
        if let Some(b) = new_bytes {
            super::generate::check_size(file, &b)?;
            writes.push((file.clone(), b));
        }
        if let Some(w) = want {
            locked_blocks.push(LockedBlock {
                file: file.clone(),
                name: BLOCK_NAME.to_string(),
                sha256: sha256_hex(w),
            });
        }
        changes.push(Change {
            path: file.clone(),
            block: Some(BLOCK_NAME.to_string()),
            action,
            reason: reason.map(str::to_string),
        });
    }
    changes.sort();

    let new_lock = IntegrationLock {
        schema: LOCK_SCHEMA,
        skill_protocol: bundle.manifest.skill_protocol,
        engine_version: bundle.manifest.engine_version.clone(),
        harnesses: bundle.manifest.harnesses.clone(),
        core_receipt: bundle.manifest.core.as_ref().map(|c| c.digest.clone()),
        files: desired
            .iter()
            .map(|(p, b)| LockedFile {
                path: p.clone(),
                sha256: sha256_hex(b),
            })
            .collect(),
        blocks: locked_blocks,
    };
    let lock_bytes = new_lock.to_toml()?.into_bytes();
    let lock_action = match read_host_file(host_root, LOCK_PATH)? {
        None => Action::Create,
        Some(b) if b == lock_bytes => Action::Unchanged,
        Some(_) => Action::Update,
    };
    Ok(InstallPlan {
        host_root: host_root.to_path_buf(),
        changes,
        lock_action,
        warnings,
        manifest: bundle.manifest,
        writes,
        removals,
        lock_bytes,
    })
}

/// Non-fatal checks: is the committed bundle what this engine would render now, and does
/// `<host>/<kb_path>` point at this KB checkout (the skill tells agents to run it)?
fn install_warnings(
    kb_root: &Path,
    loc: &ProfileLocation,
    host_root: &Path,
    bundle: &super::Bundle,
) -> Vec<Diagnostic> {
    let mut w = Vec::new();
    match load_settings(kb_root, loc).and_then(|(c, s)| render_bundle(kb_root, &c, &s, loc)) {
        Ok(fresh) if &fresh == bundle => {}
        Ok(_) => w.push(
            Diagnostic::warning(
                "BUNDLE_STALE",
                "the committed bundle differs from what `kbw integrate --generate` renders now; installing the committed (reviewed) bundle",
            )
            .at_path(generated_dir(loc)),
        ),
        Err(e) => w.push(
            Diagnostic::warning(
                "BUNDLE_UNVERIFIED",
                format!("cannot re-render the bundle to check it: {}", e.message),
            )
            .at_path(generated_dir(loc)),
        ),
    }
    let kb = kb_root.canonicalize().ok();
    let mounted = host_root.join(&bundle.manifest.kb_path).canonicalize().ok();
    if kb.is_none() || kb != mounted {
        w.push(Diagnostic::warning(
            "KB_PATH_MISMATCH",
            format!(
                "the skill tells agents to run `{}/kbw`, but that path in the host is not this KB checkout",
                bundle.manifest.kb_path
            ),
        ));
    }
    w
}

/// Apply a plan. Refuses (writing nothing) while conflicts remain.
pub fn apply_install(plan: &InstallPlan) -> Result<()> {
    if !plan.conflicts().is_empty() {
        return Err(conflict_error(plan));
    }
    let root = &plan.host_root;
    for (path, bytes) in &plan.writes {
        atomic_write(&safe_join(root, path)?, bytes)?;
    }
    for path in &plan.removals {
        let abs = safe_join(root, path)?;
        std::fs::remove_file(&abs).map_err(|e| KbError::io(abs.display(), e))?;
        prune_empty_dirs(root, &abs);
    }
    if plan.lock_action.writes() {
        atomic_write(&safe_join(root, LOCK_PATH)?, &plan.lock_bytes)?;
    }
    Ok(())
}

/// Installed skill state of a host, for `doctor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillState {
    /// No `.kbw/integration.lock` in the host.
    NotInstalled,
    /// Installed skill protocol equals the engine's.
    Current,
    /// Installed skill protocol differs: agents must re-read the skill / start a new session
    /// after the host is re-integrated.
    Outdated,
    /// The lock exists but cannot be parsed.
    LockInvalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillStatus {
    pub state: SkillState,
    /// Skill protocol served by this engine.
    pub engine_skill_protocol: u32,
    /// Skill protocol of the committed bundle in the KB (`None` when absent or unreadable).
    pub bundle_skill_protocol: Option<u32>,
    /// Skill protocol recorded in the host lock.
    pub installed_skill_protocol: Option<u32>,
    pub installed_engine_version: Option<String>,
    pub installed_harnesses: Vec<Harness>,
    /// Human-readable explanation and next step.
    pub message: String,
}

/// Report the installed skill protocol of `host_root` against the engine and the committed
/// bundle of the project profile in `kb_root`.
pub fn skill_status(kb_root: &Path, host_root: &Path) -> Result<SkillStatus> {
    let loc = ProfileLocation::for_profile(Profile::Project);
    let manifest_path = format!("{}/{MANIFEST_FILE}", generated_dir(&loc));
    let bundle_skill_protocol = WorkingTreeSource::new(kb_root)
        .read_path(&manifest_path)
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| toml::from_str::<BundleManifest>(&t).ok())
        .map(|m| m.skill_protocol);
    let mut status = SkillStatus {
        state: SkillState::NotInstalled,
        engine_skill_protocol: SKILL_PROTOCOL,
        bundle_skill_protocol,
        installed_skill_protocol: None,
        installed_engine_version: None,
        installed_harnesses: Vec::new(),
        message: String::new(),
    };
    let lock = match read_host_file(host_root, LOCK_PATH)? {
        None => {
            status.message =
                "no kb integration is installed in the host; run `kbw integrate --apply` there"
                    .into();
            return Ok(status);
        }
        Some(bytes) => match parse_lock(&bytes) {
            Ok(l) => l,
            Err(e) => {
                status.state = SkillState::LockInvalid;
                status.message = e.message;
                return Ok(status);
            }
        },
    };
    status.installed_skill_protocol = Some(lock.skill_protocol);
    status.installed_engine_version = Some(lock.engine_version.clone());
    status.installed_harnesses = lock.harnesses.clone();
    if lock.skill_protocol == SKILL_PROTOCOL {
        status.state = SkillState::Current;
        status.message = format!("installed skill protocol {SKILL_PROTOCOL} is current");
    } else {
        status.state = SkillState::Outdated;
        status.message = format!(
            "installed skill protocol {} differs from the engine's {SKILL_PROTOCOL}: regenerate the bundle, run `kbw integrate --apply` in the host, then re-read the skill or start a new agent session",
            lock.skill_protocol
        );
    }
    if bundle_skill_protocol.is_some_and(|p| p != SKILL_PROTOCOL) {
        status.message.push_str(
            "; the committed bundle is also outdated (`kbw integrate --generate --apply`)",
        );
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions() {
        let h = sha256_hex(b"old");
        let d = |w: Option<&[u8]>, c: Option<&[u8]>, l: Option<&str>, f| {
            decide(w, c, l, f).map(|(a, _)| a)
        };
        assert_eq!(d(Some(b"n"), None, None, false), Some(Action::Create));
        assert_eq!(
            d(Some(b"n"), Some(b"n"), None, false),
            Some(Action::Unchanged)
        );
        assert_eq!(
            d(Some(b"n"), Some(b"old"), Some(&h), false),
            Some(Action::Update)
        );
        assert_eq!(
            d(Some(b"n"), Some(b"edited"), Some(&h), false),
            Some(Action::Conflict)
        );
        assert_eq!(
            d(Some(b"n"), Some(b"edited"), Some(&h), true),
            Some(Action::Update)
        );
        assert_eq!(
            d(Some(b"n"), Some(b"theirs"), None, false),
            Some(Action::Conflict)
        );
        assert_eq!(d(None, Some(b"old"), Some(&h), false), Some(Action::Remove));
        assert_eq!(
            d(None, Some(b"edited"), Some(&h), false),
            Some(Action::Conflict)
        );
        assert_eq!(d(None, None, Some(&h), false), None);
    }

    #[test]
    fn lock_round_trip_is_strict() {
        let lock = IntegrationLock {
            schema: 1,
            skill_protocol: 1,
            engine_version: "0.1.0".into(),
            harnesses: vec![Harness::Claude],
            core_receipt: None,
            files: vec![LockedFile {
                path: ".claude/skills/kb/SKILL.md".into(),
                sha256: "00".into(),
            }],
            blocks: vec![LockedBlock {
                file: "CLAUDE.md".into(),
                name: BLOCK_NAME.into(),
                sha256: "11".into(),
            }],
        };
        let text = lock.to_toml().unwrap();
        assert_eq!(parse_lock(text.as_bytes()).unwrap(), lock);
        assert!(parse_lock(format!("{text}\nextra = 1\n").as_bytes()).is_err());

        // A lock may only name kb-managed locations (it drives updates and removals).
        let mut foreign = lock.clone();
        foreign.files[0].path = "src/main.rs".into();
        assert!(parse_lock(foreign.to_toml().unwrap().as_bytes()).is_err());
        foreign.files[0].path = ".claude/skills/kb".into();
        assert!(parse_lock(foreign.to_toml().unwrap().as_bytes()).is_err());
        foreign.files[0].path = ".claude/skills/kb/../../../x".into();
        assert!(parse_lock(foreign.to_toml().unwrap().as_bytes()).is_err());
        let mut foreign = lock.clone();
        foreign.blocks[0].file = "README.md".into();
        assert!(parse_lock(foreign.to_toml().unwrap().as_bytes()).is_err());
    }
}
