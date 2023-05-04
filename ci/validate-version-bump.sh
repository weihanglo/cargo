#!/bin/bash
# This script checks if a crate needs a version bump.
#
# At the time of writing, it doesn't check what kind of bump is required.
# In the future, we could take SemVer compatibliity into account, like
# integrating `cargo-semver-checks` of else
#
# Inputs:
#     PULL_REQUEST_URL    A pull request url on GitHub.

set -euo pipefail

changed_crates=$(
  gh pr view $PULL_REQUEST_URL \
    --json files \
    -q '.files[].path | match("^(crates|credential|benches)/(.*?)/") | .captures[1].string' \
    | sort -u
)

if  [ -z "$changed_crates" ]
then
    echo "No file changed in sub crates."
    exit 0
fi

output=$(echo $changed_crates | xargs printf -- '--package %s\n' | xargs cargo unpublished --check-version-bump)


if  [ -z "$output" ]
then
    echo "No version bump needed for sub crates."
    exit 0
fi

echo "$output"

gh pr comment $PULL_REQUEST_URL --body "$output"

exit 1
