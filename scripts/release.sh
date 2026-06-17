#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <version>" >&2
  exit 1
fi

version="$1"
tag="v${version}"

if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]; then
  echo "invalid version: $version" >&2
  exit 1
fi

if ! command -v jj >/dev/null 2>&1; then
  echo "jj is required" >&2
  exit 1
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "gh is required" >&2
  exit 1
fi

trunk=""
for candidate in main master trunk; do
  if git show-ref --verify --quiet "refs/remotes/origin/${candidate}" || \
     git show-ref --verify --quiet "refs/heads/${candidate}" || \
     jj bookmark list "$candidate" >/dev/null 2>&1; then
    trunk="$candidate"
    break
  fi
done

if [[ -z "$trunk" ]]; then
  trunk="main"
fi

jj bookmark set "$trunk" -r @
jj git push --remote origin --bookmark "$trunk" --allow-empty-description

if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
  echo "tag ${tag} already exists locally" >&2
  exit 1
fi

commit_id="$(jj log -r @ --no-graph -T 'commit_id')"
git tag "$tag" "$commit_id"
git push origin "$tag"

gh release create "$tag" --generate-notes
