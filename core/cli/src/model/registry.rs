//! Project registries: owners, repos, modules, features, concepts (docs/architecture.md §4).
//! Adding a registry entry never requires rebuilding the CLI.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::diag::Diagnostic;
use crate::glob::{RepoGlob, written_qualifier};
use crate::model::ids::{MAX_REGISTRY_ID, REGISTRY_ID_PATTERN, check_registry_id};
use crate::normalize::AliasPattern;

pub const REGISTRY_FILES: [&str; 6] = [
    "owners.toml",
    "repos.toml",
    "modules.toml",
    "features.toml",
    "concepts.toml",
    "change-types.toml",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    /// Registry id, unique within its registry file.
    #[schemars(regex(pattern = REGISTRY_ID_PATTERN), length(max = MAX_REGISTRY_ID))]
    pub id: String,
    pub title: String,
    /// Authority restriction: when non-empty, the owner may only own records whose
    /// implied repos are within this list.
    #[serde(default)]
    pub repos: Vec<String>,
    /// May own product-wide records.
    #[serde(default)]
    pub product: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Repo {
    /// Registry id, unique within its registry file.
    #[schemars(regex(pattern = REGISTRY_ID_PATTERN), length(max = MAX_REGISTRY_ID))]
    pub id: String,
    pub title: String,
    /// Remote URLs identifying the host repository (normalized before matching).
    #[serde(default)]
    pub remotes: Vec<String>,
    /// Host-relative file whose first line is the host code version (semver).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Module {
    /// Registry id, unique within its registry file.
    #[schemars(regex(pattern = REGISTRY_ID_PATTERN), length(max = MAX_REGISTRY_ID))]
    pub id: String,
    pub repo: String,
    pub title: String,
    /// Repo-relative globs of files belonging to the module.
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Feature {
    /// Registry id, unique within its registry file.
    #[schemars(regex(pattern = REGISTRY_ID_PATTERN), length(max = MAX_REGISTRY_ID))]
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub repos: Vec<String>,
    /// Globs (optionally `repo:`-qualified) of files implementing the feature.
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Concept {
    /// Registry id, unique within its registry file.
    #[schemars(regex(pattern = REGISTRY_ID_PATTERN), length(max = MAX_REGISTRY_ID))]
    pub id: String,
    pub title: String,
    /// Multilingual aliases; see normalization rules.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Disambiguation hints: globs (optionally `repo:`-qualified).
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnersFile {
    /// Document schema version (1 or 2).
    #[schemars(extend("enum" = [1, 2]))]
    pub schema: u32,
    #[serde(default)]
    pub owner: Vec<Owner>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReposFile {
    /// Document schema version (1 or 2).
    #[schemars(extend("enum" = [1, 2]))]
    pub schema: u32,
    #[serde(default)]
    pub repo: Vec<Repo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModulesFile {
    /// Document schema version (1 or 2).
    #[schemars(extend("enum" = [1, 2]))]
    pub schema: u32,
    #[serde(default)]
    pub module: Vec<Module>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FeaturesFile {
    /// Document schema version (1 or 2).
    #[schemars(extend("enum" = [1, 2]))]
    pub schema: u32,
    #[serde(default)]
    pub feature: Vec<Feature>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConceptsFile {
    /// Document schema version (1 or 2).
    #[schemars(extend("enum" = [1, 2]))]
    pub schema: u32,
    #[serde(default)]
    pub concept: Vec<Concept>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangeType {
    #[schemars(regex(pattern = crate::model::ids::LOCAL_ID_PATTERN), length(max = crate::model::ids::MAX_LOCAL_ID))]
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    /// Identifiers in task descriptions or changed filenames; not a source parser.
    #[serde(default)]
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangeTypesFile {
    #[schemars(extend("const" = 2))]
    pub schema: u32,
    #[serde(default)]
    pub change_type: Vec<ChangeType>,
}

/// Serializable registry data (stored in the index snapshot row).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryData {
    pub owners: Vec<Owner>,
    pub repos: Vec<Repo>,
    pub modules: Vec<Module>,
    pub features: Vec<Feature>,
    pub concepts: Vec<Concept>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub change_types: Vec<ChangeType>,
}

/// Registry with compiled globs and alias patterns for lookups.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub data: RegistryData,
    module_globs: Vec<(usize, Vec<RepoGlob>)>,
    feature_globs: Vec<(usize, Vec<RepoGlob>)>,
    concept_globs: Vec<(usize, Vec<RepoGlob>)>,
    concept_aliases: Vec<(AliasPattern, String)>,
}

/// Parse a registry file of type `T` strictly. Returns diagnostics on failure.
pub fn parse_registry_file<T: serde::de::DeserializeOwned>(
    path: &str,
    text: &str,
) -> Result<T, Diagnostic> {
    toml::from_str::<T>(text)
        .map_err(|e| Diagnostic::error("REGISTRY_PARSE", format!("{e}")).at_path(path.to_string()))
}

impl Registry {
    /// Build a registry from data, compiling globs and aliases. Invalid globs/aliases are
    /// skipped here and reported by [`Registry::validate`].
    pub fn new(data: RegistryData) -> Registry {
        let compile = |specs: &[String]| -> Vec<RepoGlob> {
            specs
                .iter()
                .filter_map(|s| RepoGlob::parse(s).ok())
                .collect()
        };
        let module_globs = data
            .modules
            .iter()
            .enumerate()
            .map(|(i, m)| (i, compile(&m.paths)))
            .collect();
        let feature_globs = data
            .features
            .iter()
            .enumerate()
            .map(|(i, f)| (i, compile(&f.paths)))
            .collect();
        let concept_globs = data
            .concepts
            .iter()
            .enumerate()
            .map(|(i, c)| (i, compile(&c.paths)))
            .collect();
        let mut concept_aliases = Vec::new();
        for c in &data.concepts {
            // The concept id itself (with `-`/`_`/`.` as spaces) is an implicit alias.
            if let Some(p) = AliasPattern::compile(&c.id) {
                concept_aliases.push((p, c.id.clone()));
            }
            for a in &c.aliases {
                if let Some(p) = AliasPattern::compile(a) {
                    concept_aliases.push((p, c.id.clone()));
                }
            }
        }
        concept_aliases.sort_by(|a, b| (a.0.key(), &a.1).cmp(&(b.0.key(), &b.1)));
        concept_aliases.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
        Registry {
            data,
            module_globs,
            feature_globs,
            concept_globs,
            concept_aliases,
        }
    }

    pub fn owner(&self, id: &str) -> Option<&Owner> {
        self.data.owners.iter().find(|o| o.id == id)
    }
    pub fn repo(&self, id: &str) -> Option<&Repo> {
        self.data.repos.iter().find(|r| r.id == id)
    }
    pub fn module(&self, id: &str) -> Option<&Module> {
        self.data.modules.iter().find(|m| m.id == id)
    }
    pub fn feature(&self, id: &str) -> Option<&Feature> {
        self.data.features.iter().find(|f| f.id == id)
    }
    pub fn change_type(&self, id: &str) -> Option<&ChangeType> {
        self.data.change_types.iter().find(|c| c.id == id)
    }
    pub fn concept(&self, id: &str) -> Option<&Concept> {
        self.data.concepts.iter().find(|c| c.id == id)
    }

    /// Modules whose globs match `path` in `repo` (sorted by id).
    pub fn modules_for_path(&self, repo: &str, path: &str) -> Vec<&Module> {
        let mut out: Vec<&Module> = self
            .module_globs
            .iter()
            .filter(|(i, globs)| {
                let m = &self.data.modules[*i];
                m.repo == repo && globs.iter().any(|g| g.matches(Some(repo), path))
            })
            .map(|(i, _)| &self.data.modules[*i])
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Features whose globs match `path` in `repo` (sorted by id).
    pub fn features_for_path(&self, repo: &str, path: &str) -> Vec<&Feature> {
        let mut out: Vec<&Feature> = self
            .feature_globs
            .iter()
            .filter(|(i, globs)| {
                let f = &self.data.features[*i];
                (f.repos.is_empty() || f.repos.iter().any(|r| r == repo))
                    && globs.iter().any(|g| g.matches(Some(repo), path))
            })
            .map(|(i, _)| &self.data.features[*i])
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Concepts whose path hints match `path` in `repo`.
    pub fn concepts_for_path(&self, repo: Option<&str>, path: &str) -> Vec<&Concept> {
        let mut out: Vec<&Concept> = self
            .concept_globs
            .iter()
            .filter(|(_, globs)| globs.iter().any(|g| g.matches(repo, path)))
            .map(|(i, _)| &self.data.concepts[*i])
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Alias matches in a normalized token stream: (alias key, concept id, token position).
    pub fn concept_alias_matches(&self, tokens: &[String]) -> Vec<(String, String, usize)> {
        let mut out = Vec::new();
        for (pat, cid) in &self.concept_aliases {
            if let Some(pos) = pat.find(tokens) {
                out.push((pat.key(), cid.clone(), pos));
            }
        }
        out.sort();
        out
    }

    /// Repo ids implied by a module or feature list (for scope subsumption / authority).
    pub fn repos_of_modules(&self, modules: &[String]) -> BTreeSet<String> {
        modules
            .iter()
            .filter_map(|m| self.module(m))
            .map(|m| m.repo.clone())
            .collect()
    }

    pub fn repos_of_features(&self, features: &[String]) -> BTreeSet<String> {
        features
            .iter()
            .filter_map(|f| self.feature(f))
            .flat_map(|f| f.repos.iter().cloned())
            .collect()
    }

    /// Identify a registry repo by a host remote URL.
    pub fn repo_for_remote(&self, url: &str) -> Option<&Repo> {
        let want = normalize_remote_url(url);
        let mut hits: Vec<&Repo> = self
            .data
            .repos
            .iter()
            .filter(|r| r.remotes.iter().any(|u| normalize_remote_url(u) == want))
            .collect();
        hits.sort_by(|a, b| a.id.cmp(&b.id));
        hits.into_iter().next()
    }

    /// Structural validation of the registry itself.
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut d = Vec::new();
        let reg_path = |f: &str| format!("registry/{f}");
        let mut check_ids = |kind: &str, file: &str, ids: Vec<&str>| {
            let mut seen = BTreeSet::new();
            for id in ids {
                if let Err(e) = check_registry_id(id) {
                    d.push(
                        Diagnostic::error("REGISTRY_ID_INVALID", format!("{kind} id `{id}`: {e}"))
                            .at_path(reg_path(file)),
                    );
                }
                if !seen.insert(id) {
                    d.push(
                        Diagnostic::error(
                            "REGISTRY_DUPLICATE_ID",
                            format!("duplicate {kind} id `{id}`"),
                        )
                        .at_path(reg_path(file)),
                    );
                }
            }
        };
        check_ids(
            "owner",
            "owners.toml",
            self.data.owners.iter().map(|x| x.id.as_str()).collect(),
        );
        check_ids(
            "repo",
            "repos.toml",
            self.data.repos.iter().map(|x| x.id.as_str()).collect(),
        );
        check_ids(
            "module",
            "modules.toml",
            self.data.modules.iter().map(|x| x.id.as_str()).collect(),
        );
        check_ids(
            "feature",
            "features.toml",
            self.data.features.iter().map(|x| x.id.as_str()).collect(),
        );
        check_ids(
            "concept",
            "concepts.toml",
            self.data.concepts.iter().map(|x| x.id.as_str()).collect(),
        );
        check_ids(
            "change type",
            "change-types.toml",
            self.data
                .change_types
                .iter()
                .map(|x| x.id.as_str())
                .collect(),
        );

        let repo_ids: BTreeSet<&str> = self.data.repos.iter().map(|r| r.id.as_str()).collect();
        let feature_ids: BTreeSet<&str> =
            self.data.features.iter().map(|f| f.id.as_str()).collect();
        let glob_check = |d: &mut Vec<Diagnostic>, file: &str, owner: &str, specs: &[String]| {
            for s in specs {
                match RepoGlob::parse(s) {
                    // Checked as written: a qualifier that is not id-shaped never matches.
                    Ok(_) => {
                        if let Some(r) = written_qualifier(s)
                            && !repo_ids.contains(r)
                        {
                            d.push(
                                Diagnostic::error(
                                    "REGISTRY_UNKNOWN_REPO",
                                    format!("`{owner}` glob `{s}` names unknown repo `{r}`"),
                                )
                                .at_path(reg_path(file)),
                            );
                        }
                    }
                    Err(e) => d.push(
                        Diagnostic::error("REGISTRY_GLOB_INVALID", format!("`{owner}`: {e}"))
                            .at_path(reg_path(file)),
                    ),
                }
            }
        };
        for o in &self.data.owners {
            for r in &o.repos {
                if !repo_ids.contains(r.as_str()) {
                    d.push(
                        Diagnostic::error(
                            "REGISTRY_UNKNOWN_REPO",
                            format!("owner `{}` names unknown repo `{r}`", o.id),
                        )
                        .at_path(reg_path("owners.toml")),
                    );
                }
            }
        }
        for r in &self.data.repos {
            if let Some(v) = &r.version_file
                && let Err(e) = crate::util::check_rel_path(v)
            {
                d.push(
                    Diagnostic::error(
                        "REGISTRY_PATH_INVALID",
                        format!("repo `{}` version_file: {e}", r.id),
                    )
                    .at_path(reg_path("repos.toml")),
                );
            }
        }
        for m in &self.data.modules {
            if !repo_ids.contains(m.repo.as_str()) {
                d.push(
                    Diagnostic::error(
                        "REGISTRY_UNKNOWN_REPO",
                        format!("module `{}` names unknown repo `{}`", m.id, m.repo),
                    )
                    .at_path(reg_path("modules.toml")),
                );
            }
            for f in &m.features {
                if !feature_ids.contains(f.as_str()) {
                    d.push(
                        Diagnostic::error(
                            "REGISTRY_UNKNOWN_FEATURE",
                            format!("module `{}` names unknown feature `{f}`", m.id),
                        )
                        .at_path(reg_path("modules.toml")),
                    );
                }
            }
            glob_check(&mut d, "modules.toml", &m.id, &m.paths);
        }
        for f in &self.data.features {
            for r in &f.repos {
                if !repo_ids.contains(r.as_str()) {
                    d.push(
                        Diagnostic::error(
                            "REGISTRY_UNKNOWN_REPO",
                            format!("feature `{}` names unknown repo `{r}`", f.id),
                        )
                        .at_path(reg_path("features.toml")),
                    );
                }
            }
            glob_check(&mut d, "features.toml", &f.id, &f.paths);
        }
        for c in &self.data.concepts {
            for a in &c.aliases {
                if AliasPattern::compile(a).is_none() {
                    d.push(
                        Diagnostic::error(
                            "REGISTRY_ALIAS_INVALID",
                            format!("concept `{}` alias `{a}` normalizes to nothing", c.id),
                        )
                        .at_path(reg_path("concepts.toml")),
                    );
                }
            }
            glob_check(&mut d, "concepts.toml", &c.id, &c.paths);
        }
        for c in &self.data.change_types {
            if let Err(e) = crate::model::ids::check_local_id(&c.id) {
                d.push(
                    Diagnostic::error("CHANGE_TYPE_INVALID", e)
                        .at_path(reg_path("change-types.toml")),
                );
            }
            for a in c.aliases.iter().chain(&c.symbols) {
                if AliasPattern::compile(a).is_none() {
                    d.push(
                        Diagnostic::error(
                            "REGISTRY_ALIAS_INVALID",
                            format!("change type {}: empty alias", c.id),
                        )
                        .at_path(reg_path("change-types.toml")),
                    );
                }
            }
            glob_check(&mut d, "change-types.toml", &c.id, &c.paths);
        }
        d
    }

    /// Map of alias key → concept ids that share it (ambiguous aliases have >1 entry).
    pub fn alias_table(&self) -> BTreeMap<String, Vec<String>> {
        let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (p, c) in &self.concept_aliases {
            m.entry(p.key()).or_default().push(c.clone());
        }
        for v in m.values_mut() {
            v.sort();
            v.dedup();
        }
        m
    }
}

/// Normalize a Git remote URL for identity comparison: strip scheme, userinfo, port-less
/// `host:path` scp syntax, trailing `.git` and `/`; lowercase the host.
pub fn normalize_remote_url(url: &str) -> String {
    let mut s = url.trim().to_string();
    for scheme in [
        "https://",
        "http://",
        "ssh://",
        "git://",
        "file://",
        "git+ssh://",
    ] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest.to_string();
            break;
        }
    }
    if let Some(at) = s.find('@') {
        let slash = s.find('/').unwrap_or(usize::MAX);
        if at < slash {
            s = s[at + 1..].to_string();
        }
    }
    // scp-like `host:path` (only when the part before ':' has no '/')
    if let Some(colon) = s.find(':') {
        let slash = s.find('/').unwrap_or(usize::MAX);
        if colon < slash {
            let (host, path) = s.split_at(colon);
            let path = path[1..]
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .trim_start_matches('/');
            s = format!("{host}/{path}");
        }
    }
    let s = s
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .trim_end_matches('/');
    match s.split_once('/') {
        Some((host, rest)) => format!("{}/{}", host.to_lowercase(), rest),
        None => s.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_normalization() {
        let a = normalize_remote_url("git@GitHub.com:acme/mobile.git");
        let b = normalize_remote_url("https://user:tok@github.com/acme/mobile");
        let c = normalize_remote_url("ssh://git@github.com/acme/mobile.git/");
        assert_eq!(a, "github.com/acme/mobile");
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(normalize_remote_url("/tmp/x/mobile.git"), "/tmp/x/mobile");
    }

    fn sample() -> Registry {
        Registry::new(RegistryData {
            owners: vec![],
            change_types: vec![],
            repos: vec![Repo {
                id: "mobile".into(),
                title: "M".into(),
                remotes: vec![],
                version_file: None,
            }],
            modules: vec![Module {
                id: "mobile.auth".into(),
                repo: "mobile".into(),
                title: "Auth".into(),
                paths: vec!["app/auth/**".into()],
                features: vec![],
            }],
            features: vec![],
            concepts: vec![
                Concept {
                    id: "ui-composition".into(),
                    title: "UI".into(),
                    aliases: vec!["композици*".into(), "composition".into()],
                    paths: vec!["**/ui/**".into()],
                },
                Concept {
                    id: "object-composition".into(),
                    title: "OO".into(),
                    aliases: vec!["композици*".into(), "composition".into()],
                    paths: vec!["**/domain/**".into()],
                },
            ],
        })
    }

    #[test]
    fn lookups() {
        let r = sample();
        assert_eq!(r.modules_for_path("mobile", "app/auth/x/Token.kt").len(), 1);
        assert!(
            r.modules_for_path("backend", "app/auth/x/Token.kt")
                .is_empty()
        );
        let toks = crate::normalize::tokens("Исправить композицию");
        let hits = r.concept_alias_matches(&toks);
        assert_eq!(hits.len(), 2);
        assert_eq!(r.alias_table()["composition"].len(), 2);
        assert_eq!(
            r.concepts_for_path(Some("mobile"), "app/ui/Screen.kt")[0].id,
            "ui-composition"
        );
        assert!(r.validate().is_empty());
    }

    #[test]
    fn glob_qualifiers_are_checked_as_written() {
        let mut data = sample().data;
        data.modules[0].paths = vec!["Mobile:app/auth/**".into(), "app/a:b/**".into()];
        let d = Registry::new(data).validate();
        let got: Vec<_> = d
            .iter()
            .map(|x| (x.code.as_str(), &x.message[..]))
            .collect();
        assert_eq!(
            got,
            [(
                "REGISTRY_UNKNOWN_REPO",
                "`mobile.auth` glob `Mobile:app/auth/**` names unknown repo `Mobile`"
            )]
        );
    }
}
