//! Pure lexical discovery over registry definitions and adapter-supplied tracked paths.

use std::collections::{BTreeMap, BTreeSet};

use crate::glob::RepoGlob;
use crate::model::Registry;
use crate::normalize::{self, AliasPattern};

use super::ResolvedPath;

/// Split identifiers at lower/upper and acronym/word boundaries before normalization.
pub fn identifier_tokens(value: &str) -> Vec<String> {
    let chars: Vec<_> = value.chars().collect();
    let mut split = String::new();
    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_uppercase()
            && i > 0
            && (chars[i - 1].is_lowercase()
                || chars[i - 1].is_ascii_digit()
                || (chars[i - 1].is_uppercase()
                    && chars.get(i + 1).is_some_and(|c| c.is_lowercase())))
        {
            split.push(' ');
        }
        split.push(ch);
    }
    normalize::tokens(&split)
}

fn contains(tokens: &[String], phrase: &[String]) -> bool {
    !phrase.is_empty() && tokens.windows(phrase.len()).any(|w| w == phrase)
}

/// Most files one identifier (a filename key or a code symbol name) may name and still
/// supply candidate paths. A word that names more (`init` for every `__init__.py`, `save`
/// declared in hundreds of files) is ambiguous and is skipped.
pub const MAX_PATHS_PER_IDENTIFIER: usize = 8;

/// Candidate paths found for task text, and the identifiers skipped as ambiguous.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// Sorted, distinct paths.
    pub paths: Vec<String>,
    /// Identifiers that name more than [`MAX_PATHS_PER_IDENTIFIER`] files, with their file
    /// counts; sorted.
    pub ambiguous: Vec<(String, usize)>,
}

impl Discovered {
    /// Collect `(identifier, path)` matches: identifiers within the cap supply their paths.
    pub fn collect<'a>(matches: impl IntoIterator<Item = (String, &'a String)>) -> Discovered {
        let mut by_identifier: BTreeMap<String, BTreeSet<&String>> = BTreeMap::new();
        for (identifier, path) in matches {
            by_identifier.entry(identifier).or_default().insert(path);
        }
        let mut paths = BTreeSet::new();
        let mut ambiguous = Vec::new();
        for (identifier, found) in by_identifier {
            if found.len() > MAX_PATHS_PER_IDENTIFIER {
                ambiguous.push((identifier, found.len()));
            } else {
                paths.extend(found.into_iter().cloned());
            }
        }
        Discovered {
            paths: paths.into_iter().collect(),
            ambiguous,
        }
    }

    /// Info note listing the skipped identifiers of `source`, if any.
    pub fn note(&self, source: &str) -> Option<crate::diag::Diagnostic> {
        if self.ambiguous.is_empty() {
            return None;
        }
        let list: Vec<String> = self
            .ambiguous
            .iter()
            .map(|(identifier, n)| format!("`{identifier}` ({n} files)"))
            .collect();
        Some(crate::diag::Diagnostic::info(
            "IDENTIFIER_AMBIGUOUS",
            format!(
                "{source} named by the task match more than {MAX_PATHS_PER_IDENTIFIER} files and \
                 supply no candidate paths: {}",
                list.join(", ")
            ),
        ))
    }
}

/// A symbol-like filename stem or explicit filename in task text names its tracked files as
/// candidates (at most [`MAX_PATHS_PER_IDENTIFIER`] per key); no disk/source scan occurs.
pub fn identifier_paths(task: &str, tracked: &[String]) -> Discovered {
    IdentifierIndex::new(tracked).find(task)
}

/// A lexical index, not code intelligence: filenames produce exact and camel-split keys.
/// Query words are not split inside identifiers, so FooBarOther cannot match FooBar.
pub struct IdentifierIndex {
    terms: BTreeMap<String, BTreeSet<String>>,
    max_words: usize,
}

impl IdentifierIndex {
    pub fn new(tracked: &[String]) -> Self {
        let mut terms: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut max_words = 0;
        for path in tracked {
            let filename = path.rsplit('/').next().unwrap_or(path);
            let stem = filename.rsplit_once('.').map_or(filename, |(s, _)| s);
            let generic = [
                "main", "lib", "mod", "index", "test", "tests", "readme", "build", "package",
            ]
            .contains(&stem.to_ascii_lowercase().as_str());
            let mut keys = Vec::new();
            if filename != stem {
                keys.push(normalize::tokens(filename));
            }
            if !generic && stem.chars().count() >= 3 {
                keys.push(normalize::tokens(stem));
                keys.push(identifier_tokens(stem));
            }
            for key in keys.into_iter().filter(|k| !k.is_empty()) {
                max_words = max_words.max(key.len());
                terms.entry(key.join(" ")).or_default().insert(path.clone());
            }
        }
        Self { terms, max_words }
    }

    pub fn find(&self, task: &str) -> Discovered {
        let words = normalize::tokens(task);
        let mut found = Vec::new();
        for size in 1..=self.max_words.min(words.len()) {
            for phrase in words.windows(size) {
                let key = phrase.join(" ");
                if let Some(paths) = self.terms.get(&key) {
                    found.extend(paths.iter().map(|p| (key.clone(), p)));
                }
            }
        }
        Discovered::collect(found)
    }
}

/// Positive evidence is useful for inclusion, but absence of a lexical match does not
/// establish that a change category is impossible. Only explicit categories prune.
pub fn change_type_hints(
    registry: &Registry,
    task: &str,
    paths: &[ResolvedPath],
    changed_text: &str,
) -> BTreeMap<String, BTreeSet<String>> {
    let tokens = normalize::tokens(task);
    let identifiers = identifier_tokens(task);
    let diff_tokens: Vec<Vec<String>> = changed_text
        .lines()
        .filter(|l| {
            (l.starts_with('+') || l.starts_with('-'))
                && !l.starts_with("+++")
                && !l.starts_with("---")
        })
        .map(identifier_tokens)
        .collect();
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for kind in &registry.data.change_types {
        for alias in std::iter::once(&kind.id).chain(&kind.aliases) {
            if AliasPattern::compile(alias).is_some_and(|p| p.matches(&tokens)) {
                found
                    .entry(kind.id.clone())
                    .or_default()
                    .insert(format!("task alias: {alias}"));
            }
        }
        for symbol in &kind.symbols {
            let wanted = identifier_tokens(symbol);
            if diff_tokens.iter().any(|tokens| contains(tokens, &wanted)) {
                found
                    .entry(kind.id.clone())
                    .or_default()
                    .insert(format!("changed text identifier: {symbol}"));
            }
            if contains(&identifiers, &wanted)
                || paths
                    .iter()
                    .any(|p| contains(&identifier_tokens(&p.path), &wanted))
            {
                found
                    .entry(kind.id.clone())
                    .or_default()
                    .insert(format!("identifier: {symbol}"));
            }
        }
        for spec in &kind.paths {
            if let Ok(glob) = RepoGlob::parse(spec) {
                for path in paths {
                    if glob.matches(path.repo.as_deref(), &path.path) {
                        found
                            .entry(kind.id.clone())
                            .or_default()
                            .insert(format!("path: {} ({spec})", path.display()));
                    }
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_naming_too_many_files_are_skipped() {
        let mut tracked: Vec<String> = (0..MAX_PATHS_PER_IDENTIFIER)
            .map(|n| format!("m{n}/Config.kt"))
            .collect();
        tracked.push("app/TokenStore.kt".into());
        let found = identifier_paths("Read Config in TokenStore", &tracked);
        assert_eq!(found.paths.len(), MAX_PATHS_PER_IDENTIFIER + 1);
        assert!(found.ambiguous.is_empty());
        assert!(found.note("tracked filenames").is_none());
        tracked.push("m9/Config.kt".into());
        let found = identifier_paths("Read Config in TokenStore", &tracked);
        assert_eq!(found.paths, vec!["app/TokenStore.kt"]);
        assert_eq!(found.ambiguous, vec![("config".to_string(), 9)]);
        let note = found.note("tracked filenames").unwrap();
        assert_eq!(note.code, "IDENTIFIER_AMBIGUOUS");
        assert!(
            note.message.contains("`config` (9 files)"),
            "{}",
            note.message
        );
    }
}
