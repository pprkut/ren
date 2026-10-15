#!/bin/bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Installs Ren as built by `just build` (target/release): the binary, the
# Servo helper where Ren looks for it (../libexec/ren/ from the binary),
# the .desktop file, the AppStream metadata, the icons and the
# documentation.
#
#     scripts/install.sh [PREFIX] [DESTDIR]
#
# PREFIX defaults to /usr/local; DESTDIR is prepended to every path (for
# packages). The environment can change: PROFILE (the cargo profile,
# release), DOCDIR (PREFIX/share/doc/ren), and NO_HELPER=1 installs
# without the Servo helper (web pages then open in the browser).

set -euo pipefail
cd "$(dirname "$0")/.."

prefix=${1:-/usr/local}
destdir=${2:-}
profile=${PROFILE:-release}
docdir=${DOCDIR:-$prefix/share/doc/ren}
id=io.github.pprkut.ren
root=$destdir$prefix

[[ -x target/$profile/ren ]] || { echo "target/$profile/ren not built: just build" >&2; exit 1; }
install -Dm755 "target/$profile/ren" "$root/bin/ren"
if [[ -z "${NO_HELPER:-}" ]]; then
    [[ -x target/$profile/ren-servo ]] \
        || { echo "target/$profile/ren-servo not built: just build, or NO_HELPER=1" >&2; exit 1; }
    install -Dm755 "target/$profile/ren-servo" "$root/libexec/ren/ren-servo"
fi

install -Dm644 "data/$id.desktop" "$root/share/applications/$id.desktop"
install -Dm644 "data/$id.metainfo.xml" "$root/share/metainfo/$id.metainfo.xml"
install -Dm644 crates/ren/icons/ren.svg "$root/share/icons/hicolor/scalable/apps/$id.svg"
for icon in data/icons/*/$id.png; do
    size=$(basename "$(dirname "$icon")")
    install -Dm644 "$icon" "$root/share/icons/hicolor/$size/apps/$id.png"
done

install -Dm644 README.md "$destdir$docdir/README.md"
install -Dm644 THIRD-PARTY-NOTICES.html "$destdir$docdir/THIRD-PARTY-NOTICES.html"
install -Dm644 LICENSES/GPL-3.0-or-later.txt "$destdir$docdir/COPYING"
echo "Installed into $root"
