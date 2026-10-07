#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Measures syncing with the Nextcloud server into a new database: the
# initial sync, an incremental one right after it, a resync (paging
# through the unread and starred items again) and another incremental
# sync. Time, CPU, requests and memory: before, at the peak, after the
# sync, and after giving freed memory back to the system (malloc_trim).
# Uses the account from the settings file (see `ren --help`).
#
# Only reads from the server: a new database has no local changes to send.
#
# Usage: scripts/measure-sync.sh
#
# Environment:
#   SETTINGS  settings file (default: $XDG_CONFIG_HOME/ren/settings.toml)
#   DB_DIR    directory for the database, outside the repository (default:
#             a new temporary directory, removed afterwards)
#   OUT_DIR   raw output (default target/measure/sync-<timestamp>)
#
# A password-command is run once per invocation of ren; to avoid repeated
# prompts, export REN_APP_PASSWORD or let gpg-agent cache the passphrase.
#
# Prints a Markdown summary, also written to $OUT_DIR/summary.md. The server
# URL is not part of the summary.

set -euo pipefail
cd "$(dirname "$0")/.."

OUT_DIR=${OUT_DIR:-target/measure/sync-$(date +%Y%m%d-%H%M%S)}

# The default features: the binary as it is used.
cargo build --release --locked
bin=target/release/ren
args=()
if [[ -n ${SETTINGS:-} ]]; then
    args+=(--settings "$SETTINGS")
fi
mkdir -p "$OUT_DIR"

if [[ -n ${DB_DIR:-} ]]; then
    mkdir -p "$DB_DIR"
    if [[ -e $DB_DIR/ren.db ]]; then
        echo "$DB_DIR/ren.db exists; the measurement needs a new database" >&2
        exit 1
    fi
else
    DB_DIR=$(mktemp -d -t ren-measure-sync.XXXXXX)
    trap 'rm -rf "$DB_DIR"' EXIT
fi
db=$DB_DIR/ren.db

log() { echo "$*" >&2; }

# Value of key $2 in a "sync: key=value ..." line $1.
field() { sed -n "s/.* $2=\([^ ]*\).*/\1/p" <<<"$1"; }

mib() { awk -v b="$1" -v d="$2" 'BEGIN { printf "%.1f", b / d }'; }

log "== check"
"$bin" "${args[@]}" --check | tee "$OUT_DIR/check.txt" >&2

steps=("initial" "incremental" "resync" "incremental again")
step_args=("" "" "--resync" "")
lines=()
for i in "${!steps[@]}"; do
    log "== ${steps[$i]}"
    # shellcheck disable=SC2086 # the step's arguments may be empty
    out=$("$bin" "${args[@]}" --sync "$db" ${step_args[$i]} 2> >(tee -a "$OUT_DIR/sync.err" >&2))
    line=$(grep '^sync:' <<<"$out")
    echo "$line" >>"$OUT_DIR/sync.log"
    lines+=("$line")
done

summary=$OUT_DIR/summary.md
{
    echo "## ren sync measurements, $(date -u +'%Y-%m-%d %H:%M UTC')"
    echo
    echo "- ren: $(git describe --always --dirty)"
    echo "- kernel: $(uname -sr)"
    echo "- CPU: $(awk -F': ' '/^model name/ { print $2; exit }' /proc/cpuinfo)"
    echo "- server: $(grep -v '^Server:' "$OUT_DIR/check.txt" | paste -sd ';' | sed 's/;/; /g')"
    echo "- one run per step, into a new database; CPU is ren's own user + system time"
    echo
    echo "| step | kind | requests | items | added | reconciled | purged | wall s | CPU s (user + sys) | RSS before MiB | peak RSS MiB | RSS after MiB | after malloc_trim MiB | database MiB |"
    echo "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
    for i in "${!steps[@]}"; do
        line=${lines[$i]}
        trimmed=$(field "$line" rss_trimmed_kib)
        [[ $trimmed == "?" ]] || trimmed=$(mib "$trimmed" 1024)
        printf '| %s | %s | %s | %s | %s | %s | %s | %.3f | %.2f | %s | %s | %s | %s | %s |\n' \
            "${steps[$i]}" "$(field "$line" kind)" "$(field "$line" requests)" \
            "$(field "$line" items)" "$(field "$line" added)" "$(field "$line" reconciled)" \
            "$(field "$line" purged)" "$(field "$line" seconds)" \
            "$(awk -v u="$(field "$line" user_seconds)" -v s="$(field "$line" sys_seconds)" 'BEGIN { print u + s }')" \
            "$(mib "$(field "$line" rss_before_kib)" 1024)" "$(mib "$(field "$line" peak_rss_kib)" 1024)" \
            "$(mib "$(field "$line" rss_after_kib)" 1024)" "$trimmed" \
            "$(mib "$(field "$line" db_bytes)" 1048576)"
    done
    echo
    echo "Raw output: \`$OUT_DIR\`"
} >"$summary"

cat "$summary"
