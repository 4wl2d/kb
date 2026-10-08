//! Repo-relative path globs with an optional `repo:` qualifier.
//!
//! Syntax: `*` matches within one segment, `**` across segments, `?` one char,
//! `[...]` classes. Patterns are `/`-separated and relative; `..`, absolute paths,
//! backslashes and NUL are rejected.

use globset::{GlobBuilder, GlobMatcher};

/// A compiled glob, optionally restricted to one registry repo.
#[derive(Debug, Clone)]
pub struct RepoGlob {
    pub repo: Option<String>,
    pub pattern: String,
    matcher: GlobMatcher,
}

impl PartialEq for RepoGlob {
    fn eq(&self, other: &Self) -> bool {
        self.repo == other.repo && self.pattern == other.pattern
    }
}

impl RepoGlob {
    /// Parse `pattern` or `repo:pattern`.
    pub fn parse(spec: &str) -> Result<RepoGlob, String> {
        let (repo, pattern) = split_repo(spec);
        validate_pattern(pattern)?;
        let matcher = GlobBuilder::new(pattern)
            .literal_separator(true)
            .backslash_escape(false)
            .build()
            .map_err(|e| format!("invalid glob `{spec}`: {e}"))?
            .compile_matcher();
        Ok(RepoGlob {
            repo: repo.map(str::to_string),
            pattern: pattern.to_string(),
            matcher,
        })
    }

    /// Does the glob match `path` in `repo`? An unqualified glob matches any repo.
    pub fn matches(&self, repo: Option<&str>, path: &str) -> bool {
        if let Some(r) = &self.repo
            && repo != Some(r.as_str())
        {
            return false;
        }
        self.matcher.is_match(path)
    }

    /// Length of the literal prefix before the first glob metacharacter (specificity).
    pub fn literal_prefix_len(&self) -> usize {
        literal_prefix(&self.pattern).len()
    }

    /// Literal directory prefix (up to and including the last `/` before the first
    /// metacharacter). Used as an index key: a path can only match if it starts with it.
    pub fn literal_dir_prefix(&self) -> String {
        let lit = literal_prefix(&self.pattern);
        match lit.rfind('/') {
            Some(i) => lit[..=i].to_string(),
            None => String::new(),
        }
    }
}

/// Split an optional `repo:` qualifier. A qualifier is a registry-id-shaped prefix.
pub fn split_repo(spec: &str) -> (Option<&str>, &str) {
    if let Some((r, p)) = spec.split_once(':')
        && !r.is_empty()
        && r.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.".contains(c))
    {
        return (Some(r), p);
    }
    (None, spec)
}

/// The qualifier as written: the text before the first `:` when it contains no `/`, `[` or
/// `{` (a `:` inside a class or brace group, as in `[a:b]/x.md`, is part of the pattern).
/// [`split_repo`] only recognizes registry-id-shaped qualifiers, so `Mobile:app/**` parses
/// as a literal pattern that never matches; validation checks this text against the registry.
pub fn written_qualifier(spec: &str) -> Option<&str> {
    spec.split_once(':')
        .map(|(r, _)| r)
        .filter(|r| !r.contains(['/', '[', '{']))
}

fn literal_prefix(p: &str) -> &str {
    let end = p.find(['*', '?', '[', '{']).unwrap_or(p.len());
    &p[..end]
}

fn validate_pattern(p: &str) -> Result<(), String> {
    if p.is_empty() {
        return Err("empty glob".into());
    }
    if p.len() > 1024 {
        return Err("glob is too long".into());
    }
    if p.starts_with('/') {
        return Err(format!("glob `{p}` must be relative"));
    }
    if p.contains('\\') || p.contains('\0') {
        return Err(format!("glob `{p}` contains a backslash or NUL"));
    }
    if p.split('/').any(|s| s == ".." || s == ".") {
        return Err(format!("glob `{p}` must not contain `.` or `..` segments"));
    }
    Ok(())
}

/// All directory prefixes of a path, from "" to the parent directory, each ending with `/`
/// (except the empty root). Used to query `literal_dir_prefix` index rows.
pub fn dir_prefixes(path: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut acc = String::new();
    let parts: Vec<&str> = path.split('/').collect();
    for seg in &parts[..parts.len().saturating_sub(1)] {
        acc.push_str(seg);
        acc.push('/');
        out.push(acc.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_semantics() {
        let g = RepoGlob::parse("app/src/**/auth/*.kt").unwrap();
        assert!(g.matches(Some("mobile"), "app/src/main/java/auth/Token.kt"));
        assert!(!g.matches(Some("mobile"), "app/src/main/java/auth/x/Token.kt"));
        assert_eq!(g.literal_dir_prefix(), "app/src/");
        let q = RepoGlob::parse("backend:src/**").unwrap();
        assert!(q.matches(Some("backend"), "src/a/b.rs"));
        assert!(!q.matches(Some("mobile"), "src/a/b.rs"));
        assert!(RepoGlob::parse("../x").is_err());
        assert!(RepoGlob::parse("/x").is_err());
        assert_eq!(written_qualifier("backend:src/**"), Some("backend"));
        assert_eq!(written_qualifier("Mobile:app/**"), Some("Mobile"));
        assert_eq!(RepoGlob::parse("Mobile:app/**").unwrap().repo, None);
        assert_eq!(written_qualifier("docs/a:b.md"), None);
        assert_eq!(written_qualifier("app/**"), None);
        // A `:` inside a class or brace group is pattern text: the glob stays unqualified.
        for spec in ["[a:b]/x.md", "{a:b,c}/x.md", "x[:]y.md"] {
            assert_eq!(written_qualifier(spec), None, "{spec}");
            let g = RepoGlob::parse(spec).unwrap();
            assert_eq!(g.repo, None, "{spec}");
        }
        assert!(
            RepoGlob::parse("[a:b]/x.md")
                .unwrap()
                .matches(Some("mobile"), "b/x.md")
        );
        assert!(
            RepoGlob::parse("{a:b,c}/x.md")
                .unwrap()
                .matches(None, "c/x.md")
        );
    }

    #[test]
    fn prefixes() {
        assert_eq!(dir_prefixes("a/b/c.rs"), vec!["", "a/", "a/b/"]);
        assert_eq!(dir_prefixes("c.rs"), vec![""]);
    }
}
