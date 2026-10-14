#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Measures memory with the window hidden in the system tray (M8): the
# first article of a dump shown (and then pages in tabs, if URLs are
# given), then the window hidden, then shown again (and the article instead
# of the page); twice, release builds with the default features. Needs a graphical session; don't touch the window
# while it runs. With URLs, also network access.
#
# Usage: scripts/measure-hidden.sh DUMP_DIR [URL...]
#   DUMP_DIR: a `ren --dump-items` directory
#
# Environment:
#   OUT_DIR   logs (default target/measure/hidden-<timestamp>)
#   RUNS      runs (default 2)
#
# Prints a Markdown summary, also written to $OUT_DIR/summary.md. It
# contains no URLs, titles or article content.

set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $# -lt 1 ]]; then
    sed -n 's/^# \?//p' "$0" | sed -n '4,19p' >&2
    exit 1
fi
DUMP=$1
shift
urls=()
for url in "$@"; do
    urls+=(--tab-url "$url")
done
OUT_DIR=${OUT_DIR:-target/measure/hidden-$(date +%Y%m%d-%H%M%S)}
RUNS=${RUNS:-2}

if [[ -z ${DISPLAY:-} && -z ${WAYLAND_DISPLAY:-} ]]; then
    echo "no graphical session (neither DISPLAY nor WAYLAND_DISPLAY is set)" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"
echo "== building" >&2
cargo build --release --locked
(cd crates/ren-servo && cargo build --release --locked)
cp target/release/ren target/release/ren-servo "$OUT_DIR/"

mib() { awk -v k="$1" 'BEGIN { printf "%.1f", k / 1024 }'; }

# The RSS in KiB of "<who> <step>:" in log $1, $2 "rss" or "helper rss".
kib() {
    grep "^ren: $2 $3:" "$1" | tail -n 1 | sed -n 's/.*: \([0-9]*\) KiB (anon.*/\1/p'
}

# "total (UI + helper)" in MiB at a step of log $1.
memory() {
    local ui helper
    ui=$(kib "$1" rss "$2")
    helper=$(kib "$1" "helper rss" "$2")
    if [[ -z $ui ]]; then
        echo "-"
    elif [[ -n $helper ]]; then
        echo "$(mib $((ui + helper))) ($(mib "$ui") + $(mib "$helper"))"
    else
        mib "$ui"
    fi
}

summary=$OUT_DIR/summary.md
{
    echo "## ren with the window hidden, $(date -u +'%Y-%m-%d %H:%M UTC')"
    echo
    echo "- ren: $(git describe --always --dirty), release build, default features"
    echo "- pages in tabs: ${#urls[@]}"
    echo "- session: ${XDG_SESSION_TYPE:-unknown}, ${XDG_CURRENT_DESKTOP:-unknown}"
    echo
    echo "RSS in MiB, total (UI + Servo's helper):"
    echo
    if [[ ${#urls[@]} -gt 0 ]]; then
        echo "| run | at the start | article shown | pages opened | hidden | shown again | article shown again |"
        echo "|---|---|---|---|---|---|---|"
    else
        echo "| run | at the start | article shown | hidden | shown again |"
        echo "|---|---|---|---|---|"
    fi
} >"$summary"
for run in $(seq 1 "$RUNS"); do
    echo "== run $run" >&2
    log=$OUT_DIR/run-$run.log
    "$OUT_DIR/ren" --dump "$DUMP" --measure-hidden "${urls[@]}" 2>"$log" || true
    if [[ ${#urls[@]} -gt 0 ]]; then
        echo "| $run | $(memory "$log" "at the start") | $(memory "$log" "article shown") | $(memory "$log" shown) | $(memory "$log" hidden) | $(memory "$log" "shown again") | $(memory "$log" "article shown again") |" >>"$summary"
    else
        echo "| $run | $(memory "$log" "at the start") | $(memory "$log" shown) | $(memory "$log" hidden) | $(memory "$log" "shown again") |" >>"$summary"
    fi
done
cat "$summary"
