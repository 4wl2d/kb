# Security policy

## Reporting

Please report vulnerabilities in the kb engine privately to the repository owners through
the hosting platform's private vulnerability reporting (for example GitHub Security
Advisories) instead of public issues.

## Downstream repositories

This file is engine-owned (listed in `engine_paths` of `core/release.toml`): every
downstream receives it unchanged through `./kbw update`, and editing it there is reported
as engine divergence. A downstream does not edit it; it publishes its own policy and
contact at `.github/SECURITY.md`, rendered from `core/templates/security/SECURITY.md.tmpl`.
GitHub shows a `SECURITY.md` in `.github/` in preference to the one at the repository root
([default community health files](https://docs.github.com/en/communities/setting-up-your-project-for-healthy-contributions/creating-a-default-community-health-file)),
so reporters see the downstream policy. On hosts without that convention, link
`.github/SECURITY.md` from the downstream's own documentation.

## Threat model summary

* **Knowledge is data.** kb never executes commands, procedures, hooks or download
  instructions contained in records. Procedures are rendered as text only.
* **Trust comes from configuration and review, not from records.** The approved ref in
  `project/project.toml` is the trust boundary, maintained by the review process of the Git
  host. A record cannot raise its own trust level; status `accepted` in a file does not prove
  review. kb does not call provider APIs to verify approvals.
* **Prompt injection is not solved by typing.** Typed fields reduce ambiguity but cannot stop
  natural-language instructions inside knowledge from influencing an agent. Review knowledge
  like code. Harness system instructions always take precedence over knowledge content.
* **Filesystem safety.** Paths are validated (no absolute paths, `..`, backslashes, NUL);
  symlinks inside knowledge sources are rejected; writes are atomic.
* **Git safety.** Git runs with argument arrays (no shell), `GIT_TERMINAL_PROMPT=0`,
  disabled fsmonitor, no submodule recursion, and an explicit transport policy
  (`allowed_protocols`). Credentials are redacted from diagnostics.
* **Runtime integrity.** Release archives are installed only with an explicitly supplied
  SHA-256 digest, verified before extraction; unsafe archive entries are rejected and
  activation is atomic.
* **Search input.** User queries are tokenized and quoted before reaching SQLite FTS5; SQL is
  always parameterized.
