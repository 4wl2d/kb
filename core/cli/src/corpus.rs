//! Loading a profile (config, registry, records) from any [`SourceTree`] into memory.
//!
//! Used by `validate` (working tree), by index builds, and by tests. Warm context queries
//! do not load a whole corpus; they go through the SQLite index.

use std::sync::Arc;

use crate::diag::{Diagnostic, normalize};
use crate::error::{ErrorCode, KbError, Result};
use crate::model::registry::{
    ChangeTypesFile, ConceptsFile, FeaturesFile, ModulesFile, OwnersFile, ReposFile,
    parse_registry_file,
};
use crate::model::{ParsedRecord, ProfileConfig, ProfileLocation, Registry, RegistryData};
use crate::parse::parse_record;
use crate::source::{SourceEntry, SourceTree};

/// One record file in a corpus.
#[derive(Debug, Clone)]
pub struct CorpusEntry {
    pub path: String,
    pub content_id: String,
    /// Parsed record, or `None` when parsing failed (see corpus diagnostics).
    pub parsed: Option<Arc<ParsedRecord>>,
    /// Raw file text (authoritative bytes for `show --raw`).
    pub raw: Arc<str>,
}

/// A fully loaded profile.
#[derive(Debug, Clone)]
pub struct Corpus {
    pub location: ProfileLocation,
    pub config: ProfileConfig,
    pub registry: Registry,
    pub entries: Vec<CorpusEntry>,
    /// Parse, registry and source diagnostics (validation adds more).
    pub diagnostics: Vec<Diagnostic>,
}

impl Corpus {
    /// Parsed records (successfully parsed entries only), in path order.
    pub fn records(&self) -> impl Iterator<Item = (&CorpusEntry, &Arc<ParsedRecord>)> {
        self.entries
            .iter()
            .filter_map(|e| e.parsed.as_ref().map(|p| (e, p)))
    }
}

/// Load and strictly parse the profile config.
pub fn load_config(source: &dyn SourceTree, loc: &ProfileLocation) -> Result<ProfileConfig> {
    let bytes = source
        .read_path(&loc.config)?
        .ok_or_else(|| match loc.profile {
            crate::model::Profile::Project => KbError::new(
                ErrorCode::ProjectNotInitialized,
                format!(
                    "project is not initialized: `{}` does not exist",
                    loc.config
                ),
            )
            .with_hint(
                "run `kbw init --name <name> --namespace <ns>` (dry-run) and then with `--apply`",
            ),
            crate::model::Profile::Maintainer => KbError::new(
                ErrorCode::ConfigInvalid,
                format!("maintainer profile config `{}` is missing", loc.config),
            ),
        })?;
    parse_config(&loc.config, &bytes)
}

pub fn parse_config(path: &str, bytes: &[u8]) -> Result<ProfileConfig> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| KbError::new(ErrorCode::ConfigInvalid, format!("`{path}` is not UTF-8")))?;
    if let Ok(t) = toml::from_str::<toml::Table>(text)
        && let Some(v) = t.get("schema").and_then(|v| v.as_integer())
        && !u32::try_from(v).is_ok_and(crate::versions::supports_document_schema)
    {
        return Err(KbError::new(
            ErrorCode::UnsupportedSchemaVersion,
            format!(
                "`{path}` has schema {v}; this engine supports {}",
                crate::versions::DOCUMENT_SCHEMA
            ),
        )
        .with_hint("run `kbw migrate` to see available migrations"));
    }
    let cfg: ProfileConfig = toml::from_str(text)
        .map_err(|e| KbError::new(ErrorCode::ConfigInvalid, format!("`{path}`: {e}")))?;
    let mut problems = Vec::new();
    if let Err(e) = crate::model::ids::check_namespace(&cfg.project.namespace) {
        problems.push(format!("project.namespace: {e}"));
    }
    if !cfg.source.approved_ref.starts_with("refs/") || cfg.source.approved_ref.contains("..") {
        problems.push(
            "source.approved_ref must be a fully qualified ref such as `refs/heads/main`".into(),
        );
    }
    if cfg.source.remote.is_empty()
        || cfg.source.remote.starts_with('-')
        || cfg.source.remote.contains(char::is_whitespace)
    {
        problems.push("source.remote must be a Git remote name".into());
    }
    if let Err(e) = crate::git::transport_policy(&cfg.source.allowed_protocols) {
        problems.push(e.message);
    }
    if cfg.knowledge.roots.is_empty() {
        problems.push("knowledge.roots must not be empty".into());
    }
    for r in &cfg.knowledge.roots {
        if let Err(e) = crate::util::check_rel_path(r.trim_end_matches('/')) {
            problems.push(format!("knowledge.roots: {e}"));
        }
        if r.starts_with("registry")
            || r.starts_with("routing-tests")
            || r.starts_with("skill-config")
        {
            problems.push(format!(
                "knowledge.roots: `{r}` overlaps a reserved profile directory"
            ));
        }
    }
    if cfg.context.default_budget == 0 {
        problems.push("context.default_budget must be positive".into());
    }
    if !problems.is_empty() {
        return Err(KbError::new(
            ErrorCode::ConfigInvalid,
            format!("`{path}`: {}", problems.join("; ")),
        ));
    }
    Ok(cfg)
}

/// Load the registry from `<profile>/registry/*.toml`; missing files are empty.
pub fn load_registry(
    source: &dyn SourceTree,
    loc: &ProfileLocation,
) -> Result<(Registry, Vec<Diagnostic>)> {
    let dir = loc.registry_dir();
    let mut diags = Vec::new();
    let mut data = RegistryData::default();
    macro_rules! load {
        ($file:literal, $ty:ty, $field:ident, $list:ident) => {{
            let path = format!("{dir}/{}", $file);
            if let Some(bytes) = source.read_path(&path)? {
                match std::str::from_utf8(&bytes) {
                    Ok(text) => match parse_registry_file::<$ty>(&path, text) {
                        Ok(f) => {
                            if !crate::versions::supports_document_schema(f.schema)
                                || ($file == "change-types.toml" && f.schema != 2)
                            {
                                diags.push(
                                    Diagnostic::error(
                                        "UNSUPPORTED_SCHEMA_VERSION",
                                        format!(
                                            "registry file schema {} is not supported",
                                            f.schema
                                        ),
                                    )
                                    .at_path(path.clone()),
                                );
                            } else {
                                data.$field = f.$list;
                            }
                        }
                        Err(d) => diags.push(d),
                    },
                    Err(_) => diags.push(
                        Diagnostic::error("REGISTRY_PARSE", "not UTF-8").at_path(path.clone()),
                    ),
                }
            }
        }};
    }
    load!("owners.toml", OwnersFile, owners, owner);
    load!("repos.toml", ReposFile, repos, repo);
    load!("modules.toml", ModulesFile, modules, module);
    load!("features.toml", FeaturesFile, features, feature);
    load!("concepts.toml", ConceptsFile, concepts, concept);
    load!(
        "change-types.toml",
        ChangeTypesFile,
        change_types,
        change_type
    );
    let registry = Registry::new(data);
    diags.extend(registry.validate().into_iter().map(|mut d| {
        if let Some(p) = &d.path {
            d.path = Some(format!("{}/{}", loc.dir, p));
        }
        d
    }));
    Ok((registry, diags))
}

/// Is this path a record file (under a knowledge root, `.md`, not `README.md`)?
pub fn is_record_path(path: &str) -> bool {
    path.ends_with(".md")
        && !path
            .rsplit('/')
            .next()
            .is_some_and(|f| f.eq_ignore_ascii_case("README.md"))
}

/// List record files of a profile.
pub fn list_records(
    source: &dyn SourceTree,
    loc: &ProfileLocation,
    cfg: &ProfileConfig,
) -> Result<(Vec<SourceEntry>, Vec<Diagnostic>)> {
    let (entries, issues) = source.list(&loc.knowledge_roots(cfg))?;
    let mut diags: Vec<Diagnostic> = issues
        .into_iter()
        .map(|i| Diagnostic::error(i.code, i.message).at_path(i.path))
        .collect();
    let mut records = Vec::new();
    for e in entries {
        if is_record_path(&e.path) {
            records.push(e);
        } else if !e
            .path
            .rsplit('/')
            .next()
            .is_some_and(|f| f.eq_ignore_ascii_case("README.md") || f == ".gitkeep")
        {
            diags.push(
                Diagnostic::warning(
                    "NON_RECORD_FILE",
                    "ignored: only `.md` records are read from knowledge roots",
                )
                .at_path(e.path.clone()),
            );
        }
    }
    Ok((records, diags))
}

/// Load a whole profile from a source.
pub fn load_corpus(source: &dyn SourceTree, loc: &ProfileLocation) -> Result<Corpus> {
    let config = load_config(source, loc)?;
    let (registry, mut diagnostics) = load_registry(source, loc)?;
    let (files, list_diags) = list_records(source, loc, &config)?;
    diagnostics.extend(list_diags);
    let contents = source.read(&files)?;
    let mut entries = Vec::with_capacity(files.len());
    for (f, bytes) in files.into_iter().zip(contents) {
        let raw: Arc<str> = Arc::from(String::from_utf8_lossy(&bytes).as_ref());
        let parsed = match parse_record(&f.path, &bytes) {
            Ok(p) => Some(Arc::new(p)),
            Err(ds) => {
                diagnostics.extend(ds);
                None
            }
        };
        entries.push(CorpusEntry {
            path: f.path,
            content_id: f.content_id,
            parsed,
            raw,
        });
    }
    normalize(&mut diagnostics);
    Ok(Corpus {
        location: loc.clone(),
        config,
        registry,
        entries,
        diagnostics,
    })
}
