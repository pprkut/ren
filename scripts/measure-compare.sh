#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Runs one measurement script on two commits and compares their results,
# for an upgrade measured on its own (a new Servo: measure-tabs.sh; a new
# Blitz: measure-articles.sh). Each commit is checked out in a worktree
# of its own under OUT_DIR, with ren's target directory (a symbolic
# link, which the helper's workspace follows too: what both share, and
# Servo's or Blitz's build of the commit not upgraded, is built only
# once), and measured with that commit's script; with ROUNDS, the two take
# turns, so a machine that warms up or gets busy affects both alike. The
# script's arguments are passed on, paths among them made absolute.
# Needs what the script needs (mostly a graphical session; don't touch
# the window while it runs) and jq.
#
# Usage: scripts/measure-compare.sh BASE HEAD SCRIPT [ARGS...]
#   BASE, HEAD  commits (a branch, a tag or a hash)
#   SCRIPT      a script in scripts/ that writes $OUT_DIR/results.json,
#               e.g. measure-tabs.sh
#
# Environment:
#   ROUNDS   runs of each commit, taking turns (default 1); a value
#            measured in several gives their median, as in bmf.sh
#   OUT_DIR  results (default target/measure/compare-<timestamp>): each
#            run's own directory, BASE's and HEAD's results.json and
#            commit (for `just measure-upload`), and summary.md
#   Others (VARIANTS, ...) go to the script.
#
# Prints a Markdown table of what was measured on both commits, also
# written to $OUT_DIR/summary.md: the values and HEAD's change.

set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD

if [[ $# -lt 3 ]]; then
    sed -n 's/^# \?//p' "$0" | sed -n '4,30p' >&2
    exit 1
fi
base=$1
head=$2
script=${3##*/}
shift 3
if [[ ! -x scripts/$script ]]; then
    echo "no such script: scripts/$script" >&2
    exit 1
fi
rounds=${ROUNDS:-1}
if [[ ! $rounds =~ ^[1-9][0-9]*$ ]]; then
    echo "ROUNDS must be a positive number" >&2
    exit 1
fi
OUT_DIR=$(realpath -m "${OUT_DIR:-target/measure/compare-$(date +%Y%m%d-%H%M%S)}")
# Not passed on to cargo (see measure-all.sh): ring's build script runs
# again when OUT_DIR changes.
export -n OUT_DIR

# The worktrees are elsewhere: paths given relative to the repository
# are made absolute.
args=()
for arg in "$@"; do
    if [[ -e $arg ]]; then
        args+=("$(realpath "$arg")")
    else
        args+=("$arg")
    fi
done

# shellcheck source=scripts/bmf.sh
source scripts/bmf.sh

mkdir -p "$OUT_DIR"
labels=(base head)
declare -A commit
commit[base]=$(git rev-parse --verify "$base^{commit}")
commit[head]=$(git rev-parse --verify "$head^{commit}")

cleanup() {
    for label in "${labels[@]}"; do
        if [[ -d $OUT_DIR/$label-src ]]; then
            # The link first: the build stays.
            rm -f "$OUT_DIR/$label-src/target"
            git worktree remove --force "$OUT_DIR/$label-src" || true
        fi
    done
}
trap cleanup EXIT

for label in "${labels[@]}"; do
    git worktree add --detach "$OUT_DIR/$label-src" "${commit[$label]}" >&2
    ln -s "$root/target" "$OUT_DIR/$label-src/target"
done

for round in $(seq "$rounds"); do
    for label in "${labels[@]}"; do
        echo "== $label (${commit[$label]:0:10}), round $round of $rounds" >&2
        (
            cd "$OUT_DIR/$label-src"
            OUT_DIR=$OUT_DIR/$label-$round "scripts/$script" "${args[@]}" >&2
        )
    done
done

# Each commit's runs together: medians from the recorded values.
for label in "${labels[@]}"; do
    mkdir -p "$OUT_DIR/$label"
    echo "${commit[$label]}" >"$OUT_DIR/$label/commit"
    bmf_init "$OUT_DIR/$label/results.json"
    for round in $(seq "$rounds"); do
        tsv=$OUT_DIR/$label-$round/results.json.tsv
        if [[ -f $tsv ]]; then
            cat "$tsv" >>"$BMF_FILE.tsv"
        fi
    done
    bmf_write
done

{
    echo "# $script: ${base} (${commit[base]:0:10}) → ${head} (${commit[head]:0:10})"
    echo
    echo "$rounds round(s) each, $(date -u +'%Y-%m-%d %H:%M UTC'). Medians; the range over the rounds in brackets."
    echo
    echo "| benchmark | measure | base | head | change |"
    echo "|---|---|---|---|---|"
    jq -rn --slurpfile b "$OUT_DIR/base/results.json" --slurpfile h "$OUT_DIR/head/results.json" '
        def unit($m): if $m == "rss" or $m == "anonymous-memory" or $m == "anonymous-memory-unsteady" or $m == "file-size" then "MiB"
                      elif $m == "latency" then "ms" else "" end;
        def scaled($m; $v): if unit($m) == "MiB" then $v / 1048576 elif unit($m) == "ms" then $v / 1e6 else $v end;
        def fmt($x): ($x * 10 | round / 10 | tostring);
        def show($m; $r):
            fmt(scaled($m; $r.value)) + (if unit($m) != "" then " " + unit($m) else "" end)
            + (if $r.lower_value and $r.lower_value != $r.upper_value then " [" + fmt(scaled($m; $r.lower_value)) + "–" + fmt(scaled($m; $r.upper_value)) + "]" else "" end);
        $b[0] as $base | $h[0] as $head
        | $base | keys[] as $bench | $base[$bench] | keys[] as $m
        | select($head[$bench][$m])
        | $base[$bench][$m] as $x | $head[$bench][$m] as $y
        | "| \($bench) | \($m) | \(show($m; $x)) | \(show($m; $y)) | "
          + (if $x.value == 0 then "–" else (($y.value - $x.value) / $x.value * 100 | . * 10 | round / 10
             | (if . > 0 then "+" else "" end) + tostring + " %") end) + " |"'
    only() {
        jq -rn --slurpfile a "$1" --slurpfile o "$2" \
            '$a[0] as $a | $o[0] as $o | $a | keys[] as $bench | $a[$bench] | keys[] as $m | select($o[$bench][$m] | not) | "\($bench) (\($m))"'
    }
    for pair in "base head" "head base"; do
        read -r a o <<<"$pair"
        missing=$(only "$OUT_DIR/$a/results.json" "$OUT_DIR/$o/results.json" | paste -sd ';' | sed 's/;/; /g')
        if [[ -n $missing ]]; then
            echo
            echo "Only measured on $a: $missing."
        fi
    done
} | tee "$OUT_DIR/summary.md"
