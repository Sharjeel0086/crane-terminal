#!/usr/bin/env bash
# Vendor libpdfium.dylib for the PDF viewer Pane.
#
# Downloads pre-built binaries from bblanchon/pdfium-binaries pinned
# to the chromium tag whose ABI matches pdfium-render's `pdfium_latest`
# feature. Bump PDFIUM_TAG together with the pdfium-render version in
# Cargo.toml — the API and ABI move in lockstep.
#
# Layout produced:
#   vendor/pdfium/mac-arm64/libpdfium.dylib
#   vendor/pdfium/mac-x86_64/libpdfium.dylib
#   vendor/pdfium/win-x86_64/pdfium.dll
#
# Idempotent: re-running with the same tag is a no-op.

set -euo pipefail

# Pinned to match pdfium-render 0.9.x → pdfium_7763 ABI.
# Bump alongside Cargo.toml's pdfium-render version.
PDFIUM_TAG="${PDFIUM_TAG:-chromium/7763}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR_DIR="$REPO_ROOT/vendor/pdfium"
STAMP="$VENDOR_DIR/.pinned-tag"

mkdir -p "$VENDOR_DIR/mac-arm64" "$VENDOR_DIR/mac-x86_64" "$VENDOR_DIR/win-x86_64"

if [[ -f "$STAMP" ]] && [[ "$(cat "$STAMP")" == "$PDFIUM_TAG" ]] \
        && [[ -f "$VENDOR_DIR/mac-arm64/libpdfium.dylib" ]] \
        && [[ -f "$VENDOR_DIR/mac-x86_64/libpdfium.dylib" ]] \
        && [[ -f "$VENDOR_DIR/win-x86_64/pdfium.dll" ]]; then
    echo "pdfium $PDFIUM_TAG already vendored"
    exit 0
fi

fetch_arch() {
    local arch_dir="$1"
    local asset="$2"
    local lib_name="$3"
    local url="https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_TAG}/${asset}"
    local tmp
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' RETURN

    echo "fetching $asset ..."
    curl --fail --location --silent --show-error --output "$tmp/pdfium.tgz" "$url"
    tar -xzf "$tmp/pdfium.tgz" -C "$tmp"
    
    # The Windows archive puts it in x64/bin/pdfium.dll or just bin/pdfium.dll,
    # let's just find it inside the extraction dir.
    local src_lib
    src_lib=$(find "$tmp" -name "$lib_name" | head -n 1)
    cp "$src_lib" "$VENDOR_DIR/$arch_dir/$lib_name"
    chmod 0644 "$VENDOR_DIR/$arch_dir/$lib_name"
}

fetch_arch mac-arm64 pdfium-mac-arm64.tgz libpdfium.dylib
fetch_arch mac-x86_64 pdfium-mac-x64.tgz libpdfium.dylib
fetch_arch win-x86_64 pdfium-win-x64.tgz pdfium.dll

echo "$PDFIUM_TAG" > "$STAMP"
echo "vendored: $VENDOR_DIR (tag $PDFIUM_TAG)"
