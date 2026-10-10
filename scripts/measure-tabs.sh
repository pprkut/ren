#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Measures web pages in Servo tabs: memory before opening a tab, with one
# and with three tabs, after closing them all, with one tab again and after
# closing it; frames while
# scrolling and the time from a painted frame to the UI. Servo runs in its
# helper process (ren-servo), with glibc's mmap threshold fixed at 1 MiB as
# in ren and with glibc's dynamic one. Needs a graphical session and network
# access; don't touch the window while it runs. Set REN_TAB_STATS=1 for
# per-second statistics of the frame path in the logs.
#
# Usage: scripts/measure-tabs.sh DUMP_DIR [URL...]
#   DUMP_DIR: a `ren --dump-items` directory; the pages are the links of
#             its first three items, unless URLs are given
#
# Environment:
#   OUT_DIR   logs (default target/measure/tabs-<timestamp>)
#
# Prints a Markdown summary, also written to $OUT_DIR/summary.md. It contains
# no URLs, titles or page content.

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
OUT_DIR=${OUT_DIR:-target/measure/tabs-$(date +%Y%m%d-%H%M%S)}

if [[ -z ${DISPLAY:-} && -z ${WAYLAND_DISPLAY:-} ]]; then
    echo "no graphical session (neither DISPLAY nor WAYLAND_DISPLAY is set)" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"
log() { echo "$*" >&2; }

log "== building"
cargo build --release --locked --no-default-features --features renderer-software,html-view
cp target/release/ren "$OUT_DIR/ren-without-tabs"
cargo build --release --locked
(cd crates/ren-servo && cargo build --release --locked)
cp target/release/ren target/release/ren-servo "$OUT_DIR/"

# name|environment
variants=(
    "fixed mmap threshold|"
    "dynamic mmap threshold in the helper|REN_SERVO_DYNAMIC_MMAP_THRESHOLD=1"
)

mib() { awk -v k="$1" 'BEGIN { printf "%.1f", k / 1024 }'; }

# KiB of field $3 (VmRSS, RssAnon) of the "<who> <when>:" line in log $1,
# where $2 is "rss" for the UI process and "helper rss" for the helper.
kib() {
    local line
    line=$(grep "^ren: $2 $4:" "$1" | tail -n 1) || {
        echo 0
        return
    }
    case $3 in
        rss) sed -n 's/.*: \([0-9]*\) KiB (anon.*/\1/p' <<<"$line" ;;
        anon) sed -n 's/.*(anon \([0-9]*\),.*/\1/p' <<<"$line" ;;
    esac
}

# The helper's shared memory (the frame buffer) in MiB at step $2.
helper_shmem() {
    local line
    line=$(grep "^ren: helper rss $2:" "$1" | tail -n 1) || {
        echo "-"
        return
    }
    mib "$(sed -n 's/.*shmem \([0-9]*\);.*/\1/p' <<<"$line")"
}

# "total (UI + helper)" RSS in MiB, or "-" if the step is missing.
memory() {
    if ! grep -q "^ren: rss $2:" "$1"; then
        echo "-"
        return
    fi
    local ui helper
    ui=$(kib "$1" rss rss "$2")
    helper=$(kib "$1" "helper rss" rss "$2")
    ui=${ui:-0}
    helper=${helper:-0}
    if [[ $helper -gt 0 ]]; then
        echo "$(mib $((ui + helper))) ($(mib "$ui") + $(mib "$helper"))"
    else
        mib "$ui"
    fi
}

summary=$OUT_DIR/summary.md
{
    echo "## ren tab measurements, $(date -u +'%Y-%m-%d %H:%M UTC')"
    echo
    echo "- ren: $(git describe --always --dirty)"
    echo "- kernel: $(uname -sr)"
    echo "- CPU: $(awk -F': ' '/^model name/ { print $2; exit }' /proc/cpuinfo)"
    echo "- GPU: $(glxinfo -B 2>/dev/null | sed -n 's/^OpenGL renderer string: //p' | head -n 1)"
    echo "- session: ${XDG_SESSION_TYPE:-unknown}, ${XDG_CURRENT_DESKTOP:-unknown}"
    if [[ ${#urls[@]} -gt 0 ]]; then
        echo "- pages: $((${#urls[@]} / 2)) given URLs"
    else
        echo "- pages: the links of the first three items"
    fi
    echo "- binary size: ren $(mib $(($(stat -c %s "$OUT_DIR/ren") / 1024))) MiB (without the tabs $(mib $(($(stat -c %s "$OUT_DIR/ren-without-tabs") / 1024))) MiB), ren-servo $(mib $(($(stat -c %s "$OUT_DIR/ren-servo") / 1024))) MiB"
    echo
    echo "| variant | before tabs | 1 tab | 3 tabs | after closing all | 1 tab again | after closing again | anon before / after closing | frame buffer (shmem) | scrolling frames in 3 s | frame to UI ms (mean / max) | Servo start / stop ms |"
    echo "|---|---|---|---|---|---|---|---|---|---|---|---|"
} >"$summary"

for variant in "${variants[@]}"; do
    IFS='|' read -r name environment <<<"$variant"
    log "== $name"
    vlog=$OUT_DIR/${name//[^a-z]/-}.log
    # shellcheck disable=SC2086
    env $environment "$OUT_DIR/ren" --dump "$DUMP" --items 5000 --measure-tabs "${urls[@]}" 2>"$vlog" || {
        log "failed, see $vlog"
        continue
    }
    frames=$(sed -n 's/^ren: scrolling: \([0-9]*\) frames.*/\1/p' "$vlog" | head -n 1)
    frame_ms=$(sed -n 's/^ren: page frames: [0-9]*, to the UI in \([0-9.]*\) ms on average, \([0-9.]*\) ms at most/\1 \/ \2/p' "$vlog" | head -n 1)
    start_ms=$(sed -n 's/^ren: Servo started in \([0-9.]*\) ms/\1/p' "$vlog" | head -n 1)
    stop_ms=$(sed -n 's/^ren: Servo stopped in \([0-9.]*\) ms/\1/p' "$vlog" | head -n 1)
    printf '| %s | %s | %s | %s | %s | %s | %s | %s / %s | %s | %s | %s | %s / %s |\n' \
        "$name" "$(memory "$vlog" "before tabs")" "$(memory "$vlog" "with 1 tab")" \
        "$(memory "$vlog" "with 3 tabs")" "$(memory "$vlog" "after closing all tabs")" \
        "$(memory "$vlog" "with 1 tab again")" "$(memory "$vlog" "after closing again")" \
        "$(mib "$(kib "$vlog" rss anon "before tabs")")" \
        "$(mib "$(kib "$vlog" rss anon "after closing all tabs")")" \
        "$(helper_shmem "$vlog" "with 1 tab")" "${frames:--}" "${frame_ms:--}" "${start_ms:--}" "${stop_ms:--}" >>"$summary"
done

{
    echo
    echo "RSS in MiB, total (UI process + helper process) while a helper runs. Anon: the UI"
    echo "process's anonymous memory, which shows what stays behind after the helper exits."
    echo "Frame buffer: the shared memory frames go through, in the helper's RSS with one tab"
    echo "(also counted in the UI process). Frame to UI: reading a painted frame back from the"
    echo "GPU into the shared buffer, plus copying it out in the UI. Servo start: until the"
    echo "first tab can be opened (starting the process; Servo itself then starts in the helper)."
    echo
    echo "Logs: \`$OUT_DIR\`"
} >>"$summary"

cat "$summary"
