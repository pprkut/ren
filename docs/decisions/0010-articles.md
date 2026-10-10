<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0010 — The article view (M6)

**Status:** done (2026-10-10), for review; measured on the desktop with
real articles on 2026-10-10.

## Context

S3 (`0004`) kept Blitz for the article pane, behind the `html-view`
feature, on one condition: reading many articles with images had to come
under the memory goal (well below 100 MiB), including the peak. S3 also
left the image cache from the plan, author colours in dark mode, a
scroll position indicator, media elements, a setting to load images
only on request, a pixel limit for images, a readable selection colour
and freeing the frame buffers. M5 added jump links within articles.

## What was built

### Images

- **Fetched through a cache on disk** (`article/cache.rs`):
  `$XDG_CACHE_HOME/ren/images`, one file per URL named after its hash
  (FNV-1a, which moved from the sync thread to `hash.rs`) and holding
  the URL, to tell collisions apart. At most 100 MiB; above that, files
  are removed in the order they were last used (reading one sets its
  modification time) down to 90 %. Files over a quarter of the cache are
  not stored. Written under a temporary name and renamed. Articles read
  again don't fetch their images again, and show them offline.
- **Checked before decoding** (`article/images.rs`). Blitz only takes
  bytes and decodes them itself with the `image` crate's default limits
  (512 MiB per allocation, no limit on the dimensions), at full size,
  into RGBA, which it copies once more while converting; a small file
  can decode to gigabytes. So the header is read first, and images over
  25 million pixels are refused.
- **Scaled to their size**: an image wider than the article's content
  (`CONTENT_WIDTH`, 46 em of 15 px = 690 CSS pixels, times the scale
  factor) or of more than 4 million pixels is decoded, scaled down
  (`thumbnail`, an area average) and handed to Blitz as a quickly
  encoded PNG. Its CSS size changes with its pixels, but an image that
  wide is shown at the content's width anyway (`max-width: 100%`); only
  very tall images are shown narrower. JPEGs are decoded with
  `jpeg-decoder` at 1/2, 1/4 or 1/8 of their size where that is still
  large enough, so a 12-megapixel photo never exists at full size; the
  `image` crate's JPEG decoder can't do that. The images are scaled for
  the scale factor known when the article is shown (the pane gets its
  size before the document is created).
- **One image is decoded at a time**: the two workers fetch at the same
  time, but decoding (ren's and then Blitz's, in its handler) takes a
  lock. Decoding is the peak of reading articles.
- **On request** (`articles.load-images`, a new setting in a new
  Articles category, on by default): when off, only images in `data:`
  URLs and in the cache are shown, so the image hosts don't learn which
  articles are read. A bar above the article ("Images from other
  servers were not loaded", Load Images) and Article → Load Images show
  the article again with its images. The setting applies from the next
  article; `--no-images` turns it off whatever the file says.
- **For the measurements**, `--cycle-articles` prints what happened to
  the images: fetched, from the cache, not loaded, scaled down, failed,
  and how often the heap was trimmed after loading.

### Memory

- **A fixed mmap threshold** (`procstat::fix_mmap_threshold`, 1 MiB,
  set with `mallopt` at startup). glibc raises its threshold for giving
  a block its own mapping (returned to the system when freed) up to
  32 MiB after such blocks are freed, and the threshold for giving back
  the top of a heap with it, to twice that. Freed images then stayed in
  the worker threads' heaps, whose tops not even `malloc_trim` gives
  back. Fixing the threshold turns that off. In S3 a threshold of
  128 KiB doubled the time to the first frame of articles with images;
  with images decoded at their size, the difference here is below the
  noise between runs.
  `REN_DYNAMIC_MMAP_THRESHOLD` keeps glibc's behaviour, for comparison.
- **The heap is trimmed** a second after the article changes (or none
  is shown any more), and when the workers have no requests left; the
  first alone often came before the images were decoded. A trim takes
  1–3 ms here.
- **A new renderer for each article.** vello_cpu's renderer keeps the
  outlines of the glyphs it painted (glifo's outline cache) and drops
  those unused for 64 frames, which can be many articles later; it also
  keeps converted copies of the images. heaptrack showed the outline
  cache as what Blitz's view "kept" in S3 (27 MiB after 100 real
  articles, 33 MiB with the synthetic ones below). vello_cpu has no way
  to clear it, but a new renderer starts empty; outlining an article's
  glyphs again doesn't make the first frame slower.
- **Without an article**, the two frame buffers, the renderer and its
  buffers for the pane's size are freed.

### The look

- **Dark mode:** the colours authors set are for a light background.
  In dark mode the stylesheet overrides them with `!important`, which
  beats inline styles: text in the article's colour, links in the
  accent, no backgrounds except its own for code and `mark`. Light mode
  keeps them.
- **Selected text** is drawn on the accent mixed with the background.
  Blitz paints the selection in a fixed light blue and doesn't support
  `::selection`, so a painter between Blitz and the renderer
  (`article/paint.rs`) replaces fills in exactly that colour; an
  author's background of exactly that blue would change too.
- **The scroll position**: Blitz draws no scroll bar for the viewport.
  A thin indicator over the article's right edge shows position and
  length; it widens under the pointer and can be dragged, or clicked to
  jump there.
- **`<video>` and `<audio>`**: Blitz plays no media. They become their
  poster image, if they have one, and a link "Video: play it on the
  page" to the item's page (to the file if there is no page). `<iframe>`s
  go through the same replacement.

### Jump links

The News app's sanitiser removes `id` and `name`, so links to a section
of the article (`#section`, also absolute ones to the item's URL) found
no target. Headings now get the slug of their text as their id (lower
case, letters and digits, words joined by hyphens, `-1`, `-2` for
repeats; `article/fragment.rs`), which is what most fragments are made
of (18 of 18 in the article from the plan). When Blitz finds no target
after a click, the fragment is compared as a slug too (`#Getting_Started`
finds "Getting started"), and failing that, the page opens at the
fragment, in a tab or the browser.

### Findings

- Blitz loads images only from `src`: `srcset` and `<picture>` are not
  used to choose a smaller image.
- Blitz's per-document image cache and pending requests go with the
  document; requests of articles no longer shown are dropped by the
  workers before fetching and again before decoding.
- Encoding the scaled image as RGBA (to save Blitz's conversion) made
  the peak worse (103 instead of 88 MiB); it stays RGB.
- Slint centres children that have a size but no position; the images
  bar was behind the article at first (6049994).

## Testing

- `article/images.rs`: target sizes, small images kept, PNG and JPEG
  scaled down (also through the scaled JPEG decoder), a PNG header
  claiming 6000×6000 pixels refused without decoding, SVG and empty
  input left to Blitz, broken images refused.
- `article/cache.rs`: storing and reading, replacing, a second cache on
  the same directory, a file of another URL (collision), removing the
  files used least recently, files too large to store.
- `article/fragment.rs`: slugs, ids for headings, repeats, headings
  with ids and other tags kept.
- `article/document.rs`: media to poster and link, author colours
  replaced only in dark mode.
- `article/paint.rs`: only fills in exactly Blitz's selection colour
  change.
- `article/html.rs`, with Blitz (in CI's full job): the scroll position
  and scrolling to an offset, jump links (Blitz's own, by slug, the page
  opened for a missing target, absolute links to the page), images
  waiting for a request, a 2000-pixel `data:` image shown at the
  content's width.
- `ren-settings`: the cache's path, the new setting.
- By hand under Xvfb: the images bar and Load Images, dark mode with
  author colours, `mark`, table cells and a selection, the media link,
  the scroll indicator.

## Measurements

### In the container, with synthetic articles

Not on the user's machine and not with real articles: the cloud
container (4 shared vCPUs), Xvfb, the software renderer, release builds
with debug line tables. Two synthetic dumps, not in the repository:

- **Images:** 120 articles of text with two images each, served on
  localhost: thirty 4000×3000 JPEGs, a progressive one, a 3000×2000 PNG,
  an 800×12,000 PNG, a small JPEG and an 8000×6000 JPEG (48 MP). 30
  articles, 300 ms apart, from the warm cache (M6) or the local server
  (before).
- **Text:** 400 articles without images, in Latin, Cyrillic, Greek,
  Chinese and Arabic script, with headings, lists, code, tables,
  quotes, links and inline styles. 300 articles, 300 ms apart.

Before M6 (a0170a1) and after (6049994), RSS in MiB:

| | before | after 1st | after the last | anon after the last | peak | first frame ms (median / p90) |
|---|---|---|---|---|---|---|
| images, before M6 | 30.5 | 51.7 | 191.6 | 156.1 | 717.8 | 29.3 / 34.1 |
| images, M6 | 30.9 | 44.7 | 49.5 | 13.8 | 86.0 | 13.8 / 20.7 |
| text, before M6 | 34.0 | 44.1 | 79.5 | 41.2 | 83.2 | 24.7 / 35.9 |
| text, M6 | 34.8 | 44.8 | 50.1 | 11.1 | 55.9 | 25.0 / 39.8 |

- **Images:** before M6, Blitz decoded the 48-megapixel JPEG at full
  size (the peak of 718 MiB) and kept full-size copies; now it is
  refused from its header, the others are decoded at their size, and
  the article's memory is given back. The first frame got faster
  because the workers no longer decode huge images next to it.
- **Text:** the heap grew by 33 MiB and levelled off after 200 articles
  (the glyph outlines); now it stays 3–4 MiB above the first article.
  First frames take as long as before.
- What each step did, with the image dump (RSS after 30 articles /
  peak, in MiB): glibc's dynamic threshold 96.6 / 102.4, fixed 50.1 /
  86.3; decoding in parallel 52.2 / 100.0. Without images, the threshold
  makes no difference (77.8 and 78.2 MiB after 300 articles, before the
  new renderer per article).
- **Binary size:** 35.1 MiB stripped, 0.7 MiB more than before M6.

### On the desktop, with real articles

By the user, at 15d7ef8, on the machine of S1 and S3 (i7-1185G7, Linux
7.2.8, KDE on X11), release builds: `just measure-articles ~/ren-dump
100`, the S3 dump, 100 articles 300 ms apart. RSS in MiB:

| variant | RSS before | after 1st | after last | peak | anon / file after last | first frame ms (median / p90 / max) | parse ms | style+layout ms | paint ms | heap trims ms (median / max) |
|---|---|---|---|---|---|---|---|---|---|---|
| build without html-view | 45.0 | 45.0 | 48.0 | 51.5 | 8.2 / 24.1 | - | - | - | - | - |
| plain text | 47.5 | 47.5 | 50.4 | 54.1 | 8.7 / 26.0 | - | - | - | - | - |
| Blitz, no images | 48.1 | 64.1 | 70.1 | 78.6 | 22.1 / 32.3 | 5.9 / 9.5 / 26.0 | 0.8 / 1.0 / 12.1 | 1.5 / 2.7 / 8.0 | 1.6 / 4.8 / 18.7 | 0.1 / 0.1 |
| Blitz, images from their servers | 48.1 | 64.3 | 76.4 | 90.3 | 26.4 / 34.3 | 4.0 / 8.0 / 16.5 | 0.8 / 1.0 / 1.5 | 1.4 / 2.4 / 7.5 | 1.8 / 4.7 / 8.2 | 0.1 / 0.1 |
| Blitz, images from the cache | 48.4 | 64.6 | 75.6 | 88.2 | 25.7 / 34.3 | 3.9 / 8.2 / 16.2 | 0.8 / 1.1 / 1.4 | 1.3 / 2.4 / 7.3 | 1.8 / 4.8 / 8.2 | 0.2 / 0.2 |
| Blitz, images from the cache, dynamic mmap threshold | 48.6 | 64.8 | 80.0 | 107.4 | 29.8 / 34.6 | 4.4 / 8.5 / 16.8 | 0.8 / 1.0 / 1.9 | 1.4 / 2.9 / 7.6 | 2.0 / 4.8 / 8.6 | 0.5 / 0.5 |

Images (52 requests; a few were dropped because the article had
changed before they were fetched): from their servers 49 fetched, 2
scaled down; from the cache 49 read from it, 2 fetched (dropped in the
first run), 2 scaled down. Binary size 35.3 MiB with `html-view`, 23.8
without.

- **The goal is met:** after 100 articles with images, 76.4 MiB, peak
  90.3 MiB (S3: 108.4 and 122.9). Blitz costs 26 MiB above plain text
  after them (S3: 60), 20 MiB without images (S3: 33).
- **Without images** the view's heap after 100 articles is 14 MiB
  smaller than in S3 (anon 22.1 against 36.1 MiB): the glyph outlines
  the old renderer kept.
- **Images** add 6 MiB after 100 articles and 12 MiB to the peak. Real
  images are small: only 2 of 49 were wider than the content, so here
  the savings come from the fixed threshold, the trims and the renderer,
  not from scaling. glibc's dynamic threshold costs 4.4 MiB after the
  articles and 19 MiB of peak (80.0 / 107.4 against 75.6 / 88.2).
- **Trims** take 0.1–0.2 ms on the desktop.
- **First frames** with images are as fast as in S3 (median 4.0 ms, p90
  8.0). Without images the median is 5.9 ms (S3: 3.7) while parsing,
  styling and painting take what they took in S3; in the container the
  same code takes as long without images as before M6. Not explained
  yet; it ran first of the Blitz variants, as in S3, so a second run
  will show whether it is variance.
- **Creating the view** still costs 16 MiB with the first article
  (S3: 16.6): fonts, Stylo and the view itself. That is the largest
  part left; a candidate for M10.
- **The baseline** before the articles is 3 MiB above S3's (45.0
  against 42.1 MiB without `html-view`): M5's store and sync code.
- **The image cache in this run** was `~/.cache/ren/images`, not the
  one in the output directory: the output directory was relative, and
  ren ignores relative XDG directories (fixed in 1aa4ef2). It was empty
  when the run started, so the variants measured what they should.

Checked by hand by the user on the desktop: the jump links in "GSoC
2026 Final Update - Jenkins Email Notifications using Outlook SMTP with
OAuth" work; dark mode reads fine with articles from heise, Ars
Technica, LWN, Anime News Network and the GitHub Blog; and
`articles.load-images = false` works.

## Open

- **Very large PNGs** are decoded at full size before they are scaled
  (the 800×12,000 PNG: 29 MiB for a moment); only JPEGs are decoded at a
  fraction. Decoding PNGs row by row into the scaled image would remove
  that; worth it if real articles show such peaks.
- **Very tall images** (over 4 million pixels at the content's width)
  are shown narrower than the content.
- **`srcset` and `<picture>`** aren't used to fetch a smaller image
  (0.7 % and 0.2 % of the articles in S3).
- **AVIF** (13 articles in S3) isn't decoded: the `image` crate's
  decoder needs dav1d.
- **Transparent images** with dark lines (diagrams, logos) are hard to
  see in dark mode.
- **Enclosures** (podcast episodes, `enclosureLink`) and media
  thumbnails of items aren't shown yet.
- **The cache's size** is fixed at 100 MiB; a setting if it matters.
- The Servo helper (M7) keeps glibc's dynamic threshold; decide with
  M7's measurements. *M7: fixed in the helper too, compared with the
  dynamic one by `just measure-tabs` (0011).*
- From `0004`, still open: select all (Ctrl+A), smooth scrolling for
  the keyboard.
- From `0009`: "Copy Link Address" in the item menu can now use the
  article view's clipboard (once the view exists).
