#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Measures fetching all unread items from the Nextcloud server for several
# batch sizes: requests, transferred JSON, wall time, CPU time and peak RSS.
# Uses the account from the settings file (see `ren --help`).
#
# Usage: scripts/measure-fetch.sh [BATCH_SIZE...]
#   BATCH_SIZE: items per request, or "all" (default: 50 200 1000 all)
#
# Environment:
#   RUNS      runs per batch size, interleaved, after one warm-up (default 3)
#   SETTINGS  settings file (default: $XDG_CONFIG_HOME/ren/settings.toml)
#   OUT_DIR   raw output (default target/measure/fetch-<timestamp>)
#
# A password-command is run once per invocation of ren; to avoid repeated
# prompts, export REN_APP_PASSWORD or let gpg-agent cache the passphrase.
# Its CPU time is not part of the measurement.
#
# Prints a Markdown summary, also written to $OUT_DIR/summary.md. The server
# URL is not part of the summary.

set -euo pipefail
cd "$(dirname "$0")/.."

RUNS=${RUNS:-3}
OUT_DIR=${OUT_DIR:-target/measure/fetch-$(date +%Y%m%d-%H%M%S)}

batch_sizes=("$@")
if [[ ${#batch_sizes[@]} -eq 0 ]]; then
    batch_sizes=(50 200 1000 all)
fi

cargo build --release --locked
bin=target/release/ren
args=()
if [[ -n ${SETTINGS:-} ]]; then
    args+=(--settings "$SETTINGS")
fi
mkdir -p "$OUT_DIR"

log() { echo "$*" >&2; }

# Value of key $2 in a "fetch: key=value ..." line $1.
field() { sed -n "s/.* $2=\([^ ]*\).*/\1/p" <<<"$1"; }

# Median of the numbers given as arguments.
median() {
    printf '%s\n' "$@" | sort -g | awk '{ v[NR] = $1 } END {
        if (NR % 2) print v[(NR + 1) / 2]; else print (v[NR / 2] + v[NR / 2 + 1]) / 2 }'
}

mib() { awk -v b="$1" -v d="$2" 'BEGIN { printf "%.1f", b / d }'; }

log "== check"
"$bin" "${args[@]}" --check | tee "$OUT_DIR/check.txt" >&2

log "== warm-up"
"$bin" "${args[@]}" --fetch-unread >/dev/null

declare -A lines
for ((run = 1; run <= RUNS; run++)); do
    for b in "${batch_sizes[@]}"; do
        log "== run $run, batch size $b"
        line=$("$bin" "${args[@]}" --fetch-unread --batch-size "$b" | grep '^fetch:')
        echo "$line" >>"$OUT_DIR/fetch.log"
        lines[$b]+="$line"$'\n'
    done
done

summary=$OUT_DIR/summary.md
{
    echo "## ren S2 fetch measurements, $(date -u +'%Y-%m-%d %H:%M UTC')"
    echo
    echo "- ren: $(git describe --always --dirty)"
    echo "- kernel: $(uname -sr)"
    echo "- CPU: $(awk -F': ' '/^model name/ { print $2; exit }' /proc/cpuinfo)"
    echo "- server: $(grep -v '^Server:' "$OUT_DIR/check.txt" | paste -sd ';' | sed 's/;/; /g')"
    echo "- median of $RUNS runs after one warm-up; CPU is ren's own user + system time while fetching"
    echo
    echo "| batch size | requests | items | repeated | JSON MiB | bodies MiB | wall s | CPU s (user + sys) | CPU % of wall | RSS before MiB | peak RSS MiB |"
    echo "|---|---|---|---|---|---|---|---|---|---|---|"
    for b in "${batch_sizes[@]}"; do
        mapfile -t runs < <(printf '%s' "${lines[$b]}")
        walls=() cpus=() peaks=()
        for line in "${runs[@]}"; do
            walls+=("$(field "$line" seconds)")
            cpus+=("$(awk -v u="$(field "$line" user_seconds)" -v s="$(field "$line" sys_seconds)" \
                'BEGIN { print u + s }')")
            peaks+=("$(field "$line" peak_rss_kib)")
        done
        first=${runs[0]}
        wall=$(median "${walls[@]}")
        cpu=$(median "${cpus[@]}")
        printf '| %s | %s | %s | %s | %s | %s | %.3f | %.2f | %s | %s | %s |\n' \
            "$b" "$(field "$first" pages)" "$(field "$first" items)" "$(field "$first" repeated)" \
            "$(mib "$(field "$first" json_bytes)" 1048576)" "$(mib "$(field "$first" body_bytes)" 1048576)" \
            "$wall" "$cpu" "$(awk -v c="$cpu" -v w="$wall" 'BEGIN { printf "%.0f", (w > 0) ? c * 100 / w : 0 }')" \
            "$(mib "$(field "$first" rss_before_kib)" 1024)" "$(mib "$(median "${peaks[@]}")" 1024)"
    done
    echo
    echo "Raw output: \`$OUT_DIR\`"
} >"$summary"

cat "$summary"
