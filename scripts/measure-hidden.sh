#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Measures memory with the window hidden in the system tray (M8): the
# first article shown (and then pages in tabs, if URLs are
# given), then the window hidden, then shown again (and the article instead
# of the page); twice, release builds with the default features. Needs a graphical session; don't touch the window
# while it runs. With URLs, also network access.
#
# Usage: scripts/measure-hidden.sh DUMP_DIR [URL...]
#   DUMP_DIR: a `ren --dump-items` directory, or - for generated items
#
# Environment:
#   OUT_DIR    logs (default target/measure/hidden-<timestamp>)
#   RUNS       runs (default 2)
#   REN        measure this ren binary instead of building one
#   REN_SERVO  the helper for the pages instead of building Servo's, e.g.
#              ren-tabs' pattern-helper (no network needed then)
#
# Prints a Markdown summary, also written to $OUT_DIR/summary.md, and the
# results as BMF JSON in $OUT_DIR/results.json (scripts/bmf.sh). They
# contain no URLs, titles or article content.

set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $# -lt 1 ]]; then
    sed -n 's/^# \?//p' "$0" | sed -n '4,23p' >&2
    exit 1
fi
if [[ $1 == - ]]; then
    data=()
else
    data=(--dump "$1")
fi
shift
urls=()
for url in "$@"; do
    urls+=(--tab-url "$url")
done
OUT_DIR=${OUT_DIR:-target/measure/hidden-$(date +%Y%m%d-%H%M%S)}
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
if [[ -n ${REN:-} ]]; then
    cp "$REN" "$OUT_DIR/ren"
else
    cargo build --release --locked
    cp target/release/ren "$OUT_DIR/"
fi
if [[ -z ${REN_SERVO:-} ]]; then
    (cd crates/ren-servo && cargo build --release --locked)
    cp target/release/ren-servo "$OUT_DIR/"
fi

mib() { awk -v k="$1" 'BEGIN { printf "%.1f", k / 1024 }'; }

# The RSS in KiB of "<who> <step>:" in log $1, $2 "rss" or "helper rss".
kib() {
    grep "^ren: $2 $3:" "$1" | tail -n 1 | sed -n 's/.*: \([0-9]*\) KiB (anon.*/\1/p'
}

# The anonymous memory in KiB of "<who> <step>:" in log $1.
anon_kib() {
    grep "^ren: $2 $3:" "$1" | tail -n 1 | sed -n 's/.*(anon \([0-9]*\),.*/\1/p'
}

# "total (UI + helper)" in MiB at step $2 of log $1; records the step's
# memory for BMF, as benchmark "window hidden: <$3, or the step>".
memory() {
    local ui helper
    ui=$(kib "$1" rss "$2")
    helper=$(kib "$1" "helper rss" "$2")
    local name="window hidden: ${3:-$2}"
    bmf_kib "$name" rss "$ui"
    bmf_kib "$name" anonymous-memory "$(anon_kib "$1" rss "$2")"
    bmf_kib "$name, helper" rss "$helper"
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
    echo "- ren: $(git describe --always --dirty), release build, ${REN:-default features}"
    echo "- pages in tabs: $((${#urls[@]} / 2))"
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
    "$OUT_DIR/ren" "${data[@]}" --measure-hidden "${urls[@]}" 2>"$log" || true
    if [[ ${#urls[@]} -gt 0 ]]; then
        echo "| $run | $(memory "$log" "at the start") | $(memory "$log" "article shown") | $(memory "$log" shown "pages opened") | $(memory "$log" hidden) | $(memory "$log" "shown again") | $(memory "$log" "article shown again") |" >>"$summary"
    else
        echo "| $run | $(memory "$log" "at the start") | $(memory "$log" shown "article shown") | $(memory "$log" hidden) | $(memory "$log" "shown again") |" >>"$summary"
    fi
done
bmf_write
cat "$summary"
