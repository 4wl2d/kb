//! Release downloads. Releases live at `<releases>/download/v<version>/<asset>` (latest:
//! `<releases>/latest/download/<asset>`), the layout of GitHub releases. `<releases>` is
//! `$KB_RELEASES_URL`, else the repository's GitHub releases. An `https://` location is
//! downloaded with `curl` (shipped with Windows 10 and later, macOS and common Linux
//! distributions); any other location is a local directory, for mirrors and offline tests.
//! Every archive is verified against `SHA256SUMS` of the same release before use.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{ErrorCode, KbError, Result};

/// GitHub releases of the repository this engine was built from.
pub const DEFAULT_RELEASES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases");

pub const SUMS: &str = "SHA256SUMS";

/// `$KB_RELEASES_URL` (without a trailing `/`), else [`DEFAULT_RELEASES`].
pub fn releases() -> String {
    std::env::var("KB_RELEASES_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .map_or_else(
            || DEFAULT_RELEASES.to_string(),
            |v| v.trim_end_matches('/').to_string(),
        )
}

/// Location of `asset` in release `version` (`None`: the latest release).
pub fn asset(releases: &str, version: Option<&str>, asset: &str) -> String {
    match version {
        Some(v) => format!("{releases}/download/v{v}/{asset}"),
        None => format!("{releases}/latest/download/{asset}"),
    }
}

/// Copy or download `location` to `dest`.
pub fn fetch(location: &str, dest: &Path, quiet: bool) -> Result<()> {
    if location.starts_with("https://") {
        if !quiet {
            eprintln!("kb: downloading {location}");
        }
        let out = Command::new("curl")
            .args([
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--tlsv1.2",
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--retry",
                "2",
                "--connect-timeout",
                "30",
                "--output",
            ])
            .arg(dest)
            .arg(location)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .output()
            .map_err(|e| {
                KbError::new(
                    ErrorCode::DownloadFailed,
                    format!("cannot run curl to download {location}: {e}"),
                )
                .with_hint("install curl, or download the files yourself and pass the archive path")
            })?;
        if !out.status.success() {
            return Err(KbError::new(
                ErrorCode::DownloadFailed,
                format!(
                    "download failed: {location}: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        return Ok(());
    }
    if location.contains("://") {
        return Err(KbError::invalid_input(format!(
            "only https:// URLs and local paths are accepted: {location}"
        )));
    }
    std::fs::copy(location, dest).map_err(|e| {
        KbError::new(
            ErrorCode::DownloadFailed,
            format!("cannot read {location}: {e}"),
        )
    })?;
    Ok(())
}

/// `(digest, file name)` pairs of a `sha256sum`-format file (`<hex>  <name>` or
/// `<hex> *<name>`); malformed lines are skipped.
pub fn parse_sums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim_end_matches('\r');
            let (hex, rest) = (l.get(..64)?, l.get(64..)?);
            let name = rest
                .strip_prefix("  ")
                .or_else(|| rest.strip_prefix(" *"))?;
            (hex.bytes().all(|b| b.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (hex.to_ascii_lowercase(), name.to_string()))
        })
        .collect()
}

/// The single digest recorded for `name`.
pub fn digest_for(text: &str, name: &str) -> Result<String> {
    let found: Vec<String> = parse_sums(text)
        .into_iter()
        .filter(|(_, n)| n == name)
        .map(|(h, _)| h)
        .collect();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(KbError::new(
            ErrorCode::ArtifactInvalid,
            format!("the checksum file does not contain exactly one entry for {name}"),
        )),
    }
}

/// A 64-hex SHA-256 digest, lowercased.
pub fn check_digest(hex: &str) -> Result<String> {
    let hex = hex.trim().to_ascii_lowercase();
    if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(hex)
    } else {
        Err(KbError::invalid_input(
            "the expected sha256 must be 64 hexadecimal characters",
        ))
    }
}

/// The archive file name of a path or URL (a URL loses its query and fragment).
pub fn source_name(source: &str) -> &str {
    let s = if source.contains("://") {
        source.split(['?', '#']).next().unwrap_or(source)
    } else {
        source
    };
    s.rsplit(['/', '\\']).next().unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_and_names() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let text =
            format!("{a}  kb-1-x.tar.gz\r\n{b} *kb-1-y.tar.gz\nnot a line\n{a}  dup\n{a}  dup\n");
        assert_eq!(digest_for(&text, "kb-1-x.tar.gz").unwrap(), a);
        assert_eq!(digest_for(&text, "kb-1-y.tar.gz").unwrap(), "b".repeat(64));
        assert!(digest_for(&text, "dup").is_err());
        assert!(digest_for(&text, "missing").is_err());
        assert_eq!(
            source_name("https://h/x/kb-1.tar.gz?token=1#f"),
            "kb-1.tar.gz"
        );
        assert_eq!(source_name("dist/kb-1.tar.gz"), "kb-1.tar.gz");
        assert_eq!(
            asset("https://h/r", Some("1.2.3"), "S"),
            "https://h/r/download/v1.2.3/S"
        );
        assert_eq!(
            asset("https://h/r", None, "S"),
            "https://h/r/latest/download/S"
        );
        assert!(check_digest("xyz").is_err());
        assert_eq!(check_digest(&b).unwrap(), "b".repeat(64));
    }
}
