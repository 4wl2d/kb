//! The query interface between the context engine and a knowledge snapshot, plus the
//! snapshot provenance types shared by `snapshot`, `index`, `context` and the CLI.
//!
//! Implementations: `index::IndexView` (SQLite, used by the CLI) and
//! `context::memory::MemoryView` (in-memory, used by tests and small working trees).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::diag::Diagnostic;
use crate::error::Result;
use crate::model::{Kind, ParsedRecord, ProfileConfig, RecordMeta, Registry};

/// Whether an entry belongs to the accepted snapshot or to the proposal overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    Accepted,
    Proposal,
}

/// A task path, qualified by the registry repo it belongs to (if known).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskPath {
    pub repo: Option<String>,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct MetaEntry {
    /// KB-root-relative source path of the record file.
    pub path: String,
    pub origin: Origin,
    pub meta: Arc<RecordMeta>,
}

#[derive(Debug, Clone)]
pub struct RecordEntry {
    pub path: String,
    pub origin: Origin,
    pub parsed: Arc<ParsedRecord>,
}

#[derive(Debug, Clone)]
pub struct RawEntry {
    pub path: String,
    pub origin: Origin,
    pub text: Arc<str>,
}

/// How a proposal relates to accepted knowledge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "kebab-case")]
pub enum ProposalChange {
    /// A new record id not present in the accepted snapshot.
    New { id: String },
    /// A changed version of an accepted record (the accepted one stays authoritative).
    Modifies { id: String },
    /// Removal of an accepted record file.
    Removes { id: String },
    /// A changed file that failed to parse.
    Invalid,
}

#[derive(Debug, Clone)]
pub struct ProposalEntry {
    pub path: String,
    pub change: ProposalChange,
    pub meta: Option<Arc<RecordMeta>>,
    /// True when the accepted file changed since the proposal's merge-base (stale proposal).
    pub stale: bool,
    pub diagnostics: Vec<Diagnostic>,
}

/// Read-only access to one snapshot (plus optional overlay).
///
/// All list results are ordered by record id (then path) so that callers are deterministic.
/// Methods taking `Origin::Accepted` never return proposals.
pub trait KnowledgeView {
    fn config(&self) -> &ProfileConfig;
    fn registry(&self) -> &Registry;
    /// Snapshot-level diagnostics (parse + validation of the accepted content).
    fn diagnostics(&self) -> &[Diagnostic];
    /// Metadata for every record of the given kinds (any status).
    fn metas_by_kind(&self, kinds: &[Kind], origin: Origin) -> Result<Vec<MetaEntry>>;
    /// Metadata for the given ids (missing ids are simply absent).
    fn metas_by_ids(&self, ids: &[String], origin: Origin) -> Result<Vec<MetaEntry>>;
    /// Accepted records whose `selectors.paths` might match any task path. May return a
    /// superset; callers re-check with [`crate::glob::RepoGlob`].
    fn path_candidates(&self, paths: &[TaskPath]) -> Result<Vec<MetaEntry>>;
    /// Accepted records whose `selectors.concepts` intersect `concepts`, or whose
    /// `selectors.aliases` occur in the normalized task `tokens`.
    fn term_candidates(&self, concepts: &[String], tokens: &[String]) -> Result<Vec<MetaEntry>>;
    /// Full-text search over accepted records: ids, best first, at most `limit`.
    /// `terms` are normalized tokens; implementations must quote them (no raw FTS syntax).
    fn fulltext(&self, terms: &[String], limit: usize) -> Result<Vec<String>>;
    /// Full parsed records.
    fn records(&self, ids: &[String], origin: Origin) -> Result<Vec<RecordEntry>>;
    /// Raw authoritative file text for one id.
    fn raw(&self, id: &str, origin: Origin) -> Result<Option<RawEntry>>;
    /// Proposal overlay entries (empty when proposals are not included).
    fn proposals(&self) -> Result<Vec<ProposalEntry>>;
}

// ---------------------------------------------------------------------------------------
// Snapshot provenance
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Freshness {
    /// The approved ref was fetched from the remote during this call.
    Verified,
    /// No remote check happened (explicit `--offline`).
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Selection {
    /// Tip of the approved ref.
    Latest,
    /// Host pin (submodule gitlink or `.kbw.toml pin`).
    Pinned,
    /// Explicit revision given by the user.
    Revision,
    /// Local working tree (never approved).
    WorkingTree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PinSource {
    Submodule,
    BindingFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinInfo {
    pub revision: String,
    pub source: PinSource,
    /// Host-relative submodule path when `source = submodule`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayInfo {
    pub digest: String,
    /// Merge-base the proposals are relative to.
    pub base: Option<String>,
    pub files: usize,
}

/// Where knowledge came from and how trustworthy/fresh it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub profile: String,
    /// Configured remote name.
    pub remote: String,
    /// Redacted remote URL (never contains credentials).
    pub source: String,
    pub approved_ref: String,
    pub selection: Selection,
    pub freshness: Freshness,
    /// Selected revision (`None` for the working tree).
    pub revision: Option<String>,
    /// Approved tip known to this call (verified or last known).
    pub latest_approved: Option<String>,
    /// Whether the selected revision is reachable from the approved tip (`None` = unknown).
    pub approved: Option<bool>,
    pub pin: Option<PinInfo>,
    pub overlay: Option<OverlayInfo>,
    pub engine_version: String,
    /// Deterministic snapshot identity (index key).
    pub key: String,
}

impl SnapshotInfo {
    /// Short label, e.g. `3f2a1c9 (latest, verified)`.
    pub fn label(&self) -> String {
        let rev = self
            .revision
            .as_deref()
            .map(|r| r.chars().take(12).collect::<String>())
            .unwrap_or_else(|| "working-tree".into());
        let sel = match self.selection {
            Selection::Latest => "latest",
            Selection::Pinned => "pinned",
            Selection::Revision => "revision",
            Selection::WorkingTree => "working-tree",
        };
        let fr = match self.freshness {
            Freshness::Verified => "verified",
            Freshness::Unverified => "unverified",
        };
        format!("{rev} ({sel}, freshness={fr})")
    }

    /// One-line provenance header of the text output of `show`, `search` and `impact`, in
    /// the wording of the `context` header: the label, whether the selected revision is
    /// approved, the approved tip known to this call, the host pin and the proposal overlay,
    /// e.g. `snapshot: 3f2a1c9e0b1d (pinned, freshness=unverified); approved=yes;
    /// latest=9c0d1e2f3a4b; pin=3f2a1c9e0b1d`.
    pub fn summary_line(&self) -> String {
        let short = |s: &str| s.chars().take(12).collect::<String>();
        let approved = match self.approved {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        };
        let mut line = format!("snapshot: {}; approved={approved}", self.label());
        if let Some(l) = &self.latest_approved {
            line.push_str(&format!("; latest={}", short(l)));
        }
        if let Some(p) = &self.pin {
            line.push_str(&format!("; pin={}", short(&p.revision)));
        }
        if let Some(ov) = &self.overlay {
            line.push_str(&format!(
                "; overlay={} ({} files)",
                short(&ov.digest),
                ov.files
            ));
        }
        line
    }

    /// True when the knowledge cannot be taken as verified approved content: freshness
    /// was not checked, or the selected revision is not (known to be) approved.
    pub fn needs_caution(&self) -> bool {
        self.freshness == Freshness::Unverified || self.approved != Some(true)
    }
}

// ---------------------------------------------------------------------------------------
// Proposal overlay (computed by `overlay`, indexed by `index`)
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayStatus {
    Added,
    Modified,
    Deleted,
}

/// One locally changed file under the profile's content prefixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayFile {
    /// KB-root-relative path.
    pub path: String,
    pub status: OverlayStatus,
    /// Working-tree bytes (None for deletions).
    pub content: Option<Vec<u8>>,
    /// `sha256:<hex>` of `content`.
    pub content_id: Option<String>,
    /// The approved snapshot changed this path since the proposal's merge-base.
    pub stale: bool,
}

/// Local proposals relative to `merge-base(HEAD, approved)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay {
    pub base: Option<String>,
    /// sha256 over sorted (path, status, content id); part of the snapshot key.
    pub digest: String,
    pub files: Vec<OverlayFile>,
}

impl Overlay {
    pub fn info(&self) -> OverlayInfo {
        OverlayInfo {
            digest: self.digest.clone(),
            base: self.base.clone(),
            files: self.files.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> SnapshotInfo {
        SnapshotInfo {
            profile: "project".into(),
            remote: "origin".into(),
            source: "/srv/kb.git".into(),
            approved_ref: "refs/heads/main".into(),
            selection: Selection::Pinned,
            freshness: Freshness::Unverified,
            revision: Some("a".repeat(40)),
            latest_approved: Some("b".repeat(40)),
            approved: Some(true),
            pin: Some(PinInfo {
                revision: "a".repeat(40),
                source: PinSource::Submodule,
                path: Some(".kb".into()),
            }),
            overlay: None,
            engine_version: "0.1.0".into(),
            key: "k".repeat(64),
        }
    }

    #[test]
    fn summary_line_names_revision_freshness_approval_tip_and_pin() {
        let mut i = info();
        assert_eq!(
            i.summary_line(),
            "snapshot: aaaaaaaaaaaa (pinned, freshness=unverified); approved=yes; \
             latest=bbbbbbbbbbbb; pin=aaaaaaaaaaaa"
        );
        assert!(i.needs_caution());
        i.freshness = Freshness::Verified;
        assert!(!i.needs_caution());
        i.selection = Selection::WorkingTree;
        i.revision = None;
        i.approved = Some(false);
        i.pin = None;
        i.latest_approved = None;
        i.overlay = Some(OverlayInfo {
            digest: "c".repeat(64),
            base: None,
            files: 2,
        });
        assert_eq!(
            i.summary_line(),
            "snapshot: working-tree (working-tree, freshness=verified); approved=no; \
             overlay=cccccccccccc (2 files)"
        );
        assert!(i.needs_caution());
    }
}
