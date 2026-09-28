<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0001 — UI toolkit: Slint (spike S1)

**Status:** draft; waiting for measurements on a real desktop.

## Context

`docs/PLAN.md` picks Slint as the UI toolkit, provisionally. Spike S1 has to
show whether Slint can carry the main window within the memory and CPU
goals, and which of its renderers (software, FemtoVG, Skia) should be the
default. Rendering articles (Blitz, S3) and full pages (Servo, S4) come
later and may change the renderer choice again.

## What was tried

Slint 1.18.1 with the winit backend. The `ren` binary opens the main window
with the three panes as placeholders:

- **Feed tree:** folders with feeds and unread counts, flattened into a
  `ListView` (Slint has no tree widget). Clicking a folder expands or
  collapses it, clicking a feed filters the item list. The tree logic is a
  plain-Rust view model (`feed_tree.rs`).
- **Item list:** 10,000 dummy items (`--items N` to change) in a virtualised
  `ListView`. The model implements `slint::Model` directly and builds a row
  only when the list asks for it, the way the store-backed list will work
  later; only item ids and read flags are kept in memory
  (`dummy.rs`, `ui/mod.rs`). Selecting an item marks it read.
- **Article pane:** title, feed, date and a plain-text body in a
  `ScrollView`.

Cargo features pick the compiled-in renderers: `renderer-software` and
`renderer-femtovg` (default), `renderer-skia` (optional). `--renderer`
selects one at runtime.

Measurement support:

- `ren --measure` prints when the window was created, when the event loop
  runs and, where the renderer supports rendering notifiers, when the first
  frame was rendered.
- `ren --autoscroll` scrolls the item list from top to bottom at
  8000 px/s, updated at ~60 Hz, then quits.
- `scripts/measure.sh` (`just measure`) runs, per renderer: repeated
  startups, an idle phase sampling RSS/PSS (with anon/file/shmem split),
  thread count and CPU time from `/proc`, and an autoscroll phase with
  Slint's own frame counter (`SLINT_DEBUG_PERFORMANCE`). It prints a
  Markdown summary and keeps logs and per-second samples.

### Findings that don't need a display

- **Dependencies.** On Linux Slint's winit backend always includes `zbus`
  with the `async-io` executor; it uses it to follow the desktop's colour
  scheme and accent colour through the XDG desktop portal. This is an async
  stack we can't opt out of, but it runs inside Slint's event loop (plus
  `async-io`'s reactor thread) and is not something we build on.
- `renderer-skia` additionally pulls in `wgpu` and downloads prebuilt Skia
  binaries at build time. Skia tries a wgpu (Vulkan) surface first, then
  OpenGL, then falls back to a software surface; the measurement summary
  records which one was actually used.
- **Binary size** (release, stripped, thin LTO): 17.7 MB with software +
  FemtoVG, 26.7 MB with Skia in addition.
- **Build:** the system libraries needed at build time are only
  `libfontconfig-dev` (CI installs it); X11, Wayland, xkbcommon, EGL/GL are
  loaded at runtime. At runtime the X11 backend needs
  `libxkbcommon-x11.so`.
- All dependency licenses fit the policy (`cargo deny check licenses`).
  Slint is used under `GPL-3.0-only`.
- The software renderer and Skia with a software surface don't support
  rendering notifiers, so for them the startup time is "until the event loop
  runs", which is right after the window is shown but may be slightly before
  the first frame is on screen.

## Measurements

To be run on the target desktop (X11 or Wayland, real GPU):

```sh
just measure            # or: scripts/measure.sh software femtovg skia
```

Plus a manual check: run `cargo run --release -- --renderer <name>` and
scroll the item list with the mouse wheel and by dragging the scrollbar,
for each renderer, and note whether it is smooth.

*Results: pending.*

## Decision

*Pending the measurements:* Slint yes/no, and the default renderer.

## Consequences

*Pending.*
