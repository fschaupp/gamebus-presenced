#!/usr/bin/env bash
# Cut a release: gate, set the version, commit, tag.
#
# Usage: ./.scripts/release.sh <X.Y.Z | major | minor | patch>
#
# Runs the full gate (fmt, clippy, tests), builds the release binaries,
# writes the new version into Cargo.toml and Cargo.lock, commits it as
# "release: vX.Y.Z", and creates the annotated tag vX.Y.Z. Nothing is
# pushed: pushing the branch and the tag stays your call, and the pushed
# tag is what triggers the GitHub release workflow.
set -euo pipefail

# Always operate on the repository root, wherever this is invoked from.
cd "$(git rev-parse --show-toplevel)"

usage() {
    echo "usage: $0 <X.Y.Z | major | minor | patch>" >&2
    exit 2
}
[ $# -eq 1 ] || usage

if [ -n "$(git status --porcelain)" ]; then
    echo "release.sh: working tree not clean, commit or stash first" >&2
    exit 1
fi

current=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
IFS=. read -r major minor patch <<<"$current"
case $1 in
major) new="$((major + 1)).0.0" ;;
minor) new="$major.$((minor + 1)).0" ;;
patch) new="$major.$minor.$((patch + 1))" ;;
[0-9]*.[0-9]*.[0-9]*) new=$1 ;;
*) usage ;;
esac

if git rev-parse -q --verify "refs/tags/v$new" >/dev/null; then
    echo "release.sh: tag v$new already exists" >&2
    exit 1
fi

echo "release.sh: $current -> $new"
sed -i "0,/^version = \".*\"$/s//version = \"$new\"/" Cargo.toml

# The same gate the release workflow runs; the build refreshes Cargo.lock
# with the new version.
cargo fmt --check
cargo clippy --all-targets --release -- -D warnings
cargo test --release
cargo build --release

git add Cargo.toml Cargo.lock
if git diff --cached --quiet; then
    # Re-releasing the version already in Cargo.toml (the first release).
    git commit --allow-empty -m "release: v$new"
else
    git commit -m "release: v$new"
fi
git tag -a "v$new" -m "gamebus-presenced $new"

echo
echo "Released v$new. Push with:"
echo "  git push <remote> $(git branch --show-current) v$new"
