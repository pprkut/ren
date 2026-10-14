<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0016 — Performance (M10)

**Status:** for review (2026-10-14). Checked and measured in the
container; the measurements on the desktop are still to be run
(`just measure-all`, see "On the desktop").

## Context

The plan's M10 asks for the measurements to be run again (idle, reading,
tabs open and closed, initial and incremental sync of a large account),
for hotspots to be profiled and fixed, and for the UI renderers to be
compared again for scrolling web pages now that M7 reworked the frame
path. Earlier decisions left these points to it: the window's CPU while a
page scrolls (0011), Ars Technica's slow frames (0012), two frames under
way in the tabs (0011), smooth wheel scrolling in the article view
(0013), the article view's last frame while a page is shown (0011), and
from the article view (0010) very large PNGs decoded at full size, very
tall images shown narrower than the content, and the 16 MiB that creating
the view costs.

## What changed

### The article's frames go while a page is shown

While a page was shown in a tab, the article view kept its two frame
buffers, its renderer (with its buffers for the view's size and its
images) and, in the Image element that wasn't drawn, the last frame:
Slint caches the value of a binding until it is read again, which for an
element's image is when it is drawn. Now the frame buffers and the
renderer go when a page is shown (`HtmlView::release_renderer`), the
Image element only exists while the article is shown (as the page's
since M7), and the article is painted again when it is shown again.

`--measure-hidden` now shows the article first and opens the pages after
it, and shows the article again at the end, so its log tells what the
article gives back while a page is shown. In the container (Xvfb, the
window at its default size, a synthetic dump without images, one page
from the stand-in helper below), the window's anonymous memory in MiB,
two runs each:

| | at the start | article shown | article and page shown | article shown again |
|---|---|---|---|---|
| before (a1becd0) | 3.6, 3.6 | - | 15.6, 15.7 | - |
| after | 3.6, 3.6 | 13.0, 13.1 | 10.0, 10.1 | 13.2, 14.2 |

With a page shown, the window keeps 5.6 MiB less (RSS 45.3 instead of
55.2 MiB); the frames grow with the window, so on the desktop it should
be more.

Tried and dropped: one frame buffer instead of two, by letting the
window drop its image before painting. The Image element's cached value
keeps a reference to the frame however the property is bound (also with
a two-way binding, `<=>`, whose linked properties cache their values
too), so every frame would have been copied once it was drawn.

### Smooth wheel scrolling in the article view

A mouse's wheel steps are spread over about 100 ms, as in the tabs: the
helper's `SmoothScroll` moved to `ren-tabs`, which the `html-view`
feature now builds too (a small crate whose dependencies are in the
build already). Each 16 ms tick scrolls 35 % of what is left; steps
below 40 pixels (touchpads) and steps with modifiers go at once, and a
wheel step stops a keyboard glide and the other way round. Wheel and
keyboard share one repeated timer: a single-shot Slint timer restarted
from its own callback doesn't fire again, so the article takes the next
part on each tick (`SmoothScroll::step`) instead of checking when it is
due. Checked under Xvfb with xdotool: the parts come 16 ms apart, each
35 % of what is left, and a step takes about 150 ms.

### The next frame of a page is painted while the window copies the last

The helper painted the next frame only after the window had answered
that it took the last one (`FrameTaken`), so the GPU's work on a frame
and the window's copy of the one before never overlapped: on heise
(10–20 ms of GPU time per frame) that capped scrolling at about 25
frames per second (0011). Now the next frame is painted and its readback
started as soon as the last is in the shared buffer; it waits for the
window's answer only before it is written into the buffer. The plan
asked for two slots in the shared buffer; that would have cost another
frame of shared memory and a change of the protocol, for no more
overlap: the helper's own copy into the buffer stays in its loop either
way. The readback's time no longer includes the wait for the window
(that is a statistic of its own, `wait`). `REN_SERVO_ONE_FRAME` paints
as before, for comparison (a variant of `just measure-tabs`).

In the container, 3 s of scrolling:

| | frames, new pacing | frames, one frame under way |
|---|---|---|
| Servo, a local page, llvmpipe | 111; 130 (`just measure-tabs`) | 101; 105 |
| synthetic page, 15 ms "GPU" per frame, window at scale 2 | 162 | 147 |
| synthetic page, 25 ms per frame, scale 2 | 106, 107 | 101, 101 |

The gain is what the window's side takes per frame (copying the frame,
and waiting for the UI thread while it draws the last one), so it is
largest when the window is busy and the GPU quick.

### A stand-in for the helper

`ren-tabs`' example `pattern-helper` speaks the helper's protocol and
paces frames the same way, but paints lines of blocks that scroll with
the wheel instead of running Servo, optionally with a simulated GPU time
per frame (`REN_PATTERN_GPU_MS`). With `REN_SERVO` pointing at it, the
window's side of the tabs can be tried without building Servo, and the
window's cost per frame measured without the page's or the GPU's. `just
measure-tabs` runs it per renderer ("synthetic page").

### Images

- **Very tall images** (more than 4 million pixels at the content's
  width) got fewer pixels than that, and Blitz shows an image at its own
  size, so they were shown narrower than the content (an 800×12,000 PNG
  at 516 instead of 690 pixels). The width they came with is now set on
  their `<img>` element when the pixel limit made them narrower (unless
  the element has a width), so the renderer scales them up to the width
  they would have had. Tested with a 700×7,000 image (shown 700 wide,
  not 632) and one with its own width attribute (kept).
- **Very large PNGs** are still decoded at full size before they are
  scaled down. Decoding them row by row into the scaled image would only
  lower the peak where such images are common: Blitz then decodes the
  scaled image again (as decoded and once more as RGBA), so for an
  800×12,000 PNG the peak would go from about 41 to 28 MB (reckoned
  from the image sizes, not measured). The image
  statistics of `--cycle-articles` now name the largest image decoded
  before scaling; the desktop run over the real articles decides.

## Where the time goes

Profiled with `perf` (release builds with line tables) under Xvfb.

### The window while a page scrolls

With the synthetic page (so the page costs nothing), the window at scale
2, 182 frames in 3 s, the window took 48 % of a core:

| | share of the window's CPU |
|---|---|
| Slint drawing the page's image, pixel by pixel (`draw_texture_line`) | 47 % |
| Slint filling the window's background under the dirty region | 15 % |
| copying the frame out of the shared buffer (ren, `Pages::pump`) | 14 % |
| the rest: presenting, Slint's work per frame, the event loop | 24 % |

Only the copy is ren's, and it is needed: Slint has no image type that
borrows memory, and it can't draw from the shared buffer. The other two
are in Slint's software renderer:

- **Drawing an image** steps through the source pixel by pixel in fixed
  point, for every image, even an opaque RGB one drawn at its own size.
  A copy of the renderer with a fast path for exactly that case (rows of
  an unscaled, opaque RGB image converted in a plain loop) took the
  window from 7.9 to 5.8 ms per frame (three runs each: 7.99, 8.11, 7.58
  against 6.00, 5.54, 5.84 ms), a quarter less. The change belongs in
  Slint (`draw_functions.rs`, 17 lines); for the review: offer it
  upstream, or carry it in ren with `[patch.crates-io]` until Slint has
  it, or leave it.
- **The background** is filled for the whole dirty region before any
  item is drawn, whatever covers it. Slint would need to know that an
  opaque item covers the region; no change in ren can skip it.

The GPU renderers draw the image as a texture instead; `just
measure-tabs` now compares them (FemtoVG, Skia) with real pages and with
the synthetic one, whose "window CPU ms per frame" column is the
window's own cost per renderer. They cost 35–60 MiB more for the whole
session (0001, 0005), which the same run shows.

### Ars Technica

Its frames take the GPU 45–74 ms (0012), against 2–3 ms for Servo's
paint; the container has no GPU to look at that. `REN_SERVO_WR_PROFILER=1`
now shows WebRender's profiler over the pages (GPU time per pass, the
number of draw calls, render targets and blur and shadow passes), which
names what on the page costs the time. To tell Servo's part from Mesa's,
servoshell 0.7 on the same page with its own profiler (Ctrl+F12) is the
comparison.

### Elsewhere

- **The item list** while it scrolls: Slint drawing text and rectangles;
  nothing of ren's above 1 %.
- **Reading articles:** after the first (about 100 ms: fonts, Stylo),
  20–25 ms each in the container; image decoding and scaling run on the
  workers.
- **The store** (`just measure-store`, 63,000 items): SQLite and the
  kernel's copies of pages; nothing of ren's. The initial sync took
  3.0–3.3 s here against 2.1 s in 0006, but M2's own code (e192f28)
  takes as long in this container: the machine, not a regression.
- **Creating the article view** (16 MiB with the first article, 0010):
  massif finds 7.6 MB of heap with an article, 3.4 of them its two frame
  buffers; the rest of the growth is file-backed (Blitz's and Stylo's
  code, the fonts: 8.6 MiB here), thread stacks and the workers' heaps.
  Nothing large to cut.

### Builds of the measurements

The measurement scripts take `OUT_DIR` from the environment and left it
there for the builds they run. `ring`'s build script runs again when
`OUT_DIR` changes, and with it `rustls` and much of Servo (cargo's
fingerprint log: `EnvVarChanged { name: "OUT_DIR", … }`), so a
measurement with an `OUT_DIR` of its own built Servo's helper again, and
so did the next build without it: 12–24 minutes each on the desktop.
The scripts no longer export it.

`measure.sh` ended its "30 s" of idle sampling at a time printed by awk
with 6 significant digits, which rounds a Unix time to 10,000 s: the
sampling stopped after one sample or ran on for up to 2.8 hours,
depending on the time of day (found when the desktop run sat in it for
more than an hour). Since S1, the idle numbers (0001, 0002) came from
windows of unknown length: a single second of RSS is still a fair
number, the idle CPU less so. Fixed; the scroll pass also stops ren
after `SCROLL_SECS` (900) now instead of waiting for it however long.

## On the desktop

Prepared as one command, to be run by the user (a display, the network
and the real server; about an hour; ren closed):

```sh
just measure-all ~/ren-dump <heise article URL> <Ars Technica article URL>
```

It runs, one after another and each with its own logs, `measure.sh`
(idle and the item list per renderer, with a copy of ren's own database),
`measure-articles.sh` (100 articles of the dump), `measure-tabs.sh` (the
pages: blocking, without, one frame under way, FemtoVG, Skia, and the
synthetic page per renderer), `measure-hidden.sh`, `measure-settings.sh`,
`measure-sync.sh` and `measure-window-sync.sh` (the account of the
settings file), and collects their summaries in
`target/measure/all-<timestamp>/report.md`. `SKIP` leaves measurements
out. Then, by hand: scroll Ars Technica's article in a tab with
`REN_SERVO_WR_PROFILER=1 target/release/ren` and note what the profiler
shows, and the same page in servoshell 0.7.

## Open

- The desktop measurements above; the conclusions on the renderers, on
  row-by-row PNG decoding and on Ars Technica depend on them.
- Slint's fast path for unscaled opaque images (above): upstream, a
  patch in ren, or neither.
- From 0001 and 0002: `--autoscroll` still scrolls by a 16 ms timer, so
  its frame rates are capped; Slint's `refresh_full_speed` mode didn't
  raise them under Xvfb. The renderers are compared by their CPU at the
  same frame rate, as in 0001. Wayland and a maximised window on a HiDPI
  screen are still only measurable on the desktop.
- From 0009: startup reads all ids of the list before the window shows
  (20–30 ms at 63,000 items); left as it is.
