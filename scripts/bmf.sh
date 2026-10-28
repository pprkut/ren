# shellcheck shell=bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The measurements' results as Bencher Metric Format (BMF) JSON, for
# Bencher (docs/decisions/0019-ci.md, "Performance tracking"). Sourced by
# the measurement scripts:
#
#   bmf_init FILE                  results go to FILE, the records first to
#                                  FILE.tsv
#   bmf BENCHMARK MEASURE VALUE    records a value; a value recorded again
#                                  (another run) gives the mean, with the
#                                  lowest and highest as bounds; empty or
#                                  "-" values are left out
#   bmf_kib BENCHMARK MEASURE KIB  a size in KiB, recorded in bytes
#   bmf_ms BENCHMARK MS            a time in milliseconds, as latency
#   bmf_write                      writes FILE from the records
#   bmf_merge FILE INPUT...        merges BMF files into FILE
#
# Measures (slugs; Bencher creates a measure when it first sees it):
#   rss, anonymous-memory  bytes; their units are set to "bytes (B)" once
#                          in Bencher, so it shows MiB
#   anonymous-memory-unsteady
#                          bytes, the same; measure-ci.sh's for the steps
#                          whose anonymous memory varies on CI's runners
#   file-size              bytes (Bencher's own)
#   latency                nanoseconds (Bencher's own)
#   cpu                    percent of one core
#   frame-rate             frames per second
#   frames                 frames counted
#
# Needs jq.

bmf_init() {
    BMF_FILE=$1
    : >"$BMF_FILE.tsv"
}

bmf() {
    [[ $3 =~ ^[0-9]+(\.[0-9]+)?$ ]] || return 0
    printf '%s\t%s\t%s\n' "$1" "$2" "$3" >>"$BMF_FILE.tsv"
}

bmf_kib() {
    [[ $3 =~ ^[0-9]+$ ]] || return 0
    bmf "$1" "$2" $(($3 * 1024))
}

bmf_ms() {
    [[ $2 =~ ^[0-9]+(\.[0-9]+)?$ ]] || return 0
    bmf "$1" latency "$(awk -v ms="$2" 'BEGIN { printf "%.0f", ms * 1e6 }')"
}

bmf_write() {
    jq -Rn '
        [inputs | split("\t") | { benchmark: .[0], measure: .[1], value: (.[2] | tonumber) }]
        | group_by([.benchmark, .measure])
        | reduce .[] as $values ({};
            ($values | map(.value)) as $v
            | .[$values[0].benchmark][$values[0].measure] =
                if ($v | length) > 1 then
                    { value: ($v | add / length), lower_value: ($v | min), upper_value: ($v | max) }
                else
                    { value: $v[0] }
                end)
    ' <"$BMF_FILE.tsv" >"$BMF_FILE"
}

bmf_merge() {
    local out=$1
    shift
    jq -s 'reduce .[] as $results ({}; . * $results)' "$@" >"$out"
}
