#!/usr/bin/env bash
#
# Build the deidentification WASM package and place it where the REDCap module
# expects it (src/wasm/dicom_deid/pkg), which is the path js/deidentify-worker.js
# imports at runtime.
#
# Run this once after cloning, and again after any change under wasm/.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATE_DIR="$ROOT_DIR/wasm/dicom_deid"
TARGET_DIR="$ROOT_DIR/src/wasm/dicom_deid/pkg"

if ! command -v wasm-pack >/dev/null 2>&1; then
    echo "wasm-pack is required: https://rustwasm.github.io/wasm-pack/installer/" >&2
    exit 1
fi

echo "==> Building dicom_deid (wasm32)"
wasm-pack build "$CRATE_DIR" --release --target web --out-dir pkg

echo "==> Installing WASM package into src/wasm/dicom_deid/pkg"
rm -rf "$TARGET_DIR"
mkdir -p "$TARGET_DIR"
cp "$CRATE_DIR/pkg/dicom_deid.js" \
   "$CRATE_DIR/pkg/dicom_deid_bg.wasm" \
   "$CRATE_DIR/pkg/dicom_deid.d.ts" \
   "$CRATE_DIR/pkg/dicom_deid_bg.wasm.d.ts" \
   "$TARGET_DIR/"

echo "==> Done: $TARGET_DIR"
