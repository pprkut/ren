#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Measures the article view on real items from a `ren --dump-items`
# directory: memory before and after opening articles, and the time to the
# first frame of each article. Compares plain text with the Blitz view
# (without images, with images from their servers and from the image cache,
# and with glibc's mmap threshold fixed on top of trimming the heap) and
# with a build without the `html-view` feature. The image cache starts empty
# in $OUT_DIR/cache. Needs a graphical session; don't touch the window while
# it runs.
#
# Usage: scripts/measure-articles.sh DUMP_DIR [ARTICLES]
#   ARTICLES: articles opened one after another (default 100)
#
# Environment:
#   OUT_DIR   logs (default target/measure/articles-<timestamp>)
#
# Prints a Markdown summary, also written to $OUT_DIR/summary.md. It contains
# no article content, titles or URLs.

set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $# -lt 1 ]]; then
    sed -n 's/^# \?//p' "$0" | sed -n '4,19p' >&2
    exit 1
fi
DUMP=$1
ARTICLES=${2:-100}
OUT_DIR=${OUT_DIR:-target/measure/articles-$(date +%Y%m%d-%H%M%S)}

if [[ -z ${DISPLAY:-} && -z ${WAYLAND_DISPLAY:-} ]]; then
    echo "no graphical session (neither DISPLAY nor WAYLAND_DISPLAY is set)" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"
log() { echo "$*" >&2; }

log "== building"
cargo build --release --locked --no-default-features --features renderer-software
cp target/release/ren "$OUT_DIR/ren-plain"
cargo build --release --locked
cp target/release/ren "$OUT_DIR/ren-html"

# The images of the articles, fetched by the "from their servers" variant.
export XDG_CACHE_HOME=$OUT_DIR/cache

# name|binary|extra arguments|environment
variants=(
    "build without html-view|ren-plain||"
    "plain text|ren-html|--plain-text|"
    "Blitz, no images|ren-html|--no-images|"
    "Blitz, images from their servers|ren-html||"
    "Blitz, images from the cache|ren-html||"
    "Blitz, images from the cache, fixed mmap threshold|ren-html||GLIBC_TUNABLES=glibc.malloc.mmap_threshold=131072"
)

mib() { awk -v k="$1" 'BEGIN { printf "%.1f", k / 1024 }'; }

# Field $2 (rss, anon, file, peak) of the "rss <when>" line matching $3 in log $1, in MiB.
rss() {
    local line
    line=$(grep "^ren: rss $3:" "$1" | tail -n 1) || {
        echo "-"
        return
    }
    case $2 in
        rss) mib "$(sed -n 's/.*: \([0-9]*\) KiB (anon.*/\1/p' <<<"$line")" ;;
        anon) mib "$(sed -n 's/.*(anon \([0-9]*\),.*/\1/p' <<<"$line")" ;;
        file) mib "$(sed -n 's/.*, file \([0-9]*\),.*/\1/p' <<<"$line")" ;;
        peak) mib "$(sed -n 's/.*peak \([0-9]*\) KiB.*/\1/p' <<<"$line")" ;;
    esac
}

# "median / p90 / max" of field $2 (total, parse, layout, paint) of the
# article timings in log $1, in ms.
timing() {
    local pattern
    case $2 in
        total) pattern='s/.*total \([0-9.]*\) ms.*/\1/p' ;;
        parse) pattern='s/.*parse \([0-9.]*\) ms.*/\1/p' ;;
        layout) pattern='s/.*style+layout \([0-9.]*\) ms.*/\1/p' ;;
        paint) pattern='s/.*paint \([0-9.]*\) ms.*/\1/p' ;;
    esac
    grep '^ren: article rendered:' "$1" | sed -n "$pattern" | sort -g | awk '
        { v[NR] = $1 }
        END {
            if (NR == 0) { print "-"; exit }
            p50 = v[int((NR + 1) / 2)]; p90 = v[int(NR * 0.9 + 0.999)]
            printf "%.1f / %.1f / %.1f", p50, p90, v[NR]
        }'
}

summary=$OUT_DIR/summary.md
{
    echo "## ren article measurements, $(date -u +'%Y-%m-%d %H:%M UTC')"
    echo
    echo "- ren: $(git describe --always --dirty)"
    echo "- kernel: $(uname -sr)"
    echo "- CPU: $(awk -F': ' '/^model name/ { print $2; exit }' /proc/cpuinfo)"
    echo "- session: ${XDG_SESSION_TYPE:-unknown}, ${XDG_CURRENT_DESKTOP:-unknown}"
    echo "- $ARTICLES articles opened one after another, 300 ms apart, 3 s settling at the end"
    echo "- binary size: without html-view $(mib $(($(stat -c %s "$OUT_DIR/ren-plain") / 1024))) MiB, with $(mib $(($(stat -c %s "$OUT_DIR/ren-html") / 1024))) MiB"
    echo
    echo "| variant | RSS before | after 1st | after last | peak | anon / file after last | first frame ms (median / p90 / max) | parse ms | style+layout ms | paint ms | heap trims ms (median / max) |"
    echo "|---|---|---|---|---|---|---|---|---|---|---|"
} >"$summary"

# "median / max" of the heap trims in log $1, in ms.
trims() {
    sed -n 's/^ren: heap trimmed in \([0-9.]*\) ms.*/\1/p' "$1" | sort -g | awk '
        { v[NR] = $1 }
        END {
            if (NR == 0) { print "-"; exit }
            printf "%.1f / %.1f", v[int((NR + 1) / 2)], v[NR]
        }'
}

images=()
for variant in "${variants[@]}"; do
    IFS='|' read -r name bin extra environment <<<"$variant"
    log "== $name"
    vlog=$OUT_DIR/${name//[^a-z]/-}.log
    # shellcheck disable=SC2086
    env $environment "$OUT_DIR/$bin" --dump "$DUMP" --items 5000 --cycle-articles "$ARTICLES" \
        $extra 2>"$vlog" || {
        log "failed, see $vlog"
        continue
    }
    after="after [0-9]* articles"
    printf '| %s | %s | %s | %s | %s | %s / %s | %s | %s | %s | %s | %s |\n' \
        "$name" "$(rss "$vlog" rss "before articles")" "$(rss "$vlog" rss "after 1 article")" \
        "$(rss "$vlog" rss "$after")" "$(rss "$vlog" peak "$after")" \
        "$(rss "$vlog" anon "$after")" "$(rss "$vlog" file "$after")" \
        "$(timing "$vlog" total)" "$(timing "$vlog" parse)" "$(timing "$vlog" layout)" \
        "$(timing "$vlog" paint)" "$(trims "$vlog")" >>"$summary"
    if stats=$(grep '^ren: images:' "$vlog" | tail -n 1); then
        images+=("- $name: ${stats#ren: images: }")
    fi
done

{
    echo
    echo "RSS in MiB. First frame: from building the document to the first painted frame of each"
    echo "article (images arrive later and are not included). Heap trims: malloc_trim a second"
    echo "after each article switch."
    echo
    echo "Images:"
    echo
    printf '%s\n' "${images[@]}"
    echo
    echo "Image cache: $(du -sh "$XDG_CACHE_HOME" 2>/dev/null | cut -f1), $(find "$XDG_CACHE_HOME" -type f 2>/dev/null | wc -l) files"
    echo
    echo "Logs: \`$OUT_DIR\`"
} >>"$summary"

cat "$summary"
