#!/bin/bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Renders the logo's PNGs for the hicolor icon theme into data/icons/
# (committed, so building and installing need no SVG renderer): the
# small logo at 16 to 32 pixels, the logo with its wires from 48 up.
# Needs rsvg-convert (librsvg). The SVGs are installed too, as the
# scalable icon.

set -euo pipefail
cd "$(dirname "$0")/.."
command -v rsvg-convert >/dev/null || { echo "rsvg-convert not found (librsvg)" >&2; exit 1; }

render() {
    local svg=$1 size=$2
    local dir=data/icons/${size}x${size}
    mkdir -p "$dir"
    rsvg-convert -w "$size" -h "$size" "crates/ren/icons/$svg" -o "$dir/io.github.pprkut.ren.png"
}

for size in 16 22 24 32; do
    render ren-small.svg "$size"
done
for size in 48 64 128 256 512; do
    render ren.svg "$size"
done
echo "Icons in data/icons"
