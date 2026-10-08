#!/bin/sh
# The fresh publishing job has already run submit-drafts.sh. No agent runs with this token.
# Usage: sh publish-drafts.sh github|gitlab source-branch target-branch description-file
set -eu
[ "${KB_PUBLISH_APPROVED:-}" = 1 ] || { printf '%s\n' 'Draft publication is not enabled' >&2; exit 10; }
kb_platform=$1
kb_branch=$2
kb_target=$3
kb_body=$4
case "$kb_platform" in github|gitlab) ;; *) printf '%s\n' 'Expected github or gitlab' >&2; exit 10 ;; esac
case "$kb_branch" in feature/kb-*) ;; *) printf '%s\n' 'Expected a feature/kb- proposal branch' >&2; exit 10 ;; esac
git check-ref-format --branch "$kb_branch" > /dev/null
git check-ref-format --branch "$kb_target" > /dev/null
# Any existing branch is a review/update decision, never force-pushed or duplicated.
kb_existing=$(git ls-remote --heads origin "refs/heads/$kb_branch")
if [ -n "$kb_existing" ]; then printf '%s\n' "Proposal branch $kb_branch already exists; update its MR through review" >&2; exit 30; fi
if git diff --cached --quiet; then printf '%s\n' 'No new draft changes; no MR created'; exit 0; fi
git switch -c "$kb_branch"
git -c user.name='KB draft bot' -c user.email='kb-drafts@example.invalid' commit -m 'Propose knowledge from a merged host change'
git push --set-upstream origin "$kb_branch"
case "$kb_platform" in
  github) gh pr create --draft --head "$kb_branch" --base "$kb_target" --title 'Draft knowledge from merged change' --body-file "$kb_body" ;;
  gitlab) glab mr create --draft --source-branch "$kb_branch" --target-branch "$kb_target" --title 'Draft knowledge from merged change' --description-file "$kb_body" --yes ;;
  *) printf '%s\n' 'Expected github or gitlab' >&2; exit 10 ;;
esac
