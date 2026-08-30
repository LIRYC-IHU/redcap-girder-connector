#!/usr/bin/env bash
#
# Bump the module version. REDCap identifies a module version by its directory
# name, so this is the single place the version lives; the release workflow
# derives the folder and zip names from it.
#
# Usage: scripts/bump-version.sh 1.2.0

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NEW_VERSION="${1:-}"

if [[ ! "$NEW_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: scripts/bump-version.sh MAJOR.MINOR.PATCH" >&2
    exit 1
fi

OLD_VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
echo "$NEW_VERSION" > "$ROOT_DIR/VERSION"

echo "$OLD_VERSION -> $NEW_VERSION"
echo
echo "Next steps:"
echo "  1. add a CHANGELOG.md entry for $NEW_VERSION"
echo "  2. git commit -am \"Release v$NEW_VERSION\""
echo "  3. git tag v$NEW_VERSION && git push --follow-tags"
echo
echo "The release workflow then publishes girder_uploader_v$NEW_VERSION.zip."
