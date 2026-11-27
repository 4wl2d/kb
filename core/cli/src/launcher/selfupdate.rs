//! `kb self update`: replace the installed `kb` with a release archive, verified against
//! the release's `SHA256SUMS`, its `BUILD-INFO` and a smoke run before the swap. The new
//! binary is staged next to the old one and moved over it by rename (on Windows the running
//! executable is renamed aside first, since it cannot be replaced while it runs).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::store::{self, TempDir};
use super::{archive, engine, fetch, platform};
use crate::cli::args::{GlobalOpts, SelfArgs, SelfCommand, SelfUpdateArgs};
use crate::error::{ErrorCode, KbError, Result};
use crate::output::CommandOutput;
use crate::versions::ENGINE_VERSION;

pub fn run(global: &GlobalOpts, args: &SelfArgs) -> Result<CommandOutput> {
    match &args.command {
        SelfCommand::Update(a) => update(global, a),
    }
}

/// Installation directories that belong to a package manager; updating there is its job.
const MANAGED: [&str; 4] = ["/Cellar/", "/homebrew/", "/nix/store/", "/linuxbrew/"];

fn installed_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().map_err(|e| KbError::io("current executable", e))?;
    let exe = crate::util::canonicalize(&exe).unwrap_or(exe);
    let shown = exe.to_string_lossy().replace('\\', "/");
    if MANAGED.iter().any(|m| shown.contains(m)) {
        return Err(KbError::new(
            ErrorCode::InvalidInput,
            format!("{} is managed by a package manager", exe.display()),
        )
        .with_hint("update kb with that package manager instead"));
    }
    Ok(exe)
}

/// Releases are versioned `MAJOR.MINOR.PATCH[-pre]`; names in `SHA256SUMS` are
/// `kb-<version>-<target>.tar.gz`.
fn version_in(sums: &str, target: &str) -> Option<String> {
    let suffix = format!("-{target}.tar.gz");
    fetch::parse_sums(sums).into_iter().find_map(|(_, name)| {
        let v = name.strip_prefix("kb-")?.strip_suffix(&suffix)?;
        (!v.is_empty()
            && v.chars()
                .all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c)))
        .then(|| v.to_string())
    })
}

fn update(global: &GlobalOpts, a: &SelfUpdateArgs) -> Result<CommandOutput> {
    let exe = installed_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| KbError::internal("the executable has no directory"))?
        .to_path_buf();
    let targets = platform::release_targets();
    if targets.is_empty() {
        return Err(KbError::new(
            ErrorCode::ArtifactInvalid,
            format!(
                "no release archives exist for this platform ({})",
                platform::BUILD_TARGET
            ),
        )
        .with_hint("build kb from source: cargo install --locked --git <repository> kb"));
    }
    remove_stale(&exe);
    // Staged next to the executable, so the final move is a rename on one filesystem.
    let work = TempDir::create_in(&dir, ".kb-update").map_err(|e| {
        e.with_hint(format!(
            "kb needs write access to {} to replace itself; reinstall it with the installer instead",
            dir.display()
        ))
    })?;
    let releases = fetch::releases();
    let wanted = a.version.as_deref().map(|v| v.trim_start_matches('v'));
    let sums_path = work.path().join(fetch::SUMS);
    fetch::fetch(
        &fetch::asset(&releases, wanted, fetch::SUMS),
        &sums_path,
        global.quiet,
    )?;
    let sums = fs::read_to_string(&sums_path).map_err(|e| KbError::io(sums_path.display(), e))?;
    let found = targets.iter().find_map(|t| {
        let v = match wanted {
            Some(v) => fetch::digest_for(&sums, &archive::file_name(v, t))
                .is_ok()
                .then(|| v.to_string()),
            None => version_in(&sums, t),
        };
        v.map(|v| (v, *t))
    });
    let Some((version, target)) = found else {
        return Err(KbError::new(
            ErrorCode::ArtifactInvalid,
            format!(
                "the release {} has no archive for this platform ({})",
                wanted.map_or_else(|| "latest".to_string(), |v| format!("v{v}")),
                targets.join(", ")
            ),
        ));
    };
    let current = ENGINE_VERSION;
    let result = |status: &str| json!({"from": current, "to": version, "status": status, "path": exe, "target": target});
    if a.check {
        let text = if version == current {
            format!("kb {current} is up to date\n")
        } else {
            format!("kb {version} is available (installed: {current}); run `kb self update`\n")
        };
        let status = if version == current {
            "up-to-date"
        } else {
            "available"
        };
        return Ok(CommandOutput::new(result(status), text));
    }
    if version == current && !a.force {
        return Ok(CommandOutput::new(
            result("up-to-date"),
            format!("kb {current} is up to date ({})\n", exe.display()),
        ));
    }
    let name = archive::file_name(&version, target);
    let expected = fetch::digest_for(&sums, &name)?;
    let archive_path = work.path().join("archive.tar.gz");
    fetch::fetch(
        &fetch::asset(&releases, Some(&version), &name),
        &archive_path,
        global.quiet,
    )?;
    let x = work.path().join("x");
    fs::create_dir(&x).map_err(|e| KbError::io(x.display(), e))?;
    let (top, info, _) =
        engine::verify_archive(&archive_path, &expected, &version, target, None, &x)?;
    let new = top.join(platform::exe_for(target));
    let fp = info.get("fingerprint").unwrap_or("unknown").to_string();
    let scratch = work.path().join("smoke");
    fs::create_dir(&scratch).map_err(|e| KbError::io(scratch.display(), e))?;
    store::smoke_expect(&new, &scratch, &version, &fp).map_err(|e| {
        KbError::new(
            ErrorCode::ArtifactInvalid,
            format!("the downloaded kb failed the smoke test ({e}); the installed kb is unchanged"),
        )
    })?;
    replace(&new, &exe)?;
    if !global.quiet {
        eprintln!("kb: updated {} from {current} to {version}", exe.display());
    }
    Ok(CommandOutput::new(
        result("updated"),
        format!("kb {version} installed at {}\n", exe.display()),
    ))
}

fn old_name(exe: &Path, n: u32) -> PathBuf {
    let name = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    exe.with_file_name(format!(".{name}.old.{n}"))
}

/// Remove executables a previous Windows update renamed aside (they could not be deleted
/// while they ran).
fn remove_stale(exe: &Path) {
    for n in 0..8 {
        let _ = fs::remove_file(old_name(exe, n));
    }
}

fn replace(new: &Path, exe: &Path) -> Result<()> {
    let io = |e: std::io::Error| {
        KbError::new(
            ErrorCode::IoError,
            format!(
                "cannot replace {}: {e}; the installed kb is unchanged",
                exe.display()
            ),
        )
    };
    if cfg!(windows) {
        // A running executable can be renamed but not overwritten.
        let aside = (0..8)
            .map(|n| old_name(exe, n))
            .find(|p| fs::symlink_metadata(p).is_err())
            .ok_or_else(|| io(std::io::Error::other("too many old executables next to it")))?;
        fs::rename(exe, &aside).map_err(io)?;
        if let Err(e) = fs::rename(new, exe) {
            let _ = fs::rename(&aside, exe);
            return Err(io(e));
        }
        let _ = fs::remove_file(&aside);
        Ok(())
    } else {
        fs::rename(new, exe).map_err(io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_version_comes_from_the_archive_name() {
        let a = "a".repeat(64);
        let sums = format!(
            "{a}  kb-1.2.3-aarch64-apple-darwin.tar.gz\n{a}  kb-1.2.3-x86_64-pc-windows-msvc.tar.gz\n"
        );
        assert_eq!(
            version_in(&sums, "aarch64-apple-darwin").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(version_in(&sums, "x86_64-unknown-linux-musl"), None);
        assert_eq!(version_in(&format!("{a}  kb-1 2-x.tar.gz\n"), "x"), None);
    }
}
