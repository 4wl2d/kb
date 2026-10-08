#!/bin/sh
# Run from the HOST checkout. Credentials authorize reads only. Raw inputs are preserved.
# Usage: sh export-merged-change.sh github|gitlab registry-id numeric-MR-id output-directory
set -eu
kb_platform=$1
kb_repo=$2
kb_number=$3
kb_out=$4
case "$kb_number" in ''|*[!0-9]*) printf '%s\n' 'MR id must be numeric' >&2; exit 10 ;; esac
mkdir "$kb_out"
case "$kb_platform" in
  github)
    gh api "repos/{owner}/{repo}/pulls/$kb_number" > "$kb_out/change.json"
    gh api --paginate --slurp "repos/{owner}/{repo}/pulls/$kb_number/comments?per_page=100" > "$kb_out/inline-pages.json"
    gh api --paginate --slurp "repos/{owner}/{repo}/pulls/$kb_number/reviews?per_page=100" > "$kb_out/review-pages.json"
    gh api --paginate --slurp "repos/{owner}/{repo}/issues/$kb_number/comments?per_page=100" > "$kb_out/discussion-pages.json"
    jq -s 'map(add) | add' "$kb_out/inline-pages.json" "$kb_out/review-pages.json" "$kb_out/discussion-pages.json" > "$kb_out/comments.json"
    git fetch --no-tags origin "refs/pull/$kb_number/head"
    ;;
  gitlab)
    glab api "projects/:id/merge_requests/$kb_number" > "$kb_out/change.json"
    glab api --paginate --output json "projects/:id/merge_requests/$kb_number/discussions?per_page=100" > "$kb_out/discussions.json"
    jq '[.[] | .notes[]]' "$kb_out/discussions.json" > "$kb_out/comments.json"
    git fetch --no-tags origin "refs/merge-requests/$kb_number/head"
    ;;
  *) printf '%s\n' 'Expected github or gitlab' >&2; exit 10 ;;
esac
kb_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
sh "$kb_script_dir/normalize-change.sh" "$kb_platform" "$kb_repo" "$kb_out" > "$kb_out/export.json"
kb_base=$(jq -er '.base | select(type == "string" and test("^[0-9a-fA-F]{40}([0-9a-fA-F]{24})?$"))' "$kb_out/export.json")
kb_head=$(jq -er '.head | select(type == "string" and test("^[0-9a-fA-F]{40}([0-9a-fA-F]{24})?$"))' "$kb_out/export.json")
git cat-file -e "$kb_base^{commit}"
git cat-file -e "$kb_head^{commit}"
# This is the reviewed MR's original diff, not an inferred first-parent integrated delta.
# Preserve platform merge/squash metadata in change.json for the human evidence review.
