//! Git-backed provenance adapters. Verification describes evidence, never semantic truth.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{KbError, Result};
use crate::git::Git;
use crate::host::{self, HostContext};
use crate::model::{Anchor, AnchorKind, AnchorStamp, Registry};
use crate::util::sha256_hex;

pub type HostRoots = BTreeMap<String, PathBuf>;
pub const MAX_ANCHOR_BYTES: u64 = 2 * 1024 * 1024;

pub fn anchor_root<'a>(
    kb_root: &'a Path,
    roots: &'a HostRoots,
    anchor: &Anchor,
) -> Result<&'a Path> {
    match &anchor.repo {
        Some(repo) => roots.get(repo).map(PathBuf::as_path).ok_or_else(|| {
            KbError::invalid_input(format!("no --repo-root or identified host for {repo}"))
        }),
        None if anchor.kind == AnchorKind::Doc => Ok(kb_root),
        None => Err(KbError::invalid_input(
            "anchor needs a repo to verify its Git evidence",
        )),
    }
}

/// Spelling only, with identifier boundaries. This deliberately does not claim that a
/// token in a comment, string or another scope is a resolved source definition.
pub fn symbol_spelling(text: &str, symbol: &str) -> bool {
    let identifier = |c: char| c.is_alphanumeric() || c == '_';
    !symbol.is_empty()
        && text.match_indices(symbol).any(|(at, _)| {
            text[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !identifier(c))
                && text[at + symbol.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !identifier(c))
        })
}

pub fn host_roots(
    host: Option<&HostContext>,
    registry: &Registry,
    repo: Option<&str>,
    explicit: &[String],
    cwd: &Path,
) -> Result<HostRoots> {
    let mut roots = HostRoots::new();
    if let Some(h) = host {
        let id = repo
            .map(str::to_string)
            .or_else(|| host::identify_repo(h, registry).map(|(id, _)| id));
        if let Some(id) = id {
            if registry.repo(&id).is_none() {
                return Err(KbError::invalid_input(format!("unknown host repo {id}")));
            }
            roots.insert(id, h.root.clone());
        }
    }
    for spec in explicit {
        let (id, path) = spec
            .split_once('=')
            .ok_or_else(|| KbError::invalid_input("--repo-root must be REPO=DIR"))?;
        if registry.repo(id).is_none() {
            return Err(KbError::invalid_input(format!("unknown host repo {id}")));
        }
        let path = Path::new(path);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        let root = crate::git::toplevel(&path).ok_or_else(|| {
            KbError::invalid_input(format!("{} is not a Git work tree", path.display()))
        })?;
        if let Some(previous) = roots.insert(id.into(), root.clone())
            && previous != root
        {
            return Err(KbError::invalid_input(format!(
                "conflicting roots for repo {id}"
            )));
        }
    }
    Ok(roots)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnchorStatus {
    Verified,
    Changed,
    Missing,
    Unverifiable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorEvidence {
    pub record: String,
    pub index: usize,
    pub anchor: Anchor,
    pub status: AnchorStatus,
    pub resolved_commit: Option<String>,
    pub stamp_verified: Option<bool>,
    pub detail: String,
}

/// Source symbols are a lexical check without a provider. File/commit and stamped bytes
/// are exact; a successful check is not proof of a definition, executed test or claim.
pub fn inspect(
    kb_root: &Path,
    roots: &HostRoots,
    record: &str,
    index: usize,
    anchor: &Anchor,
    at: Option<&str>,
) -> AnchorEvidence {
    let mut evidence = AnchorEvidence {
        record: record.into(),
        index,
        anchor: anchor.clone(),
        status: AnchorStatus::Unverifiable,
        resolved_commit: None,
        stamp_verified: None,
        detail: String::new(),
    };
    let checked = (|| -> Result<()> {
        let root = anchor_root(kb_root, roots, anchor)?;
        let git = Git::new(root);
        let override_at = if anchor.kind == AnchorKind::Change && anchor.path.is_none() {
            None
        } else {
            at
        };
        let revision = override_at
            .or(anchor.commit.as_deref())
            .or(anchor.stamp.as_ref().map(|s| s.commit.as_str()))
            .unwrap_or("HEAD");
        let Some(commit) = git.resolve_commit(revision)? else {
            evidence.status = AnchorStatus::Missing;
            evidence.detail = format!("commit {revision} is missing or ambiguous");
            return Ok(());
        };
        evidence.resolved_commit = Some(commit.clone());
        let Some(path) = &anchor.path else {
            if anchor.kind == AnchorKind::Change && anchor.commit.is_some() {
                evidence.status = AnchorStatus::Verified;
                evidence.detail = "change commit exists; review acceptance is not inferred".into();
                return Ok(());
            }
            return Err(KbError::invalid_input(
                "external change reference is not locally verifiable",
            ));
        };
        let Some(bytes) = host::facts::blob_at(root, &commit, path, MAX_ANCHOR_BYTES)? else {
            evidence.status = AnchorStatus::Missing;
            evidence.detail = format!("{path} is absent at {commit}");
            return Ok(());
        };
        if let Some(symbol) = &anchor.symbol
            && anchor.stamp.is_none()
        {
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| KbError::invalid_input("symbol anchor is not UTF-8 text"))?;
            if !symbol_spelling(text, symbol) {
                evidence.status = AnchorStatus::Missing;
                evidence.detail = format!(
                    "symbol spelling {symbol:?} is absent; a provider is needed for qualified-symbol resolution"
                );
                return Ok(());
            }
        }
        if let Some(stamp) = &anchor.stamp {
            let selected = match line_span(&bytes, stamp.start_line, stamp.end_line) {
                Ok(span) => span,
                Err(error) if !commit.starts_with(&stamp.commit) => {
                    evidence.status = AnchorStatus::Changed;
                    evidence.stamp_verified = Some(false);
                    evidence.detail = format!("stamped range no longer exists: {}", error.message);
                    return Ok(());
                }
                Err(error) => return Err(error),
            };
            let matched = sha256_hex(selected) == stamp.sha256;
            evidence.stamp_verified = Some(matched);
            evidence.status = if matched {
                AnchorStatus::Verified
            } else {
                AnchorStatus::Changed
            };
            evidence.detail = if matched {
                "stamped source span matches; symbol identity and claim semantics are not re-evaluated"
            } else {
                "stamped source span changed"
            }
            .into();
        } else {
            evidence.status = AnchorStatus::Verified;
            evidence.detail = "file/commit exist; no drift stamp (symbols, when present, are lexical matches only)".into();
        }
        Ok(())
    })();
    if let Err(error) = checked {
        evidence.detail = error.message;
    }
    evidence
}

/// Exact bytes for an inclusive, one-based span, preserving line endings. Empty files
/// have one empty line so their content can be stamped without inventing a byte.
pub fn line_span(bytes: &[u8], start: u32, end: u32) -> Result<&[u8]> {
    if start == 0 || end < start {
        return Err(KbError::invalid_input("invalid source line span"));
    }
    if bytes.is_empty() {
        return if start == 1 && end == 1 {
            Ok(bytes)
        } else {
            Err(KbError::invalid_input(
                "source line span is outside empty file",
            ))
        };
    }
    let mut begin = None;
    let mut offset = 0;
    for (line, chunk) in (1..).zip(bytes.split_inclusive(|b| *b == b'\n')) {
        if line == start {
            begin = Some(offset);
        }
        offset += chunk.len();
        if line == end {
            return begin
                .map(|a| &bytes[a..offset])
                .ok_or_else(|| KbError::invalid_input("source line span starts after its end"));
        }
    }
    Err(KbError::invalid_input(
        "source line span is outside the file",
    ))
}

pub fn whole_file_stamp(commit: String, bytes: &[u8]) -> Result<AnchorStamp> {
    std::str::from_utf8(bytes)
        .map_err(|_| KbError::invalid_input("stamping requires UTF-8 source text"))?;
    let lines = bytes.split_inclusive(|b| *b == b'\n').count().max(1);
    Ok(AnchorStamp {
        commit,
        start_line: 1,
        end_line: u32::try_from(lines)
            .map_err(|_| KbError::invalid_input("too many source lines"))?,
        sha256: sha256_hex(bytes),
    })
}
