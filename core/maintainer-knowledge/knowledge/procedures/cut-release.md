+++
schema = 1
id = "kb.procedure.cut-release"
kind = "procedure"
title = "Cut an upstream release"
status = "accepted"
owner = "maintainers"
preconditions = [
  "CI is green on the release commit.",
  "`engine_version` in core/release.toml equals the crate version and CHANGELOG.md has an entry.",
  "The repository owner has enabled GitHub Actions with permission to create releases.",
]
expected = ["The release workflow builds the macOS arm64, Linux x86_64 and arm64 (musl) and Windows x86_64 archives, verifies each with its installer and `kb self update`, and publishes them with SHA256SUMS."]

[scope]
modules = ["kb.launcher"]

[selectors]
concepts = ["runtime"]
aliases = ["release", "tag", "релиз"]

[[steps]]
id = "tag"
text = "Create and push an annotated tag `v<engine_version>` from the release commit (owner action)."

[[steps]]
id = "verify"
text = "Install the release with install.sh (or install.ps1 on Windows) on a clean machine, run `kb engine info` in a clean checkout of the same tag (the installed kb serves it), and run `kb engine install` in a checkout whose engine is that release."
+++
