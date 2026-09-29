//! Text normalization for aliases and queries (docs/architecture.md §4.1).
//!
//! Steps: Unicode NFKC; Unicode lowercase; `ё` → `е`; every non-alphanumeric char becomes
//! a space; whitespace is collapsed. Matching is on whole-token sequences; an alias ending
//! in `*` matches its last token by prefix. There is no stemming.

use unicode_normalization::UnicodeNormalization;

/// Normalize free text into a single space-separated token string.
pub fn normalize(text: &str) -> String {
    tokens(text).join(" ")
}

/// Normalize free text into tokens.
pub fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.nfkc() {
        for lc in ch.to_lowercase() {
            let lc = if lc == 'ё' { 'е' } else { lc };
            if lc.is_alphanumeric() {
                cur.push(lc);
            } else if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A compiled alias phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasPattern {
    /// Normalized tokens. When `prefix` is true, the last token matches by prefix.
    pub tokens: Vec<String>,
    pub prefix: bool,
}

impl AliasPattern {
    /// Compile an alias. Returns `None` if the alias normalizes to nothing.
    pub fn compile(alias: &str) -> Option<AliasPattern> {
        let trimmed = alias.trim();
        let prefix = trimmed.ends_with('*');
        let body = trimmed.trim_end_matches('*');
        let toks = tokens(body);
        if toks.is_empty() {
            return None;
        }
        Some(AliasPattern {
            tokens: toks,
            prefix,
        })
    }

    /// Canonical string form (used as index key): tokens joined by space, `*` suffix if prefix.
    pub fn key(&self) -> String {
        let mut k = self.tokens.join(" ");
        if self.prefix {
            k.push('*');
        }
        k
    }

    /// Does the pattern occur as a contiguous token sequence in `hay`?
    pub fn matches(&self, hay: &[String]) -> bool {
        self.find(hay).is_some()
    }

    /// Position of the first occurrence in `hay`.
    pub fn find(&self, hay: &[String]) -> Option<usize> {
        let n = self.tokens.len();
        if n == 0 || hay.len() < n {
            return None;
        }
        'outer: for start in 0..=hay.len() - n {
            for (i, t) in self.tokens.iter().enumerate() {
                let h = &hay[start + i];
                let last = i + 1 == n;
                let ok = if last && self.prefix {
                    h.starts_with(t.as_str())
                } else {
                    h == t
                };
                if !ok {
                    continue 'outer;
                }
            }
            return Some(start);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_cyrillic_and_punctuation() {
        assert_eq!(
            normalize("  Композиция — ЁЛКА!  UI-Composition "),
            "композиция елка ui composition"
        );
        assert_eq!(tokens("ﬁle"), vec!["file"]); // NFKC ligature
    }

    #[test]
    fn alias_matching() {
        let hay = tokens("Нужно поправить композицию экранов в UI");
        assert!(AliasPattern::compile("композици*").unwrap().matches(&hay));
        assert!(!AliasPattern::compile("композиция").unwrap().matches(&hay));
        assert!(AliasPattern::compile("в ui").unwrap().matches(&hay));
        assert!(AliasPattern::compile("***").is_none());
        assert_eq!(
            AliasPattern::compile("Token  Refresh*").unwrap().key(),
            "token refresh*"
        );
    }
}
