//! `kb integrate --generate`: render the canonical skill (`core/skills`) with the typed
//! project settings (`<profile>/skill-config/skill.toml`) and the profile config into the
//! committed bundle `<profile>/skill-config/generated/`.
//!
//! Bundle layout (paths relative to `generated/`):
//!
//! ```text
//! manifest.toml                   versions, harnesses, kb_path, sha256 of every other file
//! skills/kb/SKILL.md              one SKILL.md valid for Claude Code, Codex and Cursor
//! skills/kb/references/*.md       lazily read references
//! blocks/AGENTS.md.block          managed block for AGENTS.md (codex, cursor)
//! blocks/CLAUDE.md.block          managed block for CLAUDE.md (claude)
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::template::{Vars, render_dir, render_one, toml_string_array};
use super::{Action, Change, prune_empty_dirs};
use crate::corpus::load_config;
use crate::error::{ErrorCode, KbError, Result};
use crate::model::{Harness, ProfileConfig, ProfileLocation, SkillConfig, SkillSnapshot};
use crate::source::{SourceTree, WorkingTreeSource};
use crate::util::{atomic_write, safe_join, sha256_hex};
use crate::versions::{ENGINE_VERSION, SKILL_PROTOCOL};

/// Canonical skill sources (KB-root-relative).
pub const SKILL_SOURCE_DIR: &str = "core/skills/kb";
pub const AGENTS_BLOCK_SOURCE: &str = "core/skills/blocks/agents.md.tmpl";
pub const CLAUDE_BLOCK_SOURCE: &str = "core/skills/blocks/claude.md.tmpl";

/// Bundle-relative paths.
pub const MANIFEST_FILE: &str = "manifest.toml";
pub const SKILL_PREFIX: &str = "skills/kb/";
pub const SKILL_FILE: &str = "skills/kb/SKILL.md";
pub const AGENTS_BLOCK_FILE: &str = "blocks/AGENTS.md.block";
pub const CLAUDE_BLOCK_FILE: &str = "blocks/CLAUDE.md.block";

/// Host-relative install locations.
pub const CLAUDE_SKILL_DIR: &str = ".claude/skills/kb";
pub const AGENTS_SKILL_DIR: &str = ".agents/skills/kb";

pub const BUNDLE_SCHEMA: u32 = 1;
pub const SKILL_CONFIG_SCHEMA: u32 = 1;
/// Maximum size of `notes` in skill.toml.
pub const MAX_NOTES_BYTES: usize = 8 * 1024;

/// `<profile>/skill-config/skill.toml`.
pub fn skill_config_path(loc: &ProfileLocation) -> String {
    format!("{}/skill.toml", loc.skill_config_dir())
}

/// `<profile>/skill-config/generated`.
pub fn generated_dir(loc: &ProfileLocation) -> String {
    format!("{}/generated", loc.skill_config_dir())
}

/// Host-relative path of the KB checkout: relative, `/`-separated, and restricted to
/// characters that are safe to paste into shell commands (`[A-Za-z0-9._-]` segments).
pub fn check_kb_path(p: &str) -> std::result::Result<(), String> {
    crate::util::check_rel_path(p)?;
    if p.len() > 200 || p.ends_with('/') {
        return Err(format!(
            "kb_path `{p}` must be at most 200 bytes without a trailing `/`"
        ));
    }
    let ok = p.split('/').all(|seg| {
        !seg.starts_with('-')
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    });
    if !ok {
        return Err(format!(
            "kb_path `{p}` may only contain ASCII letters, digits, `.`, `_`, `-` and `/`, and no segment may start with `-`"
        ));
    }
    Ok(())
}

/// A project display name: 1..=100 characters, single line, no control characters.
pub fn check_display_name(name: &str) -> std::result::Result<(), String> {
    let n = name.chars().count();
    if name.trim().is_empty() || n > 100 || name.chars().any(char::is_control) {
        return Err("project name must be a single line of 1..=100 characters".into());
    }
    Ok(())
}

/// Parse and validate `skill.toml` strictly.
pub fn parse_skill_config(path: &str, bytes: &[u8]) -> Result<SkillConfig> {
    let invalid = |m: String| KbError::new(ErrorCode::ConfigInvalid, format!("`{path}`: {m}"));
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("not UTF-8".into()))?;
    let cfg: SkillConfig = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
    if cfg.schema != SKILL_CONFIG_SCHEMA {
        return Err(KbError::new(
            ErrorCode::UnsupportedSchemaVersion,
            format!(
                "`{path}` has schema {}; this engine supports {SKILL_CONFIG_SCHEMA}",
                cfg.schema
            ),
        ));
    }
    let mut problems = Vec::new();
    if cfg.harnesses.is_empty() {
        problems.push("harnesses must list at least one of claude, codex, cursor".to_string());
    }
    let mut sorted = cfg.harnesses.clone();
    sorted.sort();
    sorted.dedup();
    if sorted.len() != cfg.harnesses.len() {
        problems.push("harnesses must not repeat an entry".into());
    }
    if let Err(e) = check_kb_path(&cfg.kb_path) {
        problems.push(e);
    }
    if let Some(n) = &cfg.notes
        && n.len() > MAX_NOTES_BYTES
    {
        problems.push(format!("notes exceed {MAX_NOTES_BYTES} bytes"));
    }
    if !problems.is_empty() {
        return Err(invalid(problems.join("; ")));
    }
    Ok(cfg)
}

/// A managed block target in the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockTarget {
    /// Host-relative instruction file.
    pub host_file: &'static str,
    /// Bundle-relative block content file.
    pub bundle_file: &'static str,
    /// KB-root-relative block template.
    pub source: &'static str,
}

/// Where the skill and blocks go for a set of harnesses.
///
/// * Claude Code reads `.claude/skills/<name>/SKILL.md` and `CLAUDE.md`.
/// * Codex reads `.agents/skills/<name>/SKILL.md` and `AGENTS.md`.
/// * Cursor reads `.agents/skills`, `.cursor/skills` and, for compatibility, `.claude/skills`
///   and `.codex/skills`; it also reads `AGENTS.md`. With Claude Code enabled, Cursor
///   therefore uses `.claude/skills` and no `.agents/skills` copy is made for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// Host-relative skill directories, sorted.
    pub skill_dirs: Vec<&'static str>,
    /// Managed blocks, sorted by host file.
    pub blocks: Vec<BlockTarget>,
}

pub fn layout(harnesses: &[Harness]) -> Layout {
    let claude = harnesses.contains(&Harness::Claude);
    let codex = harnesses.contains(&Harness::Codex);
    let cursor = harnesses.contains(&Harness::Cursor);
    let mut skill_dirs = Vec::new();
    if codex || (cursor && !claude) {
        skill_dirs.push(AGENTS_SKILL_DIR);
    }
    if claude {
        skill_dirs.push(CLAUDE_SKILL_DIR);
    }
    let mut blocks = Vec::new();
    if codex || cursor {
        blocks.push(BlockTarget {
            host_file: "AGENTS.md",
            bundle_file: AGENTS_BLOCK_FILE,
            source: AGENTS_BLOCK_SOURCE,
        });
    }
    if claude {
        blocks.push(BlockTarget {
            host_file: "CLAUDE.md",
            bundle_file: CLAUDE_BLOCK_FILE,
            source: CLAUDE_BLOCK_SOURCE,
        });
    }
    Layout { skill_dirs, blocks }
}

/// `generated/manifest.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    pub schema: u32,
    pub skill_protocol: u32,
    pub engine_version: String,
    pub harnesses: Vec<Harness>,
    pub kb_path: String,
    #[serde(default, rename = "file")]
    pub files: Vec<BundleFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleFile {
    pub path: String,
    pub sha256: String,
}

impl BundleManifest {
    pub fn to_toml(&self) -> Result<String> {
        let body = toml::to_string(self)
            .map_err(|e| KbError::internal(format!("cannot serialize bundle manifest: {e}")))?;
        Ok(format!(
            "# Generated by `kbw integrate --generate`; do not edit. Reviewed and committed with the KB.\n{body}"
        ))
    }
}

/// A rendered skill bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    pub manifest: BundleManifest,
    /// Bundle-relative path → bytes (without `manifest.toml`).
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Bundle {
    /// All files including `manifest.toml`.
    pub fn all_files(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut all = self.files.clone();
        all.insert(
            MANIFEST_FILE.to_string(),
            self.manifest.to_toml()?.into_bytes(),
        );
        Ok(all)
    }

    pub fn layout(&self) -> Layout {
        layout(&self.manifest.harnesses)
    }
}

fn skill_vars(config: &ProfileConfig, skill: &SkillConfig, layout: &Layout) -> Vars {
    let snapshot_arg = match skill.snapshot {
        SkillSnapshot::Auto => "",
        SkillSnapshot::Latest => " --snapshot latest",
        SkillSnapshot::Pinned => " --snapshot pinned",
    };
    let agents_dir = if layout.skill_dirs.contains(&AGENTS_SKILL_DIR) {
        AGENTS_SKILL_DIR
    } else {
        CLAUDE_SKILL_DIR
    };
    let notes = skill
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("No project notes are configured (`notes` in project/skill-config/skill.toml).");
    let mut v = Vars::new();
    v.text("project_name", config.project.name.clone())
        .text("namespace", config.project.namespace.clone())
        .text("kb_path", skill.kb_path.clone())
        .text("skill_protocol", SKILL_PROTOCOL.to_string())
        .text("engine_version", ENGINE_VERSION)
        .text("default_intent", skill.default_intent.as_str())
        .text("notes", notes)
        .text("agents_skill_path", format!("{agents_dir}/SKILL.md"))
        .text("claude_skill_path", format!("{CLAUDE_SKILL_DIR}/SKILL.md"))
        .fragment("snapshot_arg", snapshot_arg);
    v
}

/// Render the bundle for a profile config and skill settings.
pub fn render_bundle(
    kb_root: &Path,
    config: &ProfileConfig,
    skill: &SkillConfig,
) -> Result<Bundle> {
    check_display_name(&config.project.name)
        .map_err(|e| KbError::new(ErrorCode::ConfigInvalid, format!("project.name: {e}")))?;
    let mut harnesses = skill.harnesses.clone();
    harnesses.sort();
    harnesses.dedup();
    let layout = layout(&harnesses);
    let vars = skill_vars(config, skill, &layout);
    let mut files = BTreeMap::new();
    for f in render_dir(kb_root, SKILL_SOURCE_DIR, &vars)? {
        files.insert(format!("{SKILL_PREFIX}{}", f.path), f.bytes);
    }
    if !files.contains_key(SKILL_FILE) {
        return Err(KbError::new(
            ErrorCode::NotFound,
            format!("`{SKILL_SOURCE_DIR}` has no SKILL.md template"),
        ));
    }
    for b in &layout.blocks {
        let r = render_one(kb_root, b.source, &vars)?;
        files.insert(
            b.bundle_file.to_string(),
            super::blocks::normalize_content(&r.bytes),
        );
    }
    let manifest = BundleManifest {
        schema: BUNDLE_SCHEMA,
        skill_protocol: SKILL_PROTOCOL,
        engine_version: ENGINE_VERSION.to_string(),
        harnesses,
        kb_path: skill.kb_path.clone(),
        files: files
            .iter()
            .map(|(p, b)| BundleFile {
                path: p.clone(),
                sha256: sha256_hex(b),
            })
            .collect(),
    };
    Ok(Bundle { manifest, files })
}

/// Is `path` a file kb may install from a bundle?
fn allowed_bundle_path(path: &str) -> bool {
    crate::util::check_rel_path(path).is_ok()
        && !path.ends_with('/')
        && (path.starts_with(SKILL_PREFIX)
            || path == AGENTS_BLOCK_FILE
            || path == CLAUDE_BLOCK_FILE)
}

/// Load the committed bundle of a profile and verify every file against its manifest.
pub fn load_bundle(kb_root: &Path, loc: &ProfileLocation) -> Result<Bundle> {
    let src = WorkingTreeSource::new(kb_root);
    let dir = generated_dir(loc);
    let manifest_path = format!("{dir}/{MANIFEST_FILE}");
    let bytes = src.read_path(&manifest_path)?.ok_or_else(|| {
        KbError::new(
            ErrorCode::NotFound,
            format!("no generated skill bundle: `{manifest_path}` does not exist"),
        )
        .with_hint(
            "in the KB checkout run `./kbw integrate --generate --apply` and commit the result",
        )
    })?;
    let stale = |m: String| {
        KbError::new(ErrorCode::DriftDetected, m).with_hint(
            "regenerate the bundle in the KB checkout with `./kbw integrate --generate --apply` and commit it",
        )
    };
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| stale(format!("`{manifest_path}` is not UTF-8")))?;
    let manifest: BundleManifest =
        toml::from_str(text).map_err(|e| stale(format!("`{manifest_path}`: {e}")))?;
    if manifest.schema != BUNDLE_SCHEMA {
        return Err(stale(format!(
            "`{manifest_path}` has bundle schema {}; this engine installs schema {BUNDLE_SCHEMA}",
            manifest.schema
        )));
    }
    if manifest.skill_protocol != SKILL_PROTOCOL {
        return Err(stale(format!(
            "the bundle was generated for skill protocol {}, this engine serves {SKILL_PROTOCOL}",
            manifest.skill_protocol
        )));
    }
    check_kb_path(&manifest.kb_path).map_err(|e| stale(format!("`{manifest_path}`: {e}")))?;
    let mut files = BTreeMap::new();
    for f in &manifest.files {
        if !allowed_bundle_path(&f.path) {
            return Err(KbError::unsafe_path(format!(
                "`{manifest_path}` lists an unexpected path `{}`",
                f.path
            )));
        }
        let p = format!("{dir}/{}", f.path);
        let b = src
            .read_path(&p)?
            .ok_or_else(|| stale(format!("bundle file `{p}` is missing")))?;
        if sha256_hex(&b) != f.sha256 {
            return Err(stale(format!(
                "bundle file `{p}` does not match its manifest (edited by hand?)"
            )));
        }
        if files.insert(f.path.clone(), b).is_some() {
            return Err(stale(format!("`{manifest_path}` lists `{}` twice", f.path)));
        }
    }
    if !files.contains_key(SKILL_FILE) {
        return Err(stale(format!("the bundle has no `{SKILL_FILE}`")));
    }
    Ok(Bundle { manifest, files })
}

/// Load `skill.toml` and the profile config of a KB working tree.
pub fn load_settings(
    kb_root: &Path,
    loc: &ProfileLocation,
) -> Result<(ProfileConfig, SkillConfig)> {
    let src = WorkingTreeSource::new(kb_root);
    let config = load_config(&src, loc)?;
    let path = skill_config_path(loc);
    let bytes = src.read_path(&path)?.ok_or_else(|| {
        KbError::new(ErrorCode::ConfigInvalid, format!("`{path}` does not exist")).with_hint(
            "`kbw init` creates it; see core/templates/project/skill-config/skill.toml.tmpl",
        )
    })?;
    Ok((config, parse_skill_config(&path, &bytes)?))
}

/// Planned regeneration of `<profile>/skill-config/generated/`.
#[derive(Debug, Clone)]
pub struct GeneratePlan {
    /// KB-root-relative changes, sorted by path.
    pub changes: Vec<Change>,
    pub bundle: Bundle,
    writes: Vec<(String, Vec<u8>)>,
    removals: Vec<String>,
}

impl GeneratePlan {
    /// True when the committed bundle already matches.
    pub fn is_clean(&self) -> bool {
        self.changes.iter().all(|c| c.action == Action::Unchanged)
    }
}

/// Compare the rendered bundle with `generated/` on disk.
pub fn plan_generate(kb_root: &Path, loc: &ProfileLocation) -> Result<GeneratePlan> {
    let (config, skill) = load_settings(kb_root, loc)?;
    let bundle = render_bundle(kb_root, &config, &skill)?;
    let dir = generated_dir(loc);
    let src = WorkingTreeSource::new(kb_root);
    let (existing, issues) = src.list(std::slice::from_ref(&dir))?;
    if let Some(i) = issues.first() {
        return Err(KbError::unsafe_path(format!("`{}`: {}", i.path, i.message)));
    }
    let mut on_disk: BTreeMap<String, String> = existing
        .into_iter()
        .map(|e| (e.path, e.content_id))
        .collect();
    let mut changes = Vec::new();
    let mut writes = Vec::new();
    for (rel, bytes) in bundle.all_files()? {
        let path = format!("{dir}/{rel}");
        let want = format!("sha256:{}", sha256_hex(&bytes));
        let action = match on_disk.remove(&path) {
            None => Action::Create,
            Some(have) if have == want => Action::Unchanged,
            Some(_) => Action::Update,
        };
        if action != Action::Unchanged {
            writes.push((path.clone(), bytes));
        }
        changes.push(Change::file(path, action));
    }
    let removals: Vec<String> = on_disk.into_keys().collect();
    for p in &removals {
        changes.push(
            Change::file(p.clone(), Action::Remove).with_reason("not part of the rendered bundle"),
        );
    }
    changes.sort();
    Ok(GeneratePlan {
        changes,
        bundle,
        writes,
        removals,
    })
}

/// Write the planned bundle (atomic per file) and remove stale bundle files.
pub fn apply_generate(kb_root: &Path, plan: &GeneratePlan) -> Result<()> {
    for (path, bytes) in &plan.writes {
        atomic_write(&safe_join(kb_root, path)?, bytes)?;
    }
    for path in &plan.removals {
        let abs = safe_join(kb_root, path)?;
        std::fs::remove_file(&abs).map_err(|e| KbError::io(abs.display(), e))?;
        prune_empty_dirs(kb_root, &abs);
    }
    Ok(())
}

/// A TOML array of harness names, e.g. `["claude", "codex"]`.
pub fn harness_array(harnesses: &[Harness]) -> String {
    let names: Vec<&str> = harnesses.iter().map(|h| h.as_str()).collect();
    toml_string_array(&names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_avoids_duplicate_skill_dirs() {
        use Harness::*;
        let l = layout(&[Claude, Cursor]);
        assert_eq!(l.skill_dirs, vec![CLAUDE_SKILL_DIR]);
        let files: Vec<_> = l.blocks.iter().map(|b| b.host_file).collect();
        assert_eq!(files, vec!["AGENTS.md", "CLAUDE.md"]);
        assert_eq!(layout(&[Cursor]).skill_dirs, vec![AGENTS_SKILL_DIR]);
        assert_eq!(layout(&[Codex]).skill_dirs, vec![AGENTS_SKILL_DIR]);
        assert_eq!(
            layout(&[Claude, Codex, Cursor]).skill_dirs,
            vec![AGENTS_SKILL_DIR, CLAUDE_SKILL_DIR]
        );
        let claude_only = layout(&[Claude]);
        assert_eq!(claude_only.blocks.len(), 1);
        assert_eq!(claude_only.blocks[0].host_file, "CLAUDE.md");
    }

    #[test]
    fn kb_path_rules() {
        for ok in [".kb", "tools/kb", "third_party/kb-2"] {
            assert!(check_kb_path(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", "/abs", "../kb", "a b", "kb;rm", ".kb/", "-kb", "a/$(x)", "a\\b",
        ] {
            assert!(check_kb_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn skill_config_is_strict() {
        let ok = b"schema = 1\nharnesses = [\"claude\"]\nkb_path = \".kb\"\n";
        let cfg = parse_skill_config("s.toml", ok).unwrap();
        assert_eq!(cfg.harnesses, vec![Harness::Claude]);
        for bad in [
            "schema = 1\nharnesses = []\nkb_path = \".kb\"\n",
            "schema = 1\nharnesses = [\"claude\", \"claude\"]\nkb_path = \".kb\"\n",
            "schema = 1\nharnesses = [\"vim\"]\nkb_path = \".kb\"\n",
            "schema = 1\nharnesses = [\"claude\"]\nkb_path = \"a b\"\n",
            "schema = 1\nharnesses = [\"claude\"]\nkb_path = \".kb\"\nsurprise = 1\n",
        ] {
            assert!(
                parse_skill_config("s.toml", bad.as_bytes()).is_err(),
                "{bad}"
            );
        }
        let e = parse_skill_config(
            "s.toml",
            b"schema = 2\nharnesses = [\"claude\"]\nkb_path = \".kb\"\n",
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::UnsupportedSchemaVersion);
    }
}
