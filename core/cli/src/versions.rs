//! Compiled-in contract versions and the bootstrap manifest (`core/release.toml`).

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{ErrorCode, KbError, Result};

pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MANIFEST_VERSION: u32 = 1;
pub const DOCUMENT_SCHEMA: u32 = 2;
pub const OLDEST_READABLE_DOCUMENT_SCHEMA: u32 = 1;
pub const PROTOCOL: u32 = 1;
pub const PROTOCOL_ID: &str = "kb.cli.v1";
pub const INDEX_SCHEMA: u32 = 2;
pub const SKILL_PROTOCOL: u32 = 2;
/// Bumped whenever parsing output for identical bytes changes (invalidates parse caches).
pub const PARSER_VERSION: u32 = 6;

pub fn supports_document_schema(version: u32) -> bool {
    (OLDEST_READABLE_DOCUMENT_SCHEMA..=DOCUMENT_SCHEMA).contains(&version)
}

/// Engine fingerprint the launcher passed to this build (`KBW_BUILD_FINGERPRINT`, set by
/// `kbw` for source builds), or "unknown" for a build made outside the launcher. As an
/// `option_env!` it is a tracked build input: cargo recompiles the engine whenever the
/// launcher's fingerprint changes, even when source mtimes look fresh, and `kbw` accepts a
/// binary as the runtime of a fingerprint only if `kb --json version` reports it.
pub const BUILD_FINGERPRINT: &str = match option_env!("KBW_BUILD_FINGERPRINT") {
    Some(fp) if !fp.is_empty() => fp,
    _ => "unknown",
};

/// Path of the manifest relative to the KB root.
pub const MANIFEST_PATH: &str = "core/release.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    /// Bootstrap manifest version (must be 1).
    #[schemars(extend("const" = 1))]
    pub manifest: u32,
    pub engine_version: String,
    pub document_schema: u32,
    pub protocol: u32,
    pub index_schema: u32,
    pub skill_protocol: u32,
    #[serde(default)]
    pub migrates_from: Vec<u32>,
    pub rust_toolchain: String,
    pub min_git: String,
    pub engine_paths: Vec<String>,
    pub project_paths: Vec<String>,
}

impl ReleaseManifest {
    pub fn parse(text: &str) -> Result<Self> {
        let m: ReleaseManifest = toml::from_str(text).map_err(|e| {
            KbError::new(
                ErrorCode::RuntimeIncompatible,
                format!("cannot parse {MANIFEST_PATH}: {e}"),
            )
        })?;
        if m.manifest != MANIFEST_VERSION {
            return Err(KbError::new(
                ErrorCode::RuntimeIncompatible,
                format!(
                    "unsupported bootstrap manifest version {} (this engine understands {})",
                    m.manifest, MANIFEST_VERSION
                ),
            ));
        }
        Ok(m)
    }

    /// The manifest this binary was built for.
    pub fn compiled() -> CompiledVersions {
        CompiledVersions::current()
    }

    /// Differences between this manifest and the running engine, as (field, manifest, runtime).
    pub fn mismatches(&self, rt: &CompiledVersions) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        let mut cmp = |field: &str, a: String, b: String| {
            if a != b {
                out.push((field.to_string(), a, b));
            }
        };
        cmp(
            "engine_version",
            self.engine_version.clone(),
            rt.engine_version.to_string(),
        );
        cmp(
            "document_schema",
            self.document_schema.to_string(),
            rt.document_schema.to_string(),
        );
        cmp(
            "protocol",
            self.protocol.to_string(),
            rt.protocol.to_string(),
        );
        cmp(
            "index_schema",
            self.index_schema.to_string(),
            rt.index_schema.to_string(),
        );
        cmp(
            "skill_protocol",
            self.skill_protocol.to_string(),
            rt.skill_protocol.to_string(),
        );
        out
    }
}

/// Versions compiled into this binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompiledVersions {
    pub engine_version: &'static str,
    pub document_schema: u32,
    pub protocol: u32,
    pub index_schema: u32,
    pub skill_protocol: u32,
    pub parser_version: u32,
    /// The launcher fingerprint of the engine inputs this binary was built from.
    pub build_fingerprint: &'static str,
}

impl CompiledVersions {
    pub fn current() -> Self {
        CompiledVersions {
            engine_version: ENGINE_VERSION,
            document_schema: DOCUMENT_SCHEMA,
            protocol: PROTOCOL,
            index_schema: INDEX_SCHEMA,
            skill_protocol: SKILL_PROTOCOL,
            parser_version: PARSER_VERSION,
            build_fingerprint: BUILD_FINGERPRINT,
        }
    }
}

/// Verify that the local manifest (the engine checkout this binary serves) matches the
/// running binary. A mismatch means the runtime cache is stale or a foreign binary is used.
pub fn check_runtime(local: &ReleaseManifest) -> Result<()> {
    let rt = CompiledVersions::current();
    let mm = local.mismatches(&rt);
    if mm.is_empty() {
        return Ok(());
    }
    Err(KbError::new(
        ErrorCode::RuntimeIncompatible,
        "the running kb binary does not match the local engine manifest",
    )
    .with_details(json!({
        "mismatches": mm.iter().map(|(f, m, r)| json!({"field": f, "manifest": m, "runtime": r})).collect::<Vec<_>>()
    }))
    .with_hint("run kb through the project launcher (./kbw), which runs the runtime built for this checkout's engine; kbw never builds implicitly: a missing runtime is reported as KBW_RUNTIME_NOT_BOOTSTRAPPED and is created explicitly with `./kbw --kbw-bootstrap` or `./kbw --kbw-install-artifact`"))
}

/// Verify that a snapshot's manifest is served by the running engine. Any difference in
/// engine version or contract versions means the snapshot needs a coordinated update.
pub fn check_snapshot_compat(snapshot: &ReleaseManifest, revision: &str) -> Result<()> {
    let rt = CompiledVersions::current();
    let mm: Vec<_> = snapshot
        .mismatches(&rt)
        .into_iter()
        .filter(|(f, _, _)| f != "skill_protocol")
        .collect();
    if mm.is_empty() {
        return Ok(());
    }
    Err(KbError::new(
        ErrorCode::UpdateRequired,
        format!("snapshot {revision} requires a different engine than the running runtime"),
    )
    .with_details(json!({
        "revision": revision,
        "mismatches": mm.iter().map(|(f, m, r)| json!({"field": f, "snapshot": m, "runtime": r})).collect::<Vec<_>>()
    }))
    .with_hint("update the KB checkout to that revision through review, then bootstrap its engine explicitly with `./kbw --kbw-bootstrap` or `./kbw --kbw-install-artifact` (kbw never builds implicitly), or select --snapshot pinned"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_manifest() -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../release.toml")).unwrap()
    }

    #[test]
    fn repository_manifest_matches_compiled_versions() {
        let m = ReleaseManifest::parse(&repo_manifest()).unwrap();
        assert!(m.mismatches(&CompiledVersions::current()).is_empty());
        check_runtime(&m).unwrap();
    }

    #[test]
    fn toolchain_pin_matches_rust_toolchain_file() {
        let m = ReleaseManifest::parse(&repo_manifest()).unwrap();
        let tc = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../rust-toolchain.toml"
        ))
        .unwrap();
        let v: toml::Table = toml::from_str(&tc).unwrap();
        assert_eq!(
            v["toolchain"]["channel"].as_str().unwrap(),
            m.rust_toolchain
        );
    }

    #[test]
    fn snapshot_with_other_engine_requires_update() {
        let mut m = ReleaseManifest::parse(&repo_manifest()).unwrap();
        m.engine_version = "99.0.0".into();
        let err = check_snapshot_compat(&m, "abc").unwrap_err();
        assert_eq!(err.code, ErrorCode::UpdateRequired);
    }

    #[test]
    fn version_output_carries_the_build_fingerprint() {
        let v = serde_json::to_value(CompiledVersions::current()).unwrap();
        assert_eq!(v["build_fingerprint"], BUILD_FINGERPRINT);
        assert!(!BUILD_FINGERPRINT.is_empty());
    }

    #[test]
    fn recovery_hints_name_the_explicit_bootstrap() {
        let mut m = ReleaseManifest::parse(&repo_manifest()).unwrap();
        m.engine_version = "99.0.0".into();
        let errors = [
            check_runtime(&m).unwrap_err(),
            check_snapshot_compat(&m, "abc").unwrap_err(),
        ];
        for err in errors {
            let hint = err.hint.unwrap();
            assert!(hint.contains("./kbw --kbw-bootstrap"), "{hint}");
            assert!(hint.contains("never builds implicitly"), "{hint}");
        }
    }

    #[test]
    fn unknown_manifest_fields_rejected() {
        let text = repo_manifest() + "\nsurprise = 1\n";
        assert!(ReleaseManifest::parse(&text).is_err());
    }
}
