#!/usr/bin/env sh
# Builds a Git repository from fixtures/pr-impact: base/ committed on `main`,
# head/ committed on `feature`, with `main` checked out.
#
#   scripts/make-pr-fixture.sh /tmp/pr-shop
#   codeatlas diff /tmp/pr-shop --base main --head feature
set -eu

dest=${1:?usage: make-pr-fixture.sh <destination directory>}
fixture=$(cd "$(dirname "$0")/../fixtures/pr-impact" && pwd)

if [ -e "$dest" ]; then
    echo "$dest already exists" >&2
    exit 1
fi

export GIT_AUTHOR_NAME=Fixture GIT_AUTHOR_EMAIL=fixture@example.com
export GIT_COMMITTER_NAME=Fixture GIT_COMMITTER_EMAIL=fixture@example.com

mkdir -p "$dest"
cp -R "$fixture/base/." "$dest/"
git -C "$dest" init --quiet --initial-branch=main
git -C "$dest" add -A
git -C "$dest" commit --quiet -m "Add the shop"
git -C "$dest" checkout --quiet -b feature
find "$dest" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
cp -R "$fixture/head/." "$dest/"
git -C "$dest" add -A
git -C "$dest" commit --quiet -m "Charge in the order's currency"
git -C "$dest" checkout --quiet main
echo "Created $dest (branches: main, feature)"
