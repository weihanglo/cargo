#!/bin/bash
# This script checks if a crate needs a version bump.
#
# At the time of writing, it doesn't check what kind of bump is required.
# In the future, we could take SemVer compatibliity into account, like
# integrating `cargo-semver-checks` of else
#
# Inputs:
#     BASE_SHA    The commit SHA of the branch where the PR wants to merge into.
#     COMMIT_SHA  The commit SHA that triggered the workflow.

set -euo pipefail

# When `BASE_SHA` is missing, we assume it is from bors merge commit,
# so `HEAD~` should find the previous commit on master branch.
base_sha=$(git rev-parse "${BASE_SHA:-HEAD~}")
commit_sha=$(git rev-parse "${COMMIT_SHA:-HEAD}")

echo "Base branch is $base_sha"
echo "The current is $commit_sha"

changed_crates=$(
    git diff --name-only "$base_sha" "$commit_sha" -- crates/ credential/ benches/ \
    | cut  -d'/' -f2 \
    | sort -u
)

git log -n 10 --oneline

git diff --name-only "$base_sha" "$commit_sha"

echo $changed_crates

if  [ -z "$changed_crates" ]
then
    echo "No file changed in sub crates."
    exit 0
fi

# output=$(
#     echo "$changed_crates" \
#     | xargs printf -- '--package %s\n' \
#     | xargs cargo unpublished --check-version-bump
# )
# 
# if  [ -z "$output" ]
# then
#     echo "No version bump needed for sub crates."
#     exit 0
# fi
# 
# echo "$output"
# exit 1
