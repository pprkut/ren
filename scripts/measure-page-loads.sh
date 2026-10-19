#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Loads one page in a Servo tab again and again, each time in a new helper
# (ren --measure-tabs with the page as its only one), to tell apart what
# changes from one load to the next (0016, "Ars Technica"): the frames
# while scrolling, the requests the page makes (REN_SERVO_REQUEST_LOG, and
# whether the first tab's requests are the same in every load), and, on
# Intel GPUs (i915), the GPU's actual clock and the power limits flagged
# while each load ran. With BUSY, the loads run once while that many CPU
# cores are kept busy and once after, which shows what a busy CPU does to
# the GPU (on a laptop's integrated GPU: it stays at its lowest clock).
# Needs a graphical session and network access; don't touch the window
# while it runs, and run it on an otherwise idle machine.
#
# Usage: scripts/measure-page-loads.sh DUMP_DIR URL
#   DUMP_DIR: a `ren --dump-items` directory (the window's demo data; no
#   account or sync is involved)
#
# Environment:
#   RUNS     loads per phase (default 8)
#   BUSY     CPU cores to keep busy in a first phase (default 0: no such
#            phase); on the i7-1185G7 of 0016, 1 made no difference and 4
#            kept the GPU at its lowest clock
#   SETTLE   seconds to wait after building, so the build's load is gone
#            (default 60)
#   OUT_DIR  logs (default target/measure/page-loads-<timestamp>)
#
# Prints a Markdown summary, also written to $OUT_DIR/summary.md.

set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 DUMP_DIR URL" >&2
    exit 1
fi
DUMP=$1
URL=$2
RUNS=${RUNS:-8}
BUSY=${BUSY:-0}
SETTLE=${SETTLE:-60}
OUT_DIR=${OUT_DIR:-target/measure/page-loads-$(date +%Y%m%d-%H%M%S)}
# Not passed on to cargo: ring's build script runs again when OUT_DIR
# changes, and with it much of Servo's helper is built again.
export -n OUT_DIR

if [[ -z ${DISPLAY:-} && -z ${WAYLAND_DISPLAY:-} ]]; then
    echo "no graphical session (neither DISPLAY nor WAYLAND_DISPLAY is set)" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"
echo "== building" >&2
cargo build --release --locked
(cd crates/ren-servo && cargo build --release --locked)
cp target/release/ren target/release/ren-servo "$OUT_DIR/"
echo "== waiting ${SETTLE} s for the build's load to go" >&2
sleep "$SETTLE"

# The GPU's actual clock, the throttle reasons and the package
# temperature, 4 times a second, if the GPU is i915's.
gt=
for dir in /sys/class/drm/card*/gt/gt0; do
    [[ -d $dir ]] && { gt=$dir; break; }
done
pkg=$(grep -l x86_pkg_temp /sys/class/thermal/thermal_zone*/type 2>/dev/null | head -n 1 || true)
samples=$OUT_DIR/gpu.txt
sampler=
if [[ -n $gt && -r $gt/rps_act_freq_mhz ]]; then
    (
        while :; do
            reasons=""
            for f in "$gt"/throttle_reason_*; do
                [[ $f == */throttle_reason_status ]] && continue
                [[ $(cat "$f" 2>/dev/null) == 1 ]] && reasons+="${f##*throttle_reason_},"
            done
            temp=-
            [[ -n $pkg ]] && temp=$(($(cat "$(dirname "$pkg")/temp") / 1000))
            echo "$(date +%s.%N | cut -c1-14) $(cat "$gt/rps_act_freq_mhz") $temp ${reasons:--}"
            sleep 0.25
        done
    ) >"$samples" &
    sampler=$!
fi
busy=()
cleanup() {
    [[ -n $sampler ]] && kill "$sampler" 2>/dev/null
    ((${#busy[@]})) && kill "${busy[@]}" 2>/dev/null
    true
}
trap cleanup EXIT

# Lines "<phase> <run> <start> <end>" of the loads.
runs=$OUT_DIR/runs.txt
: >"$runs"
load() {
    local phase=$1 run=$2 start
    start=$(date +%s.%N | cut -c1-14)
    env REN_TAB_STATS=1 REN_SERVO_REQUEST_LOG="$OUT_DIR/$phase-$run.requests" \
        "$OUT_DIR/ren" --dump "$DUMP" --items 50 --measure-tabs --tab-url "$URL" \
        2>"$OUT_DIR/$phase-$run.log" || echo "ren failed, see $OUT_DIR/$phase-$run.log" >&2
    echo "$phase $run $start $(date +%s.%N | cut -c1-14)" >>"$runs"
}

if ((BUSY > 0)); then
    for _ in $(seq "$BUSY"); do
        yes >/dev/null &
        busy+=($!)
    done
    echo "== $BUSY cores busy; warming up for 20 s" >&2
    sleep 20
    for run in $(seq "$RUNS"); do
        echo "== busy: load $run of $RUNS" >&2
        load busy "$run"
    done
    kill "${busy[@]}" 2>/dev/null
    wait "${busy[@]}" 2>/dev/null || true
    busy=()
    echo "== cores idle again; waiting 10 s" >&2
    sleep 10
fi
for run in $(seq "$RUNS"); do
    echo "== idle: load $run of $RUNS" >&2
    load idle "$run"
done
cleanup
sampler=

# The first tab's requests without query strings, as one line per load,
# to name each distinct set A, B, ….
declare -A sets
next=A
set_of() {
    local key
    key=$(awk -F'\t' '$2 == "0" && $3 == "request" { sub(/[?#].*/, "", $6); print $4, $5, $6 }' "$1" \
        | sort | md5sum | cut -c1-12)
    if [[ -z ${sets[$key]:-} ]]; then
        sets[$key]=$next
        next=$(echo "$next" | tr 'A-Y' 'B-Z')
    fi
    echo "${sets[$key]}"
}

# "median max" of the GPU's busy clock, the temperature range and the
# throttle reasons flagged between two times.
gpu() {
    [[ -s $samples ]] || { echo "- | - | -"; return; }
    awk -v a="$1" -v b="$2" '
        $1 >= a && $1 <= b {
            n++
            if ($2 > 0) f[++m] = $2
            if ($3 != "-") { if (t0 == "" || $3 < t0) t0 = $3; if ($3 > t1) t1 = $3 }
            split($4, r, ",")
            for (i in r) if (r[i] != "" && r[i] != "-") c[r[i]]++
        }
        END {
            if (m) { asort(f); med = f[int((m + 1) / 2)]; max = f[m] } else { med = 0; max = 0 }
            out = ""
            for (k in c) out = out (out ? ", " : "") k " " int(100 * c[k] / n) " %"
            printf "%s / %s | %s–%s | %s\n", med, max, t0, t1, (out ? out : "-")
        }' "$samples"
}

summary=$OUT_DIR/summary.md
{
    echo "## One page loaded again and again, $(date -u +'%Y-%m-%d %H:%M UTC')"
    echo
    echo "- ren: $(git describe --always --dirty), release build, default features"
    echo "- page: $URL"
    echo "- loads per phase: $RUNS$( ((BUSY > 0)) && echo ", first with BUSY=$BUSY busy cores")"
    echo "- session: ${XDG_SESSION_TYPE:-unknown}, ${XDG_CURRENT_DESKTOP:-unknown}"
    echo
    echo "Requests are the first tab's; \"set\" names each distinct list of them"
    echo "(without query strings). GPU clock in MHz while busy, from i915."
    echo
    echo "| phase | load | frames in 3 s | requests (blocked) | set | GPU MHz median / max | °C | power limits flagged |"
    echo "|---|---|---|---|---|---|---|---|"
    while read -r phase run start end; do
        log=$OUT_DIR/$phase-$run.log
        requests=$OUT_DIR/$phase-$run.requests
        frames=$(sed -n 's/^ren: scrolling: \([0-9]*\) frames.*/\1/p' "$log" | head -n 1)
        total=$(awk -F'\t' '$2 == "0" && $3 == "request"' "$requests" 2>/dev/null | wc -l)
        blocked=$(awk -F'\t' '$2 == "0" && $3 == "request" && $4 == "blocked"' "$requests" 2>/dev/null | wc -l)
        echo "| $phase | $run | ${frames:--} | $total ($blocked) | $(set_of "$requests") | $(gpu "$start" "$end") |"
    done <"$runs"
} | tee "$summary"
