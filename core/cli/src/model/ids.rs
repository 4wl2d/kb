//! Identifier rules.

pub const MAX_RECORD_ID: usize = 128;
pub const MAX_REGISTRY_ID: usize = 64;
pub const MAX_LOCAL_ID: usize = 64;
pub const MAX_NAMESPACE: usize = 32;

// Regular-expression forms of the rules below, used by the generated JSON Schemas.
// `tests/schema_conformance.rs` checks that each pattern agrees with its function.

/// Pattern form of [`check_record_id`] (length limit: [`MAX_RECORD_ID`]).
pub const RECORD_ID_PATTERN: &str = r"^[a-z][a-z0-9]*(-[a-z0-9]+)*(\.[a-z0-9]+(-[a-z0-9]+)*)+$";
/// Pattern form of [`check_registry_id`] (length limit: [`MAX_REGISTRY_ID`]).
pub const REGISTRY_ID_PATTERN: &str = r"^[a-z0-9]+([-_.][a-z0-9]+)*$";
/// Pattern form of [`check_local_id`] (length limit: [`MAX_LOCAL_ID`]).
pub const LOCAL_ID_PATTERN: &str = r"^[a-z0-9]+(-[a-z0-9]+)*$";
/// Pattern form of [`check_namespace`] (length limit: [`MAX_NAMESPACE`]).
pub const NAMESPACE_PATTERN: &str = r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$";
/// Override target `<record-id>#<setting-name>` (part length limits are runtime-only).
pub const OVERRIDE_TARGET_PATTERN: &str =
    r"^[a-z][a-z0-9]*(-[a-z0-9]+)*(\.[a-z0-9]+(-[a-z0-9]+)*)+#[a-z0-9]+(-[a-z0-9]+)*$";
/// Text that is not blank (contains a non-whitespace character).
pub const NON_BLANK_PATTERN: &str = r"\S";
/// A single non-blank line.
pub const SINGLE_LINE_PATTERN: &str = r"^[^\n]*\S[^\n]*$";
/// Abbreviated or full commit id.
pub const COMMIT_PATTERN: &str = r"^[0-9a-fA-F]{7,64}$";

fn is_lower_alnum(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

/// `seg(-seg)*` where seg is `[a-z0-9]+`.
fn is_kebab(s: &str) -> bool {
    !s.is_empty()
        && s.split('-')
            .all(|p| !p.is_empty() && p.chars().all(is_lower_alnum))
}

/// Record id: `ns.seg(.seg)*`, namespace starts with a letter, each segment kebab-case.
pub fn check_record_id(id: &str) -> Result<(), String> {
    if id.len() > MAX_RECORD_ID {
        return Err(format!("id is longer than {MAX_RECORD_ID} bytes"));
    }
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() < 2 {
        return Err("id must be namespaced: `<namespace>.<name>`".into());
    }
    if !parts[0].starts_with(|c: char| c.is_ascii_lowercase()) {
        return Err("namespace must start with a lowercase letter".into());
    }
    if !parts.iter().all(|p| is_kebab(p)) {
        return Err("segments must be lowercase kebab-case (`[a-z0-9]+(-[a-z0-9]+)*`)".into());
    }
    Ok(())
}

pub fn namespace_of(id: &str) -> &str {
    id.split('.').next().unwrap_or("")
}

/// Registry id: `[a-z0-9]+([-_.][a-z0-9]+)*`, ≤ 64 bytes.
pub fn check_registry_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > MAX_REGISTRY_ID {
        return Err(format!("must be 1..={MAX_REGISTRY_ID} bytes"));
    }
    let mut prev_sep = true;
    for c in id.chars() {
        if is_lower_alnum(c) {
            prev_sep = false;
        } else if "-_.".contains(c) {
            if prev_sep {
                return Err("separators must be between alphanumerics".into());
            }
            prev_sep = true;
        } else {
            return Err(format!("invalid character `{c}`"));
        }
    }
    if prev_sep {
        return Err("must not end with a separator".into());
    }
    Ok(())
}

/// Local id inside a record (statement, exception, item, party, setting name).
pub fn check_local_id(id: &str) -> Result<(), String> {
    if id.len() > MAX_LOCAL_ID {
        return Err(format!("longer than {MAX_LOCAL_ID} bytes"));
    }
    if !is_kebab(id) {
        return Err("must be lowercase kebab-case".into());
    }
    Ok(())
}

/// Namespace rule for profiles.
pub fn check_namespace(ns: &str) -> Result<(), String> {
    if !ns.starts_with(|c: char| c.is_ascii_lowercase()) || !is_kebab(ns) || ns.len() > 32 {
        return Err(
            "namespace must be lowercase kebab-case, start with a letter, ≤ 32 bytes".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_ids() {
        assert!(check_record_id("acme.mobile.token-refresh").is_ok());
        assert!(check_record_id("acme.x1").is_ok());
        for bad in [
            "acme",
            "Acme.x",
            "1acme.x",
            "acme..x",
            "acme.x_y",
            "acme.-x",
            "acme.x-",
            "acme.Кот",
        ] {
            assert!(check_record_id(bad).is_err(), "{bad}");
        }
        assert_eq!(namespace_of("acme.a.b"), "acme");
    }

    #[test]
    fn registry_ids() {
        assert!(check_registry_id("mobile.auth").is_ok());
        assert!(check_registry_id("ui_composition-2").is_ok());
        for bad in ["", "-a", "a-", "a..b", "A", "a b"] {
            assert!(check_registry_id(bad).is_err(), "{bad}");
        }
    }
}
