//! External-provider adapter plus pure graph analysis. No source parsing or network client.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use crate::error::{KbError, Result};
use crate::git::Git;
use crate::model::*;
use crate::process::{Limits, capture};
use crate::util::{check_rel_path, read_file_limited, sha256_hex};

pub const MAX_PROVIDER_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_SYMBOLS: usize = 100_000;
pub const MAX_REFS: usize = 500_000;

#[derive(Debug, Clone)]
pub struct Provider {
    pub program: Option<PathBuf>,
    pub args: Vec<String>,
    pub files: Vec<PathBuf>,
    pub timeout_seconds: u64,
}

impl Default for Provider {
    fn default() -> Self {
        Self {
            program: None,
            args: Vec::new(),
            files: Vec::new(),
            timeout_seconds: 30,
        }
    }
}

impl Provider {
    pub fn configured(&self) -> bool {
        self.program.is_some() || !self.files.is_empty()
    }

    pub fn load(&self, request: &CodeRequest) -> Result<CodeResponse> {
        if !(1..=600).contains(&self.timeout_seconds) {
            return Err(KbError::invalid_input(
                "provider timeout must be 1..600 seconds",
            ));
        }
        let response = if let Some(program) = &self.program {
            if !self.files.is_empty() {
                return Err(KbError::invalid_input(
                    "choose a provider program or provider files, not both",
                ));
            }
            // A provider killed at the deadline cannot remove its temporary checkout itself.
            let scratch = Scratch::create()?;
            let mut command = Command::new(program);
            command
                .args(&self.args)
                .current_dir(&request.root)
                .env("TMPDIR", &scratch.0)
                .env("CODEGRAPH_NO_DAEMON", "1")
                .env("CODEGRAPH_TELEMETRY", "0")
                .env("CODEGRAPH_NO_UPDATE_CHECK", "1")
                .env("DO_NOT_TRACK", "1");
            let out = capture(
                &mut command,
                &serde_json::to_vec(request)?,
                Limits {
                    timeout: Duration::from_secs(self.timeout_seconds),
                    stdout: MAX_PROVIDER_BYTES,
                    stderr: 1024 * 1024,
                },
            )?;
            if out.exit_code != 0 {
                return Err(KbError::invalid_input(format!(
                    "code provider exited {}: {}",
                    out.exit_code,
                    crate::git::redact(&String::from_utf8_lossy(&out.stderr))
                )));
            }
            serde_json::from_slice::<CodeResponse>(&out.stdout)
                .map_err(|e| KbError::invalid_input(format!("invalid kb.code.v1 output: {e}")))?
        } else {
            let mut matching = Vec::new();
            for path in &self.files {
                let bytes = read_file_limited(path, MAX_PROVIDER_BYTES as u64)?;
                let response: CodeResponse = serde_json::from_slice(&bytes).map_err(|e| {
                    KbError::invalid_input(format!(
                        "invalid provider fixture {}: {e}",
                        path.display()
                    ))
                })?;
                if response.repo == request.repo && response.commit == request.commit {
                    matching.push(response);
                }
            }
            if matching.len() != 1 {
                return Err(KbError::invalid_input(format!(
                    "need exactly one provider response for {} at {} (found {})",
                    request.repo,
                    request.commit,
                    matching.len()
                )));
            }
            matching.remove(0)
        };
        validate(response, request)
    }

    /// [`Provider::load`] for [`brief`], the only consumer of `similar`, which it turns into
    /// precedent candidates. Similarity answers the generating request's task and
    /// identifiers, which a pinned response does not record; replaying it would mislabel
    /// precedents, so files mode drops it and says so.
    pub fn load_for_brief(&self, request: &CodeRequest) -> Result<CodeResponse> {
        let mut response = self.load(request)?;
        if self.program.is_none() && !response.similar.is_empty() {
            response.similar.clear();
            response.limitations.push(PINNED_SIMILAR.into());
            response.limitations.sort();
            response.limitations.dedup();
        }
        Ok(response)
    }
}

pub const PINNED_SIMILAR: &str =
    "A pinned provider response does not replay its task-specific similar candidates.";

/// Engine-owned temporary directory for one provider call, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn create() -> Result<Self> {
        let base = std::env::temp_dir();
        let mut attempt = 0;
        loop {
            let path = base.join(format!(
                "kb-provider-{}-{:016x}",
                std::process::id(),
                crate::util::unique_suffix()
            ));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 16 => {
                    attempt += 1;
                }
                Err(e) => return Err(KbError::io("provider temporary directory", e)),
            }
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn request(
    root: &Path,
    repo: &str,
    revision: &str,
    operation: CodeOperation,
) -> Result<CodeRequest> {
    let commit = Git::new(root).resolve_commit(revision)?.ok_or_else(|| {
        KbError::invalid_input(format!("unknown provider host revision {revision}"))
    })?;
    Ok(CodeRequest {
        protocol: CodeProtocol::V1,
        operation,
        root: root
            .to_str()
            .ok_or_else(|| KbError::invalid_input("host root is not UTF-8"))?
            .into(),
        repo: repo.into(),
        commit,
        paths: Vec::new(),
        identifiers: Vec::new(),
        task: None,
        depth: 2,
    })
}

pub fn validate(mut response: CodeResponse, request: &CodeRequest) -> Result<CodeResponse> {
    if !matches!(request.commit.len(), 40 | 64) || !crate::model::is_commit(&request.commit) {
        return Err(KbError::invalid_input(
            "provider requests require a full immutable commit id",
        ));
    }
    if response.repo != request.repo || response.commit != request.commit {
        return Err(KbError::invalid_input(
            "provider repo/commit does not match the immutable request",
        ));
    }
    if !response.capabilities.contains(&request.operation) {
        return Err(KbError::invalid_input(
            "provider does not support the requested operation",
        ));
    }
    let single_line = |s: &str, max: usize| {
        !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
    };
    if !single_line(&response.tool.name, 200) || !single_line(&response.tool.version, 200) {
        return Err(KbError::invalid_input(
            "provider must name its tool and version",
        ));
    }
    if response.limitations.len() > 32 || response.limitations.iter().any(|s| !single_line(s, 2048))
    {
        return Err(KbError::invalid_input(
            "provider limitations must be at most 32 bounded single-line explanations",
        ));
    }
    if response.symbols.len() > MAX_SYMBOLS
        || response.refs.len() > MAX_REFS
        || response.similar.len() > MAX_SYMBOLS
    {
        return Err(KbError::invalid_input(
            "provider result exceeds symbol/reference limits",
        ));
    }
    response
        .symbols
        .sort_by(|a, b| (&a.path, a.start_line, &a.id).cmp(&(&b.path, b.start_line, &b.id)));
    let mut ids = BTreeSet::new();
    let mut symbol_paths = BTreeMap::new();
    for symbol in &response.symbols {
        if !single_line(&symbol.id, 1024)
            || !ids.insert(symbol.id.clone())
            || !single_line(&symbol.name, 1024)
            || !single_line(&symbol.kind, 64)
            || symbol
                .signature
                .as_ref()
                .is_some_and(|s| s.len() > 8192 || s.contains('\0'))
        {
            return Err(KbError::invalid_input(
                "provider symbols need unique bounded ids and nonempty names",
            ));
        }
        check_rel_path(&symbol.path).map_err(KbError::unsafe_path)?;
        symbol_paths.insert(symbol.id.as_str(), symbol.path.as_str());
    }
    let paths: BTreeSet<String> = response.symbols.iter().map(|s| s.path.clone()).collect();
    let mut line_counts = BTreeMap::new();
    // Symbols are sorted by path, the order in which their files are visited.
    let mut pending = response.symbols.iter().peekable();
    crate::host::facts::for_each_blob_at(
        Path::new(&request.root),
        &request.commit,
        &paths,
        crate::provenance::MAX_ANCHOR_BYTES,
        |path, blob| {
            let blob = blob.ok_or_else(|| {
                KbError::invalid_input(format!(
                    "provider file {path} is absent at the requested commit"
                ))
            })?;
            let lines = blob.split_inclusive(|b| *b == b'\n').count().max(1);
            line_counts.insert(path.to_string(), lines);
            while let Some(symbol) = pending.next_if(|s| s.path == path) {
                if (symbol.extent == CodeExtent::Line && symbol.start_line != symbol.end_line)
                    || (symbol.extent == CodeExtent::File
                        && (symbol.start_line != 1 || symbol.end_line as usize != lines))
                {
                    return Err(KbError::invalid_input(
                        "provider extent disagrees with its source span",
                    ));
                }
                let bytes = crate::provenance::line_span(blob, symbol.start_line, symbol.end_line)?;
                if sha256_hex(bytes) != symbol.sha256 {
                    return Err(KbError::invalid_input(format!(
                        "provider stamp for {} does not match Git content",
                        symbol.id
                    )));
                }
            }
            Ok(())
        },
    )?;
    for edge in &response.refs {
        if !ids.contains(&edge.from)
            || !ids.contains(&edge.to)
            || edge.line == 0
            || symbol_paths
                .get(edge.from.as_str())
                .is_some_and(|path| edge.line as usize > line_counts[*path])
        {
            return Err(KbError::invalid_input(
                "provider reference names a missing symbol or invalid line",
            ));
        }
    }
    for similar in &response.similar {
        if !ids.contains(&similar.symbol)
            || similar.score > 1000
            || !single_line(&similar.reason, 2048)
        {
            return Err(KbError::invalid_input(
                "invalid provider similarity candidate",
            ));
        }
    }
    response
        .symbols
        .sort_by(|a, b| (&a.path, a.start_line, &a.id).cmp(&(&b.path, b.start_line, &b.id)));
    response.refs.sort();
    response.refs.dedup();
    response.similar.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.symbol.cmp(&b.symbol))
            .then_with(|| a.reason.cmp(&b.reason))
    });
    let mut similar_ids = BTreeSet::new();
    response
        .similar
        .retain(|s| similar_ids.insert(s.symbol.clone()));
    response.capabilities.sort();
    response.capabilities.dedup();
    response.limitations.sort();
    response.limitations.dedup();
    Ok(response)
}

#[derive(Debug, Clone, Serialize)]
pub struct Dependent {
    pub symbol: String,
    pub path: String,
    pub depth: u32,
    pub confidence: CodeConfidence,
}

/// Incoming references, bounded by depth. File-level source nodes conservatively expand
/// to symbols in that file; they remain explicitly possible relationships.
pub fn dependents(
    response: &CodeResponse,
    changed: &BTreeSet<String>,
    depth: u32,
) -> Vec<Dependent> {
    let symbols: BTreeMap<_, _> = response
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let mut frontier: BTreeMap<_, _> = response
        .symbols
        .iter()
        .filter(|s| changed.contains(&s.path))
        .map(|s| (s.id.as_str(), CodeConfidence::Resolved))
        .collect();
    let mut visited: BTreeSet<_> = frontier.keys().copied().collect();
    let mut out = Vec::new();
    for distance in 1..=depth {
        let mut next: BTreeMap<&str, CodeConfidence> = BTreeMap::new();
        let mut found: BTreeMap<&str, CodeConfidence> = BTreeMap::new();
        for edge in &response.refs {
            let Some(prior) = frontier.get(edge.to.as_str()) else {
                continue;
            };
            if visited.contains(edge.from.as_str()) {
                continue;
            }
            let Some(symbol) = symbols.get(edge.from.as_str()) else {
                continue;
            };
            let confidence = edge.confidence.max(*prior);
            found
                .entry(symbol.id.as_str())
                .and_modify(|c| *c = (*c).min(confidence))
                .or_insert(confidence);
            next.entry(symbol.id.as_str())
                .and_modify(|c| *c = (*c).min(confidence))
                .or_insert(confidence);
            if symbol.extent == CodeExtent::File {
                for member in response.symbols.iter().filter(|s| s.path == symbol.path) {
                    next.entry(member.id.as_str())
                        .or_insert(CodeConfidence::Possible);
                }
            }
        }
        for (id, confidence) in found {
            let symbol = symbols[id];
            if !changed.contains(&symbol.path) {
                out.push(Dependent {
                    symbol: id.into(),
                    path: symbol.path.clone(),
                    depth: distance,
                    confidence,
                });
            }
        }
        next.retain(|id, _| !visited.contains(id));
        visited.extend(next.keys().copied());
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    out.sort_by(|a, b| {
        (&a.path, a.depth, &a.symbol, a.confidence).cmp(&(
            &b.path,
            b.depth,
            &b.symbol,
            b.confidence,
        ))
    });
    out.dedup_by(|a, b| a.symbol == b.symbol);
    out
}

pub fn fan_in(response: &CodeResponse, registry: &Registry) -> BTreeMap<String, u64> {
    let symbols: BTreeMap<_, _> = response
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let mut callers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in &response.refs {
        let (Some(from), Some(to)) = (
            symbols.get(edge.from.as_str()),
            symbols.get(edge.to.as_str()),
        ) else {
            continue;
        };
        let source_modules: BTreeSet<_> = registry
            .modules_for_path(&response.repo, &from.path)
            .into_iter()
            .map(|m| &m.id)
            .collect();
        for target in registry.modules_for_path(&response.repo, &to.path) {
            if !source_modules.contains(&target.id) {
                callers
                    .entry(target.id.clone())
                    .or_default()
                    .insert(from.path.clone());
            }
        }
    }
    callers
        .into_iter()
        .map(|(id, files)| (id, files.len() as u64))
        .collect()
}

pub fn consumers(response: &CodeResponse, targets: &BTreeSet<String>) -> Vec<Consumer> {
    let symbols: BTreeMap<_, _> = response
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let mut out = BTreeSet::new();
    for edge in &response.refs {
        if !targets.contains(&edge.to) {
            continue;
        }
        if let Some(from) = symbols.get(edge.from.as_str()) {
            out.insert((
                from.path.clone(),
                (from.extent != CodeExtent::File).then(|| from.name.clone()),
            ));
        }
    }
    out.into_iter()
        .map(|(path, symbol)| Consumer {
            repo: response.repo.clone(),
            path,
            symbol,
        })
        .collect()
}

pub fn contract_consumers(response: &CodeResponse, record: &Record) -> Vec<Consumer> {
    if !matches!(record, Record::Contract(_)) {
        return Vec::new();
    }
    let targets: BTreeSet<_> = response
        .symbols
        .iter()
        .filter(|symbol| {
            record.common().anchors.iter().any(|anchor| {
                anchor.repo.as_deref() == Some(&response.repo)
                    && anchor.path.as_deref() == Some(&symbol.path)
                    && anchor
                        .symbol
                        .as_ref()
                        .is_none_or(|name| name == &symbol.name)
            })
        })
        .map(|s| s.id.clone())
        .collect();
    consumers(response, &targets)
}

pub fn info(response: &CodeResponse) -> Result<crate::context::code::CodeInfo> {
    Ok(crate::context::code::CodeInfo {
        repo: response.repo.clone(),
        commit: response.commit.clone(),
        tool: response.tool.clone(),
        complete: response.complete,
        limitations: response.limitations.clone(),
        digest: sha256_hex(
            crate::context::canonical_json(&serde_json::to_value(response)?).as_bytes(),
        ),
    })
}

/// Files declaring symbols the task names. A name (case-insensitive leaf) declared in more
/// than [`MAX_PATHS_PER_IDENTIFIER`](crate::context::discovery::MAX_PATHS_PER_IDENTIFIER)
/// files is ambiguous and supplies no path.
pub fn paths_for_task(
    response: &CodeResponse,
    task: &str,
) -> crate::context::discovery::Discovered {
    let tokens = crate::normalize::tokens(task);
    crate::context::discovery::Discovered::collect(
        response
            .symbols
            .iter()
            .filter(|s| symbol_mentioned(&s.name, &tokens))
            .map(|s| (leaf(&s.name).to_lowercase(), &s.path)),
    )
}

fn leaf(name: &str) -> &str {
    name.rsplit([':', '.', '/'])
        .find(|s| !s.is_empty())
        .unwrap_or(name)
}

fn symbol_mentioned(name: &str, tokens: &[String]) -> bool {
    let leaf = leaf(name);
    if leaf.chars().count() < 4
        || ["main", "test", "tests", "build", "default"]
            .contains(&leaf.to_ascii_lowercase().as_str())
    {
        return false;
    }
    crate::normalize::AliasPattern::compile(name).is_some_and(|p| p.matches(tokens))
        || crate::normalize::AliasPattern::compile(leaf).is_some_and(|p| p.matches(tokens))
        || crate::normalize::AliasPattern::compile(
            &crate::context::discovery::identifier_tokens(leaf).join(" "),
        )
        .is_some_and(|p| p.matches(tokens))
}

/// Compose bounded candidate units, reading their actual source from the pinned Git tree.
/// Budgeting later either includes or excludes each whole unit. Candidates whose source is
/// not UTF-8 are omitted, never failing the command, and reported as a limitation.
pub fn brief(
    response: &CodeResponse,
    request: &CodeRequest,
    limit: usize,
) -> Result<(Vec<crate::context::code::CodeEvidence>, Option<String>)> {
    let tokens = crate::normalize::tokens(request.task.as_deref().unwrap_or_default());
    let mut paths: BTreeSet<_> = request.paths.iter().cloned().collect();
    paths.extend(paths_for_task(response, request.task.as_deref().unwrap_or_default()).paths);
    let callers: BTreeMap<_, _> = dependents(response, &paths, 2)
        .into_iter()
        .map(|d| (d.symbol, d.confidence))
        .collect();
    let similar: BTreeMap<_, _> = response
        .similar
        .iter()
        .map(|s| (s.symbol.as_str(), s))
        .collect();
    let mut candidates = Vec::new();
    for symbol in &response.symbols {
        let mentioned = symbol_mentioned(&symbol.name, &tokens);
        let direct = paths.contains(&symbol.path);
        let caller = callers.contains_key(&symbol.id);
        let analog = similar.get(symbol.id.as_str());
        if !mentioned && !direct && !caller && analog.is_none() {
            continue;
        }
        let (priority, role, reason) = if symbol.test && (direct || caller) {
            (
                0,
                "test",
                "existing test candidate; execution is not inferred".to_string(),
            )
        } else if mentioned {
            (1, "symbol", "identifier named in the task".into())
        } else if caller {
            (
                2,
                "consumer",
                if callers[&symbol.id] == CodeConfidence::Possible {
                    "possible static consumer of the touched code".into()
                } else {
                    "resolved static reference to the touched code".into()
                },
            )
        } else if direct {
            (3, "symbol", "declared in the touched source".into())
        } else {
            (4, "analog", analog.unwrap().reason.clone())
        };
        candidates.push((priority, symbol, role, reason));
    }
    candidates.sort_by(|a, b| {
        (a.0, &a.1.path, a.1.start_line, &a.1.id).cmp(&(b.0, &b.1.path, b.1.start_line, &b.1.id))
    });
    candidates.truncate(limit);
    let mut out = Vec::new();
    let mut not_utf8 = 0;
    for (_, symbol, role, reason) in candidates {
        let bytes = crate::host::facts::blob_at(
            Path::new(&request.root),
            &request.commit,
            &symbol.path,
            crate::provenance::MAX_ANCHOR_BYTES,
        )?
        .ok_or_else(|| {
            KbError::invalid_input("provider source disappeared from its pinned Git tree")
        })?;
        let span = crate::provenance::line_span(&bytes, symbol.start_line, symbol.end_line)?;
        let Ok(source) = std::str::from_utf8(span).map(str::to_string) else {
            not_utf8 += 1;
            continue;
        };
        out.push(crate::context::code::CodeEvidence {
            repo: response.repo.clone(),
            commit: response.commit.clone(),
            tool: response.tool.clone(),
            symbol: symbol.clone(),
            role: role.into(),
            reason,
            source,
        });
    }
    let omitted = (not_utf8 > 0)
        .then(|| format!("Omitted {not_utf8} code unit(s) whose source is not UTF-8."));
    Ok((out, omitted))
}
