#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Runs all of ren's measurements one after another (M10) and collects
# their summaries in one report: the window idle and scrolling the item
# list per renderer (measure.sh), reading articles (measure-articles.sh),
# pages in tabs (measure-tabs.sh), the window hidden in the tray
# (measure-hidden.sh), the settings dialog (measure-settings.sh), and
# syncing the account of the settings file (measure-sync.sh,
# measure-window-sync.sh). Needs a graphical session, network access and
# the real server; takes about an hour. Don't touch the windows while it
# runs.
#
# Usage: scripts/measure-all.sh DUMP_DIR [URL...]
#   DUMP_DIR: a `ren --dump-items` directory, for the articles and the
#             pages (the links of its first three items, unless URLs are
#             given)
#
# Environment:
#   OUT_DIR   logs and the report (default target/measure/all-<timestamp>)
#   SKIP      measurements to leave out, comma-separated: idle, articles,
#             tabs, hidden, settings, sync, window-sync
#   DATABASE  the database for the idle measurement (default: ren's own,
#             $XDG_DATA_HOME/ren/ren.db, if it exists; ren must not run);
#             measure.sh works on a copy of it
#
# Writes the report to $OUT_DIR/report.md and prints it. Like the
# summaries it is made of, it contains no URLs, titles or content.

set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $# -lt 1 ]]; then
    sed -n 's/^# \?//p' "$0" | sed -n '4,28p' >&2
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
if [[ -z ${DATABASE:-} ]]; then
    DATABASE=${XDG_DATA_HOME:-$HOME/.local/share}/ren/ren.db
    [[ -e $DATABASE ]] || DATABASE=
fi

mkdir -p "$OUT_DIR"
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

export DATABASE
run idle scripts/measure.sh software femtovg skia
run articles scripts/measure-articles.sh "$DUMP" 100
run tabs scripts/measure-tabs.sh "$DUMP" "$@"
run hidden scripts/measure-hidden.sh "$DUMP" "$@"
run settings scripts/measure-settings.sh
run sync scripts/measure-sync.sh
run window-sync scripts/measure-window-sync.sh

cat "$report"
