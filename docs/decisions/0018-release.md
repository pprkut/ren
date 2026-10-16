<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0018 — Release and packaging (M11)

**Status:** done (2026-10-15), for review. Checked in the container: the
notices, the metadata's validation, the install layout and the release
tarballs; not yet on the desktop.

## Context

The plan's M11 asks for the third-party notices, generated with
cargo-about for both workspaces into a committed
`THIRD-PARTY-NOTICES.html`, regenerated with `just notices` and checked
by CI against `Cargo.lock`, with what isn't a crate added by hand (the
bundled icons, libraries vendored inside crates such as SpiderMonkey's);
and for a `.desktop` file, the app icon (M10b's logo), AppStream
metadata, the install layout (the binary, the Servo helper in
`libexec/ren/`, notices, icons) and release tarballs. Distributions'
build files stay out of the repository: they belong to the packagers'
own repositories, which build from the release tarballs below.

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

`just notices` (`scripts/notices.sh`) runs `cargo about generate
--format json --all-features --frozen` for both workspaces (after `cargo
fetch`), and jq merges their licences, each text once (compared without
whitespace) with the crates using it, without Ren's own crates, and
writes the HTML: `notices/header.html` (by hand: an introduction, the
logo, Lucide's icons), the libraries inside crates, and the crates.

cargo-about's usual way, a Handlebars template (`about.hbs`), renders
only its own scan of one workspace, and the two can't be scanned as one
(their stylo versions). A template per workspace would list the
licences they share twice: 1.5 MB of licence texts instead of 0.8, in
the binary and in About Ren's memory. So the merge stays, in jq.

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
- **CI** checks that the file matches (`scripts/notices.sh --check`, a
  job of its own, with cargo-about 0.9.2: another version may word it
  differently).

The file is 740 KiB, in the binary (About Ren's licences page, M8b) and
installed as documentation. The page costs 1.8 MiB while it's shown (0.5
with M8b's placeholder), given back when it closes (`--measure-settings`
in the container).

## The install layout

`just install`, from what `just build` built, with the variables of
COSMIC's justfiles (System76's Rust desktop apps, the closest thing to
a convention for Rust apps installed with just; packagers set them by
name, as with make's): `prefix` (`/usr/local`; COSMIC's is `/usr`, as
its apps come with the distribution) and `rootdir`, prepended to every
path for packages, so `just prefix=/usr rootdir="$PKG" install`.
`just uninstall` with the same variables removes it.

```
bin/ren
libexec/ren/ren-servo                  (Ren looks in ../libexec/ren/)
share/applications/io.github.pprkut.ren.desktop
share/metainfo/io.github.pprkut.ren.metainfo.xml
share/icons/hicolor/scalable/apps/io.github.pprkut.ren.svg
share/icons/hicolor/{16..512}x{..}/apps/io.github.pprkut.ren.png
share/doc/ren/{README.md,THIRD-PARTY-NOTICES.html,COPYING}
```

`docdir=…` moves the documentation (e.g. to `/usr/doc/ren-VERSION`),
`helper=false` leaves out the helper.

## Release tarballs

The source is the tag's archive, as GitHub makes it. `just vendor`
writes `vendor.tar.xz` (COSMIC's `vendor`, compressed), to go with the
release: the crates of both workspaces (`cargo vendor
--versioned-dirs`, one directory, as the lock files may want different
versions of a crate) and the `.cargo/config.toml` that uses it. `just
build-vendored` unpacks it into the source (`vendor-extract`) and builds
both workspaces with `--frozen`: the helper's own configuration only
sets its target directory, and cargo merges the parent's. 141 MiB
(1,097 crates; Servo's and SpiderMonkey's sources are most of it).
Checked: both workspaces resolve from it with an empty cargo home, and
a crate builds.

## On the desktop

Not checked there yet; worth a look before the release:

- `just install` into a prefix of your own (`just prefix="$HOME/.local"
  install`, or `just prefix=/usr rootdir="$PKG" install` into a staging
  directory): Ren in the
  launcher under its `.desktop` file's name and icon, and the icon in
  the task bar on Wayland (matched by the app id) and X11;
- starting the installed Ren: the helper found in `libexec/ren/`;
- About Ren → the licences page, with the generated notices.

## Open

- Building the helper offline from `vendor.tar.xz`: SpiderMonkey's build
  from source (`MOZJS_FROM_SOURCE=1`) needs clang, libclang, Python 3
  and a C++ compiler, and several GB of disk; not tried yet.
- The AppStream release needs the date of the release; the
  screenshot's URL works once `data/` is on master.
