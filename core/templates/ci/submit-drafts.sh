#!/bin/sh
# Run in a fresh KB checkout in the publishing job; agent output is flat *.md data only.
# Usage: sh submit-drafts.sh absolute-drafts-dir host-checkout registry-id reports-dir
set -eu
kb_drafts=$1
kb_host=$2
kb_repo=$3
kb_reports=$4
mkdir "$kb_reports"
if [ -n "$(git status --porcelain --untracked-files=normal)" ]; then
    printf '%s\n' 'Submission needs a clean KB checkout' >&2
    exit 30
fi
kb_count=0
for kb_file in "$kb_drafts"/*.md; do
    [ -e "$kb_file" ] || continue
    if [ ! -f "$kb_file" ] || [ -L "$kb_file" ]; then
        printf '%s\n' 'Only regular draft Markdown files are accepted' >&2
        exit 10
    fi
    kb_count=$((kb_count + 1))
    [ "$kb_count" -le 100 ] || { printf '%s\n' 'At most 100 drafts per change' >&2; exit 10; }
    ./kbw propose submit "$kb_file" --host "$kb_host" --repo "$kb_repo" --snapshot working-tree --offline --json --apply > "$kb_reports/$kb_count.json"
    kb_path=$(jq -er '.result.draft | select(.status == "draft") | .path' "$kb_reports/$kb_count.json")
    git add -- "$kb_path"
done
./kbw validate --strict --json > "$kb_reports/validation.json"
./kbw eval routing --json > "$kb_reports/routing.json"
