#!/usr/bin/env bash
# Build the wasm bundle into crates/tt-web/dist.
#
#   scripts/build_web.sh [--public-url /repo/] [--release]
#
# GitHub Pages project sites need --public-url "/<repository>/".
set -euo pipefail
cd "$(dirname "$0")/.."

PUBLIC_URL=""
PROFILE="--release"
while [ $# -gt 0 ]; do
    case "$1" in
        --public-url) PUBLIC_URL="--public-url $2"; shift 2 ;;
        --debug) PROFILE=""; shift ;;
        *) echo "unknown argument: $1" >&2; exit 1 ;;
    esac
done

[ -d assets ] || scripts/prepare_web_assets.sh

# Trunk can only copy directories below the crate root; stage the shared
# `assets/` tree inside the web crate (gitignored) before building.
rm -rf crates/tt-web/assets
cp -R assets crates/tt-web/assets

# Trunk downloads a wasm-bindgen CLI matching the crate version automatically;
# no local toolchain copy is needed (CI does the same).
cd crates/tt-web
env -u NO_COLOR trunk build ${PROFILE} ${PUBLIC_URL:-}
echo "web build: $(du -sh dist | cut -f1) in crates/tt-web/dist"
