//! Task scope resolution (docs/architecture.md §3.3, §4.1 and §5 step 2).
//!
//! Every scope dimension of a task is either `Known(set)` or `Unknown`:
//!
//! * repos: Known when `--repo` is given, the host repo is identified, or the task names a
//!   repo explicitly: a `repo:path` qualifier, the repo of an explicit `--module`, or the only
//!   repo of an explicit `--feature` declared in exactly one repo. Explicit `--repo` values
//!   take precedence over the host repo; explicitly named repos are added to either.
//! * modules: Known when `--path` or `--module` is given (paths map through registry globs).
//! * features: Known when `--feature`, `--path` or `--module` is given (explicit features,
//!   features of resolved modules, features whose `paths` match).
//!
//! A `--path` may name a file or a directory, and context assembly cannot look at the disk.
//! A path therefore maps to the modules (features) whose globs match it as given, match its
//! directory form `path/` (the module root: `app/src/auth` for `app/src/auth/**`), or may
//! match inside that directory: a glob whose literal directory prefix starts with `path/`
//! (`app/src` contains `app/src/auth/**`), or, for a path that does not look like a file, a
//! glob whose literal directory prefix is an ancestor of `path/` and whose remaining segments
//! can descend into it (`app/src/main` may contain `app/src/**/auth/**`; `app/src/*/auth/**`
//! is tested segment by segment). This over-approximates like the parent-directory rule; a
//! glob whose rest cannot be split into segments (a `{...}` alternation) counts as possible.
//! A path that still maps to no module keeps the modules dimension Known (and empty) only
//! when it looks like a file (its last segment has an extension, e.g. `Main.kt`); otherwise
//! it may be a directory whose modules the registry cannot name (or a mistyped path), so the
//! modules and features dimensions become Unknown (`PATH_SCOPE_UNKNOWN`): module-scoped
//! obligations are Undetermined instead of dropped.
//!
//! Concepts come from explicit `--concept` ids and from registry aliases matched against the
//! normalized task text. A matched phrase that maps to several concepts is disambiguated by
//! explicit concepts, then by concepts already matched unambiguously in the same task, then by
//! concept path hints matching task paths; otherwise it is reported as an [`Ambiguity`].

use std::collections::{BTreeMap, BTreeSet};

use globset::{GlobBuilder, GlobMatcher};
use serde_json::{Value, json};

use crate::diag::Diagnostic;
use crate::error::{ErrorCode, KbError, Result};
use crate::glob::{RepoGlob, split_repo};
use crate::model::Registry;
use crate::normalize;
use crate::util::check_rel_path;

use super::{ContextRequest, TaskEnv};

/// Maximum task text size in bytes.
pub const MAX_TASK_BYTES: usize = 8 * 1024;
/// Maximum number of `--path` values.
pub const MAX_PATHS: usize = 512;
/// Maximum number of values in any other repeatable scope option.
pub const MAX_SCOPE_IDS: usize = 512;

/// One task scope dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DimScope {
    Known(BTreeSet<String>),
    Unknown,
}

impl DimScope {
    pub fn known(&self) -> Option<&BTreeSet<String>> {
        match self {
            DimScope::Known(s) => Some(s),
            DimScope::Unknown => None,
        }
    }

    pub fn is_known(&self) -> bool {
        matches!(self, DimScope::Known(_))
    }

    /// Short text form: `a,b`, `none` (known but empty) or `unknown`.
    pub fn label(&self) -> String {
        match self {
            DimScope::Known(s) if s.is_empty() => "none".into(),
            DimScope::Known(s) => s.iter().cloned().collect::<Vec<_>>().join(","),
            DimScope::Unknown => "unknown".into(),
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            DimScope::Known(s) => json!({"state": "known", "values": s}),
            DimScope::Unknown => json!({"state": "unknown", "values": []}),
        }
    }
}

/// A task path with the registry entities it maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    /// Registry repo of the path (`None` when neither qualified nor inferable).
    pub repo: Option<String>,
    /// Repo-relative path.
    pub path: String,
    pub modules: Vec<String>,
    pub features: Vec<String>,
}

impl ResolvedPath {
    /// `repo:path` or `path`.
    pub fn display(&self) -> String {
        match &self.repo {
            Some(r) => format!("{r}:{}", self.path),
            None => self.path.clone(),
        }
    }
}

/// A task phrase that matches aliases of several concepts and could not be disambiguated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ambiguity {
    /// The normalized task phrase that matched.
    pub phrase: String,
    /// Alias keys (normalized, `*` suffix for prefix aliases) that matched the phrase.
    pub aliases: Vec<String>,
    /// Candidate concept ids.
    pub candidates: Vec<String>,
    /// Best supplementary record offered per candidate concept: (concept, record id).
    pub offered: Vec<(String, String)>,
}

/// The resolved scope of a task.
#[derive(Debug, Clone)]
pub struct TaskScope {
    pub repos: DimScope,
    pub modules: DimScope,
    pub features: DimScope,
    /// Resolved concepts and how each was resolved (`explicit`, `alias <key>`, ...).
    pub concepts: BTreeMap<String, BTreeSet<String>>,
    pub paths: Vec<ResolvedPath>,
    /// Registry repo of the host (only when known to the registry).
    pub host_repo: Option<String>,
    pub host_repo_source: Option<String>,
    pub host_head: Option<String>,
    /// Host code versions per repo (`--host-version` overrides detected versions).
    pub host_versions: BTreeMap<String, semver::Version>,
    /// Registry modules / features that belong only to repos outside the Known task repos
    /// (empty when the repos are Unknown). A record constrained to them cannot apply.
    pub foreign_modules: BTreeSet<String>,
    pub foreign_features: BTreeSet<String>,
    /// Normalized task tokens.
    pub tokens: Vec<String>,
    pub ambiguities: Vec<Ambiguity>,
    /// Non-fatal findings made while resolving the scope.
    pub notes: Vec<Diagnostic>,
}

/// Resolve the task scope. Errors: `INVALID_INPUT` for limits and unsafe paths,
/// `UNKNOWN_SCOPE` for explicit ids that are not in the registry.
pub fn resolve(req: &ContextRequest, env: &TaskEnv, registry: &Registry) -> Result<TaskScope> {
    check_limits(req)?;
    let mut notes = Vec::new();

    let mut unknown: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
    let mut note_unknown = |dim: &'static str, id: &str, exists: bool| {
        if !exists {
            unknown.entry(dim).or_default().insert(id.to_string());
        }
    };
    for r in &req.repos {
        note_unknown("repos", r, registry.repo(r).is_some());
    }
    for m in &req.modules {
        note_unknown("modules", m, registry.module(m).is_some());
    }
    for f in &req.features {
        note_unknown("features", f, registry.feature(f).is_some());
    }
    for c in &req.concepts {
        note_unknown("concepts", c, registry.concept(c).is_some());
    }
    for (r, _) in &req.host_versions {
        note_unknown("repos", r, registry.repo(r).is_some());
    }
    let mut specs: Vec<(Option<String>, String)> = Vec::new();
    for spec in &req.paths {
        let (repo, path) = split_repo(spec);
        let path = strip_dot_slash(path);
        check_rel_path(path.trim_end_matches('/'))
            .map_err(|e| KbError::invalid_input(format!("--path `{spec}`: {e}")))?;
        if let Some(r) = repo {
            note_unknown("repos", r, registry.repo(r).is_some());
        }
        specs.push((repo.map(str::to_string), path.to_string()));
    }
    if !unknown.is_empty() {
        let list: Vec<String> = unknown
            .iter()
            .flat_map(|(dim, ids)| ids.iter().map(move |id| format!("{dim}: {id}")))
            .collect();
        return Err(KbError::new(
            ErrorCode::UnknownScope,
            format!("unknown registry ids: {}", list.join(", ")),
        )
        .with_details(json!(unknown))
        .with_hint("check project/registry/*.toml; ids are case-sensitive"));
    }

    let host_repo = match &env.host_repo {
        Some(r) if registry.repo(r).is_some() => Some(r.clone()),
        Some(r) => {
            notes.push(Diagnostic::warning(
                "HOST_REPO_UNKNOWN",
                format!("host repo `{r}` is not in the registry; the repo scope is unknown"),
            ));
            None
        }
        None => None,
    };

    // Repos dimension.
    let mut repos: BTreeSet<String> = req.repos.iter().cloned().collect();
    if repos.is_empty()
        && let Some(h) = &host_repo
    {
        repos.insert(h.clone());
    }
    for (r, _) in &specs {
        if let Some(r) = r {
            repos.insert(r.clone());
        }
    }
    // An explicit module (or a feature declared in exactly one repo) names its repo as
    // explicitly as a qualified path does.
    for m in &req.modules {
        if let Some(module) = registry.module(m) {
            repos.insert(module.repo.clone());
        }
    }
    for f in &req.features {
        if let Some(feature) = registry.feature(f)
            && let [only] = feature.repos.as_slice()
        {
            repos.insert(only.clone());
        }
    }
    let repos_dim = if repos.is_empty() {
        DimScope::Unknown
    } else {
        DimScope::Known(repos)
    };

    // Paths → modules / features / concept hints.
    let default_repo = host_repo.clone().or_else(|| match req.repos.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    });
    let all_repos: Vec<String> = registry.data.repos.iter().map(|r| r.id.clone()).collect();
    let prefixes = if specs.is_empty() {
        GlobPrefixes::default()
    } else {
        GlobPrefixes::of(registry)
    };
    let mut paths = Vec::new();
    let mut hint_concepts: BTreeSet<String> = BTreeSet::new();
    let mut unresolved = Vec::new();
    for (repo, path) in specs {
        let repo = repo.or_else(|| default_repo.clone());
        let lookup: Vec<String> = match &repo {
            Some(r) => vec![r.clone()],
            None => all_repos.clone(),
        };
        // The path as given and, when it differs, its directory form `path/`.
        let as_dir = format!("{}/", path.trim_end_matches('/'));
        let dir_like = !is_file_like(&path);
        let mut forms = vec![path.as_str()];
        if as_dir != path {
            forms.push(as_dir.as_str());
        }
        let mut modules = BTreeSet::new();
        let mut features = BTreeSet::new();
        for r in &lookup {
            for form in &forms {
                modules.extend(
                    registry
                        .modules_for_path(r, form)
                        .into_iter()
                        .map(|m| m.id.clone()),
                );
                features.extend(
                    registry
                        .features_for_path(r, form)
                        .into_iter()
                        .map(|f| f.id.clone()),
                );
                hint_concepts.extend(
                    registry
                        .concepts_for_path(Some(r), form)
                        .into_iter()
                        .map(|c| c.id.clone()),
                );
            }
            modules.extend(prefixes.modules_inside(registry, r, &as_dir, dir_like));
            features.extend(prefixes.features_inside(registry, r, &as_dir, dir_like));
        }
        for form in &forms {
            hint_concepts.extend(
                registry
                    .concepts_for_path(None, form)
                    .into_iter()
                    .map(|c| c.id.clone()),
            );
        }
        let resolved = ResolvedPath {
            repo,
            path,
            modules: modules.into_iter().collect(),
            features: features.into_iter().collect(),
        };
        if resolved.modules.is_empty() && dir_like {
            unresolved.push(resolved.display());
        }
        paths.push(resolved);
    }
    paths.sort_by(|a, b| (&a.repo, &a.path).cmp(&(&b.repo, &b.path)));
    paths.dedup();
    unresolved.sort();
    unresolved.dedup();
    for p in &unresolved {
        notes.push(Diagnostic::warning(
            "PATH_SCOPE_UNKNOWN",
            format!(
                "path `{p}` maps to no registry module and may be a directory; the module and \
                 feature scope is unknown (name files inside it, or pass --module)"
            ),
        ));
    }

    let modules_dim = if (req.paths.is_empty() && req.modules.is_empty()) || !unresolved.is_empty()
    {
        DimScope::Unknown
    } else {
        let mut set: BTreeSet<String> = req.modules.iter().cloned().collect();
        for p in &paths {
            set.extend(p.modules.iter().cloned());
        }
        DimScope::Known(set)
    };
    let features_dim =
        if (req.features.is_empty() && req.paths.is_empty() && req.modules.is_empty())
            || !unresolved.is_empty()
        {
            DimScope::Unknown
        } else {
            let mut set: BTreeSet<String> = req.features.iter().cloned().collect();
            if let Some(mods) = modules_dim.known() {
                for m in mods {
                    if let Some(module) = registry.module(m) {
                        set.extend(module.features.iter().cloned());
                    }
                }
            }
            for p in &paths {
                set.extend(p.features.iter().cloned());
            }
            DimScope::Known(set)
        };

    let (foreign_modules, foreign_features) = match repos_dim.known() {
        Some(known) => (
            registry
                .data
                .modules
                .iter()
                .filter(|m| !known.contains(&m.repo))
                .map(|m| m.id.clone())
                .collect(),
            registry
                .data
                .features
                .iter()
                .filter(|f| !f.repos.is_empty() && f.repos.iter().all(|r| !known.contains(r)))
                .map(|f| f.id.clone())
                .collect(),
        ),
        None => (BTreeSet::new(), BTreeSet::new()),
    };

    let tokens = req
        .task
        .as_deref()
        .map(normalize::tokens)
        .unwrap_or_default();
    let mut concepts: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for c in &req.concepts {
        concepts
            .entry(c.clone())
            .or_default()
            .insert("explicit".into());
    }
    let ambiguities = match_concepts(registry, &tokens, &hint_concepts, &mut concepts);

    let mut host_versions = env.host_versions.clone();
    for (r, v) in &req.host_versions {
        host_versions.insert(r.clone(), v.clone());
    }

    Ok(TaskScope {
        repos: repos_dim,
        modules: modules_dim,
        features: features_dim,
        concepts,
        paths,
        host_repo,
        host_repo_source: env.host_repo_source.clone(),
        host_head: env.host_head.clone(),
        host_versions,
        foreign_modules,
        foreign_features,
        tokens,
        ambiguities,
        notes,
    })
}

fn check_limits(req: &ContextRequest) -> Result<()> {
    if let Some(t) = &req.task
        && t.len() > MAX_TASK_BYTES
    {
        return Err(KbError::invalid_input(format!(
            "--task is {} bytes; the limit is {MAX_TASK_BYTES} bytes",
            t.len()
        )));
    }
    if req.paths.len() > MAX_PATHS {
        return Err(KbError::invalid_input(format!(
            "{} --path values given; the limit is {MAX_PATHS}",
            req.paths.len()
        )));
    }
    for (name, n) in [
        ("--repo", req.repos.len()),
        ("--module", req.modules.len()),
        ("--feature", req.features.len()),
        ("--concept", req.concepts.len()),
        ("--host-version", req.host_versions.len()),
    ] {
        if n > MAX_SCOPE_IDS {
            return Err(KbError::invalid_input(format!(
                "{n} {name} values given; the limit is {MAX_SCOPE_IDS}"
            )));
        }
    }
    Ok(())
}

fn strip_dot_slash(mut p: &str) -> &str {
    while let Some(rest) = p.strip_prefix("./") {
        p = rest;
    }
    p
}

/// Does the path look like a file: no trailing `/` and an extension on its last segment
/// (`TokenStore.kt`, not `auth`, `Makefile` or `.github`)?
fn is_file_like(path: &str) -> bool {
    if path.ends_with('/') {
        return false;
    }
    let last = path.rsplit('/').next().unwrap_or(path);
    last.char_indices()
        .any(|(i, c)| c == '.' && i > 0 && i + 1 < last.len())
}

/// Registry module and feature globs indexed for the directory rules: a directory path may
/// contain a glob's matches when the glob's literal directory prefix lies inside it, or when
/// the prefix is an ancestor of it and the rest of the glob can descend into it.
#[derive(Default)]
struct GlobPrefixes {
    modules: Vec<PrefixGlob>,
    features: Vec<PrefixGlob>,
}

/// One registry glob for the directory rules.
struct PrefixGlob {
    /// Index into the registry's module (feature) list.
    index: usize,
    /// Repo qualifier of the glob.
    qualifier: Option<String>,
    /// Literal directory prefix (empty or ending with `/`).
    prefix: String,
    /// The `/`-separated segments after `prefix`; `None` when they cannot be split at `/`
    /// (a `{...}` alternation, a class that does not compile per segment): the glob may then
    /// match anywhere below `prefix`.
    rest: Option<Vec<Segment>>,
}

/// One glob segment after the literal directory prefix.
enum Segment {
    /// `**`: any number of path segments.
    Deep,
    Literal(String),
    /// A segment with wildcards, matched against one path segment.
    Wild(GlobMatcher),
}

impl PrefixGlob {
    fn parse(index: usize, spec: &str) -> Option<PrefixGlob> {
        let glob = RepoGlob::parse(spec).ok()?;
        let prefix = glob.literal_dir_prefix();
        let rest = glob.pattern[prefix.len()..].to_string();
        let rest = if rest.contains('{') {
            None
        } else {
            rest.split('/').map(Segment::parse).collect()
        };
        Some(PrefixGlob {
            index,
            qualifier: glob.repo,
            prefix,
            rest,
        })
    }

    /// May the glob match a path inside directory `dir` (ends with `/`) of `repo`? The
    /// ancestor rule applies only to paths that may be directories (`dir_like`).
    fn may_be_inside(&self, repo: &str, dir: &str, dir_like: bool) -> bool {
        if self.qualifier.as_deref().is_some_and(|q| q != repo) {
            return false;
        }
        if self.prefix.starts_with(dir) {
            return true;
        }
        match dir.strip_prefix(self.prefix.as_str()) {
            Some(below) if dir_like => may_descend(self.rest.as_deref(), below),
            _ => false,
        }
    }
}

impl Segment {
    fn parse(seg: &str) -> Option<Segment> {
        if seg == "**" {
            return Some(Segment::Deep);
        }
        if !seg.contains(['*', '?', '[']) {
            return Some(Segment::Literal(seg.to_string()));
        }
        let glob = GlobBuilder::new(seg)
            .literal_separator(true)
            .backslash_escape(false)
            .build()
            .ok()?;
        Some(Segment::Wild(glob.compile_matcher()))
    }
}

/// Can glob segments `rest` match a path that continues below the relative directory
/// `below` (empty or ending with `/`)? Every directory segment must be matched by a glob
/// segment (a `**` matches the rest), and a non-empty glob segment must remain for the part
/// inside the directory. `None` (unsplittable rest) is possible.
fn may_descend(rest: Option<&[Segment]>, below: &str) -> bool {
    let Some(rest) = rest else {
        return true;
    };
    let mut segments = rest.iter();
    for part in below.split('/').filter(|p| !p.is_empty()) {
        match segments.next() {
            None => return false,
            Some(Segment::Deep) => return true,
            Some(Segment::Literal(l)) if l != part => return false,
            Some(Segment::Wild(m)) if !m.is_match(part) => return false,
            Some(_) => {}
        }
    }
    segments.any(|s| !matches!(s, Segment::Literal(l) if l.is_empty()))
}

impl GlobPrefixes {
    fn of(registry: &Registry) -> GlobPrefixes {
        let globs = |i: usize, specs: &[String]| {
            specs
                .iter()
                .filter_map(|g| PrefixGlob::parse(i, g))
                .collect::<Vec<_>>()
        };
        GlobPrefixes {
            modules: (registry.data.modules.iter().enumerate())
                .flat_map(|(i, m)| globs(i, &m.paths))
                .collect(),
            features: (registry.data.features.iter().enumerate())
                .flat_map(|(i, f)| globs(i, &f.paths))
                .collect(),
        }
    }

    /// Modules of `repo` with a glob that may match inside directory `dir` (ends with `/`).
    fn modules_inside(
        &self,
        registry: &Registry,
        repo: &str,
        dir: &str,
        dir_like: bool,
    ) -> Vec<String> {
        self.modules
            .iter()
            .filter(|g| g.may_be_inside(repo, dir, dir_like))
            .map(|g| &registry.data.modules[g.index])
            .filter(|m| m.repo == repo)
            .map(|m| m.id.clone())
            .collect()
    }

    /// Features available in `repo` with a glob that may match inside directory `dir`.
    fn features_inside(
        &self,
        registry: &Registry,
        repo: &str,
        dir: &str,
        dir_like: bool,
    ) -> Vec<String> {
        self.features
            .iter()
            .filter(|g| g.may_be_inside(repo, dir, dir_like))
            .map(|g| &registry.data.features[g.index])
            .filter(|f| f.repos.is_empty() || f.repos.iter().any(|r| r == repo))
            .map(|f| f.id.clone())
            .collect()
    }
}

/// One alias match group: all concepts whose aliases matched the same token span.
struct PhraseGroup {
    start: usize,
    len: usize,
    aliases: BTreeSet<String>,
    concepts: BTreeSet<String>,
}

/// Match concept aliases against the task tokens, adding resolved concepts to `concepts`
/// and returning unresolved ambiguities.
fn match_concepts(
    registry: &Registry,
    tokens: &[String],
    hints: &BTreeSet<String>,
    concepts: &mut BTreeMap<String, BTreeSet<String>>,
) -> Vec<Ambiguity> {
    let mut groups: BTreeMap<(usize, usize), PhraseGroup> = BTreeMap::new();
    for (key, concept, start) in registry.concept_alias_matches(tokens) {
        let len = key.trim_end_matches('*').split(' ').count();
        let g = groups.entry((start, len)).or_insert_with(|| PhraseGroup {
            start,
            len,
            aliases: BTreeSet::new(),
            concepts: BTreeSet::new(),
        });
        g.aliases.insert(key);
        g.concepts.insert(concept);
    }
    // A span strictly inside a longer matched span is less specific and is dropped.
    let spans: Vec<(usize, usize)> = groups.keys().copied().collect();
    groups.retain(|&(s, l), _| {
        !spans
            .iter()
            .any(|&(s2, l2)| (s2, l2) != (s, l) && s2 <= s && s + l <= s2 + l2)
    });

    let explicit: BTreeSet<String> = concepts.keys().cloned().collect();
    let (single, multi): (Vec<PhraseGroup>, Vec<PhraseGroup>) =
        groups.into_values().partition(|g| g.concepts.len() == 1);
    let mut unambiguous = BTreeSet::new();
    for g in single {
        for c in &g.concepts {
            let alias = g.aliases.iter().next().cloned().unwrap_or_default();
            concepts
                .entry(c.clone())
                .or_default()
                .insert(format!("alias `{alias}`"));
            unambiguous.insert(c.clone());
        }
    }
    let mut out = Vec::new();
    for g in multi {
        let phrase = tokens[g.start..g.start + g.len].join(" ");
        let alias_list = g.aliases.iter().cloned().collect::<Vec<_>>().join(", ");
        let pick = |set: &BTreeSet<String>| -> BTreeSet<String> {
            g.concepts.intersection(set).cloned().collect()
        };
        let by_explicit = pick(&explicit);
        let by_text = pick(&unambiguous);
        let by_hint = pick(hints);
        let resolved = if !by_explicit.is_empty() {
            Some((by_explicit, "explicit --concept"))
        } else if !by_text.is_empty() {
            Some((by_text, "unambiguous mention in the task"))
        } else if by_hint.len() == 1 {
            Some((by_hint.clone(), "concept path hint"))
        } else {
            None
        };
        match resolved {
            Some((set, how)) => {
                for c in set {
                    concepts
                        .entry(c)
                        .or_default()
                        .insert(format!("alias `{alias_list}` (disambiguated by {how})"));
                }
            }
            None => {
                let candidates = if by_hint.len() > 1 {
                    by_hint
                } else {
                    g.concepts.clone()
                };
                out.push(Ambiguity {
                    phrase,
                    aliases: g.aliases.into_iter().collect(),
                    candidates: candidates.into_iter().collect(),
                    offered: Vec::new(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.phrase.cmp(&b.phrase));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RegistryData;
    use crate::model::registry::Concept;

    fn registry() -> Registry {
        let concept = |id: &str, aliases: &[&str], paths: &[&str]| Concept {
            id: id.into(),
            title: id.into(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            paths: paths.iter().map(|s| s.to_string()).collect(),
        };
        Registry::new(RegistryData {
            concepts: vec![
                concept(
                    "ui-composition",
                    &["композици*", "composition"],
                    &["**/ui/**"],
                ),
                concept(
                    "object-composition",
                    &["композици*", "composition", "композиция объектов"],
                    &["**/domain/**"],
                ),
            ],
            ..Default::default()
        })
    }

    fn run(text: &str, hints: &[&str], explicit: &[&str]) -> (Vec<String>, Vec<Ambiguity>) {
        let mut concepts: BTreeMap<String, BTreeSet<String>> = explicit
            .iter()
            .map(|c| (c.to_string(), BTreeSet::from(["explicit".to_string()])))
            .collect();
        let hints = hints.iter().map(|s| s.to_string()).collect();
        let amb = match_concepts(&registry(), &normalize::tokens(text), &hints, &mut concepts);
        (concepts.into_keys().collect(), amb)
    }

    #[test]
    fn shared_alias_is_ambiguous_without_hints() {
        let (concepts, amb) = run("Поправить композицию", &[], &[]);
        assert!(concepts.is_empty());
        assert_eq!(amb.len(), 1);
        assert_eq!(amb[0].phrase, "композицию");
        assert_eq!(amb[0].aliases, vec!["композици*"]);
        assert_eq!(
            amb[0].candidates,
            vec!["object-composition", "ui-composition"]
        );
    }

    #[test]
    fn ambiguity_resolved_by_explicit_concept_then_hint() {
        let (concepts, amb) = run("композиция", &[], &["ui-composition"]);
        assert!(amb.is_empty());
        assert_eq!(concepts, vec!["ui-composition"]);
        let (concepts, amb) = run("композиция", &["object-composition"], &[]);
        assert!(amb.is_empty());
        assert_eq!(concepts, vec!["object-composition"]);
    }

    #[test]
    fn longer_alias_span_wins() {
        let (concepts, amb) = run("Композиция объектов в домене", &[], &[]);
        assert!(amb.is_empty(), "{amb:?}");
        assert_eq!(concepts, vec!["object-composition"]);
    }

    #[test]
    fn file_like_paths() {
        for file in ["app/src/Main.kt", "build.gradle.kts", "a/.b/c.rs"] {
            assert!(is_file_like(file), "{file}");
        }
        for dir in [
            "app/src/auth",
            "app/src/auth/",
            "Makefile",
            ".github",
            "a/b.",
            "x.kt/",
        ] {
            assert!(!is_file_like(dir), "{dir}");
        }
    }

    #[test]
    fn directory_may_contain_wildcard_globs() {
        let inside = |glob: &str, dir: &str| {
            PrefixGlob::parse(0, glob)
                .unwrap()
                .may_be_inside("mobile", dir, true)
        };
        // The literal prefix lies inside the directory (parent-directory rule).
        assert!(inside("app/src/auth/**", "app/"));
        assert!(inside("app/src/**/auth/**", "app/src/"));
        // The literal prefix is an ancestor and the rest can descend into the directory.
        for dir in ["app/src/main/", "app/src/main/java/"] {
            assert!(inside("app/src/**/auth/**", dir), "{dir}");
            assert!(inside("**/auth/**", dir), "{dir}");
        }
        assert!(inside("app/src/*/auth/**", "app/src/main/"));
        assert!(inside("app/src/*/auth/*.kt", "app/src/main/auth/"));
        assert!(inside("app/src/m?in/auth/**", "app/src/main/"));
        assert!(inside("app/src/[mt]*/auth/**", "app/src/main/"));
        // Alternations may span segments: possible anywhere below the prefix.
        assert!(inside("app/src/{main,test/x}/auth/**", "app/src/other/"));
        // A segment that cannot match, or a pattern that ends at or above the directory.
        assert!(!inside("app/src/*/auth/**", "app/src/main/ui/"));
        assert!(!inside("app/src/main/auth/**", "app/src/test/"));
        assert!(!inside("app/src/[t]*/auth/**", "app/src/main/"));
        assert!(!inside("app/src/*", "app/src/main/"));
        assert!(!inside("app/src/*.kt", "app/src/main/"));
        assert!(!inside("app/src/*/auth/*.kt", "app/src/main/auth/x/"));
        assert!(!inside("app/src/Main.kt", "app/src/Main.kt/"));
        // Another repo, or a path that looks like a file.
        assert!(!inside("backend:src/**/auth/**", "src/main/"));
        let file = PrefixGlob::parse(0, "app/src/**/auth/**").unwrap();
        assert!(!file.may_be_inside("mobile", "app/src/main/Main.kt/", false));
    }

    #[test]
    fn unambiguous_mention_resolves_shared_alias() {
        // The concept id itself is an implicit alias ("ui composition").
        let (concepts, amb) = run("ui composition: fix композицию", &[], &[]);
        assert!(amb.is_empty(), "{amb:?}");
        assert_eq!(concepts, vec!["ui-composition"]);
    }
}
