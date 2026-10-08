#!/bin/sh
# CI glue, run from the downstream KB root after bootstrapping ./kbw.
set -eu
kb_hosts=${KB_EVIDENCE_HOSTS:-project/ci/hosts.tsv}
kb_reports=${KB_REPORT_DIR:-.cache/knowledge-ci}
kb_day=${KB_REVIEW_DATE:-$(date -u +%Y-%m-%d)}
mkdir -p "$kb_reports"
kb_reports=$(mktemp -d "$kb_reports/run.XXXXXX")
set -- --snapshot working-tree --offline --json
if [ -f "$kb_hosts" ]; then
    kb_tab=$(printf '\t')
    while IFS="$kb_tab" read -r kb_repo kb_path kb_sha kb_extra || [ -n "$kb_repo" ]; do
        case "$kb_repo" in ''|'#'*) continue ;; esac
        if [ -z "$kb_path" ] || [ -z "$kb_sha" ] || [ -n "$kb_extra" ]; then
            printf '%s\n' "Invalid host mapping for $kb_repo: expected repo, path and SHA" >&2
            exit 10
        fi
        kb_actual=$(git -C "$kb_path" rev-parse --verify HEAD)
        if [ "$kb_actual" != "$kb_sha" ]; then
            printf '%s\n' "Evidence checkout $kb_repo is not at pinned commit $kb_sha" >&2
            exit 30
        fi
        set -- "$@" --repo-root "$kb_repo=$kb_path"
    done < "$kb_hosts"
fi
kb_failed=0
# Keep every raw result and its native status; do not stop after the first evidence gap.
run_check() {
    kb_name=$1
    shift
    kb_code=0
    "$@" > "$kb_reports/$kb_name.json" 2> "$kb_reports/$kb_name.stderr" || kb_code=$?
    printf '%s\t%s\n' "$kb_name" "$kb_code" >> "$kb_reports/status.tsv"
    if [ "$kb_code" -ne 0 ]; then kb_failed=1; fi
}
run_check validate ./kbw validate --strict --on "$kb_day" --json
run_check routing ./kbw eval routing --strict --json
run_check anchors ./kbw anchors check "$@" --strict
run_check drift ./kbw drift "$@" --since verified --check
run_check ledger ./kbw ledger "$@" --on "$kb_day" --sample 10 --seed "$kb_day" --check
exit "$kb_failed"
