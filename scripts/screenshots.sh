#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Takes the README's screenshots: Ren with a made-up account
# (scripts/screenshots/showcase.py) on a display of its own (Xvfb), so
# neither the desktop's theme nor real feed data shows. The articles'
# images and the page opened in a tab are served from
# scripts/screenshots/site on 127.0.0.1; nothing goes to the network.
#
#   main-light.webp  an article, light
#   main-dark.webp   the same, dark
#   tab.webp         its web page in a tab (Servo's helper)
#
# Usage: scripts/screenshots.sh
#
# Environment:
#   REN      the ren binary (default target/release/ren, from `just
#            build`; Servo's helper is looked for next to it)
#   OUT_DIR  where the screenshots go (default docs/screenshots)
#   PORT     the local web server's port (default 8737)
#
# Needs Xvfb, xdotool, ImageMagick (magick), rsvg-convert, cwebp and
# Python 3.

set -euo pipefail
cd "$(dirname "$0")/.."

REN=$(realpath "${REN:-target/release/ren}")
OUT_DIR=${OUT_DIR:-docs/screenshots}
PORT=${PORT:-8737}
WIDTH=1280
HEIGHT=800

for tool in Xvfb xdotool magick rsvg-convert cwebp python3; do
    command -v "$tool" >/dev/null || { echo "$tool is missing" >&2; exit 1; }
done
[[ -x $REN ]] || { echo "$REN not found: run just build, or set REN" >&2; exit 1; }

tmp=$(mktemp -d)
pids=()
cleanup() {
    for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
    rm -rf "$tmp"
}
trap cleanup EXIT

# The site: the page, its chart as PNG (the article view and Servo get
# the same image).
mkdir -p "$tmp/site"
cp scripts/screenshots/site/pond.html "$tmp/site/"
rsvg-convert -w 1280 scripts/screenshots/site/pond-chart.svg -o "$tmp/site/pond-chart.png"
python3 -m http.server --bind 127.0.0.1 --directory "$tmp/site" "$PORT" >"$tmp/http.log" 2>&1 &
pids+=($!)

# A throwaway profile: no account, no syncs, no tray (Xvfb has none), no
# filter lists to download.
export XDG_CONFIG_HOME=$tmp/config XDG_STATE_HOME=$tmp/state
export XDG_DATA_HOME=$tmp/data XDG_CACHE_HOME=$tmp/cache
mkdir -p "$XDG_CONFIG_HOME/ren" "$XDG_STATE_HOME" "$XDG_DATA_HOME"
cat >"$XDG_CONFIG_HOME/ren/settings.toml" <<'EOF'
[sync]
interval-minutes = 0

[pages]
block-content = false

[window]
tray-icon = false
EOF
scripts/screenshots/showcase.py "$tmp/dump" "$XDG_CACHE_HOME/ren/favicons" "http://127.0.0.1:$PORT"

# A display of its own; Xvfb picks a free number.
exec {fd}<> <(:)
Xvfb -displayfd "$fd" -screen 0 "${WIDTH}x${HEIGHT}x24" -nolisten tcp >"$tmp/xvfb.log" 2>&1 &
pids+=($!)
read -r -t 10 display <&"$fd" || { echo "Xvfb didn't start" >&2; cat "$tmp/xvfb.log" >&2; exit 1; }
export DISPLAY=:$display
unset WAYLAND_DISPLAY
export TZ=UTC

mkdir -p "$OUT_DIR"

# The window of the ren with PID $1 (by its PID, never its title).
window_of() {
    local window
    for _ in $(seq 40); do
        window=$(xdotool search --pid "$1" 2>/dev/null | head -n 1) || true
        [[ -n $window ]] && { echo "$window"; return; }
        sleep 0.25
    done
    echo "ren's window didn't appear" >&2
    return 1
}

shot() {
    magick import -window "$1" "$tmp/$2.png"
    cwebp -quiet -lossless -z 9 "$tmp/$2.png" -o "$OUT_DIR/$2.webp"
    echo "$OUT_DIR/$2.webp"
}

for scheme in light dark; do
    "$REN" --dump "$tmp/dump" --color-scheme "$scheme" --bundled-icons >"$tmp/ren-$scheme.log" 2>&1 &
    ren=$!
    pids+=("$ren")
    window=$(window_of "$ren")
    xdotool windowsize "$window" "$WIDTH" "$HEIGHT"
    # The pointer in a corner, where it highlights nothing.
    xdotool mousemove "$((WIDTH - 1))" "$((HEIGHT - 1))"
    sleep 2
    xdotool windowfocus "$window"
    # J opens the first article.
    xdotool key j
    sleep 2
    shot "$window" "main-$scheme"
    if [[ $scheme == light ]]; then
        # O opens its page in a tab; the helper starts and loads it.
        xdotool key o
        sleep 10
        shot "$window" tab
    fi
    kill "$ren"
    wait "$ren" 2>/dev/null || true
done
