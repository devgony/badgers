#!/usr/bin/env bash

# Print the commit represented by a tag from `git ls-remote --tags` output.
# Annotated tags use the peeled ^{} ref; lightweight tags use the direct ref.
tag_commit_from_ls_remote() {
  local tag=$1
  local direct=
  local peeled=
  local oid
  local ref
  while read -r oid ref; do
    case "$ref" in
      "refs/tags/$tag") direct=$oid ;;
      "refs/tags/$tag^{}") peeled=$oid ;;
    esac
  done
  if [[ -n "$peeled" ]]; then
    printf '%s\n' "$peeled"
  elif [[ -n "$direct" ]]; then
    printf '%s\n' "$direct"
  else
    return 1
  fi
}

remote_tag_commit() {
  local tag=$1
  git ls-remote --tags origin "refs/tags/$tag" "refs/tags/$tag^{}" |
    tag_commit_from_ls_remote "$tag"
}

remote_main_commit() {
  local oid
  local ref
  while read -r oid ref; do
    if [[ "$ref" == refs/heads/main ]]; then
      printf '%s\n' "$oid"
      return 0
    fi
  done < <(git ls-remote --heads origin refs/heads/main)
  return 1
}

github_release_exists() {
  gh release view "$1" >/dev/null 2>&1
}

create_release_reconciled() {
  local tag=$1
  # No --target: the tag already exists on origin at this point, so GitHub
  # attaches the release to it. target_commitish rejects tag names (HTTP 422).
  if gh release create "$tag" \
    --title "$tag" \
    --generate-notes; then
    return 0
  else
    local create_status=$?
  fi

  if github_release_exists "$tag"; then
    echo "GitHub reported an error creating $tag, but the release now exists; continuing."
    return 0
  fi
  echo "error: GitHub Release $tag was not found after creation failed; rerun make release to resume" >&2
  return "$create_status"
}

advance_major_tag() {
  local version=$1
  local commit=$2
  local major=${version%%.*}
  local expected
  expected=$(git ls-remote --refs origin "refs/tags/v$major" | cut -f1)
  if git push --force-with-lease="refs/tags/v$major:$expected" \
    origin "$commit:refs/tags/v$major"; then
    :
  else
    local push_status=$?
    local remote_commit
    remote_commit=$(remote_tag_commit "v$major" || true)
    if [[ "$remote_commit" != "$commit" ]]; then
      return "$push_status"
    fi
    echo "Major tag push reported an error, but origin contains v$major at $commit; continuing."
  fi
  git tag --force "v$major" "$commit" >/dev/null
}
