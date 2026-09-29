## Summary

<!-- What changes and why. -->

## Knowledge base impact

Fill in the machine-readable block below; CI (`kbw impact --check`) verifies that it is
present and well formed, and reviewers judge the reasoning. Keep it near the top of the
description.

* `kb_change = "none"`: no knowledge is affected; `reason` explains why.
* `kb_change = "linked"`: knowledge changes in a separate KB merge request; uncomment and
  set `kb_revision` (the reviewed KB commit, 7-64 hex characters) and/or `change_id`
  (shared id of both requests).
* `kb_change = "included"`: this merge request itself carries the knowledge change (in a
  host repository: the KB submodule pointer update).

`reason` is required in every mode. Keep unused optional keys commented out: an empty
value is rejected.

<!-- kb-impact:v1
kb_change = "none"
reason = ""
# kb_revision = "<KB commit>"
# change_id = "<shared id>"
-->

### Linked knowledge change (only for `kb_change = "linked"`)

- [ ] Knowledge proposal opened in the KB repository as `status = "draft"` records with
      evidence, or as changes to accepted records.
- [ ] This code was tested against that KB revision (`kbw context --snapshot <revision>` or
      `--include-proposals`).
- [ ] The KB merge request was reviewed and merged into the approved ref first.
- [ ] The host pin (KB submodule pointer) is updated through review, in this or a follow-up
      merge request.
- [ ] Both merge requests share the same `change_id`; neither references a commit SHA of the
      other that does not exist yet (no cyclic SHA dependencies).

Merging two repositories is never atomic: until the pin update lands, the host runs with the
previous KB revision.
