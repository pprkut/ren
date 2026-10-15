<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0018 — Release and packaging (M11)

**Status:** done (2026-10-15), for review. Checked in the container: the
notices, the metadata's validation, the install layout, the release
tarballs and the SlackBuild without Servo (see "The SlackBuild"); not
yet on Slackware or the desktop.

## Context

The plan's M11 asks for the third-party notices, generated with
cargo-about for both workspaces into a committed
`THIRD-PARTY-NOTICES.html`, regenerated with `just notices` and checked
by CI against `Cargo.lock`, with what isn't a crate added by hand (the
bundled icons, libraries vendored inside crates such as SpiderMonkey's);
and for a `.desktop` file, the app icon (M10b's logo), AppStream
metadata, the install layout (the binary, the Servo helper in
`libexec/ren/`, notices, icons) and a SlackBuild.

## The application's id

`io.github.pprkut.ren`, after the repository: the `.desktop` file's
(`data/io.github.pprkut.ren.desktop`), the icon's and the AppStream
metadata's (`data/io.github.pprkut.ren.metainfo.xml`), and the windows':
`slint::set_xdg_app_id` sets Wayland's app id and X11's window class
(checked: `WM_CLASS = "", "io.github.pprkut.ren"`), by which Plasma and
GNOME match the window to its `.desktop` file and icon. The plan's M10b
kept ids lower case; the name users see is "Ren".

The `.desktop` file: Network, Feed and News, `SingleMainWindow` (M10b's
one instance), `StartupNotify` (its token goes to the running Ren on a
second start). The AppStream metadata: summary, description, the
homepage, a screenshot (`data/screenshot.png`, served from the
repository), OARS rating, keyboard and pointer, and 0.1.0 as a
development release until it's tagged. Both validate
(`desktop-file-validate`, `appstreamcli validate --pedantic`).

## Icons

The hicolor theme gets the logo as SVG (`scalable/apps`) and as PNGs
from 16 to 512 pixels: the small logo (without its wires) at 16 to 32,
the logo with its wires from 48. Desktops scale SVGs anyway, but the
fixed sizes are PNGs in the icon theme specification, and the small
sizes need the other drawing. `just icons` renders them with
`rsvg-convert`; they're committed (160 KiB), so installing needs no SVG
renderer.

## Third-party notices

`just notices` (`scripts/notices.py`) runs `cargo about generate
--format json --all-features --frozen` for both workspaces (after `cargo
fetch`), merges their licences, each text once (compared without
whitespace) with the crates using it, without Ren's own crates, and
writes the HTML: `notices/header.html` (by hand: an introduction, the
logo, Lucide's icons), the libraries inside crates, and the crates.

- **Offline**, so the result depends only on the lock files: cargo-about
  fetches licence files missing from crates from their repositories
  otherwise, which may change or fail. A crate without its file gets the
  licence's standard text.
- **Not the crates only tests and build scripts use**
  (`ignore-dev-dependencies`, `ignore-build-dependencies`): they aren't
  in the binaries. 790 crates remain, under 174 distinct texts (90 MIT,
  40 Apache-2.0 with their copyright lines, 20 ISC, …), including
  Slint's GPL 3, webpki-roots' CDLA-Permissive-2.0 and ksni's Unlicense
  (deny.toml's exceptions, in `about.toml` too).
- **Libraries inside crates:** SpiderMonkey's source in mozjs_sys brings
  ICU 78, V8's irregexp, double-conversion, fdlibm, LZ4 and function2.
  Their licence files are read from the crate; ICU's isn't in it, so
  `notices/icu-78.txt` is ICU 78's `LICENSE` (Unicode-3.0 and the notices
  of its own third-party parts). zlib comes from the libz-sys crate and
  is listed with the crates.
- **CI** checks that the file matches (`scripts/notices.py --check`, a
  job of its own, with cargo-about 0.9.2: another version may word it
  differently).

The file is 740 KiB, in the binary (About Ren's licences page, M8b) and
installed as documentation. The page costs 1.8 MiB while it's shown (0.5
with M8b's placeholder), given back when it closes (`--measure-settings`
in the container).

## The install layout

`just install [prefix] [destdir]` (`scripts/install.sh`), from what `just
build` built:

```
bin/ren
libexec/ren/ren-servo                  (Ren looks in ../libexec/ren/)
share/applications/io.github.pprkut.ren.desktop
share/metainfo/io.github.pprkut.ren.metainfo.xml
share/icons/hicolor/scalable/apps/io.github.pprkut.ren.svg
share/icons/hicolor/{16..512}x{..}/apps/io.github.pprkut.ren.png
share/doc/ren/{README.md,THIRD-PARTY-NOTICES.html,COPYING}
```

`DOCDIR` moves the documentation (Slackware's `/usr/doc/ren-VERSION`),
`NO_HELPER=1` leaves out the helper.

## Release tarballs

`just dist` (`scripts/dist.sh`) writes `target/dist/ren-VERSION.tar.xz`
(`git archive` of the commit) and `ren-VERSION-vendor.tar.xz`: the
crates of both workspaces (`cargo vendor --versioned-dirs`, one
directory, as the lock files may want different versions of a crate)
and the `.cargo/config.toml` that uses it. Unpacked into the source,
both workspaces build offline: the helper's own configuration only sets
its target directory, and cargo merges the parent's. 2.6 MiB and 152 MiB
(1,097 crates; Servo's and SpiderMonkey's sources are most of it).
Checked here: both workspaces resolve with an empty cargo home, and a
crate builds from them.

## The SlackBuild

`packaging/slackware/` has `ren.SlackBuild`, `ren.info`, `slack-desc`,
`doinst.sh` (desktop database, icon cache) and a README, in
SlackBuilds.org's form. It builds offline from the two tarballs, with a
cargo home of its own and `MOZJS_FROM_SOURCE=1`, so SpiderMonkey is
built from the crate's source instead of downloaded; it takes Rust from
rust-opt if installed and refuses one older than 1.92 (Slint 1.18's,
now `rust-version`; 1.88 fails). `WITHOUT_SERVO=yes` builds without web
page tabs (also without the window's side, so pages open in the browser).
x86_64 and aarch64; `ren.info`'s checksums are filled in when the
tarballs of a release exist.

Tried here with `WITHOUT_SERVO=yes` (a Slackware `makepkg` stand-in that
tars the package; Ubuntu's Rust 1.97): the build from the tarballs is
offline and takes 8m40s on four cores (Ren with the article view), the
package holds the files of the install layout, the SlackBuild and the
install scripts, and its `ren` runs (41 MiB, stripped). With Servo, the
helper's build adds SpiderMonkey's from source; CI's Servo job builds the
helper, but with the prebuilt SpiderMonkey.

## Open

- The SlackBuild with Servo, and on Slackware itself: SpiderMonkey's
  build from source needs clang, libclang, Python 3 and a C++ compiler,
  and several GB of disk.
- `ren.info` and the AppStream release need the date and checksums of the
  release; the screenshot's URL works once `data/` is on master.
