<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0004 — Article view: Blitz (spike S3)

**Status:** accepted (2026-10-03).

## Context

`docs/PLAN.md` renders article bodies with Blitz (Stylo for CSS, Taffy and
Parley for layout, painted on the CPU with vello_cpu) into the article pane
of the Slint window, without JavaScript or Blitz's own networking. S3 checks
this on real items from the S2 dump: scrolling, link clicks, text selection
with copy, cursor changes, images and a light/dark stylesheet, and records
memory, render time and CSS coverage.
*Decides:* Blitz vs. litehtml vs. native rich text.

## What was tried

### Blitz version

The latest stable release, 0.2.4 (October 2025), selects and copies text
only inside text inputs, not in the document. Document-wide text selection
(`set_text_selection`, `get_selected_text`, Ctrl+C through the shell's
clipboard hook) is in **0.3.0-beta.2** (August 2026), a pre-release on
crates.io. The spike uses that, pinned with `=`. Blitz features: `floats`,
`svg`, `system-fonts`, `complex-scripts`; not `accessibility`, `woff` or
custom widgets.

### Embedding (`article/html.rs`, `ui/article.rs`)

- **Rendering:** the article is an `HtmlDocument` with the pane's size in
  physical pixels as its viewport. Each frame is resolved (style, layout)
  and painted by `blitz_paint` into a `VelloCpuImageRenderer`, straight into
  one of two `SharedPixelBuffer`s, and shown as a Slint `Image` (premultiplied
  RGBA). The buffers alternate, so painting never copies the frame Slint is
  showing. Frames are only rendered when Blitz asks for a redraw (input,
  hover, a loaded image, a resize), never continuously.
- **Input:** a `TouchArea` over the image forwards pointer down/up/move and
  wheel events, a `FocusScope` keys. Arrows, Page Up/Down, Space and
  Home/End scroll; Ctrl+C goes to Blitz, which copies the selection. A
  context menu offers Copy.
- **Services Blitz asks the embedder for:**
  - *Shell:* redraw requests (from any thread; they wake Slint's event loop
    with `invoke_from_event_loop`) and the clipboard, implemented with
    `arboard`, which Slint's winit backend already brings.
  - *Navigation:* link clicks are collected and, for the spike, logged and
    shown in the status bar. Relative links resolve against the item URL.
  - *Net:* images are fetched by two worker threads with ureq (system trust
    store, 20 MiB limit, 60 s timeout); Blitz decodes them on the same
    threads. `data:` URLs are decoded locally. Requests of articles no longer
    shown are dropped. `--no-images` turns off remote fetching.
- **Cursor:** after each event `get_cursor()` decides between default,
  pointer (links) and text.
- **Cost control:** the Blitz view is created with the first article, not at
  startup. One font collection is shared by all documents (Blitz would scan
  the system fonts again for every document otherwise). Styling runs
  sequentially (no rayon pool). Animations are not driven: a page with an
  endless CSS animation would otherwise render at 60 fps.
- **Article document** (`article/document.rs`): a small HTML page around the
  body with a header (title linking to the item URL, feed, author, date) and
  a stylesheet in the UI palette's colours (background, text, accent for
  links), readable line length and line height, images limited to the pane
  width. Title, author and URL are not sanitised by the News app and are
  escaped. `<iframe>`s are replaced by links to their source: embeds need
  JavaScript, and loading them would cost memory and tell the embedding site
  what is being read.

### Plain text

Without the `html-view` feature (or with `--plain-text`) the body is
converted to text by a small tag stripper (`article/text.rs`: block
elements to paragraphs, list bullets, entities, `<pre>` kept, alt text of
images) and shown in the Slint pane.

### Real items

`--dump <dir>` shows the items of an S2 dump instead of dummy data: folders,
feeds and the list fields are loaded (at most `--items`), bodies are read
from the dump files when an article is opened, so memory use is close to
the store-backed app.

### Measurement support

- `--cycle-articles N` opens the first N articles one after another (300 ms
  apart), logs the time to each article's first frame (parse, style and
  layout, paint) and RSS (anon/file/shmem) before, after the first and after
  the last article, then quits.
- `scripts/measure-articles.sh` (`just measure-articles`) runs that for a
  build without `html-view`, plain text, Blitz without and with images, and
  prints a table.
- `scripts/survey-articles.py` (`just survey-articles`) counts the elements,
  attributes, inline CSS properties and media in the dumped bodies. Its
  output has no content, titles or URLs.

## Findings that don't need real items

Checked with a synthetic dump (headings, lists, a table, `<pre>`, a
blockquote, a floated `data:` image, a figure, an iframe, right-to-left
Arabic, emoji) under Xvfb:

- Selection by dragging, Ctrl+C into the system clipboard, link clicks
  (absolute and relative), wheel and keyboard scrolling, floats, tables,
  images with `width`/`height`, right-to-left text all work.
- **Bug found and fixed:** `VelloCpuImageRenderer::render` doesn't reset its
  render context, so without an explicit `reset()` every frame painted all
  earlier frames again, and switching to an article with a different image
  panicked in vello_cpu ("image not found in registry").
- Blitz's overlay scrollbars (`scrollbars` feature) are not drawn for the
  document viewport, only for scroll containers, so the pane shows no
  scroll position yet.
- Memory does not grow per article: opening 300 articles in a row levels
  off after about 100. What it levels off at is to be measured on real items.
- Italic text looked wrong in the container, which has no italic sans-serif
  font; on the desktop it is fine.
- **Dependencies:** about 120 crates more (Stylo, Taffy, html5ever,
  vello_cpu, …). Parley 0.11.1 and usvg 0.48.1 are the same versions Slint
  uses, so they are shared. All licenses fit the policy (Stylo is MPL-2.0).
- **Binary size** (release): 20.1 MiB without `html-view`, 31.1 MiB with it.
- A clean debug build takes about 2 minutes longer; CI's default job now
  builds without `html-view`, a separate job builds everything for pull
  requests, master and weekly.

## Measurements

On the S2 dump (62,831 articles), i7-1185G7, Linux 7.2.8, KDE on X11.

### Survey of the article HTML

`just survey-articles ~/ren-dump`; counts only.

- **Body size:** median 496 bytes, p90 8.2 KB, p99 22 KB, largest 209 KB;
  17 empty bodies.
- **Features** (share of articles containing it): images 58.4 % (56,367
  images; `width`/`height` attributes 7.4 %, `srcset` 0.7 %, WebP 0.4 %,
  SVG files 0.2 %, AVIF 13 articles, `http:` 40 articles, lazy-loaded
  images without a usable `src` none), blockquote 3.4 %, `pre` 1.9 %,
  iframe 1.3 %, table 0.6 %, figure 0.3 %, floats 0.2 % (inline style) +
  0.2 % (`align`), `picture` 0.2 %, `sup`/`sub` 0.2 % each, `kbd` 0.1 %,
  video 0.1 %, `center` 13 articles, `details` 4, audio 1, `font` 1.
- **Elements:** mostly `p`, `a`, `span`, `img`, `li`, `div`, `em`,
  `strong`, `h2`, `code`; then table parts, `ul`, `br`, `h3`, `pre`, `h4`.
- **Inline CSS** (23,643 `style` attributes): `font-size` 7,585,
  `font-weight` 7,408, `color` 3,783, `height` 1,291, `width` 1,061,
  `text-align` 1,025, margins and paddings, `clear` 579, `border` 509,
  `background-color` 466, `vertical-align` 349, `line-height` 285,
  `font-family` 284, `float` 153; presentational attributes `bgcolor` 716,
  `align` 462, `border` 133, `cellpadding`/`cellspacing`.

### Memory and render time

`just measure-articles ~/ren-dump 100` at 451cb6e (before the fixes
below): 100 articles opened 300 ms apart, RSS in MiB.

| variant | RSS before | after 1st | after last | peak | anon / file after last | first frame ms (median / p90 / max) | parse ms | style+layout ms | paint ms |
|---|---|---|---|---|---|---|---|---|---|
| build without html-view | 42.2 | 42.9 | 45.5 | 49.4 | 8.3 / 21.5 | - | - | - | - |
| plain text | 44.4 | 45.1 | 48.1 | 51.7 | 8.9 / 23.6 | - | - | - | - |
| Blitz, no images | 44.8 | 61.5 | 81.2 | 81.2 | 36.1 / 29.5 | 3.8 / 8.7 / 27.3 | 0.7 / 0.9 / 12.6 | 1.4 / 3.5 / 15.6 | 1.7 / 4.6 / 8.1 |
| Blitz, with images | 44.8 | 61.6 | 109.8 | 124.0 | 62.5 / 31.7 | 4.1 / 8.5 / 17.4 | 0.7 / 1.0 / 2.7 | 1.5 / 3.5 / 8.2 | 1.8 / 4.6 / 8.5 |

Binary size: 20.1 MiB without `html-view`, 31.0 MiB with it.

Again at 649cdba, after the fixes below (the image cache cleared on every
article switch and capped at 16 MiB):

| variant | RSS before | after 1st | after last | peak | anon / file after last | first frame ms (median / p90 / max) | parse ms | style+layout ms | paint ms |
|---|---|---|---|---|---|---|---|---|---|
| build without html-view | 42.1 | 42.8 | 45.9 | 49.5 | 8.3 / 21.9 | - | - | - | - |
| plain text | 44.3 | 45.1 | 48.0 | 51.7 | 8.9 / 23.6 | - | - | - | - |
| Blitz, no images | 44.8 | 61.4 | 81.1 | 81.1 | 36.1 / 29.5 | 3.7 / 7.7 / 28.8 | 0.7 / 0.8 / 13.5 | 1.4 / 2.8 / 8.9 | 1.6 / 4.6 / 8.3 |
| Blitz, with images | 44.9 | 61.3 | 108.4 | 122.9 | 61.2 / 31.6 | 4.0 / 7.9 / 16.4 | 0.7 / 0.8 / 2.7 | 1.5 / 2.7 / 7.4 | 1.8 / 4.6 / 8.3 |

### Manual check

Selecting text and copying it with Ctrl+C, links, the cursor, images,
keyboard and wheel scrolling work on real articles. Two problems, both
fixed afterwards (see below) and confirmed fixed: the right-click menu
didn't open, and with a high-resolution wheel (Logitech MX Master 4)
scrolling felt choppy; a fast flick didn't scroll at first and then
jumped.

### Reading the numbers

- **Speed is not an issue.** The first frame of an article takes 4 ms
  (median) and 8.5 ms (p90); the slowest of 100 took 27 ms. Parsing,
  styling and layout, and painting each take a few milliseconds.
- **Memory is.** Blitz costs nothing until the first article (the view is
  created then), and 17 MiB with it. After 100 articles RSS is 33 MiB
  above plain text without images (27 MiB of it heap, levelling off, as
  seen with synthetic articles) and 62 MiB above with images, with a peak
  of 124 MiB. That is above the goal of staying well below 100 MiB while
  reading. Images account for about half. vello_cpu's image cache (up to
  64 MiB, pruned only while painting) looked like the cause, but clearing
  and capping it changed little (108.4 instead of 109.8 MiB after 100
  articles, peak 122.9 instead of 124.0). What remains is not explained
  yet. Likely: Blitz decodes images at full resolution (a 4000×3000 photo
  is 46 MiB as RGBA), and glibc's malloc keeps freed memory of that size
  in the heap instead of returning it (its mmap threshold grows after
  large frees), so RSS stays at the high-water mark of the largest images
  seen.
- **CSS coverage is good for what feeds use.** The common elements,
  inline styles, floats with `clear`, tables and figures render; real
  articles looked right. What doesn't fit the view: `<video>`/`<audio>`
  (0.1 %; Blitz has no media playback), iframes (1.3 %; replaced by links on
  purpose), and possibly AVIF (13 articles) and `picture`/`srcset` source
  selection (unverified). The inline `color`, `background-color` and
  `bgcolor` (several thousand uses) are written for light backgrounds and
  will clash with the dark stylesheet.

### Fixes after the measurement

- The right-click menu: the `TouchArea` forwarding input to Blitz took the
  right click; it now opens the menu itself.
- Scrolling: every input event rendered a frame at once, so a burst of
  high-resolution wheel events queued up behind the frames. Events are
  still applied at once, but the frame is rendered after the queued
  events, once per event-loop pass.
- Images: vello_cpu's image cache is cleared when another article is shown
  and capped at 16 MiB. It saved only about 1 MiB (see the second table).

## Decision

- **Blitz: yes**, behind `html-view`, on by default. It renders real
  articles correctly and quickly and does everything S3 asked for: text
  selection with copy, link clicks, cursors, images, light and dark. The
  alternatives don't do better where it matters: litehtml leaves selection,
  painting and image handling to the embedder (and brings a C++ build),
  and Slint's own text can't show tables, floats or inline images.
- **Version:** 0.3.0-beta.2, pinned, because the stable 0.2 can't select
  document text. Move to 0.3.0 when it is released.
- **Memory is the condition:** M6 has to bring reading many articles with
  images under the 100 MiB goal before the view is considered done (see
  the consequences).

## Consequences

For M6:

- **Images at display size:** decode (or downscale after decoding) to at
  most the pane width × scale factor, instead of keeping full-resolution
  RGBA; together with the on-disk image cache from the plan.
- **Find where the memory goes**, with a heap profiler (e.g. heaptrack):
  the heap growth without images (27 MiB after 100 articles, levelling off;
  Stylo, Parley and vello_cpu caches are the suspects) and the 28 MiB more
  with images. First check how much is freed memory glibc keeps: run
  `just measure-articles` with
  `GLIBC_TUNABLES=glibc.malloc.mmap_threshold=131072` (large blocks always
  from mmap, returned when freed), and try `malloc_trim` after an article
  switch.
- **Re-measure** with `just measure-articles` after each of these.
- **Dark mode:** neutralise author colours (`color`, `background-color`,
  `bgcolor`) in dark mode, or invert only where contrast is too low.
- **Scroll position:** Blitz draws no scrollbar for the viewport; add an
  indicator (a Slint scrollbar driven by the document's scroll position
  and height).
- **Scrolling feel:** check the once-per-pass rendering with a
  high-resolution wheel; consider smooth (animated) scrolling for the
  keyboard.
- **Media:** `<video>`/`<audio>` as a poster image or link that opens the
  full page (S4); check AVIF, `srcset` and `<picture>`.
- **Privacy:** loading remote images tells the image hosts what is being
  read; a setting to load them only on request (akregator has the same).
- Animations stay off; select all (Ctrl+A) is not supported by Blitz's
  selection API in a usable form yet.
- **S4:** check whether Blitz's Stylo and Servo's can be unified.
