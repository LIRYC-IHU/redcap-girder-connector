#!/usr/bin/env bash
#
# Package the module for REDCap.
#
# REDCap versions external modules by directory name, so a release is a zip
# containing a single `girder_uploader_v<VERSION>` folder. VERSION is read from
# the VERSION file at the repository root.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
MODULE_NAME="girder_uploader"
MODULE_DIR="${MODULE_NAME}_v${VERSION}"
DIST_DIR="$ROOT_DIR/dist"
STAGING_DIR="$DIST_DIR/$MODULE_DIR"

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "VERSION must be MAJOR.MINOR.PATCH, got '$VERSION'" >&2
    exit 1
fi

"$ROOT_DIR/scripts/build.sh"

echo "==> Staging $MODULE_DIR"
rm -rf "$STAGING_DIR" "$DIST_DIR/$MODULE_DIR.zip"
mkdir -p "$STAGING_DIR"
# -L so the WASM package is copied even when src/wasm is a symlink.
cp -RL "$ROOT_DIR/src/." "$STAGING_DIR/"
find "$STAGING_DIR" -name '.DS_Store' -delete

for required in config.json GirderUploaderModule.php js/girder-uploader.js \
                js/girder-uploader-core.js js/deidentify-worker.js \
                wasm/dicom_deid/pkg/dicom_deid_bg.wasm; do
    if [[ ! -f "$STAGING_DIR/$required" ]]; then
        echo "missing from the package: $required" >&2
        exit 1
    fi
done

echo "==> Zipping dist/$MODULE_DIR.zip"
(cd "$DIST_DIR" && zip -qr "$MODULE_DIR.zip" "$MODULE_DIR")

echo "==> Done: dist/$MODULE_DIR.zip"
