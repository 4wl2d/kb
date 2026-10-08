//! `kb init`: create the downstream project structure (`project/`) from `core/templates`.
//!
//! The plan is computed without side effects; `apply` writes it. Existing files are never
//! overwritten, except `project/README.md` (the upstream placeholder), which is replaced by
//! the project README. An existing `project/project.toml` means the project is already
//! initialized. The plan also renders the skill bundle (`project/skill-config/generated/`)
//! so that `kbw integrate --generate --check` passes right after init, and installs the
//! downstream GitHub/GitLab KB CI and review templates: the workflow
//! `.github/workflows/kb-knowledge.yml` and the files in `KB_REVIEW_FILES`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::json;

use crate::corpus::parse_config;
use crate::error::{ErrorCode, KbError, Result};
use crate::git::Git;
use crate::integrate::generate::{
    check_display_name, check_kb_path, generated_dir, harness_array, parse_skill_config,
    render_bundle, skill_config_path,
};
use crate::integrate::template::{RenderedFile, Vars, render_dir, render_one, toml_string};
use crate::integrate::{Action, Change};
use crate::model::ids::check_namespace;
use crate::model::{Harness, Profile, ProfileLocation, UPSTREAM_FILE, UpstreamConfig};
use crate::util::{atomic_write, read_file_limited, safe_join};

/// Project skeleton templates (rendered into `project/`).
pub const PROJECT_TEMPLATE_DIR: &str = "core/templates/project";
/// Synthetic examples: `core/templates/examples/<name>/project/`.
pub const EXAMPLES_DIR: &str = "core/templates/examples";
pub const UPSTREAM_TEMPLATE: &str = "core/templates/project/upstream.toml.tmpl";
/// Downstream KB CI workflow template and its install location.
pub const KB_CI_TEMPLATE: &str = "core/templates/ci/github/kb-knowledge.yml";
pub const KB_CI_TARGET: &str = ".github/workflows/kb-knowledge.yml";
/// Downstream-owned copies; upstream updates never replace them automatically.
pub const KB_REVIEW_FILES: [(&str, &str); 3] = [
    (
        "core/templates/ci/gitlab/kb-knowledge.gitlab-ci.yml",
        ".gitlab/ci/kb-knowledge.yml",
    ),
    (
        "core/templates/mr/kb_review.md",
        ".github/pull_request_template.md",
    ),
    (
        "core/templates/mr/kb_review.md",
        ".gitlab/merge_request_templates/Knowledge.md",
    ),
];
/// The only pre-existing file init replaces.
pub const PROJECT_README: &str = "project/README.md";
/// Namespace of every shipped synthetic example.
pub const EXAMPLE_NAMESPACE: &str = "example";

pub const DEFAULT_HARNESSES: [Harness; 3] = [Harness::Claude, Harness::Codex, Harness::Cursor];

/// Parameters of `kb init`. Organization names and URLs are always explicit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOptions {
    pub name: Option<String>,
    pub namespace: Option<String>,
    pub remote: String,
    pub approved_ref: String,
    pub kb_path: String,
    /// Empty means the default `[claude, codex, cursor]`.
    pub harnesses: Vec<Harness>,
    pub upstream_url: Option<String>,
    pub example: Option<String>,
}

impl Default for InitOptions {
    fn default() -> Self {
        InitOptions {
            name: None,
            namespace: None,
            remote: "origin".into(),
            approved_ref: "refs/heads/main".into(),
            kb_path: ".kb".into(),
            harnesses: Vec::new(),
            upstream_url: None,
            example: None,
        }
    }
}

/// A computed init plan. Serializes to the deterministic `init` result.
#[derive(Debug, Clone, Serialize)]
pub struct InitPlan {
    pub example: Option<String>,
    pub name: String,
    pub namespace: String,
    pub remote: String,
    pub approved_ref: String,
    pub kb_path: String,
    pub harnesses: Vec<Harness>,
    pub upstream_url: Option<String>,
    /// KB `HEAD` recorded as the upstream base (`None` when not resolvable).
    pub upstream_revision: Option<String>,
    /// KB-root-relative changes, sorted by path.
    pub changes: Vec<Change>,
    #[serde(skip)]
    files: BTreeMap<String, Vec<u8>>,
}

impl InitPlan {
    /// Number of files `apply` writes.
    pub fn pending_writes(&self) -> usize {
        self.changes.iter().filter(|c| c.action.writes()).count()
    }
}

fn usage(msg: impl Into<String>) -> KbError {
    KbError::new(ErrorCode::Usage, msg)
}

fn check_remote(r: &str) -> std::result::Result<(), String> {
    let ok = !r.is_empty()
        && r.len() <= 100
        && r.starts_with(|c: char| c.is_ascii_alphanumeric())
        && r.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "remote `{r}` must be a Git remote name (`[A-Za-z0-9][A-Za-z0-9._-]*`)"
        ))
    }
}

/// A fully qualified ref following Git's ref-name rules (without running Git).
pub fn check_approved_ref(r: &str) -> std::result::Result<(), String> {
    let bad_char = r
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c));
    let bad = !r.starts_with("refs/")
        || r.len() > 200
        || bad_char
        || r.contains("..")
        || r.contains("//")
        || r.contains("@{")
        || r.ends_with('/')
        || r.ends_with('.')
        || r.ends_with(".lock")
        || r.split('/').any(|s| s.is_empty() || s.starts_with('.'));
    if bad {
        return Err(format!(
            "approved ref `{r}` must be a valid fully qualified ref such as `refs/heads/main`"
        ));
    }
    Ok(())
}

fn check_upstream_url(u: &str) -> std::result::Result<(), String> {
    if u.is_empty() || u.len() > 500 || u.starts_with('-') {
        return Err("upstream URL must be 1..=500 bytes and must not start with `-`".into());
    }
    if u.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("upstream URL must not contain whitespace or control characters".into());
    }
    if crate::git::redact(u) != u {
        return Err(
            "upstream URL must not embed credentials; configure a credential helper instead".into(),
        );
    }
    Ok(())
}

fn check_example_name(n: &str) -> std::result::Result<(), String> {
    if n.is_empty()
        || n.len() > 64
        || !n
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!("example name `{n}` is invalid"));
    }
    Ok(())
}

/// Available example names (sorted).
pub fn list_examples(kb_root: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(kb_root.join(EXAMPLES_DIR))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().join("project").is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| check_example_name(n).is_ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// The KB checkout's `HEAD` commit when the KB root is itself a Git work tree root.
fn kb_head(kb_root: &Path) -> Option<String> {
    let top = crate::git::toplevel(kb_root)?.canonicalize().ok()?;
    if top != kb_root.canonicalize().ok()? {
        return None;
    }
    Git::new(kb_root).resolve_commit("HEAD").ok().flatten()
}

fn upstream_vars(vars: &mut Vars, url: Option<&str>, revision: Option<&str>) {
    let url_line = match url {
        Some(u) => format!("url = {}", toml_string(u)),
        None => "# url = \"<upstream repository URL>\"   # not set: pass --upstream-url to `kbw init` or edit this line".into(),
    };
    let rev_line = match revision {
        Some(r) => format!("revision = {}", toml_string(r)),
        None => {
            "# revision = \"<upstream commit>\"   # the KB had no resolvable Git HEAD at init time"
                .into()
        }
    };
    vars.fragment("upstream_url_line", url_line)
        .fragment("upstream_revision_line", rev_line);
}

/// Set a string value in a `toml_edit` document, keeping the key's comments.
fn set_string(doc: &mut toml_edit::DocumentMut, path: &[&str], value: &str) {
    let mut item = doc.as_item_mut();
    for k in path {
        item = &mut item[*k];
    }
    let decor = item.as_value().map(|v| v.decor().clone());
    *item = toml_edit::value(value);
    if let (Some(d), Some(v)) = (decor, item.as_value_mut()) {
        *v.decor_mut() = d;
    }
}

fn edit_toml(
    path: &str,
    bytes: &[u8],
    edit: impl FnOnce(&mut toml_edit::DocumentMut),
) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| KbError::invalid_input(format!("`{path}` is not UTF-8")))?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| KbError::invalid_input(format!("`{path}`: {e}")))?;
    edit(&mut doc);
    Ok(doc.to_string().into_bytes())
}

fn project_files(prefix: &str, rendered: Vec<RenderedFile>) -> BTreeMap<String, Vec<u8>> {
    rendered
        .into_iter()
        .map(|f| (format!("{prefix}/{}", f.path), f.bytes))
        .collect()
}

/// Compute the init plan. Reads the KB checkout; writes nothing.
pub fn plan(kb_root: &Path, opts: &InitOptions) -> Result<InitPlan> {
    let loc = ProfileLocation::for_profile(Profile::Project);
    let config_abs = safe_join(kb_root, &loc.config)?;
    if std::fs::symlink_metadata(&config_abs).is_ok() {
        return Err(KbError::new(
            ErrorCode::AlreadyInitialized,
            format!(
                "the project is already initialized: `{}` exists",
                loc.config
            ),
        )
        .with_details(json!({"path": loc.config}))
        .with_hint("edit project/ directly; `kbw validate` checks it"));
    }
    check_remote(&opts.remote).map_err(KbError::invalid_input)?;
    check_approved_ref(&opts.approved_ref).map_err(KbError::invalid_input)?;
    check_kb_path(&opts.kb_path).map_err(KbError::invalid_input)?;
    if let Some(u) = &opts.upstream_url {
        check_upstream_url(u).map_err(KbError::invalid_input)?;
    }
    if let Some(n) = &opts.name {
        check_display_name(n).map_err(KbError::invalid_input)?;
    }
    let mut harnesses = if opts.harnesses.is_empty() {
        DEFAULT_HARNESSES.to_vec()
    } else {
        opts.harnesses.clone()
    };
    harnesses.sort();
    harnesses.dedup();
    let revision = kb_head(kb_root);

    let mut vars = Vars::new();
    vars.text("remote", opts.remote.clone())
        .text("approved_ref", opts.approved_ref.clone())
        .text("kb_path", opts.kb_path.clone())
        .fragment("harnesses", harness_array(&harnesses));
    upstream_vars(&mut vars, opts.upstream_url.as_deref(), revision.as_deref());

    let config_path = loc.config.clone();
    let skill_path = skill_config_path(&loc);
    let mut files = match &opts.example {
        None => {
            let namespace = opts
                .namespace
                .as_deref()
                .ok_or_else(|| usage("`kb init` needs --namespace <ns> (and --name <name>)"))?;
            let name = opts
                .name
                .as_deref()
                .ok_or_else(|| usage("`kb init` needs --name <name> (and --namespace <ns>)"))?;
            check_namespace(namespace).map_err(KbError::invalid_input)?;
            vars.text("name", name).text("namespace", namespace);
            project_files(&loc.dir, render_dir(kb_root, PROJECT_TEMPLATE_DIR, &vars)?)
        }
        Some(example) => {
            if opts.namespace.is_some() {
                return Err(usage(format!(
                    "--namespace cannot be combined with --example (the example's namespace is fixed to `{EXAMPLE_NAMESPACE}`)"
                )));
            }
            check_example_name(example).map_err(KbError::invalid_input)?;
            let dir = format!("{EXAMPLES_DIR}/{example}/project");
            if !safe_join(kb_root, &dir)?.is_dir() {
                return Err(KbError::new(
                    ErrorCode::NotFound,
                    format!("unknown example `{example}`"),
                )
                .with_details(json!({"available": list_examples(kb_root)})));
            }
            vars.text("namespace", EXAMPLE_NAMESPACE);
            let mut files = project_files(&loc.dir, render_dir(kb_root, &dir, &vars)?);
            let cfg = files.get(&config_path).ok_or_else(|| {
                KbError::new(
                    ErrorCode::NotFound,
                    format!("example `{example}` has no project.toml"),
                )
            })?;
            let patched = edit_toml(&config_path, cfg, |doc| {
                set_string(doc, &["source", "remote"], &opts.remote);
                set_string(doc, &["source", "approved_ref"], &opts.approved_ref);
                if let Some(n) = &opts.name {
                    set_string(doc, &["project", "name"], n);
                }
            })?;
            files.insert(config_path.clone(), patched);
            if let Some(skill) = files.get(&skill_path) {
                let explicit = !opts.harnesses.is_empty();
                let patched = edit_toml(&skill_path, skill, |doc| {
                    set_string(doc, &["kb_path"], &opts.kb_path);
                    if explicit {
                        let arr: toml_edit::Array = harnesses.iter().map(|h| h.as_str()).collect();
                        doc["harnesses"] = toml_edit::value(arr);
                    }
                })?;
                files.insert(skill_path.clone(), patched);
            }
            let up = render_one(kb_root, UPSTREAM_TEMPLATE, &vars)?;
            files.insert(UPSTREAM_FILE.to_string(), up.bytes);
            files
        }
    };
    let ci = render_one(kb_root, KB_CI_TEMPLATE, &vars)?;
    files.insert(KB_CI_TARGET.to_string(), ci.bytes);
    for (template, target) in KB_REVIEW_FILES {
        files.insert(target.into(), render_one(kb_root, template, &vars)?.bytes);
    }

    // Validate what will be written with the same strict parsers the engine uses later.
    let cfg_bytes = files
        .get(&config_path)
        .ok_or_else(|| KbError::internal(format!("templates did not produce `{config_path}`")))?;
    let config = parse_config(&config_path, cfg_bytes)?;
    if opts.example.is_some() && config.project.namespace != EXAMPLE_NAMESPACE {
        return Err(KbError::invalid_input(format!(
            "example namespace must be `{EXAMPLE_NAMESPACE}`"
        )));
    }
    let up_bytes = files
        .get(UPSTREAM_FILE)
        .ok_or_else(|| KbError::internal(format!("templates did not produce `{UPSTREAM_FILE}`")))?;
    let up_text = std::str::from_utf8(up_bytes)
        .map_err(|_| KbError::internal(format!("`{UPSTREAM_FILE}` is not UTF-8")))?;
    toml::from_str::<UpstreamConfig>(up_text)
        .map_err(|e| KbError::internal(format!("rendered `{UPSTREAM_FILE}` is invalid: {e}")))?;
    // The bundle follows the skill settings that will be in effect: an existing skill.toml is
    // kept, so it wins over the template.
    let existing_skill = match safe_join(kb_root, &skill_path)? {
        p if p.is_file() => Some(read_file_limited(&p, 1024 * 1024)?),
        _ => None,
    };
    let skill_bytes = existing_skill
        .as_ref()
        .or_else(|| files.get(&skill_path))
        .ok_or_else(|| KbError::internal(format!("templates did not produce `{skill_path}`")))?;
    let skill = parse_skill_config(&skill_path, skill_bytes)?;
    let bundle = render_bundle(kb_root, &config, &skill, &loc)?;
    let gen_dir = generated_dir(&loc);
    for (rel, bytes) in bundle.all_files()? {
        files.insert(format!("{gen_dir}/{rel}"), bytes);
    }

    let mut changes = Vec::with_capacity(files.len());
    for (path, bytes) in &files {
        changes.push(plan_file(kb_root, path, bytes)?);
    }
    Ok(InitPlan {
        example: opts.example.clone(),
        name: config.project.name.clone(),
        namespace: config.project.namespace.clone(),
        remote: config.source.remote.clone(),
        approved_ref: config.source.approved_ref.clone(),
        kb_path: skill.kb_path.clone(),
        harnesses: skill.harnesses.clone(),
        upstream_url: opts.upstream_url.clone(),
        upstream_revision: revision,
        changes,
        files,
    })
}

fn plan_file(kb_root: &Path, path: &str, bytes: &[u8]) -> Result<Change> {
    let abs = safe_join(kb_root, path)?;
    match std::fs::symlink_metadata(&abs) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(Change::file(path, Action::Create))
        }
        Err(e) => Err(KbError::io(abs.display(), e)),
        Ok(md) if !md.is_file() => Ok(Change::file(path, Action::Keep)
            .with_reason("exists and is not a regular file; never overwritten")),
        Ok(_) => {
            let current = read_file_limited(&abs, crate::source::MAX_SOURCE_FILE_BYTES)?;
            Ok(if current == bytes {
                Change::file(path, Action::Unchanged)
            } else if path == PROJECT_README {
                Change::file(path, Action::Replace).with_reason(
                    "the upstream placeholder README is replaced by the project README",
                )
            } else {
                Change::file(path, Action::Keep).with_reason("exists; init never overwrites files")
            })
        }
    }
}

/// Write the plan. Returns the number of files written.
pub fn apply(kb_root: &Path, plan: &InitPlan) -> Result<usize> {
    let mut written = 0;
    for c in &plan.changes {
        if !matches!(c.action, Action::Create | Action::Replace) {
            continue;
        }
        let abs = safe_join(kb_root, &c.path)?;
        if c.action == Action::Create && std::fs::symlink_metadata(&abs).is_ok() {
            return Err(KbError::new(
                ErrorCode::Conflict,
                format!(
                    "`{}` appeared while init was running; nothing was overwritten",
                    c.path
                ),
            ));
        }
        let bytes = plan
            .files
            .get(&c.path)
            .ok_or_else(|| KbError::internal(format!("no planned content for `{}`", c.path)))?;
        atomic_write(&abs, bytes)?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_ref_rules() {
        for ok in [
            "refs/heads/main",
            "refs/heads/release/2.x",
            "refs/tags/approved",
        ] {
            assert!(check_approved_ref(ok).is_ok(), "{ok}");
        }
        for bad in [
            "main",
            "refs/heads/",
            "refs/heads/a..b",
            "refs/heads/a b",
            "refs/heads/x.lock",
            "refs//x",
            "refs/heads/.hidden",
            "refs/heads/a@{1}",
            "refs/heads/a:b",
        ] {
            assert!(check_approved_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn remote_and_url_rules() {
        assert!(check_remote("origin").is_ok());
        assert!(check_remote("--upload-pack=x").is_err());
        assert!(check_remote("a b").is_err());
        assert!(check_upstream_url("https://example.invalid/org/kb.git").is_ok());
        assert!(check_upstream_url("https://user:tok@example.invalid/kb.git").is_err());
        assert!(check_upstream_url("-oProxyCommand=x").is_err());
    }
}
