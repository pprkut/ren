#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Runs the measurements that are steady on CI's shared runners
# (docs/decisions/0019-ci.md, "Performance tracking"), for Bencher: ren's
# release binary's size, stripped and unstripped; the window's memory with
# the window hidden (measure-hidden.sh, generated items and one page) and
# with the settings dialog (measure-settings.sh); the tabs' memory
# (measure-tabs.sh, its synthetic variant); and the store
# (`just measure-store`). The pages come from ren-tabs' pattern-helper, a
# stand-in for Servo's helper, so neither Servo nor the network is
# needed. Without a display it runs under Xvfb (xvfb-run), with the
# software renderer. Time, CPU and frames vary too much on shared runners
# and are left out, but for the store's.
#
# Usage: scripts/measure-ci.sh
#
# Environment:
#   OUT_DIR   logs and results (default target/measure/ci-<timestamp>)
#   RUNS      runs of measure-hidden.sh and measure-settings.sh (default 3)
#   FEATURES  cargo features instead of the default ones, e.g.
#             "renderer-software,servo" (built with --no-default-features;
#             the tabs need servo)
#
# Writes a Markdown summary to $OUT_DIR/summary.md and the results as BMF
# JSON to $OUT_DIR/results.json (scripts/bmf.sh), and prints the summary.

set -euo pipefail
cd "$(dirname "$0")/.."

if [[ -z ${DISPLAY:-} && -z ${WAYLAND_DISPLAY:-} ]]; then
    # The screen of a common desktop; the window opens at its default size.
    exec xvfb-run -a -s "-screen 0 1920x1080x24" "$0" "$@"
fi

OUT_DIR=${OUT_DIR:-target/measure/ci-$(date +%Y%m%d-%H%M%S)}
# Not passed on to cargo: ring's build script runs again when OUT_DIR
# changes.
export -n OUT_DIR
export RUNS=${RUNS:-3}
# One build for both sizes: ren is stripped here, as cargo does for the
# release profile. For every build, so the scripts' builds (pattern-helper)
# share the dependencies built for ren.
export CARGO_PROFILE_RELEASE_STRIP=false
# The software renderer, whatever the environment says.
unset SLINT_BACKEND

mkdir -p "$OUT_DIR"
# shellcheck source=scripts/bmf.sh
source scripts/bmf.sh
bmf_init "$OUT_DIR/binary.json"
log() { echo "$*" >&2; }

log "== building"
# For the scripts' summaries, which measure the binary built here.
export FEATURES=${FEATURES:-}
features=()
if [[ -n $FEATURES ]]; then
    features=(--no-default-features --features "$FEATURES")
fi
cargo build --release --locked "${features[@]}"
cp target/release/ren "$OUT_DIR/ren-unstripped"
strip -o "$OUT_DIR/ren" "$OUT_DIR/ren-unstripped"
cargo build --release --locked -p ren-tabs --example pattern-helper
cp target/release/examples/pattern-helper "$OUT_DIR/"

ren_size=$(stat -c %s "$OUT_DIR/ren")
unstripped_size=$(stat -c %s "$OUT_DIR/ren-unstripped")
bmf "binary: ren" file-size "$ren_size"
bmf "binary: ren, unstripped" file-size "$unstripped_size"
bmf_write

# The pages: the stand-in paints the same page for any of them.
pages=(https://example.org/1 https://example.org/2 https://example.org/3)
REN=$(realpath "$OUT_DIR/ren")
export REN
helper=$(realpath "$OUT_DIR/pattern-helper")

log "== window hidden"
OUT_DIR=$OUT_DIR/hidden REN_SERVO=$helper \
    scripts/measure-hidden.sh - "${pages[0]}" >/dev/null
log "== settings"
OUT_DIR=$OUT_DIR/settings scripts/measure-settings.sh >/dev/null
log "== tabs"
OUT_DIR=$OUT_DIR/tabs VARIANTS=synthetic scripts/measure-tabs.sh - "${pages[@]}" >/dev/null
log "== store"
just measure-store 63000 "$OUT_DIR/store" >/dev/null

mib() { awk -v b="$1" 'BEGIN { printf "%.2f", b / 1048576 }'; }

{
    echo "# ren measurements in CI, $(date -u +'%Y-%m-%d %H:%M UTC')"
    echo
    echo "- ren: $(git describe --always --dirty), release build, features: ${FEATURES:-default}"
    echo "- kernel: $(uname -sr)"
    echo "- CPU: $(awk -F': ' '/^model name/ { print $2; exit }' /proc/cpuinfo)"
    echo "- binary size: ren $(mib "$ren_size") MiB, unstripped $(mib "$unstripped_size") MiB"
    echo "- pages from pattern-helper, the stand-in for Servo's helper"
    for name in hidden settings tabs store; do
        echo
        # One level down: each summary starts with a heading of its own.
        sed 's/^#/##/' "$OUT_DIR/$name/summary.md"
    done
} >"$OUT_DIR/summary.md"

# Only what is steady on shared runners: memory and sizes, and the store's
# timings, to see whether they are steady enough for thresholds. The
# anonymous memory after the window is shown again with a page varies by
# up to a MiB: a measure of its own, with a wider threshold.
bmf_merge "$OUT_DIR/all.json" "$OUT_DIR/binary.json" \
    "$OUT_DIR"/{hidden,settings,tabs,store}/results.json
jq 'with_entries(
        if (.key | startswith("store: ")) then .
        else .value |= with_entries(select(.key | IN("rss", "anonymous-memory", "file-size")))
        end)
    | with_entries(select(.value != {}))
    | with_entries(
        if (.key | IN("window hidden: shown again", "window hidden: article shown again")) then
            .value |= with_entries(
                if .key == "anonymous-memory" then .key = "anonymous-memory-unsteady" else . end)
        else . end)' "$OUT_DIR/all.json" >"$OUT_DIR/results.json"

cat "$OUT_DIR/summary.md"
