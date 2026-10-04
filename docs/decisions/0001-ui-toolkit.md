<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0001 — UI toolkit: Slint (spike S1)

**Status:** accepted (2026-09-28). S4 confirmed the software renderer as
the default (see [0005](0005-web-tabs.md)).

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

Cargo features pick the compiled-in renderers: `renderer-software`,
`renderer-femtovg` and `renderer-skia`. `--renderer` selects one at
runtime. (During the measurements software and FemtoVG were compiled in by
default; see "Consequences" for the defaults now.)

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
- **Binary size** (release, stripped, thin LTO): 17.1 MB with the software
  renderer only, 17.7 MB with FemtoVG, 26.7 MB with FemtoVG and Skia.
- **Build:** the system libraries needed at build time are only
  `libfontconfig-dev` (CI installs it); X11, Wayland, xkbcommon, EGL/GL are
  loaded at runtime. At runtime the X11 backend needs
  `libxkbcommon-x11.so`.
- All dependency licenses fit the policy (`cargo deny check licenses`).
  Slint is used under `GPL-3.0-only`.
- The software renderer and Skia on a software or wgpu surface don't
  support rendering notifiers, so for them the startup time is "until the event loop
  runs", which is right after the window is shown but may be slightly before
  the first frame is on screen.

## Measurements

Run with `just measure` at f387802 on the target machine:

- CPU: 11th Gen Intel Core i7-1185G7 @ 3.00GHz, GPU: Mesa Intel Iris Xe
  (TGL GT2); Linux 7.2.8, KDE on X11
- 10,000 items; idle: 30 s after 5 s settling; startup: median of 5 runs;
  CPU % is of one core

| renderer | startup ms (window / first frame) | idle RSS MiB | idle PSS MiB | anon / file / shmem MiB | threads | idle CPU % | peak RSS MiB | scroll CPU % | scroll fps (median / min) | RSS after scroll MiB |
|---|---|---|---|---|---|---|---|---|---|---|
| software | 33.8 / 36.8\* | 41.7 | 22.9 | 4.2 / 21.9 / 15.6 | 5 | 0.0 | 41.7 | 15.6 | 30 / 30 | 42.5 |
| femtovg | 35.1 / 115.5 | 91.4 | 36.2 | 19.6 / 71.8 / 0.0 | 9 | 0.0 | 91.4 | 9.4 | 29 / 29 | 92.3 |
| skia | 33.7 / 36.4\* | 89.2 | 45.9 | 17.8 / 71.3 / 0.1 | 9 | 0.0 | 89.2 | 12.4 | 30 / 30 | 89.5 |

Slint backends in use: software; FemtoVG with OpenGL; Skia with wgpu
(32 bpp surface).

\* No rendering notifier (software, and Skia on wgpu): time until the event
loop runs, not until the first frame.

Manual check: scrolling with the mouse wheel and the scrollbar is smooth
with all three renderers. Skia felt the fastest, but the differences are
small.

### Reading the numbers

- **Idle:** no renderer uses measurable CPU when idle, and the window is
  up in about 35 ms. FemtoVG needs about 80 ms more for its first frame,
  presumably for setting up OpenGL.
- **Memory:** software is the smallest by every measure. The GPU
  renderers use about 70 MiB of file-backed pages more (Mesa's GL/Vulkan
  drivers). Those are shared with other GL/Vulkan processes and are clean
  pages, which is why PSS is a fairer comparison: 22.9 MiB for software
  against 36.2 (FemtoVG) and 45.9 (Skia). Private heap (anon) is 4.2 MiB
  against 18–20 MiB. The software renderer's 15.6 MiB shmem are its shared
  frame buffers with the X server; they grow with the window size.
  Scrolling through all 10,000 items adds less than 1 MiB for every
  renderer, so the lazily built list model does its job.
- **Scrolling:** every renderer ran at 30 fps during the autoscroll. Since
  the frame rate is the same for all of them, it most likely comes from the
  way the benchmark paces updates (a 16 ms timer against 60 Hz frame pacing
  on the composited X11 session), not from what the renderers can do; the
  same benchmark under Xvfb ran at 60 fps. So the CPU numbers can be
  compared at the same frame rate: FemtoVG 9.4 %, Skia 12.4 %, software
  15.6 % of one core. This matches the manual check that all three scroll
  smoothly.

## Decision

- **Slint: yes.** It meets the goals of the phase: no idle CPU, fast
  startup, a smooth virtualised list with 10,000 items, and with the software
  renderer 42 MiB RSS for the whole window, leaving room under the 100 MiB
  goal for the article view.
- **Default renderer: software**, for now. Low memory is a primary
  requirement, and software uses half the RSS of the GPU renderers and about
  4× less private memory, with no GPU driver loaded at all. The price is
  about 3–6 percentage points more CPU of one core, only while scrolling, and no visible
  difference in smoothness.
- The choice is revisited in **S4**: Servo's zero-copy frame path needs a
  wgpu-based renderer, and Skia already ran on wgpu here. S4 compares
  Skia/wgpu with zero-copy Servo frames against software with CPU readback;
  the plan's option of making the renderer a setting stays open. Blitz (S3)
  paints on the CPU into a Slint image and works with any renderer.
  *S4 result:* software stays. Servo's frames are read back to the CPU;
  the zero-copy path (tried with femtovg on wgpu) cost 60 MiB more for the
  whole session and wasn't faster ([0005](0005-web-tabs.md)).

## Consequences

- The software renderer is the default and the only renderer compiled in
  by default (`renderer-software`). `renderer-femtovg` and `renderer-skia`
  are opt-in features; `scripts/measure.sh` enables them itself.
  `--renderer` or `$SLINT_BACKEND` picks one at runtime.
- zbus with `async-io` comes with Slint's winit backend on Linux and stays.
- Open issues:
  - Make `--autoscroll` driven by frames instead of a 16 ms timer, so the
    frame rate numbers show what each renderer can do. Needed before the
    M10 re-measurements, not for this decision.
  - Only measured on X11. Wayland (KWin) should be checked, at the latest
    in M10.
  - Window size and HiDPI scaling increase the software renderer's buffers
    and its CPU use while scrolling; re-measure with a maximised window on a
    HiDPI screen in M10.
  - Slint's `accessibility` feature (AccessKit) is disabled so far. Measure
    its cost and turn it on by M8.
  - First-frame time is not available for the software renderer or for Skia
    on wgpu (no rendering notifier).
