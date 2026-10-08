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

/// A symbol-like filename stem or explicit filename in task text scopes to every matching
/// tracked file. Duplicate stems stay ambiguous candidates; no disk/source scan occurs.
pub fn identifier_paths(task: &str, tracked: &[String]) -> Vec<String> {
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

    pub fn find(&self, task: &str) -> Vec<String> {
        let words = normalize::tokens(task);
        let mut found = BTreeSet::new();
        for size in 1..=self.max_words.min(words.len()) {
            for phrase in words.windows(size) {
                if let Some(paths) = self.terms.get(&phrase.join(" ")) {
                    found.extend(paths.iter().cloned());
                }
            }
        }
        found.into_iter().collect()
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
