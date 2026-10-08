#!/bin/sh
# Normalize saved platform responses offline. No API calls and no execution of text.
# Usage: sh normalize-change.sh github|gitlab registry-id raw-directory
# raw-directory: change.json plus comments.json (flat array including reviews/threads).
set -eu
kb_platform=$1
kb_repo=$2
kb_raw=$3
case "$kb_platform" in
  github)
    jq -e -n --arg repo "$kb_repo" --slurpfile mr "$kb_raw/change.json" --slurpfile comments "$kb_raw/comments.json" '
      $mr[0] as $m | if $m.merged != true then error("PR is not merged") else
      {protocol:"kb.change.v1",repo:$repo,base:$m.base.sha,head:$m.head.sha,title:$m.title,merged:true,
       review_comments: ($comments[0] | map(select((.body // "") != "") |
         {author:(.user.login // "unknown"),body:.body,path:(.path // null),commit:(.commit_id // null)}))} end'
    ;;
  gitlab)
    jq -e -n --arg repo "$kb_repo" --slurpfile mr "$kb_raw/change.json" --slurpfile comments "$kb_raw/comments.json" '
      $mr[0] as $m | if $m.state != "merged" then error("MR is not merged") else
      {protocol:"kb.change.v1",repo:$repo,base:$m.diff_refs.base_sha,head:$m.diff_refs.head_sha,title:$m.title,merged:true,
       review_comments: ($comments[0] | map(select((.system // false) == false and (.body // "") != "") |
         {author:(.author.username // "unknown"),body:.body,path:(.position.new_path // .position.old_path // null),commit:(.position.head_sha // null)}))} end'
    ;;
  *) printf '%s\n' 'Expected github or gitlab' >&2; exit 10 ;;
esac
