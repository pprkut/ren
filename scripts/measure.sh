#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Measures the ren window for each variant: startup time, memory and CPU
# while idle, and CPU/frame rate while scrolling the item list. Needs a
# graphical session (X11 or Wayland); don't touch the window while it runs.
#
# Usage: scripts/measure.sh [VARIANT...]
#   VARIANT: a Slint renderer with the default style and the winit backend:
#   software, femtovg, skia, or a specific Skia variant: skia-opengl,
#   skia-wgpu or skia-software; or the native Qt style (needs the Qt 6
#   development files): qt (Qt backend) or qt-software (winit backend with
#   the software renderer). Default: software femtovg skia.
#
# Environment:
#   STARTUP_RUNS  number of startup measurements per renderer (default 5)
#   SETTLE_SECS   wait after startup before idle sampling starts (default 5)
#   IDLE_SECS     length of the idle measurement (default 30)
#   ITEMS         number of generated items (default 10000)
#   DATABASE      measure with a copy of this database (e.g. ren's own,
#                 ~/.local/share/ren/ren.db, with ren closed) instead of
#                 generated items; the copy goes to a temporary directory
#   OUT_DIR       logs and samples (default target/measure/<timestamp>)
#
# Prints a Markdown summary, also written to $OUT_DIR/summary.md.

set -euo pipefail
cd "$(dirname "$0")/.."

STARTUP_RUNS=${STARTUP_RUNS:-5}
SETTLE_SECS=${SETTLE_SECS:-5}
IDLE_SECS=${IDLE_SECS:-30}
ITEMS=${ITEMS:-10000}
OUT_DIR=${OUT_DIR:-target/measure/$(date +%Y%m%d-%H%M%S)}

# What the window shows. A database holds real feed data, so its copy
# stays outside the repository.
if [[ -n ${DATABASE:-} ]]; then
    db_dir=$(mktemp -d -t ren-measure.XXXXXX)
    trap 'rm -rf "$db_dir"' EXIT
    cp "$DATABASE" "$db_dir/ren.db"
    if [[ -e $DATABASE-wal ]]; then
        cp "$DATABASE-wal" "$db_dir/ren.db-wal"
    fi
    data=(--database "$db_dir/ren.db")
    data_note="a copy of a database ($(du -m "$DATABASE" | cut -f1) MiB)"
else
    data=(--items "$ITEMS")
    data_note="$ITEMS generated items"
fi

renderers=("$@")
if [[ ${#renderers[@]} -eq 0 ]]; then
    renderers=(software femtovg skia)
fi

features=renderer-software,renderer-femtovg
need_qt=false
for r in "${renderers[@]}"; do
    case $r in
        software | femtovg) ;;
        skia*) features+=,renderer-skia ;;
        qt | qt-software) need_qt=true ;;
        *)
            echo "unknown variant: $r" >&2
            exit 1
            ;;
    esac
done

if [[ -z ${DISPLAY:-} && -z ${WAYLAND_DISPLAY:-} ]]; then
    echo "no graphical session (neither DISPLAY nor WAYLAND_DISPLAY is set)" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"
# Each variant's binary is copied, as the builds overwrite each other.
cargo build --release --locked --features "$features"
cp target/release/ren "$OUT_DIR/ren"
if $need_qt; then
    # qttypes looks for qmake; distributions often only ship qmake6.
    if ! command -v qmake >/dev/null && command -v qmake6 >/dev/null; then
        export QMAKE=${QMAKE:-$(command -v qmake6)}
    fi
    cargo build --release --locked --features renderer-software,style-qt
    cp target/release/ren "$OUT_DIR/ren-qt"
fi

# Binary and arguments of a variant.
variant() {
    case $1 in
        qt) echo "$OUT_DIR/ren-qt --backend qt" ;;
        qt-software) echo "$OUT_DIR/ren-qt --backend winit --renderer software" ;;
        *) echo "$OUT_DIR/ren --renderer $1" ;;
    esac
}
clk_tck=$(getconf CLK_TCK)

log() { echo "$*" >&2; }

now() { date +%s.%N; }

# User + system CPU time of a process, in clock ticks.
cpu_ticks() {
    local stat
    stat=$(<"/proc/$1/stat")
    # Fields after "pid (comm) " start with field 3; utime and stime are 14 and 15.
    read -r -a f <<<"${stat##*) }"
    echo $((f[11] + f[12]))
}

# "rss_kb anon_kb file_kb shmem_kb pss_kb threads" of a process.
mem_sample() {
    local pss
    pss=$(awk '/^Pss:/ { print $2 }' "/proc/$1/smaps_rollup")
    awk -v pss="$pss" '
        /^VmRSS:/ { rss = $2 }
        /^RssAnon:/ { anon = $2 }
        /^RssFile:/ { file = $2 }
        /^RssShmem:/ { shmem = $2 }
        /^Threads:/ { threads = $2 }
        END { print rss, anon, file, shmem, pss, threads }
    ' "/proc/$1/status"
}

# Milliseconds from a "ren: <event> after <n> ms" line in log $1.
startup_ms() {
    sed -n "s/^ren: $2 after \([0-9.]*\) ms$/\1/p" "$1" | grep .
}

peak_rss() { awk '/^VmHWM:/ { print $2 }' "/proc/$1/status"; }

stop() {
    kill "$1" 2>/dev/null || true
    wait "$1" 2>/dev/null || true
}

# Waits until a line matching $2 shows up in log file $1 ($3 s at most,
# default 30).
wait_for_line() {
    local i
    for ((i = 0; i < ${3:-30} * 10; i++)); do
        grep -q "$2" "$1" && return 0
        sleep 0.1
    done
    return 1
}

# Samples process $1 every $3 seconds into CSV $2 while it runs, for at most
# $4 seconds (0: until it exits). Prints "cpu_percent last_sample_line".
sample() {
    local pid=$1 csv=$2 interval=$3 limit=$4
    local t0 c0 t c m line="" end last_t="" last_c=""
    echo "time_s,rss_kb,anon_kb,file_kb,shmem_kb,pss_kb,threads,cpu_ticks" >"$csv"
    t0=$(now)
    c0=$(cpu_ticks "$pid")
    end=$(awk -v t="$t0" -v l="$limit" 'BEGIN { print (l > 0) ? t + l : 1e18 }')
    while kill -0 "$pid" 2>/dev/null; do
        sleep "$interval"
        # The process may exit between the check and the reads.
        c=$(cpu_ticks "$pid" 2>/dev/null) || break
        m=$(mem_sample "$pid" 2>/dev/null) || break
        t=$(now)
        line=$m last_t=$t last_c=$c
        echo "$(awk -v t="$t" -v t0="$t0" 'BEGIN { printf "%.2f", t - t0 }'),${line// /,},$c" >>"$csv"
        awk -v t="$t" -v e="$end" 'BEGIN { exit !(t >= e) }' && break
    done
    awk -v c0="$c0" -v c="${last_c:-$c0}" -v t0="$t0" -v t="${last_t:-$t0}" -v hz="$clk_tck" \
        'BEGIN { d = t - t0; printf "%.1f", (d > 0) ? (c - c0) * 100 / hz / d : 0 }'
    echo " $line"
}

mib() { awk -v k="$1" 'BEGIN { printf "%.1f", k / 1024 }'; }

# Median of the numbers given as arguments.
median() {
    [[ $# -gt 0 && -n $1 ]] || {
        echo -
        return
    }
    printf '%s\n' "$@" | sort -g | awk '{ v[NR] = $1 } END {
        if (NR % 2) print v[(NR + 1) / 2];
        else printf "%.1f", (v[NR / 2] + v[NR / 2 + 1]) / 2 }'
}

summary=$OUT_DIR/summary.md
{
    echo "## ren measurements, $(date -u +'%Y-%m-%d %H:%M UTC')"
    echo
    echo "- ren: $(git describe --always --dirty), $data_note"
    echo "- slint: $(awk '/^name = "slint"$/ { getline; print $3 }' Cargo.lock | tr -d '"')"
    echo "- kernel: $(uname -sr)"
    echo "- CPU: $(awk -F': ' '/^model name/ { print $2; exit }' /proc/cpuinfo)"
    echo "- session: ${XDG_SESSION_TYPE:-unknown}, ${XDG_CURRENT_DESKTOP:-unknown} (WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-}, DISPLAY=${DISPLAY:-})"
    if command -v glxinfo >/dev/null; then
        echo "- GL: $(glxinfo -B 2>/dev/null | awk -F': ' '/OpenGL renderer string/ { print $2 }')"
    fi
    echo "- idle: ${IDLE_SECS} s after ${SETTLE_SECS} s settling; startup: median of ${STARTUP_RUNS} runs"
    echo
    echo "| variant | startup ms (window / first frame) | idle RSS MiB | idle PSS MiB | anon / file / shmem MiB | threads | idle CPU % | peak RSS MiB | scroll CPU % | scroll fps (median / min) | RSS after scroll MiB |"
    echo "|---|---|---|---|---|---|---|---|---|---|---|"
} >"$summary"

backends=()
for r in "${renderers[@]}"; do
    read -r -a run <<<"$(variant "$r")"
    log "== $r: startup"
    created=() first=() first_note=""
    for ((i = 1; i <= STARTUP_RUNS; i++)); do
        slog=$OUT_DIR/$r-startup-$i.log
        "${run[@]}" "${data[@]}" --measure 2>"$slog" &
        pid=$!
        wait_for_line "$slog" "event loop running" || log "no startup line in $slog"
        # Not every renderer reports frames; give it some time.
        wait_for_line "$slog" "first frame rendered" 5 || first_note="*"
        stop "$pid"
        created+=("$(startup_ms "$slog" "window created")")
        first+=("$(startup_ms "$slog" "first frame rendered" || startup_ms "$slog" "event loop running")")
        sleep 1
    done

    log "== $r: idle"
    ilog=$OUT_DIR/$r-idle.log
    "${run[@]}" "${data[@]}" 2>"$ilog" &
    pid=$!
    sleep "$SETTLE_SECS"
    read -r idle_cpu rss anon file shmem pss threads < <(sample "$pid" "$OUT_DIR/$r-idle.csv" 1 "$IDLE_SECS")
    peak=$(peak_rss "$pid")
    stop "$pid"

    log "== $r: scroll"
    clog=$OUT_DIR/$r-scroll.log
    SLINT_DEBUG_PERFORMANCE=refresh_lazy,console \
        "${run[@]}" "${data[@]}" --autoscroll 2>"$clog" &
    pid=$!
    wait_for_line "$clog" "autoscroll started" || log "autoscroll did not start, see $clog"
    read -r scroll_cpu scroll_rss _ < <(sample "$pid" "$OUT_DIR/$r-scroll.csv" 0.5 0)
    wait "$pid" 2>/dev/null || true
    # Frame rate reports between start and end of the scroll, without the
    # first one (partial second).
    mapfile -t fps < <(awk '/autoscroll started/ { on = 1; next } /autoscroll over/ { on = 0 }
        on && /average frames per second:/ && ++n > 1' "$clog" |
        sed -n 's/.*average frames per second: \([0-9]*\).*/\1/p')
    fps_median=$(median "${fps[@]}")
    fps_min=$(median "$(printf '%s\n' "${fps[@]}" | sort -n | head -1)")

    backends+=("$r: $(sed -n 's/^Slint: Build config: [a-z]*; Backend: //p' "$clog" | head -1 | grep . || echo "${run[*]:1}")")

    printf '| %s | %s / %s%s | %s | %s | %s / %s / %s | %s | %s | %s | %s | %s / %s | %s |\n' \
        "$r" "$(median "${created[@]}")" "$(median "${first[@]}")" "$first_note" \
        "$(mib "$rss")" "$(mib "$pss")" "$(mib "$anon")" "$(mib "$file")" "$(mib "$shmem")" \
        "$threads" "$idle_cpu" "$(mib "$peak")" "$scroll_cpu" "$fps_median" "$fps_min" \
        "$(mib "${scroll_rss:-0}")" >>"$summary"
done

{
    echo
    echo "Slint backends in use:"
    printf -- '- %s\n' "${backends[@]}"
    echo
    echo "\\* no rendering notifier: time until the event loop runs, not until the first frame."
    echo
    echo "Logs and per-second samples: \`$OUT_DIR\`"
} >>"$summary"

cat "$summary"
