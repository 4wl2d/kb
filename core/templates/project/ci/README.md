# Knowledge CI

Init installs `.github/workflows/kb-knowledge.yml`, `.gitlab/ci/kb-knowledge.yml` and KB
review templates. Include the GitLab file from the team's `.gitlab-ci.yml`; init never
overwrites an existing CI entrypoint. Both platforms run the shared engine-owned evidence
check script. Configure host checkout steps and pinned `hosts.tsv` mappings before relying
on source claims. No host checkout or credential is inferred from a remote URL.

The jobs run strict validation, Tier A routing, anchor checks, owner drift queue, ledger,
generated integration checks and engine/schema divergence checks. Raw reports and native
exit statuses are retained even after a failure. `--on` is explicitly set to the job's UTC
date for calendar deadlines. Source support and routing metrics do not establish agent
quality or prove a test was executed.

Host accrual jobs are opt-in templates under `core/templates/ci/{github,gitlab}`; they need
a reviewed team agent wrapper and separate publisher authority. Updating the engine only
updates templates, not these downstream-owned CI files.
