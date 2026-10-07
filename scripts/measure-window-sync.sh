#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Measures syncing in the window: ren opens a new database, runs a full
# sync and an incremental one on its sync thread, and quits. Time, CPU and
# memory before and after each sync, the peak, and how long reading the
# lists again takes while the items arrive. Uses the account from the
# settings file (see `ren --help`). Needs a graphical session; don't touch
# the window while it runs.
#
# Only reads from the server: a new database has no local changes to send.
#
# Usage: scripts/measure-window-sync.sh
#
# Environment:
#   SETTINGS  settings file (default: $XDG_CONFIG_HOME/ren/settings.toml)
#   OUT_DIR   raw output (default target/measure/window-sync-<timestamp>)
#
# Prints a Markdown summary, also written to $OUT_DIR/summary.md. The
# server URL is not part of it.

set -euo pipefail
cd "$(dirname "$0")/.."

OUT_DIR=${OUT_DIR:-target/measure/window-sync-$(date +%Y%m%d-%H%M%S)}

if [[ -z ${DISPLAY:-} && -z ${WAYLAND_DISPLAY:-} ]]; then
    echo "no graphical session (neither DISPLAY nor WAYLAND_DISPLAY is set)" >&2
    exit 1
fi

# The default features: the binary as it is used.
cargo build --release --locked
bin=target/release/ren
args=()
if [[ -n ${SETTINGS:-} ]]; then
    args+=(--settings "$SETTINGS")
fi
mkdir -p "$OUT_DIR"
# Real feed data stays outside the repository.
db_dir=$(mktemp -d -t ren-measure-window-sync.XXXXXX)
trap 'rm -rf "$db_dir"' EXIT

log=$OUT_DIR/ren.log
"$bin" "${args[@]}" --database "$db_dir/ren.db" --measure-sync 2>"$log"

# MiB from the "ren: rss $1: <n> KiB (... peak <m> KiB)" line; $2: field 1
# (RSS) or 2 (peak).
rss() {
    sed -n "s/^ren: rss $1: \([0-9]*\) KiB.*peak \([0-9]*\) KiB)$/\\$2/p" "$log" |
        awk '{ printf "%.1f", $1 / 1024 }'
}

took() { sed -n "s/^ren: sync $1 took \(.*\)$/\1/p" "$log"; }

result() { sed -n 's/^ren: sync result: //p' "$log" | sed -n "${1}p"; }

reloads=$(sed -n 's/^ren: lists read again in \([0-9.]*\) ms.*/\1/p' "$log" |
    awk '{ n++; s += $1; if ($1 > m) m = $1 } END {
        if (n) printf "%d times, %.1f ms on average, %.1f ms at most", n, s / n, m }')

summary=$OUT_DIR/summary.md
{
    echo "## ren window sync measurements, $(date -u +'%Y-%m-%d %H:%M UTC')"
    echo
    echo "- ren: $(git describe --always --dirty)"
    echo "- kernel: $(uname -sr)"
    echo "- CPU: $(awk -F': ' '/^model name/ { print $2; exit }' /proc/cpuinfo)"
    echo
    echo "| | time, CPU | RSS after MiB | result |"
    echo "|---|---|---|---|"
    echo "| before | | $(rss "before the syncs" 1) | |"
    echo "| full sync | $(took 1) | $(rss "after sync 1" 1) | $(result 1) |"
    echo "| incremental sync | $(took 2) | $(rss "after sync 2" 1) | $(result 2) |"
    echo "| idle afterwards | | $(rss "idle after the syncs" 1) | |"
    echo
    echo "- peak RSS: $(rss "idle after the syncs" 2) MiB"
    echo "- lists read again: ${reloads:-never}"
} | tee "$summary"
