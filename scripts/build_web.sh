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

[ -d crates/tt-web/assets ] || scripts/prepare_web_assets.sh

# Trunk needs a wasm-bindgen CLI matching the crate version (0.2.128 here).
# Locally we keep one in tools/bin; in CI trunk downloads the matching version.
[ -d tools/bin ] && export PATH="$PWD/tools/bin:$PATH"
cd crates/tt-web
env -u NO_COLOR trunk build ${PROFILE} ${PUBLIC_URL:-}
echo "web build: $(du -sh dist | cut -f1) in crates/tt-web/dist"
