#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Measures web pages in Servo tabs: memory before opening a tab, with one
# and with three tabs, after closing them all, with one tab again and after
# closing it; frames while
# scrolling and the time from a painted frame to the UI. Servo runs in its
# helper process (ren-servo), with content blocking (the filter lists of
# the settings, downloaded first if they are old) and without, with the
# frame pacing from before M10 (one frame under way), and with the GPU
# renderers (FemtoVG, Skia) instead of the software renderer. The
# synthetic variants replace Servo with a stand-in that paints a simple
# page (pattern-helper), so the window's cost per frame shows on its own,
# per renderer. Needs a graphical session and network access; don't touch
# the window while it runs. Run it on an idle machine: on a laptop's
# integrated GPU, a busy CPU keeps the GPU at its lowest clock, and heavy
# pages then scroll at a fraction of their frame rate (0016). The logs have per-second statistics of the
# frame path and of the requests blocked (REN_TAB_STATS).
#
# Usage: scripts/measure-tabs.sh DUMP_DIR [URL...]
#   DUMP_DIR: a `ren --dump-items` directory; the pages are the links of
#             its first three items, unless URLs are given. Or - for
#             generated items, with URLs.
#
# Environment:
#   OUT_DIR   logs (default target/measure/tabs-<timestamp>)
#   VARIANTS  the variants to run, comma-separated (default: all): blocking,
#             no-blocking, one-frame, femtovg, skia, synthetic,
#             synthetic-femtovg, synthetic-skia. Servo's helper is only
#             built (and the filter lists only downloaded) for the ones
#             that aren't synthetic.
#   REN       measure this ren binary (with the servo feature) instead of
#             building one
#
# Prints a Markdown summary, also written to $OUT_DIR/summary.md, and the
# results as BMF JSON in $OUT_DIR/results.json (scripts/bmf.sh). They
# contain no URLs, titles or page content.

set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $# -lt 1 ]]; then
    sed -n 's/^# \?//p' "$0" | sed -n '4,38p' >&2
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
OUT_DIR=${OUT_DIR:-target/measure/tabs-$(date +%Y%m%d-%H%M%S)}
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
bmf_init "$OUT_DIR/results.json"
log() { echo "$*" >&2; }

# key|name|environment|binary|arguments
synthetic="REN_SERVO=$OUT_DIR/pattern-helper"
all_variants=(
    "blocking|blocking||ren|"
    "no-blocking|without blocking|REN_SERVO_NO_BLOCKING=1|ren|"
    "one-frame|one frame under way (before M10)|REN_SERVO_ONE_FRAME=1|ren|"
    "femtovg|FemtoVG renderer||ren-gpu|--renderer femtovg"
    "skia|Skia renderer||ren-gpu|--renderer skia"
    "synthetic|synthetic page|$synthetic|ren|"
    "synthetic-femtovg|synthetic page, FemtoVG|$synthetic|ren-gpu|--renderer femtovg"
    "synthetic-skia|synthetic page, Skia|$synthetic|ren-gpu|--renderer skia"
)
variants=()
servo=false
for variant in "${all_variants[@]}"; do
    if [[ -z ${VARIANTS:-} || ,$VARIANTS, == *,${variant%%|*},* ]]; then
        variants+=("$variant")
        [[ $variant == *"|$synthetic|"* ]] || servo=true
    fi
done

log "== building"
if [[ -z ${REN:-} ]]; then
    cargo build --release --locked --no-default-features --features renderer-software,html-view
    cp target/release/ren "$OUT_DIR/ren-without-tabs"
fi
if [[ " ${variants[*]} " == *"|ren-gpu|"* ]]; then
    cargo build --release --locked --features renderer-femtovg,renderer-skia
    cp target/release/ren "$OUT_DIR/ren-gpu"
fi
cargo build --release --locked -p ren-tabs --example pattern-helper
cp target/release/examples/pattern-helper "$OUT_DIR/"
if [[ -n ${REN:-} ]]; then
    cp "$REN" "$OUT_DIR/ren"
else
    cargo build --release --locked
    cp target/release/ren "$OUT_DIR/"
fi
if $servo; then
    (cd crates/ren-servo && cargo build --release --locked)
    cp target/release/ren-servo "$OUT_DIR/"

    log "== filter lists"
    "$OUT_DIR/ren" --update-filters >&2 || log "not all filter lists could be downloaded"
fi

mib() { awk -v k="$1" 'BEGIN { printf "%.1f", k / 1024 }'; }

# The size in MiB of binary $1 in $OUT_DIR, or "-" if it wasn't built.
size() {
    if [[ -e $OUT_DIR/$1 ]]; then
        mib $(($(stat -c %s "$OUT_DIR/$1") / 1024))
    else
        echo "-"
    fi
}

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

# Requests the helper blocked and checked in log $1, from its statistics.
requests() {
    sed -n 's/^ren: helper stats over .* checked=\([0-9]*\).*/\1/p' "$1" | awk '{ n += $1 } END { printf "%d", n }'
}
blocked() {
    sed -n 's/^ren: helper stats over .* blocked=\([0-9]*\).*/\1/p' "$1" | awk '{ n += $1 } END { printf "%d", n }'
}

# "total (UI + helper)" RSS in MiB, or "-" if the step is missing; records
# the step's memory for BMF as benchmark "tabs, <variant $3>: <step>".
memory() {
    if ! grep -q "^ren: rss $2:" "$1"; then
        echo "-"
        return
    fi
    local ui helper
    ui=$(kib "$1" rss rss "$2")
    helper=$(kib "$1" "helper rss" rss "$2")
    # Recorded before the defaults: a value that can't be read is left
    # out, not recorded as 0.
    bmf_kib "tabs, $3: $2" rss "$ui"
    bmf_kib "tabs, $3: $2" anonymous-memory "$(kib "$1" rss anon "$2")"
    ui=${ui:-0}
    helper=${helper:-0}
    if [[ $helper -gt 0 ]]; then
        bmf_kib "tabs, $3: $2, helper" rss "$helper"
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
    echo "- binary size: ren $(size ren) MiB (without the tabs $(size ren-without-tabs) MiB), ren-servo $(size ren-servo) MiB"
    echo
    echo "| variant | before tabs | 1 tab | 3 tabs | after closing all | 1 tab again | after closing again | anon before / after closing | frame buffer (shmem) | scrolling frames in 3 s | CPU % while scrolling (window / helper) | window CPU ms per frame | frame to UI ms (mean / max) | Servo start / stop ms | requests blocked / checked |"
    echo "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
} >"$summary"

for variant in "${variants[@]}"; do
    IFS='|' read -r key name environment binary arguments <<<"$variant"
    log "== $name"
    vlog=$OUT_DIR/$key.log
    # shellcheck disable=SC2086
    env REN_TAB_STATS=1 $environment "$OUT_DIR/$binary" $arguments "${data[@]}" --items 5000 \
        --measure-tabs "${urls[@]}" 2>"$vlog" || {
        log "failed, see $vlog"
        continue
    }
    frames=$(sed -n 's/^ren: scrolling: \([0-9]*\) frames.*/\1/p' "$vlog" | head -n 1)
    cpu=$(sed -n 's/^ren: scrolling CPU: window \([0-9?]*\) %, helper \([0-9?]*\) %/\1 \/ \2/p' "$vlog" | head -n 1)
    # The window's CPU time per frame while scrolling (3 s).
    per_frame=$(awk -v cpu="${cpu%% *}" -v n="${frames:-0}" \
        'BEGIN { if (n > 0 && cpu ~ /^[0-9]+$/) printf "%.1f", cpu / 100 * 3000 / n; else print "-" }')
    frame_ms=$(sed -n 's/^ren: page frames: [0-9]*, to the UI in \([0-9.]*\) ms on average, \([0-9.]*\) ms at most/\1 \/ \2/p' "$vlog" | head -n 1)
    start_ms=$(sed -n 's/^ren: Servo started in \([0-9.]*\) ms/\1/p' "$vlog" | head -n 1)
    stop_ms=$(sed -n 's/^ren: Servo stopped in \([0-9.]*\) ms/\1/p' "$vlog" | head -n 1)
    bmf "tabs, $key: scrolling" frames "$frames"
    bmf "tabs, $key: scrolling" cpu "${cpu%% *}"
    bmf "tabs, $key: scrolling, helper" cpu "${cpu##* }"
    bmf_ms "tabs, $key: frame to UI" "${frame_ms%% *}"
    bmf_ms "tabs, $key: Servo start" "$start_ms"
    bmf_ms "tabs, $key: Servo stop" "$stop_ms"
    printf '| %s | %s | %s | %s | %s | %s | %s | %s / %s | %s | %s | %s | %s | %s | %s / %s | %s / %s |\n' \
        "$name" "$(memory "$vlog" "before tabs" "$key")" "$(memory "$vlog" "with 1 tab" "$key")" \
        "$(memory "$vlog" "with 3 tabs" "$key")" "$(memory "$vlog" "after closing all tabs" "$key")" \
        "$(memory "$vlog" "with 1 tab again" "$key")" "$(memory "$vlog" "after closing again" "$key")" \
        "$(mib "$(kib "$vlog" rss anon "before tabs")")" \
        "$(mib "$(kib "$vlog" rss anon "after closing all tabs")")" \
        "$(helper_shmem "$vlog" "with 1 tab")" "${frames:--}" "${cpu:--}" "$per_frame" "${frame_ms:--}" "${start_ms:--}" "${stop_ms:--}" \
        "$(blocked "$vlog")" "$(requests "$vlog")" >>"$summary"
done

{
    echo
    echo "RSS in MiB, total (UI process + helper process) while a helper runs. Anon: the UI"
    echo "process's anonymous memory, which shows what stays behind after the helper exits."
    echo "Frame buffer: the shared memory frames go through, in the helper's RSS with one tab"
    echo "(also counted in the UI process). CPU: percent of one core during the 3 s of"
    echo "scrolling; per frame: the window's CPU time divided by its frames (the synthetic"
    echo "page costs the helper almost nothing, so there it is what the window's drawing costs)."
    echo "Frame to UI: reading a painted frame back from the GPU into the shared"
    echo "buffer, plus copying it out in the UI. Servo start: until the first tab can be opened"
    echo "(starting the process; Servo itself then starts in the helper). Requests: of all"
    echo "the pages' requests the helper checked (none without blocking), those it blocked."
    echo
    echo "Logs: \`$OUT_DIR\`"
} >>"$summary"
bmf_write

cat "$summary"
