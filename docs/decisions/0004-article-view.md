<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0004 — Article view: Blitz (spike S3)

**Status:** draft; waiting for measurements and checks on real items.

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
  font; to check on the desktop.
- **Dependencies:** about 120 crates more (Stylo, Taffy, html5ever,
  vello_cpu, …). Parley 0.11.1 and usvg 0.48.1 are the same versions Slint
  uses, so they are shared. All licenses fit the policy (Stylo is MPL-2.0).
- **Binary size** (release): 20.1 MiB without `html-view`, 31.1 MiB with it.
- A clean debug build takes about 2 minutes longer; CI's default job now
  builds without `html-view`, a separate job builds everything for pull
  requests, master and weekly.

## Measurements

To be run with the S2 dump:

```sh
just survey-articles ~/ren-dump
just measure-articles ~/ren-dump 100
```

And by hand (`ren --dump ~/ren-dump`, also with `--color-scheme dark`):
selecting and copying text (Ctrl+C and the context menu), clicking links,
the cursor over links and text, images, italics, scrolling with wheel and
keys, and how a varied set of real articles looks compared to the web view.

*Results: pending.*

## Decision

*Pending.*

## Consequences

*Pending.*
