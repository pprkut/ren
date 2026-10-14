<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0016 — Performance (M10)

**Status:** for review (2026-10-14). Checked and measured in the
container; measured on the desktop by the user at bd641b9 (`just
measure-all` and Ars Technica with WebRender's profiler, see "On the
desktop" and "Ars Technica").

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

On the desktop (the user, at bd641b9, the same car article as in 0011
and 0012), the profiler showed a cheap page: 76 visible primitives, 51
draw calls, 2 colour passes, no alpha passes, no blob rasterisation;
WebRender's GPU time 5.8 ms per frame on average, at most 22 ms, and its
CPU time 10.5 ms (spikes up to 11 ms in the texture cache update and
the renderer, as images arrive); 52 frames per second while scrolling.
Its GPU memory: 121 MB (at most 139), of which 40 MB standalone
textures, 34 MB the texture atlas, 25 MB picture tiles and 17 MB render
targets; on Iris Xe that is system memory. `just measure-tabs` with
the article as the only page:

| run | 1 tab (UI + helper) | 3 tabs | frames in 3 s | CPU % window / helper | frame to UI ms mean / max | requests blocked / checked |
|---|---|---|---|---|---|---|
| 15:46, blocking | 325.1 (64.5 + 260.7) | 501.3 | 172 | 36 / 33 | 8.5 / 51.3 | 30 / 279 |
| 15:46, blocking, one frame under way | 331.1 (64.8 + 266.3) | 505.6 | 151 | 39 / 37 | 15.9 / 94.7 | 30 / 279 |
| 15:49, blocking | 325.7 (64.3 + 261.4) | 501.9 | 53 | 19 / 39 | 47.3 / 144.0 | 39 / 371 |
| 15:49, without blocking | 417.1 (64.9 + 352.1) | 735.2 | 88 | 27 / 43 | 37.1 / 129.9 | - |
| 0012, blocking | 324.7 (62.1 + 262.6) | 500.9 | 48 | 16 / 32 | 58.3 / 156.1 | 38 / 370 |

**The page loads differently from one visit to the next,** and that
decides whether it scrolls: loads with about 280 requests scroll at 57
frames per second, loads with about 370 (as in 0012) at 16–18, with or
without content blocking, the GPU then taking 35–50 ms per frame. So
what is slow is something the page only loads on some visits (an ad
rotation, a video, an embed), not the page as such, not the blocking,
and not the frame path. The profiler capture above most likely came from
a fast load: WebRender's own work in it is far below what the slow
loads cost. Between the two fast runs the new pacing shows: 14 % more
frames, and frames reach the window in half the time.

The profiler on a slow load (the user, reloading until the frame rate
dropped):

| WebRender | fast load | slow load |
|---|---|---|
| frames per second | 52 | 36 |
| CPU per frame, recent mean / max | 10.5 / 25.9 ms | 77.1 / 215.7 ms |
| renderer CPU, mean / max | 1.3 / 11.6 ms | 12.2 / 175.4 ms |
| GPU per frame (graph), mean / max | 5.8 / 22.0 ms | 7.5 / 60.0 ms |
| visible primitives, draw calls | 76, 51 | 142, 48 |
| standalone textures, MB (max) | 40.1 (57.5) | 48.3 (68.7) |
| GPU memory, MB (max) | 121.1 (139.1) | 131.1 (150.5) |
| image batches in the GPU timeline | none | several |

The slow load draws large images that the fast one doesn't (image
batches, 8–11 MB more of standalone textures), and the time goes mostly
to WebRender's renderer on the CPU (up to 175 ms in a frame) rather than
to the GPU (up to 60 ms): uploading large textures, or the driver
waiting for the GPU, in WebRender with Mesa. An animated image (an ad or
a teaser) uploaded again with each frame would fit. It is not in ren's
frame path: the readback waits for it, the window's part stays at 6–11
ms per frame. Which resource it is is still to be found (see "Open");
then it is a report for Servo, with the page and the profiler's numbers.

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
(idle and the item list per renderer, with the dump's items),
`measure-articles.sh` (100 articles of the dump), `measure-tabs.sh` (the
pages: blocking, without, one frame under way, FemtoVG, Skia, and the
synthetic page per renderer), `measure-hidden.sh`, `measure-settings.sh`,
`measure-sync.sh` and `measure-window-sync.sh` (the account of the
settings file), and collects their summaries in
`target/measure/all-<timestamp>/report.md`. `SKIP` leaves measurements
out. Then, by hand: scroll Ars Technica's article in a tab with
`REN_SERVO_WR_PROFILER=1 target/release/ren` and note what the profiler
shows, and the same page in servoshell 0.7.

### The run

By the user, at bd641b9, on the machine of the earlier measurements
(i7-1185G7, Iris Xe, Mesa, Linux 7.2.8, KDE on X11), release builds;
`~/ren-dump` (at most 10,000 of its items), heise's and Ars Technica's
articles as the pages (the tabs scroll the first, heise). RSS in MiB.

**The window, idle and scrolling the item list:**

| renderer | idle RSS / PSS | anon / file / shmem | idle CPU % | scrolling CPU % (fps median / min) |
|---|---|---|---|---|
| software | 53.8 / 35.0 | 7.8 / 30.4 / 15.7 | 0.0 | 54.0 (63 / 53) |
| FemtoVG | 103.5 / 53.4 | 23.2 / 80.2 / 0.0 | 0.0 | 27.9 (60 / 25) |
| Skia | 100.4 / 61.3 | 21.4 / 78.9 / 0.1 | 0.0 | 34.6 (60 / 52) |

As in 0001: the GPU renderers halve the CPU of scrolling the list and
cost about 47–50 MiB more.

**Pages, scrolled for 3 s** (heise, with content blocking unless noted;
the synthetic page from `pattern-helper` costs the helper nothing):

| variant | before tabs | 1 tab (UI + helper) | frames | CPU % window / helper | window ms per frame | frame to UI ms mean / max |
|---|---|---|---|---|---|---|
| software | 50.4 | 491.0 (64.3 + 426.6) | 132 | 27 / 41 | 6.1 | 9.0 / 88.9 |
| without blocking | 51.0 | 623.0 (65.0 + 558.0) | 93 | 25 / 183 | 8.1 | 23.4 / 245.2 |
| one frame under way | 50.8 | 498.4 (64.8 + 433.6) | 131 | 26 / 42 | 6.0 | 8.8 / 43.1 |
| FemtoVG | 102.4 | 566.6 (116.5 + 450.1) | 93 | 42 / 48 | 13.5 | 26.3 / 235.8 |
| Skia | 100.4 | 581.2 (124.0 + 457.2) | 87 | 87 / 41 | 30.0 | 26.3 / 168.1 |
| synthetic, software | 50.8 | 53.9 | 187 | 40 / 15 | 6.4 | 1.0 / 6.3 |
| synthetic, FemtoVG | 103.5 | 106.5 | 182 | 68 / 15 | 11.2 | 1.5 / 14.8 |
| synthetic, Skia | 100.4 | 105.9 | 108 | 99 / 10 | 27.5 | 1.1 / 5.1 |

**Articles** (100 of the dump, 300 ms apart; with M6's numbers, 0010):

| variant | after the 1st | after the last | peak | first frame ms median / p90 |
|---|---|---|---|---|
| Blitz, no images | 66.4 | 72.8 (M6: 70.1) | 81.1 (78.6) | 6.3 / 10.9 |
| Blitz, images from their servers | 67.0 | 79.4 (76.4) | 101.1 (90.3) | 4.3 / 9.2 |
| Blitz, images from the cache | 67.2 | 78.9 (75.6) | 103.9 (88.2) | 4.2 / 9.7 |
| the same, glibc's dynamic mmap threshold | 67.4 | 86.9 (80.0) | 114.0 (107.4) | 4.6 / 9.8 |

67 image requests (M6: 52; M8 added the items' thumbnails and image
enclosures), 2 scaled down; the largest image decoded before scaling
took 5.9 MiB.

**Hidden in the tray** (two pages): at the start 50.3–51.0, the article
66.6–67.3, the pages opened 556–564 (UI 73.9–74.5), hidden 58.4–58.9,
shown again 555–564, the article again 562–571 (UI 80.7–81.2). In M8
(0013) the window with the article and the pages took 85.6–86.4 MiB and
hidden 61.6–62.4.

**The settings dialog:** 10.3 MiB while open, 1.4 after closing it;
About 4.3, given back; as in M8b.

**Sync:** the account has only 98 unread and starred items now, so the
syncs took 0.4–0.5 s and the window 53.7 MiB after them; a large account
is only measured with the store's synthetic one (`just measure-store`,
above).

### What it says

- **No renderer setting.** For pages the GPU renderers are worse, not
  better: the window takes 11–14 ms (FemtoVG) and 27–30 ms (Skia) per
  frame against 6 with the software renderer, real page or synthetic,
  and scrolls fewer frames (Skia saturates a core), while costing 50 MiB
  more for the whole session. They upload every frame as a new texture,
  which is likely what costs them; the software renderer only copies it.
  For the item list they save CPU, as in 0001, but not enough to pay for
  the memory. The software renderer stays the only one.
- **No row-by-row PNG decoding.** The largest image decoded at full size
  before scaling took 5.9 MiB over 100 real articles; the 29 MiB case
  of M6's synthetic dump doesn't occur.
- **The frame pacing helps where frames are expensive:** on Ars
  Technica 172 frames in 3 s against 151 and half the time to the
  window (8.5 against 15.9 ms); on heise nothing (132 against 131),
  whose frames reach the window in 9 ms with either.
- **Ars Technica is slow only on some loads** (57 frames per second
  with ~280 requests, 16–18 with ~370, as in 0012); see "Ars Technica"
  above.
- **The window with pages gives back more:** 74 MiB with the article and
  two pages against 86 in M8: the article's frames and renderer go while
  a page is shown.
- **The article peak is above 100 MiB** with images (101–104 against
  88–90 in M6), while the memory after the articles is as in M6 (+3).
  More images per article (thumbnails, enclosures) are the likely cause,
  not confirmed; see "Open".

## Open

- **Which image Ars Technica's slow loads add:** the profiler shows
  large images and the renderer stalling on them; a list of the hosts
  (or resource types) a load requests, compared between a fast and a
  slow load, would name it, for a report to Servo. The article's images
  still look corrupted in the tab (as in 0011), so the slow loads may be
  a symptom of the same fault in Servo's image handling; servoshell 0.7
  on the same page tells whether both are Servo's.
- **WebRender's GPU memory** is a large part of a heavy page's cost
  (121 MB on Ars Technica, in system memory on Iris Xe): its texture
  cache and picture tiles could be bounded for background tabs or in
  general. A lead for a later milestone, not measured further here.
- **The article peak above 100 MiB** with images: find which article
  and image make it (the run logs only every 100 articles), and whether
  thumbnails decoded at full size are the cause.
- Slint's fast path for unscaled opaque images (above): upstream, a
  patch in ren, or neither.
- From 0001 and 0002: `--autoscroll` still scrolls by a 16 ms timer, so
  its frame rates are capped; Slint's `refresh_full_speed` mode didn't
  raise them under Xvfb. The renderers are compared by their CPU at the
  same frame rate, as in 0001. Wayland and a maximised window on a HiDPI
  screen are still only measurable on the desktop.
- From 0009: startup reads all ids of the list before the window shows
  (20–30 ms at 63,000 items); left as it is.
