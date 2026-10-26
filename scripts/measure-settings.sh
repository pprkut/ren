#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Measures what the settings dialog and About Ren cost (M8b): ren with a
# new, empty home (no account, no database) opens the settings dialog,
# shows its web page settings, closes it, then opens About Ren, shows its
# third-party licenses and closes it; RSS at each step. Twice, release
# builds with the default features. Needs a graphical session; don't
# touch the windows while it runs.
#
# Usage: scripts/measure-settings.sh
#
# Environment:
#   OUT_DIR   logs (default target/measure/settings-<timestamp>)
#   RUNS      runs (default 2)
#   FEATURES  cargo features instead of the default ones, e.g.
#             "renderer-software,servo" (built with --no-default-features)
#
# Prints a Markdown summary, also written to $OUT_DIR/summary.md, and the
# results as BMF JSON in $OUT_DIR/results.json (scripts/bmf.sh).

set -euo pipefail
cd "$(dirname "$0")/.."

OUT_DIR=${OUT_DIR:-target/measure/settings-$(date +%Y%m%d-%H%M%S)}
# Not passed on to cargo: ring's build script runs again when OUT_DIR
# changes, and with it much of Servo's helper is built again.
export -n OUT_DIR
RUNS=${RUNS:-2}

if [[ -z ${DISPLAY:-} && -z ${WAYLAND_DISPLAY:-} ]]; then
    echo "no graphical session (neither DISPLAY nor WAYLAND_DISPLAY is set)" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"
# shellcheck source=scripts/bmf.sh
source scripts/bmf.sh
bmf_init "$OUT_DIR/results.json"
echo "== building" >&2
if [[ -n ${FEATURES:-} ]]; then
    cargo build --release --locked --no-default-features --features "$FEATURES"
else
    cargo build --release --locked
fi
cp target/release/ren "$OUT_DIR/"

# The RSS in MiB at a step of log $1; records the step's memory for BMF.
rss() {
    local line
    line=$({ grep "^ren: rss $2:" "$1" || true; } | tail -n 1)
    bmf_kib "settings: $2" rss "$(sed -n 's/.*: \([0-9]*\) KiB (anon.*/\1/p' <<<"$line")"
    bmf_kib "settings: $2" anonymous-memory "$(sed -n 's/.*(anon \([0-9]*\),.*/\1/p' <<<"$line")"
    sed -n 's/.*: \([0-9]*\) KiB (anon.*/\1/p' <<<"$line" |
        awk '{ printf "%.1f", $1 / 1024 }'
}

steps=("at the start" "settings open" "web pages shown" "settings closed" "about open" "licenses shown" "about closed")
summary=$OUT_DIR/summary.md
{
    echo "## The settings dialog, $(date -u +'%Y-%m-%d %H:%M UTC')"
    echo
    echo "- ren: $(git describe --always --dirty), release build, features: ${FEATURES:-default}"
    echo "- session: ${XDG_SESSION_TYPE:-unknown}, ${XDG_CURRENT_DESKTOP:-unknown}"
    echo
    echo "RSS in MiB:"
    echo
    echo "| run | at the start | settings open | web pages shown | settings closed | About open | licenses shown | About closed |"
    echo "|---|---|---|---|---|---|---|---|"
} >"$summary"
for run in $(seq 1 "$RUNS"); do
    echo "== run $run" >&2
    log=$OUT_DIR/run-$run.log
    home=$OUT_DIR/home-$run
    rm -rf "$home"
    mkdir -p "$home"
    # XDG directories must be absolute.
    home=$(realpath "$home")
    # A home of its own: the user's settings, database and keyring stay
    # out of it.
    HOME=$home XDG_CONFIG_HOME=$home/config XDG_DATA_HOME=$home/data \
        XDG_STATE_HOME=$home/state XDG_CACHE_HOME=$home/cache \
        "$OUT_DIR/ren" --measure-settings 2>"$log" || true
    row="| $run"
    for step in "${steps[@]}"; do
        row+=" | $(rss "$log" "$step")"
    done
    echo "$row |" >>"$summary"
done
bmf_write
cat "$summary"
