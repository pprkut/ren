<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Performance

Low memory and processor use come first (see "Design principles" in
[`ARCHITECTURE.md`](ARCHITECTURE.md)). This is how they are measured and
what Ren costs now. How each number came about, and what was tried, is in
the decision records; the latest full run is in
[0016](decisions/0016-performance.md).

## Measuring

The measurements are scripts in `scripts/`, run through `just`:

| Recipe | Measures | Needs |
|--------|----------|-------|
| `just measure` | startup, idle memory and CPU, scrolling the item list, per renderer | a display |
| `just measure-articles DUMP` | the article view over the articles of a `--dump-items` directory | a display |
| `just measure-tabs DUMP [URL…]` | memory and frames of pages in tabs | a display, the network |
| `just measure-hidden DUMP [URL…]` | memory with the window hidden in the tray | a display, the network |
| `just measure-settings` | memory with the settings dialog and About Ren open and closed | a display |
| `just measure-sync` | syncing into a new database: initial, incremental, resync | the real server |
| `just measure-window-sync` | the same in the window | a display, the real server |
| `just measure-fetch [SIZE…]` | fetching all unread items per batch size | the real server |
| `just measure-page-loads DUMP URL` | one page loaded again and again, each in a new helper | a display, the network |
| `just measure-store` | the store with a synthetic account of 63,000 items | — |
| `just measure-all DUMP URL…` | all of the above, into one report (about an hour) | all of it |
| `just measure-compare BASE HEAD SCRIPT …` | one script on two commits, in turns, compared | what the script needs |

`ren --dump-items DIR` writes the account's items into a directory for
them; dumps stay outside the repository. The results go to
`target/measure/`, as `summary.md` and as Bencher Metric Format JSON
(`results.json`).

Pages need an idle machine: under CPU load an integrated GPU can stay at
its lowest clock, which slows Servo's frames down threefold (0016, "Ars
Technica"). Measure an upgrade of Blitz or Servo on its own with `just
measure-compare`, so its effect isn't mixed with other changes.

### In CI

`performance.yml` runs `just measure-ci` for every pull request and push
to `master`: what is steady on GitHub's shared runners, under Xvfb with
the software renderer. That is the window's memory at each stage of
`measure-hidden` and `measure-settings`, the tabs' with `pattern-helper`
(a stand-in for Servo's helper), `measure-store`, and the release
binary's size. Time, CPU and frames are noise there and stay on the
desktop. [Bencher](https://bencher.dev) (project `ren`) keeps the
history and reports regressions on the pull request; thresholds are a
share above `master`'s mean: anonymous memory 2 %, RSS 5 %, size 1 %.
The desktop's runs are uploaded as a testbed of their own with `just
measure-upload` (0019, "Performance tracking").

## What it costs

On the desktop (i7-1185G7, Iris Xe, Mesa, KDE on X11), release builds,
the software renderer, from 0016's run (at bd641b9; Servo 0.7, Blitz
0.3.0-beta.3). RSS in MiB.

| | RSS |
|---|---|
| The window at idle, 10,000 items | 53.8 (PSS 35.0) |
| … with the real account, 63,500 items (0009) | 48 |
| After 100 articles, with images | 79.4 (peak 101.1) |
| After 100 articles, text only | 72.8 (peak 81.1) |
| One heavy page (heise) in a tab, window + helper | 64.3 + 426.6 |
| The article and two pages shown, the window | 74 |
| The same, hidden in the tray, all | 58.4–58.9 |
| The settings dialog open / after closing it | +10.3 / +1.4 |
| A sync of 63,000 items (0007) | peak 20–21, 18 after |

- **Idle CPU** is 0.0 %. Scrolling the item list takes 54 % of a core at
  60 frames per second; scrolling a page in a tab, 27 % for the window
  and 41 % for the helper, at about 44 frames per second on heise.
- **Content blocking** saves the most on heavy pages: on heise 132
  instead of 93 frames in 3 s and 41 instead of 183 % of a core for the
  helper, 130 MiB less (0016), for about 11 MiB once for the engine
  (0012).
- **The first frame of an article** takes 10–14 ms (median) since Blitz
  0.3.0-beta.4, 13–22 % less than before (0016).
- **Syncing** the real account of 63,000 items takes the server's 2
  minutes and 1.9 s of CPU; an incremental sync 4.6 s for three requests
  (0007).
- **The GPU renderers** halve the CPU of scrolling the item list but
  cost about 50 MiB more and take 2–5 times the window's CPU per page
  frame, so there is no renderer setting (0016).

## Open

What is known and not yet done is in GitHub issues: the article peak
above 100 MiB with images
([#71](https://github.com/pprkut/ren/issues/71)), WebRender's GPU
memory on heavy pages, bounded neither for background tabs nor in
general ([#70](https://github.com/pprkut/ren/issues/70)), a fast path in
Slint for unscaled opaque images, a quarter of the window's CPU while a
page scrolls ([#72](https://github.com/pprkut/ren/issues/72)), the two
Parley versions since Blitz 0.3.0-beta.4
([#69](https://github.com/pprkut/ren/issues/69)), and the store's purge
on every sync ([#48](https://github.com/pprkut/ren/issues/48)).
