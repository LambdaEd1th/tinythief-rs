#!/usr/bin/env bash
# Acceptance run for the Rust/wgpu Tiny Thief host.
#
#   scripts/verify.sh [path/to/TinyThief.app]
#
# Produces build/verify/*.png and fails if the host does not reach the
# interactive menu or if the scripted click does not change the screen.
set -euo pipefail
cd "$(dirname "$0")/.."

APP="${1:-../TinyThief.app}"
OUT="build/verify"
mkdir -p "$OUT"
rm -rf saves

echo "== build"
cargo build -p tt-host --quiet

echo "== unit tests (asset pipeline)"
cargo test -p tt-assets -p tt-platform --quiet

echo "== boot to interactive menu (600 frames = 20s of game time)"
./target/debug/tt-host "$APP" TinyThief.swf --capture "$OUT/menu.png" --frames 600

echo "== no default unlocks: a fresh save must contain no story progress"
cargo run -q -p tt-host --example dump_sol -- saves/TinyThief.swf/tinyThiefDataNew.sol \
    > "$OUT/fresh_save.txt" 2>/dev/null
if grep -qE '^history[0-9]' "$OUT/fresh_save.txt"; then
    echo "FAIL: fresh save already contains story unlocks"
    grep -E '^history[0-9]' "$OUT/fresh_save.txt"
    exit 1
fi
if grep -q '^NumberPremiumHints' "$OUT/fresh_save.txt"; then
    echo "FAIL: fresh save already contains a hint balance"
    exit 1
fi

echo "== scripted playthrough: menu -> episode intro -> level card -> gameplay"
./target/debug/tt-host "$APP" TinyThief.swf \
    --capture "$OUT/gameplay.png" --frames 4200 \
    --click 512,660@640 --click 512,672@1500 --click 512,592@2200 >/dev/null

echo "== tap the floor (gameplay input), capture at 3600"
./target/debug/tt-host "$APP" TinyThief.swf \
    --capture "$OUT/gameplay_walk.png" --frames 3600 \
    --click 512,660@640 --click 512,672@1500 --click 512,592@2200 \
    --click 700,600@2400 >/dev/null

echo "== save data written?"
SAVES=$(find saves -name '*.sol' | wc -l | tr -d ' ')
[ "$SAVES" -ge 1 ] || { echo "FAIL: no SharedObject written"; exit 1; }

echo "== interaction must change the screen"
if cmp -s "$OUT/menu.png" "$OUT/gameplay.png"; then
    echo "FAIL: clicks did not change the screen"
    exit 1
fi

echo "== lifecycle pause must freeze the frame"
./target/debug/tt-host "$APP" TinyThief.swf --capture "$OUT/pause_end.png" --frames 900 \
    --pause-after 700 --pause-frames 150 \
    --snapshot 720:"$OUT/pause_a.png" --snapshot 840:"$OUT/pause_b.png" >/dev/null
if ! cmp -s "$OUT/pause_a.png" "$OUT/pause_b.png"; then
    echo "FAIL: simulator pause did not freeze the stage"
    exit 1
fi

echo "== captures must be deterministic"
./target/debug/tt-host "$APP" TinyThief.swf --capture "$OUT/det_a.png" --frames 900 >/dev/null
./target/debug/tt-host "$APP" TinyThief.swf --capture "$OUT/det_b.png" --frames 900 >/dev/null
if ! cmp -s "$OUT/det_a.png" "$OUT/det_b.png"; then
    echo "FAIL: two identical runs produced different frames"
    exit 1
fi

echo "== free in-app purchase: buy an episode without paying"
rm -rf saves
# Full scripted path: menu -> comic -> level card -> gameplay -> hints book -> story map -> unlock popup.
C1="--click 512,660@640 --click 512,672@1500 --click 512,592@2200 --click 965,60@2600 --click 845,390@2900"

./target/debug/tt-host "$APP" TinyThief.swf --capture "$OUT/free_iap_popup.png" --frames 3800 \
    $C1 --click 930,520@3300 > "$OUT/free_iap_popup.log" 2>&1
grep -q "IAPInterfaceOnProductInfoReceived" "$OUT/free_iap_popup.log" \
    || { echo "FAIL: product info was never delivered to AS3"; exit 1; }

./target/debug/tt-host "$APP" TinyThief.swf --capture "$OUT/free_iap_owned.png" --frames 4200 \
    $C1 --click 930,520@3300 --click 340,556@3600 > "$OUT/free_iap_buy.log" 2>&1
grep -q "granting purchase for free" "$OUT/free_iap_buy.log" \
    || { echo "FAIL: the Buy button did not trigger a purchase"; exit 1; }
grep -q "IAPInterfaceOnPaymentCompleted" "$OUT/free_iap_buy.log" \
    || { echo "FAIL: purchase result was not delivered to AS3"; exit 1; }
cargo run -q -p tt-host --example dump_sol -- saves/TinyThief.swf/tinyThiefDataNew.sol \
    > "$OUT/save_dump.txt" 2>/dev/null
grep -qE '^history4 +\[true' "$OUT/save_dump.txt" \
    || { echo "FAIL: episode was not unlocked in the save"; head -5 "$OUT/save_dump.txt"; exit 1; }
echo "   episode unlocked by a free purchase: $(grep -E '^history4 ' "$OUT/save_dump.txt")"

echo "== platform services are no-ops (must not write state files)"
if [ -d state ]; then
    echo "FAIL: null platform created a state directory"
    exit 1
fi

echo "== SharedObject must survive a second run (Session_ID increments)"
RUST_LOG=info,tt_bridge=debug ./target/debug/tt-host "$APP" TinyThief.swf \
    --capture "$OUT/rerun.png" --frames 700 > "$OUT/rerun.log" 2>&1
grep -q 'Session_ID": Number(' "$OUT/rerun.log" \
    || { echo "FAIL: no session counter in the second run"; exit 1; }
if grep -q 'Session_ID": Number(1.0)' "$OUT/rerun.log"; then
    echo "FAIL: session counter reset -> save data was not read back"
    exit 1
fi

echo "== checksums"
md5 -q "$OUT/menu.png" "$OUT/gameplay.png" "$OUT/gameplay_walk.png" 2>/dev/null \
    || md5sum "$OUT/menu.png" "$OUT/gameplay.png" "$OUT/gameplay_walk.png"

echo "OK: menu -> gameplay verified, $SAVES save file(s)"
