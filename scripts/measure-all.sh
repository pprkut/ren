#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Runs all of ren's measurements one after another (M10) and collects
# their summaries in one report: the window idle and scrolling the item
# list per renderer (measure.sh), reading articles (measure-articles.sh),
# pages in tabs (measure-tabs.sh), the window hidden in the tray
# (measure-hidden.sh), the settings dialog (measure-settings.sh), the
# store with a synthetic account (ren-store's example measure), and
# syncing the account of the settings file (measure-sync.sh,
# measure-window-sync.sh). Needs a graphical session, network access and
# the real server; takes about an hour. Don't touch the windows while it
# runs.
#
# Usage: scripts/measure-all.sh DUMP_DIR [URL...]
#   DUMP_DIR: a `ren --dump-items` directory: the items of the idle
#             window, the articles, and the pages (the links of its first
#             three items, unless URLs are given)
#
# Environment:
#   OUT_DIR   logs and the report (default target/measure/all-<timestamp>)
#   SKIP      measurements to leave out, comma-separated: idle, articles,
#             tabs, hidden, settings, store, sync, window-sync
#   DATABASE  a database for the idle measurement instead of the dump's
#             items (e.g. ren's own, $XDG_DATA_HOME/ren/ren.db, with ren
#             closed); measure.sh works on a copy of it
#
# Writes the report to $OUT_DIR/report.md and prints it. Like the
# summaries it is made of, it contains no URLs, titles or content. The
# measurements' BMF results are merged into $OUT_DIR/results.json, with
# the commit measured in $OUT_DIR/commit, for `just measure-upload`.

set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $# -lt 1 ]]; then
    sed -n 's/^# \?//p' "$0" | sed -n '4,32p' >&2
    exit 1
fi
DUMP=$1
shift
OUT_DIR=${OUT_DIR:-target/measure/all-$(date +%Y%m%d-%H%M%S)}
# Not passed on to cargo: ring's build script runs again when OUT_DIR
# changes, and with it much of Servo's helper is built again.
export -n OUT_DIR

if [[ -z ${DISPLAY:-} && -z ${WAYLAND_DISPLAY:-} ]]; then
    echo "no graphical session (neither DISPLAY nor WAYLAND_DISPLAY is set)" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"
# shellcheck source=scripts/bmf.sh
source scripts/bmf.sh
# What measure-upload.sh uploads: the commit, unless there are changes
# that aren't committed.
if git diff --quiet HEAD; then
    git rev-parse HEAD >"$OUT_DIR/commit"
else
    echo "not committed" >"$OUT_DIR/commit"
fi
report=$OUT_DIR/report.md
{
    echo "# ren measurements, $(date -u +'%Y-%m-%d %H:%M UTC')"
    echo
    echo "- ren: $(git describe --always --dirty)"
    echo
} >"$report"

skipped() { [[ ,${SKIP:-}, == *,$1,* ]]; }

# Runs measurement $1 (a script and its arguments follow) with its own
# output directory, and adds its summary to the report.
run() {
    local name=$1 script=$2
    shift 2
    if skipped "$name"; then
        echo "== $name: skipped" >&2
        return
    fi
    echo "== $name" >&2
    local dir=$OUT_DIR/$name
    # Progress shows as it goes, and is kept.
    if OUT_DIR=$dir "$script" "$@" >"$OUT_DIR/$name.out" 2> >(tee "$OUT_DIR/$name.err" >&2); then
        # Each summary starts with a heading of its own.
        cat "$dir/summary.md" >>"$report"
    else
        echo "## $name: failed, see \`$OUT_DIR/$name.err\`" >>"$report"
    fi
    echo >>"$report"
}

# The store's measurement, as `just measure-store` runs it. Not with
# OUT_DIR: see above.
measure_store() {
    mkdir -p "$OUT_DIR"
    env -u OUT_DIR cargo run --release --locked -p ren-store --example measure -- \
        63000 "$OUT_DIR" "$OUT_DIR/results.json" >"$OUT_DIR/summary.md"
}

# The dump's items, so that every measurement shows the same data.
DUMP=$DUMP run idle scripts/measure.sh software femtovg skia
run articles scripts/measure-articles.sh "$DUMP" 100
run tabs scripts/measure-tabs.sh "$DUMP" "$@"
run hidden scripts/measure-hidden.sh "$DUMP" "$@"
run settings scripts/measure-settings.sh
run store measure_store
run sync scripts/measure-sync.sh
run window-sync scripts/measure-window-sync.sh

results=()
for name in idle articles tabs hidden settings store sync window-sync; do
    if [[ -e $OUT_DIR/$name/results.json ]]; then
        results+=("$OUT_DIR/$name/results.json")
    fi
done
if [[ ${#results[@]} -gt 0 ]]; then
    bmf_merge "$OUT_DIR/results.json" "${results[@]}"
fi

cat "$report"
