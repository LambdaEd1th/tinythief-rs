#!/usr/bin/env bash
# Copy the (encrypted) Tiny Thief app bundle into the web crate for static hosting.
#
#   scripts/prepare_web_assets.sh [path/to/TinyThief.app]
#
# The browser host decrypts these files in memory at runtime; the plaintext
# `decrypted/` tree is NOT needed (and must not be published).
#
# NOTE: the assets are Rovio's copyrighted content. Publish them only if you
# have the right to do so.
set -euo pipefail
cd "$(dirname "$0")/.."

APP="${1:-../TinyThief.app}"
DEST="crates/tt-web/assets"

[ -f "$APP/TinyThief.swf" ] || { echo "no TinyThief.app at $APP" >&2; exit 1; }

rm -rf "$DEST"
mkdir -p "$DEST"
cp "$APP/TinyThief.swf" "$DEST/"
for dir in swf config sounds; do
    [ -d "$APP/$dir" ] && cp -R "$APP/$dir" "$DEST/"
done

echo "assets prepared: $(du -sh "$DEST" | cut -f1) in $DEST"
