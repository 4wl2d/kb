//! Git and work-tree adapter for declarative verification. Repository scripts are never run.
use std::collections::BTreeSet;
use std::path::Path;

use super::{AddedLine, ChangedSource, Input, Message};
use crate::error::{KbError, Result};
use crate::git::Git;
use crate::impact::{ChangeStatus, HostDiff};
use crate::knowledge::RecordEntry;
use crate::model::VerifyProbe;
use crate::util::{read_file_limited, safe_join};

pub struct Options<'a> {
    pub mode: &'a str,
    /// Index read by staged mode instead of the default (a commit hook's pending index).
    pub index_file: Option<&'a Path>,
    pub branch: Option<&'a str>,
    pub commit_message: Option<&'a Path>,
    pub only: &'a BTreeSet<String>,
}

pub fn gather(
    root: &Path,
    repo: &str,
    diff: HostDiff,
    records: &[RecordEntry],
    options: &Options<'_>,
) -> Result<Input> {
    let git = Git::new(root);
    let branch = if let Some(branch) = options.branch {
        if branch.is_empty() || branch.len() > 1024 || branch.chars().any(char::is_control) {
            return Err(KbError::invalid_input(
                "--branch must be a bounded single line",
            ));
        }
        Some(branch.to_string())
    } else if diff.head.is_some() && diff.head != git.resolve_commit("HEAD")? {
        None
    } else {
        let out = git.output(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        out.ok()
            .then(|| out.stdout_str().trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let branch_source = if options.branch.is_some() {
        "argument"
    } else if branch.is_some() {
        "git"
    } else {
        "unavailable"
    };
    let need_messages = options.only.is_empty() || options.only.contains("commit-message");
    let messages = if !need_messages {
        Vec::new()
    } else if let Some(path) = options.commit_message {
        let bytes = read_file_limited(path, 1024 * 1024)?;
        vec![Message {
            commit: None,
            text: String::from_utf8(bytes)
                .map_err(|_| KbError::invalid_input("commit message is not UTF-8"))?,
        }]
    } else if diff.merge_base == "empty-tree" {
        Vec::new()
    } else {
        let head = diff.head.as_deref().unwrap_or("HEAD");
        let range = format!("{}..{head}", diff.merge_base);
        let ids = git.run_bytes_limited(
            &["rev-list", "--reverse", "--max-count=1001", &range, "--"],
            128 * 1024,
        )?;
        let ids =
            std::str::from_utf8(&ids).map_err(|_| KbError::invalid_input("invalid commit list"))?;
        let ids: Vec<_> = ids.lines().collect();
        if ids.len() > 1000 {
            return Err(KbError::invalid_input(
                "over 1000 messages in the verification range; split the range",
            ));
        }
        let mut messages = Vec::new();
        let mut total = 0;
        for commit in ids {
            let bytes = git.run_bytes_limited(
                &["show", "--no-patch", "--format=%B", commit, "--"],
                1024 * 1024,
            )?;
            total += bytes.len();
            if total > 8 * 1024 * 1024 {
                return Err(KbError::invalid_input(
                    "commit message corpus exceeds 8 MiB",
                ));
            }
            messages.push(Message {
                commit: Some(commit.into()),
                text: String::from_utf8(bytes)
                    .map_err(|_| KbError::invalid_input("commit message is not UTF-8"))?,
            });
        }
        messages
    };
    let need_lines = options.only.is_empty() || options.only.contains("banned-api");
    let patterns = if need_lines {
        records
            .iter()
            .flat_map(|r| r.parsed.record.normative())
            .flat_map(|s| s.verify.iter())
            .filter_map(|p| {
                if let VerifyProbe::BannedApi { paths, .. } = p {
                    Some(paths)
                } else {
                    None
                }
            })
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let patterns = super::globs(&patterns)?;
    // Staged patches read the index being committed (only staged mode names one).
    let patch_git = Git::new(root).with_index_file(options.index_file);
    let mut files = Vec::new();
    let mut total = 0usize;
    for file in &diff.files {
        let mut source = ChangedSource {
            path: file.path.clone(),
            status: file.status,
            added: Vec::new(),
            error: None,
        };
        if file.status != ChangeStatus::Deleted && super::matches(&patterns, repo, &file.path) {
            let found = (|| -> Result<Vec<AddedLine>> {
                if diff.merge_base == "empty-tree" && options.mode == "working-tree" {
                    let bytes = read_file_limited(&safe_join(root, &file.path)?, 2 * 1024 * 1024)?;
                    let text = std::str::from_utf8(&bytes)
                        .map_err(|_| KbError::invalid_input("new file is not UTF-8"))?;
                    return Ok(text
                        .split_inclusive('\n')
                        .enumerate()
                        .map(|(i, line)| AddedLine {
                            line: i as u32 + 1,
                            text: line.trim_end_matches('\n').into(),
                        })
                        .collect());
                }
                let mut args = vec![
                    "--literal-pathspecs",
                    "diff",
                    "--unified=0",
                    "-M",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                ];
                if options.mode == "staged" {
                    args.push("--cached");
                }
                if diff.merge_base != "empty-tree" {
                    args.push(&diff.merge_base);
                }
                if let Some(head) = &diff.head {
                    args.push(head);
                }
                args.push("--");
                args.push(&file.path);
                if let Some(old) = &file.old_path {
                    args.push(old);
                }
                let patch = patch_git.run_bytes_limited(&args, 8 * 1024 * 1024)?;
                if patch.is_empty()
                    && options.mode == "working-tree"
                    && file.status == ChangeStatus::Added
                {
                    let bytes = read_file_limited(&safe_join(root, &file.path)?, 2 * 1024 * 1024)?;
                    let text = std::str::from_utf8(&bytes)
                        .map_err(|_| KbError::invalid_input("new file is not UTF-8"))?;
                    Ok(text
                        .split_inclusive('\n')
                        .enumerate()
                        .map(|(i, line)| AddedLine {
                            line: i as u32 + 1,
                            text: line.trim_end_matches('\n').into(),
                        })
                        .collect())
                } else {
                    let text = std::str::from_utf8(&patch)
                        .map_err(|_| KbError::invalid_input("patch is not UTF-8"))?;
                    added_lines(text)
                }
            })();
            match found {
                Ok(lines) => {
                    total += lines.iter().map(|l| l.text.len()).sum::<usize>();
                    source.added = lines;
                }
                Err(error) => source.error = Some(error.message),
            }
            if total > 32 * 1024 * 1024 {
                return Err(KbError::invalid_input(
                    "added verification source exceeds 32 MiB; split the diff",
                ));
            }
        }
        files.push(source);
    }
    Ok(Input {
        repo: repo.into(),
        mode: options.mode.into(),
        diff,
        branch,
        branch_source,
        messages,
        files,
        code: None,
    })
}

fn added_lines(patch: &str) -> Result<Vec<AddedLine>> {
    let mut out = Vec::new();
    let mut current = 0u32;
    let mut remaining = 0u32;
    for line in patch
        .split_inclusive('\n')
        .map(|s| s.trim_end_matches('\n'))
    {
        if line.starts_with("@@ ") {
            let part = line
                .split_once(" +")
                .and_then(|(_, s)| s.split_whitespace().next())
                .ok_or_else(|| KbError::invalid_input("invalid Git hunk"))?;
            let (start, count) = part.split_once(',').unwrap_or((part, "1"));
            current = start
                .parse()
                .map_err(|_| KbError::invalid_input("invalid hunk line"))?;
            remaining = count
                .parse()
                .map_err(|_| KbError::invalid_input("invalid hunk size"))?;
        } else if remaining > 0 {
            if let Some(added) = line.strip_prefix('+') {
                out.push(AddedLine {
                    line: current,
                    text: added.into(),
                });
                current += 1;
                remaining -= 1;
            } else if line.starts_with(' ') {
                current += 1;
                remaining -= 1;
            }
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            return Err(KbError::invalid_input(
                "binary patch has no verifiable source lines",
            ));
        }
    }
    if remaining != 0 {
        return Err(KbError::invalid_input("truncated Git hunk"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn added_line_parser_preserves_plus_prefixes_and_counts_context() {
        let lines=added_lines("--- a/file\n+++ b/file\n@@ -2,1 +2,3 @@\n-old\n+++ source\n+new\n context\n@@ -8,1 +10,0 @@\n-removed\n").unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|l| (l.line, l.text.as_str()))
                .collect::<Vec<_>>(),
            vec![(2, "++ source"), (3, "new")]
        );
        assert!(added_lines("Binary files a/x and b/x differ\n").is_err());
        assert!(added_lines("@@ -1,0 +1,2 @@\n+only one\n").is_err());
    }
}
