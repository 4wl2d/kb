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
expected = ["The release workflow builds macOS arm64 and Linux x86_64 archives and SHA256SUMS; the owner reviews and publishes the release."]

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
text = "Download an archive, compare its digest with SHA256SUMS, and install it with `./kbw --kbw-install-artifact <archive> --sha256 <digest>` on a clean checkout of the same tag."
+++
