//! Append-only local delivery observations. No task text, source snippets or network.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::context::{ContextResult, TaskScope, UnitBody, Verdict};
use crate::error::{KbError, Result};
use crate::impact::HostDiff;
use crate::model::{Applicability, BudgetUnit, Registry, Scope, Status};
use crate::util::safe_join;

const LOG: &str = "usage/delivery.v1.jsonl";
const MAX_LOG: u64 = 256 * 1024 * 1024;
const MAX_LINE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub repo: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveredUnit {
    pub id: String,
    pub record: String,
    pub tier: String,
    pub delivery: String,
    pub knowledge: bool,
    pub code: bool,
    pub scope: Option<Scope>,
    pub applicability: Option<Applicability>,
    pub domain_features: Vec<String>,
    pub locations: Vec<Location>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub protocol: String,
    pub subject: String,
    pub receipt: String,
    pub repo: Option<String>,
    pub host_head: Option<String>,
    pub snapshot_content_digest: Option<String>,
    pub budget_used: u64,
    pub budget_unit: BudgetUnit,
    pub units: Vec<DeliveredUnit>,
    pub requires: Vec<(String, String)>,
}

impl Call {
    fn from_context(subject: &str, result: &ContextResult) -> Self {
        let records: BTreeMap<_, _> = result
            .units
            .iter()
            .filter_map(|u| match &u.body {
                UnitBody::Record(record) => Some((u.id.as_str(), record)),
                _ => None,
            })
            .collect();
        let units = result
            .units
            .iter()
            .map(|unit| {
                let parent = if matches!(unit.body, UnitBody::Section(_)) {
                    unit.id
                        .rsplit_once('#')
                        .map(|(id, _)| id)
                        .unwrap_or(&unit.id)
                } else {
                    &unit.id
                };
                let record = records.get(parent);
                let mut locations = Vec::new();
                let mut domain_features = Vec::new();
                let scope = record.map(|r| r.record.common().scope.clone());
                let applicability = record.and_then(|r| r.record.common().applicability.cloned());
                if let Some(record) = record {
                    let meta = record.meta();
                    domain_features = meta.scope.features;
                    domain_features.extend(meta.feature);
                    for anchor in meta.anchors {
                        if let (Some(repo), Some(path)) = (anchor.repo, anchor.path) {
                            locations.push(Location { repo, path });
                        }
                    }
                    locations.extend(meta.consumers.into_iter().map(|c| Location {
                        repo: c.repo,
                        path: c.path,
                    }));
                }
                if let UnitBody::Code(code) = &unit.body {
                    locations.push(Location {
                        repo: code.repo.clone(),
                        path: code.symbol.path.clone(),
                    });
                }
                domain_features.sort();
                domain_features.dedup();
                DeliveredUnit {
                    id: unit.id.clone(),
                    record: unit.record_id.clone(),
                    tier: unit.tier.as_str().into(),
                    delivery: unit.reuse.map(|r| r.as_str()).unwrap_or("full").into(),
                    knowledge: unit.origin == crate::knowledge::Origin::Accepted
                        && unit.status == Status::Accepted,
                    code: matches!(unit.body, UnitBody::Code(_)),
                    scope,
                    applicability,
                    domain_features,
                    locations,
                }
            })
            .collect();
        Self {
            protocol: "kb.usage.v1".into(),
            subject: subject.into(),
            receipt: result.receipt_id().into(),
            repo: result.header.scope.host_repo.clone(),
            host_head: result.header.scope.host_head.clone(),
            snapshot_content_digest: result.header.snapshot.content_digest.clone(),
            budget_used: result.footer.budget.used,
            budget_unit: result.footer.budget.unit,
            units,
            requires: result.requires.clone(),
        }
    }
}

fn lock(file: &File, shared: bool) -> Result<()> {
    let start = Instant::now();
    loop {
        let result = if shared {
            file.try_lock_shared()
        } else {
            file.try_lock()
        };
        match result {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) if start.elapsed() < Duration::from_secs(5) => {
                std::thread::park_timeout(Duration::from_millis(10))
            }
            Err(error) => {
                return Err(KbError::invalid_input(format!(
                    "local usage log lock unavailable: {error}"
                )));
            }
        }
    }
}

pub fn append(cache: &Path, subject: &str, result: &ContextResult) -> Result<()> {
    append_call(cache, &Call::from_context(subject, result))
}

fn append_call(cache: &Path, call: &Call) -> Result<()> {
    let bytes = serde_json::to_vec(call)?;
    if bytes.len() + 1 > MAX_LINE {
        return Err(KbError::invalid_input("usage entry exceeds 4 MiB"));
    }
    let path = safe_join(cache, LOG)?;
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|e| KbError::io("usage directory", e))?;
    let mut options = OpenOptions::new();
    options.create(true).read(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|e| KbError::io("usage log", e))?;
    lock(&file, false)?;
    // Preserve an interrupted final line and start the new observation on a fresh line.
    if file
        .metadata()
        .map_err(|e| KbError::io("usage metadata", e))?
        .len()
        > 0
    {
        file.seek(SeekFrom::End(-1))
            .map_err(|e| KbError::io("usage tail", e))?;
        let mut last = [0];
        file.read_exact(&mut last)
            .map_err(|e| KbError::io("usage tail", e))?;
        if last[0] != b'\n' {
            file.write_all(b"\n")
                .map_err(|e| KbError::io("usage separator", e))?;
        }
    }
    let mut line = bytes;
    line.push(b'\n');
    file.write_all(&line)
        .and_then(|_| file.sync_data())
        .map_err(|e| KbError::io("append usage log", e))
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadLog {
    pub calls: Vec<Call>,
    pub invalid_lines: Vec<usize>,
}

pub fn read(cache: &Path, subject: &str, receipts: &BTreeSet<String>) -> Result<ReadLog> {
    let path = safe_join(cache, LOG)?;
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ReadLog {
                calls: Vec::new(),
                invalid_lines: Vec::new(),
            });
        }
        Err(e) => return Err(KbError::io("read usage log", e)),
    };
    lock(&file, true)?;
    if file
        .metadata()
        .map_err(|e| KbError::io("usage metadata", e))?
        .len()
        > MAX_LOG
    {
        return Err(KbError::invalid_input(
            "usage log exceeds the 256 MiB report limit; use an explicit preserved subset for analysis",
        ));
    }
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut calls = Vec::new();
    let mut invalid_lines = Vec::new();
    let mut index = 0;
    loop {
        line.clear();
        let count = reader
            .read_until(b'\n', &mut line)
            .map_err(|e| KbError::io("usage line", e))?;
        if count == 0 {
            break;
        }
        index += 1;
        if count > MAX_LINE || line.last() != Some(&b'\n') {
            invalid_lines.push(index);
            continue;
        }
        let Ok(call) = serde_json::from_slice::<Call>(&line) else {
            invalid_lines.push(index);
            continue;
        };
        if call.protocol != "kb.usage.v1"
            || call.units.len() > 100_000
            || call.budget_used > 1_000_000_000
        {
            invalid_lines.push(index);
            continue;
        }
        if call.subject == subject && (receipts.is_empty() || receipts.contains(&call.receipt)) {
            calls.push(call);
        }
    }
    Ok(ReadLog {
        calls,
        invalid_lines,
    })
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DomainDelivery {
    pub knowledge: BTreeSet<String>,
    pub code: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageReport {
    pub calls: usize,
    pub receipts: Vec<String>,
    pub tokens_est: u64,
    pub bytes: u64,
    pub delivered_but_irrelevant: BTreeMap<String, usize>,
    pub unclassified: BTreeSet<String>,
    pub domain_delivery: BTreeMap<String, DomainDelivery>,
    pub touched_but_undelivered: Vec<String>,
    pub without_domain_knowledge: Vec<String>,
    pub unmapped_paths: Vec<String>,
    pub invalid_log_lines: Vec<usize>,
    pub different_snapshot_calls: usize,
    pub note: &'static str,
}

pub fn report(
    log: &ReadLog,
    repo: &str,
    diff: &HostDiff,
    registry: &Registry,
    task: &TaskScope,
    snapshot_digest: Option<&str>,
) -> UsageReport {
    let touched: BTreeSet<_> = diff
        .files
        .iter()
        .flat_map(|f| std::iter::once(&f.path).chain(f.old_path.iter()))
        .collect();
    let modules: BTreeSet<_> = touched
        .iter()
        .flat_map(|path| registry.modules_for_path(repo, path))
        .map(|m| m.id.clone())
        .collect();
    let mut domain_delivery: BTreeMap<_, _> = modules
        .iter()
        .map(|id| (id.clone(), DomainDelivery::default()))
        .collect();
    let mut delivered_but_irrelevant = BTreeMap::new();
    let mut unclassified = BTreeSet::new();
    let mut receipts = BTreeSet::new();
    let mut tokens_est = 0;
    let mut bytes = 0;
    let mut different_snapshot_calls = 0;
    for call in &log.calls {
        receipts.insert(call.receipt.clone());
        if call.snapshot_content_digest.as_deref() != snapshot_digest {
            different_snapshot_calls += 1;
        }
        match call.budget_unit {
            BudgetUnit::TokensEst => tokens_est += call.budget_used,
            BudgetUnit::Bytes => bytes += call.budget_used,
        }
        let mut relevant: BTreeSet<String> = call
            .units
            .iter()
            .filter(|u| {
                u.scope.as_ref().is_some_and(|scope| {
                    crate::context::evaluate_saved_scope(scope, u.applicability.as_ref(), task)
                        .verdict()
                        == Verdict::Applies
                })
            })
            .map(|u| u.id.clone())
            .collect();
        loop {
            let mut added = false;
            for (from, to) in &call.requires {
                if relevant.contains(from) {
                    added |= relevant.insert(to.clone());
                }
            }
            if !added {
                break;
            }
        }
        for unit in &call.units {
            let parent = (unit.tier == "section")
                .then(|| unit.id.rsplit_once('#').map(|(id, _)| id))
                .flatten();
            let relevant_unit =
                relevant.contains(&unit.id) || parent.is_some_and(|id| relevant.contains(id));
            if !relevant_unit {
                let verdict = unit.scope.as_ref().map(|scope| {
                    crate::context::evaluate_saved_scope(scope, unit.applicability.as_ref(), task)
                        .verdict()
                });
                if verdict == Some(Verdict::NotApplicable) {
                    *delivered_but_irrelevant.entry(unit.id.clone()).or_default() += 1;
                } else {
                    unclassified.insert(unit.id.clone());
                }
            }
            let mut domains: BTreeSet<_> = unit
                .scope
                .as_ref()
                .map(|s| s.modules.iter().cloned().collect())
                .unwrap_or_default();
            for module in &registry.data.modules {
                if module.repo == repo
                    && module
                        .features
                        .iter()
                        .any(|f| unit.domain_features.contains(f))
                {
                    domains.insert(module.id.clone());
                }
            }
            for location in &unit.locations {
                if location.repo == repo {
                    domains.extend(
                        registry
                            .modules_for_path(repo, &location.path)
                            .iter()
                            .map(|m| m.id.clone()),
                    );
                }
            }
            for id in domains.intersection(&modules) {
                let delivery = domain_delivery.get_mut(id).unwrap();
                if unit.knowledge && relevant_unit {
                    delivery.knowledge.insert(unit.record.clone());
                }
                if unit.code {
                    delivery.code.insert(unit.id.clone());
                }
            }
        }
    }
    let touched_but_undelivered = domain_delivery
        .iter()
        .filter(|(_, v)| v.knowledge.is_empty() && v.code.is_empty())
        .map(|(id, _)| id.clone())
        .collect();
    let without_domain_knowledge = domain_delivery
        .iter()
        .filter(|(_, v)| v.knowledge.is_empty())
        .map(|(id, _)| id.clone())
        .collect();
    let unmapped_paths = touched
        .iter()
        .filter(|p| registry.modules_for_path(repo, p).is_empty())
        .map(|p| (*p).clone())
        .collect();
    UsageReport {
        calls: log.calls.len(),
        receipts: receipts.into_iter().collect(),
        tokens_est,
        bytes,
        delivered_but_irrelevant,
        unclassified,
        domain_delivery,
        touched_but_undelivered,
        without_domain_knowledge,
        unmapped_paths,
        invalid_log_lines: log.invalid_lines.clone(),
        different_snapshot_calls,
        note: "Local delivery observations only, not model consultation or compliance. 'Irrelevant' means outside the final declared scope; it is a proxy, not a semantic judgment. Required dependencies remain relevant. Product/repo-wide rules do not mask missing domain knowledge. Code consumers may matter without being edited. Use explicit receipt ids for a task-specific report; this log is not a tamper-proof audit.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_appends_are_whole_private_records_and_partial_tail_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let subject = "0".repeat(64);
        let call = Call {
            protocol: "kb.usage.v1".into(),
            subject: subject.clone(),
            receipt: format!("sha256:{}", "1".repeat(64)),
            repo: Some("synthetic".into()),
            host_head: None,
            snapshot_content_digest: None,
            budget_used: 1,
            budget_unit: BudgetUnit::TokensEst,
            units: Vec::new(),
            requires: Vec::new(),
        };
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..8 {
                let call = &call;
                let path = dir.path();
                handles.push(scope.spawn(move || append_call(path, call).unwrap()));
            }
            for handle in handles {
                handle.join().unwrap();
            }
        });
        let first = read(dir.path(), &subject, &BTreeSet::new()).unwrap();
        assert_eq!(first.calls.len(), 8);
        assert!(first.invalid_lines.is_empty());
        let path = dir.path().join(LOG);
        let before = std::fs::read(&path).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{interrupted")
            .unwrap();
        append_call(dir.path(), &call).unwrap();
        let after = std::fs::read(&path).unwrap();
        assert!(after.starts_with(&before));
        assert!(
            after
                .windows(b"{interrupted\n".len())
                .any(|w| w == b"{interrupted\n")
        );
        let second = read(dir.path(), &subject, &BTreeSet::new()).unwrap();
        assert_eq!(second.calls.len(), 9);
        assert_eq!(second.invalid_lines, vec![9]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
