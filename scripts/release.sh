#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
source scripts/release_helpers.sh

current_version=$(python3 scripts/release_version.py current)
version_files=(Cargo.toml Cargo.lock crates/badgers-cli/Cargo.toml)

# Preflight and recovery run before prompting so an interrupted release resumes
# without accidentally selecting and creating another version.
[[ -z "$(git status --porcelain)" ]] || {
  echo 'error: the worktree must be clean' >&2
  git status --short >&2
  exit 1
}
[[ "$(git branch --show-current)" == main ]] || {
  echo 'error: releases must be created from main' >&2
  exit 1
}
gh auth status >/dev/null
git fetch --quiet origin main
head=$(git rev-parse HEAD)
[[ "$head" == "$(git rev-parse origin/main)" ]] || {
  echo 'error: local main must match origin/main' >&2
  exit 1
}

current_tag="v$current_version"
remote_current_tag=$(remote_tag_commit "$current_tag" || true)
if [[ -n "$remote_current_tag" ]] &&
  git merge-base --is-ancestor "$remote_current_tag" "$head"; then
  if ! github_release_exists "$current_tag"; then
    echo "Resuming $current_tag: its commit and immutable tag are already on origin."
    create_release_reconciled "$current_tag"
    advance_major_tag "$current_version" "$remote_current_tag"
    echo "Published GitHub Release $current_tag; crates and binaries are being packaged by GitHub Actions."
    exit 0
  fi

  current_major_tag="v${current_version%%.*}"
  remote_major_tag=$(remote_tag_commit "$current_major_tag" || true)
  if [[ "$remote_major_tag" != "$remote_current_tag" ]]; then
    echo "Resuming $current_tag: its GitHub Release exists but $current_major_tag has not advanced."
    advance_major_tag "$current_version" "$remote_current_tag"
    echo "Completed release bookkeeping for $current_tag."
    exit 0
  fi
fi

bump=${BUMP:-}
if [[ -z "$bump" ]]; then
  if [[ ! -t 0 ]]; then
    echo 'error: interactive release selection requires a terminal; set BUMP=major, minor, or patch' >&2
    exit 2
  fi
  printf 'Current version: %s\n' "$current_version"
  printf '%s\n' \
    'Select the next version:' \
    '  1) major' \
    '  2) minor' \
    '  3) patch'
  read -r -p 'Bump [1/2/3 or major/minor/patch]: ' selection
  case "$selection" in
    1 | major) bump=major ;;
    2 | minor) bump=minor ;;
    3 | patch) bump=patch ;;
    *)
      echo 'error: selection must be exactly 1, 2, 3, major, minor, or patch' >&2
      exit 2
      ;;
  esac
fi

case "$bump" in
  major | minor | patch) ;;
  *)
    echo 'error: BUMP must be exactly major, minor, or patch' >&2
    exit 2
    ;;
esac

version=$(python3 scripts/release_version.py next "$bump")
tag="v$version"
if git rev-parse --verify --quiet "refs/tags/$tag" >/dev/null; then
  echo "error: tag $tag already exists" >&2
  exit 1
fi
if git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1; then
  echo "error: tag $tag already exists on origin" >&2
  exit 1
fi
if gh release view "$tag" >/dev/null 2>&1; then
  echo "error: release $tag already exists" >&2
  exit 1
fi

original_head=$head
tag_created=false
cleanup_allowed=true
restore_on_failure() {
  status=$?
  trap - EXIT
  if [[ $status -ne 0 && "$cleanup_allowed" == true ]]; then
    if [[ "$tag_created" == true ]]; then
      git tag --delete "$tag" >/dev/null
    fi
    if [[ "$(git rev-parse HEAD)" != "$original_head" ]]; then
      git reset --hard "$original_head" >/dev/null
    else
      git restore -- "${version_files[@]}"
    fi
  fi
  exit "$status"
}
trap restore_on_failure EXIT

echo "Preparing $tag from $current_version ($bump bump)"
python3 scripts/release_version.py bump "$bump" >/dev/null

python3 -m unittest discover -s scripts -t .
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
[[ "$(python3 scripts/release_version.py current)" == "$version" ]]

git add -- "${version_files[@]}"
git commit -m "chore: bump badgers to version $version" -- "${version_files[@]}"
git tag -a "$tag" -m "$tag"
tag_created=true

# Push the release commit and immutable version tag together, or neither ref.
cleanup_allowed=false
release_commit=$(git rev-parse HEAD)
if git push --atomic origin "HEAD:refs/heads/main" "refs/tags/$tag"; then
  :
else
  push_status=$?
  remote_main=$(remote_main_commit || true)
  remote_tag=$(remote_tag_commit "$tag" || true)
  if [[ "$remote_main" == "$release_commit" && "$remote_tag" == "$release_commit" ]]; then
    echo "Atomic push reported an error, but origin contains both $tag and the release commit; continuing."
  elif [[ "$remote_main" == "$original_head" && -z "$remote_tag" ]]; then
    cleanup_allowed=true
    echo 'error: atomic push failed before either release ref changed' >&2
    exit "$push_status"
  else
    echo 'error: atomic push left an unexpected remote state; local release state was preserved for manual reconciliation' >&2
    exit "$push_status"
  fi
fi

create_release_reconciled "$tag"
advance_major_tag "$version" "$release_commit"

trap - EXIT
echo "Published GitHub Release $tag; crates and binaries are being packaged by GitHub Actions."
