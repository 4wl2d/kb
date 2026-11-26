//! `kb doc <path|symbol>`: source locations and the knowledge records that document them.
//! The full views use the same scope rules as `context --intent implement`. A symbol is
//! first located through the `sym` route.

use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::path::Path;

use super::Ctx;
use super::agent::{self, Detail};
use super::args::DocArgs;
use super::cmd_code::{read_around, shell_word};
use crate::answer::{Answer, Item};
use crate::context::{ContextResult, ResolvedPath, Tier, UnitBody};
use crate::error::Result;
use crate::glob::{RepoGlob, split_repo};
use crate::knowledge::RecordEntry;
use crate::model::tools::Capability;
use crate::model::{Anchor, AnchorKind, Common};
use crate::output::{CommandOutput, Format};
use crate::tools::{self, CodeQuery, Roots};
use crate::util::PathExt;
use crate::util::safe_join;

const KNOWLEDGE: &str = "knowledge";

/// Is `subject` a path rather than a symbol name?
pub fn is_path(subject: &str, cwd: &Path, code_root: &Path) -> bool {
    if subject.contains("::") {
        return false;
    }
    subject.contains('/')
        || split_repo(subject).0.is_some()
        || cwd.join(subject).exists()
        || code_root.join(subject).exists()
}

pub fn run(ctx: &Ctx, args: &DocArgs) -> Result<CommandOutput> {
    let ws = agent::workspace(ctx)?;
    let lines = agent::lines(args.answer.lines);
    let cwd = ctx
        .env
        .cwd
        .canonical()
        .unwrap_or_else(|_| ctx.env.cwd.clone());
    let compact = matches!(ctx.format, Format::Compact | Format::Terse);
    let path_subject = is_path(&args.subject, &cwd, &ws.roots.code);
    let mut answer = Answer::new(
        "doc",
        if compact && path_subject {
            ""
        } else {
            &args.subject
        },
    );
    let paths = if path_subject {
        let path =
            super::cmd_context::host_relative(&args.subject, &cwd, Some(ws.roots.code.as_path()))?;
        if compact {
            if let Some(file) = existing_doc(&ws.roots, &path) {
                answer
                    .items
                    .push(Item::new("guide", KNOWLEDGE).name(file_read(file)));
            } else {
                answer
                    .items
                    .push(Item::new("path", KNOWLEDGE).at(&path, None));
            }
        }
        vec![path]
    } else {
        let q = CodeQuery {
            capability: Capability::Sym,
            name: args.subject.clone(),
            file: None,
            kind: None,
            depth: 1,
            limit: lines,
        };
        let outcome = tools::answer_code(&ws.registry, &ws.roots, &q, args.answer.tool.as_deref())?;
        answer.source(outcome.source.clone());
        let mut paths: Vec<String> = Vec::new();
        for mut item in outcome.items.into_iter().filter(|i| i.kind == "definition") {
            if let Some(p) = &item.path
                && !paths.contains(p)
            {
                paths.push(p.clone());
                if compact && paths.len() <= 3 {
                    if let Some(line) = item.line
                        && answer.next.is_empty()
                    {
                        answer.next(read_around(p, line));
                    }
                    item.detail = None;
                    answer.items.push(item);
                }
            }
        }
        if !outcome.found || paths.is_empty() {
            for n in outcome.notes {
                answer.note(n);
            }
            let failure = agent::not_found(format!(
                "`{}` is neither a path nor a symbol a connected tool knows",
                args.subject
            ))
            .with_hint("pass a host-relative path, or check the name with `kb sym <name>`");
            return Ok(agent::output(answer, args.answer.lines, Some(failure)));
        }
        if paths.len() > 3 {
            answer.note(format!(
                "`{}` is defined in {} files; documenting the first 3",
                args.subject,
                paths.len()
            ));
            paths.truncate(3);
        }
        if compact {
            for n in outcome.notes {
                answer.note(n);
            }
        } else {
            answer.note(format!(
                "`{}` is defined in {}",
                args.subject,
                paths.join(", ")
            ));
        }
        paths
    };
    if compact {
        agent::knowledge(ctx, &ws, &paths, Detail::CompactDoc, &mut answer)?;
        return Ok(agent::output(answer, args.answer.lines, None));
    }
    agent::knowledge(ctx, &ws, &paths, Detail::Doc, &mut answer)?;
    if let [path] = paths.as_slice()
        && is_doc(path)
    {
        answer.next(file_read(path));
    } else if answer.items.iter().any(|item| item.kind == "feature") {
        let path_args: Vec<String> = paths
            .iter()
            .map(|p| format!("--path {}", shell_word(p)))
            .collect();
        answer.next(format!(
            "kb context --intent implement {} --format terse",
            path_args.join(" ")
        ));
    }
    Ok(agent::output(answer, args.answer.lines, None))
}

fn is_doc(path: &str) -> bool {
    [".md", ".mdx", ".rst", ".adoc"]
        .iter()
        .any(|ext| path.ends_with(ext))
}

fn file_read(path: &str) -> String {
    let relative = if path.starts_with('-') { "./" } else { "" };
    format!("kb read {relative}{}", shell_word(path))
}

fn existing_doc<'a>(roots: &Roots, spec: &'a str) -> Option<&'a str> {
    let (repo, path) = split_repo(spec);
    (repo.is_none_or(|r| roots.repo.as_deref() == Some(r))
        && is_doc(path)
        && !path
            .split('/')
            .any(|part| part.eq_ignore_ascii_case(".git"))
        && safe_join(&roots.code, path).is_ok_and(|p| p.is_file()))
    .then_some(path)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Relevance {
    Anchor,
    Selector(Reverse<usize>),
    FeatureOrModule,
    RepositoryWide,
}

/// Does a source, test or documentation anchor name one of `paths`?
pub(super) fn anchored(anchors: &[Anchor], paths: &[ResolvedPath], code_is_kb: bool) -> bool {
    anchors.iter().any(|a| {
        matches!(
            a.kind,
            AnchorKind::Source | AnchorKind::Test | AnchorKind::Doc
        ) && paths.iter().any(|p| {
            a.path.as_deref() == Some(&p.path)
                && (a.repo.is_some() && a.repo == p.repo || a.repo.is_none() && code_is_kb)
        })
    })
}

fn relevance(record: Common<'_>, paths: &[ResolvedPath], code_is_kb: bool) -> Relevance {
    if anchored(record.anchors, paths, code_is_kb) {
        Relevance::Anchor
    } else if let Some(prefix) = record
        .selectors
        .paths
        .iter()
        .filter_map(|selector| RepoGlob::parse(selector).ok())
        .filter(|g| paths.iter().any(|p| g.matches(p.repo.as_deref(), &p.path)))
        .map(|g| g.literal_prefix_len())
        .max()
    {
        Relevance::Selector(Reverse(prefix))
    } else if record.scope.modules.is_empty() && record.scope.features.is_empty() {
        Relevance::RepositoryWide
    } else {
        Relevance::FeatureOrModule
    }
}

fn anchor_locator(anchor: &Anchor, roots: &Roots) -> Option<String> {
    let path = anchor.path.as_deref()?;
    match &anchor.repo {
        Some(repo) if Some(repo) != roots.repo.as_ref() => Some(format!("{repo}:{path}")),
        None if roots.kb != roots.code => Some(
            roots
                .kb_mount
                .as_ref()
                .map_or_else(|| format!("kb:{path}"), |mount| format!("{mount}/{path}")),
        ),
        _ => Some(path.to_string()),
    }
}

pub(super) fn render_compact_knowledge(
    result: &ContextResult,
    anchored: &[RecordEntry],
    roots: &Roots,
    answer: &mut Answer,
) {
    let mut records = result
        .units
        .iter()
        .filter_map(|unit| {
            let UnitBody::Record(parsed) = &unit.body else {
                return None;
            };
            let record = parsed.record.common();
            Some((
                relevance(record, &result.header.scope.paths, roots.code == roots.kb),
                unit.tier,
                record,
            ))
        })
        .collect::<Vec<_>>();
    // Records that cite the path without being scoped to it rank with the other anchors.
    records.extend(anchored.iter().map(|entry| {
        (
            Relevance::Anchor,
            Tier::Supplementary,
            entry.parsed.record.common(),
        )
    }));
    records.sort_by_key(|(relevance, tier, record)| (*relevance, *tier, record.id));

    if let Some((_, _, record)) = records
        .first()
        .filter(|(relevance, _, _)| *relevance != Relevance::RepositoryWide)
    {
        answer.next(format!("kb show {}", shell_word(record.id)));
    }

    let mut seen = BTreeSet::new();
    for (relevance, _, record) in records {
        if relevance == Relevance::RepositoryWide {
            answer
                .items
                .push(Item::new("guidance", KNOWLEDGE).name(record.id));
            continue;
        }
        answer.items.push(
            Item::new(record.kind.as_str(), KNOWLEDGE)
                .name(record.id)
                .detail(record.title),
        );
        for anchor in record.anchors.iter().filter(|a| a.kind == AnchorKind::Doc) {
            if let Some(path) = anchor_locator(anchor, roots)
                && seen.insert((path.clone(), anchor.symbol.as_deref()))
            {
                let mut item = Item::new("doc", KNOWLEDGE).at(path, None);
                item.name.clone_from(&anchor.symbol);
                answer.items.push(item);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_path;
    use std::path::Path;

    #[test]
    fn paths_and_symbols_are_told_apart() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(is_path("src/lib.rs", here, here));
        assert!(is_path("Cargo.toml", here, here));
        assert!(is_path("mobile:app/auth/x.kt", here, here));
        assert!(!is_path("TokenRefresher::refresh", here, here));
        assert!(!is_path("refresh", here, here));
    }
}
