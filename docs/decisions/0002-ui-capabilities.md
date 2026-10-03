<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0002 — UI capabilities of Slint (spike S1b)

**Status:** proposed (2026-10-03), pending review of the Qt style
decision.

## Context

S1 showed that Slint meets the memory and CPU goals with a plain
three-pane window (`0001-ui-toolkit.md`). Before building on it, S1b
checks that it can also carry the desktop UI we want, with akregator as the
reference: a real tree, a table with sortable and resizable columns, a
native-looking selection, splitters, two pane arrangements, a menu bar, a
tool bar, context menus, icons, and light and dark mode. It also tries
Slint's native Qt style.

Text selection and links in the article are not part of S1b: Blitz
handles them (S3).

## What was built

Still on dummy data (`--items`), now with authors and a new / unread /
read status (the newest 2 % of the items count as "new from the latest
sync").

- **Feed tree:** "All items" is the root, as in akregator; folders and
  feeds get branch lines, chevrons and icons. Selecting a folder shows the
  items of its feeds; the chevron or a double click expands and collapses
  it. Arrow keys, Home and End navigate (Left collapses or goes to the
  parent, Right expands or goes to the first child). The view model
  (`feed_tree.rs`) computes the rows with their branch line data and
  handles navigation; `feed_tree.slint` only draws.
- **Item list:** two presentations of the same list element.
  - Above the article (akregator's arrangement), a table with Title, Feed,
    Author and Date columns, resizable widths, and sorting by clicking a
    header (again to reverse).
  - Beside the article, compact two-line rows (title; feed, author and
    date), as in S1; a table is too wide there.

  Rows alternate their background and are coloured by status like in
  akregator (new red, unread green, read in the normal text colour).
  Arrow keys, Page Up/Down, Home and End navigate. Sorting, filtering and
  the key logic live in `item_list.rs`.
- **Selection:** modelled on Breeze's item views, a tinted accent fill with
  an accent border while the view has focus, a lighter fill without.
- **Panes:** splitters between the tree, the list and the article. The two
  arrangements are switched at runtime from the View menu
  (`--arrangement` sets the initial one). Both use the same element
  instances, positioned by hand, so selection, focus and scroll positions
  carry over.
- **Menu bar** (File, Edit, View, Go, Feed, Article, Settings, Help) with
  keyboard shortcuts, check marks, a submenu and icons; a **tool bar** with
  flat icon buttons, which can be hidden from the Settings menu; **context
  menus** on feeds, folders and items; a **search field** above the item
  list (Ctrl+F) that filters by title and author; a **status bar** with the
  unread count and short messages. Actions the dummy data supports work
  (next/previous (unread) article, mark read/unread, mark a feed or folder
  as read, sort, search); the others say in the status bar that the spike
  doesn't implement them.
- **Icons** from the desktop's icon theme (KDE's from `kdeglobals`, else
  GTK's) through the `freedesktop-icons` crate, with a bundled monochrome
  fallback set from Lucide (ISC/MIT) tinted with the text colour. If the
  theme lacks any of the icons, the whole fallback set is used so styles
  never mix. `--bundled-icons` forces it.
- **Light and dark** follow the desktop; `--color-scheme` forces one (and
  the matching icon theme variant, e.g. `breeze` / `breeze-dark`).
- **Native Qt style** behind the `style-qt` cargo feature (needs the Qt 6
  development files at build time, so it is off by default and not built
  in CI). `--backend` picks the Slint backend.

All of it was checked by driving the real window (X11 input through XTest,
only while the ren window was active) and taking screenshots:

| | default style | Qt style |
|---|---|---|
| list beside, dark | `0002/beside-dark.webp` | |
| list beside, light | `0002/beside-light.webp` | |
| list above, dark | `0002/above-dark.webp` | `0002/qt-above-dark.webp` (Qt backend), `0002/qt-winit-above-dark.webp` (winit, software renderer) |
| list above, light | `0002/above-light.webp` | |
| menus | `0002/view-menu.webp`, `0002/context-menu.webp` | |
| bundled icons | `0002/bundled-icons.webp` | |

## Findings

### Worked as hoped

- Everything on the list above could be built with Slint 1.18's public
  API. The pieces it doesn't ship (tree, table with per-row styling,
  splitter, tool bar) are small custom components: the tree about 280
  lines of Slint including its context menu, the table and compact list
  about 400, the splitter 40, the tool bar and status bar 100.
- The menu bar, context menus and shortcuts are built in, render well with
  the default style, and are keyboard-navigable (opening a context menu and
  choosing an entry with the arrow keys and Return works).
- `ListView` stays virtualised with custom rows of two different heights;
  only visible rows exist, including their context menus.
- SVG icons from the Breeze theme render crisply with the software
  renderer at 2× scale.
- **The Qt style works with both backends.** With Slint's Qt backend, Qt
  draws the window, the menu bar is a native `QMenuBar` and fonts come from
  Qt's (KDE's) settings. With winit and the software renderer, Qt only
  paints the native widgets (scroll bars, line edit, table headers) into
  Slint's frame, and Slint draws the window and the menu bar. In both, the
  palette comes from Qt, so our custom widgets take Breeze's colours too.
  The result is visibly closer to akregator (always-visible Breeze scroll
  bars, Breeze colours) than the default style.

### Needed workarounds

- **No mnemonics in menus.** Slint 1.18 shows `&File` literally; there are
  no Alt+letter access keys. The menus are reachable with the mouse and the
  shortcuts only.
- **Checkable menu items toggle themselves**, so radio-style groups (the
  two arrangements) need their check marks set by hand in `activated`.
- **Drag handles that move with what they resize** (splitters, column
  resizers) must apply only the remaining offset from where they were
  grabbed; the naive "start size plus pointer delta" follows only half the
  drag.
- **No "scroll to row" API on `ListView`.** Keeping the selection in view
  is computed from the (uniform) row height.
- **A right click doesn't select the row** before its context menu opens;
  the actions still apply to the right-clicked row. Selecting it first
  would need the menu to be shown programmatically from a `TouchArea`.
- **Slint has no public clipboard API** (relevant for S3; `arboard`).
- Generated setters of Slint globals can't be stored as function pointers
  (they are not generic over the global's lifetime); closures work.

### Costs to keep in mind

- We maintain the tree, table, splitter and tool bar components ourselves.
  They are small, but accessibility (AccessKit) for them is our job too
  (see 0001: still to measure and enable by M8).
- In the debug build, selecting an article lags noticeably while the long
  plain-text body is laid out; see the measurements for the release build.
- The feed tree model is rebuilt whenever an unread count changes. That is
  fine for tens of feeds; M5 should update rows in place.
- The Qt style needs Qt 6 at build time and links QtCore, QtGui, QtWidgets
  and QtDBus.

## Measurements

Run with `scripts/measure.sh software femtovg skia qt qt-software` at
fc2e3a5 on the same machine as S1 (i7-1185G7, Mesa Iris Xe, Linux 7.2.8,
KDE on X11), 10,000 items, default arrangement (list beside the article,
compact rows); idle: 30 s after 5 s settling; startup: median of 5 runs;
CPU % is of one core. `qt` is the Qt style on Slint's Qt backend,
`qt-software` the Qt style on winit with the software renderer.

| variant | startup ms (window / first frame) | idle RSS MiB | idle PSS MiB | anon / file / shmem MiB | threads | idle CPU % | peak RSS MiB | scroll CPU % | scroll fps (median / min) | RSS after scroll MiB |
|---|---|---|---|---|---|---|---|---|---|---|
| software | 40.4 / 46.4\* | 43.1 | 24.2 | 4.5 / 23.0 / 15.6 | 5 | 0.0 | 43.1 | 48.3 | 63 / 61 | 43.6 |
| femtovg | 34.5 / 126.6 | 92.2 | 35.9 | 19.9 / 72.4 / 0.0 | 9 | 0.0 | 92.2 | 22.5 | 60 / 60 | 93.1 |
| skia | 39.6 / 44.3\* | 88.3 | 44.0 | 17.1 / 71.2 / 0.1 | 9 | 0.0 | 88.3 | 28.5 | 60 / 60 | 93.6 |
| qt | 84.9 / 90.8\* | 94.0 | 28.0 | 13.2 / 66.7 / 14.1 | 7 | 0.0 | 94.0 | 87.7 | 39 / 22 | 95.2 |
| qt-software | 101.7 / 105.6\* | 98.5 | 32.7 | 13.2 / 69.7 / 15.6 | 11 | 0.0 | 111.1 | 77.6 | 62 / 61 | 107.3 |

\* No rendering notifier: time until the event loop runs.

### Reading the numbers

- **The richer UI costs little memory.** With the default style and the
  software renderer, idle RSS went from 41.7 MiB (S1) to 43.1 MiB and PSS
  from 22.9 to 24.2 MiB: icons, menus, tool bar, the tree lines and the
  extra row content together add about 1.5 MiB. Startup is still about
  45 ms, and idle CPU is still zero for every variant.
- **Scrolling costs more per frame.** This run reached about 60 fps for all
  winit variants, where S1 measured 30 fps for all of them on the same
  machine; S1 already suspected the benchmark's pacing on the composited
  session, and the difference between the two runs isn't explained. At the
  same frame rate the comparison holds: software rendering takes
  0.77 % of a core per frame against 0.52 % in S1, so the richer rows cost
  about half again as much to draw. 48 % of a core while scrolling at
  8,000 px/s, the benchmark's deliberately extreme speed, is acceptable;
  the GPU renderers need about half of that, as in S1.
- **The Qt style is cheap in memory on KDE, expensive in CPU.** Its RSS is
  over twice the default (94–98 MiB), but most of that is the Qt
  libraries, which on a KDE desktop are shared with every other Qt
  process: PSS rises by only 3.8 MiB with the Qt backend and 8.5 MiB with
  winit. Startup roughly doubles (85–100 ms). Scrolling is where it hurts:
  with the Qt backend, scrolling reached only 39 fps (22 at worst) at 88 %
  of a core; with winit and the software renderer it kept 62 fps but at
  78 % of a core, 60 % more than the default style. The native widgets are
  painted by Qt into images every frame, which is what costs.
- In the release build selecting an article is immediate; the lag seen
  while testing was the debug build laying out the long dummy body.

## Decision

- **Slint stays.** Every part of the akregator-style UI could be built,
  with the default style and the software renderer it costs about 1.5 MiB
  more than S1's placeholder window, and none of the workarounds is a
  blocker. The missing menu mnemonics are the most visible gap; they are
  tracked as an open issue rather than a reason to pivot.
- **We maintain the tree, table (with its compact mode), splitter, tool
  bar and status bar components ourselves**, kept presentation-only on top
  of plain-Rust view models.
- **The default stays the default style with the software renderer.**
- **The Qt style remains an opt-in build option, not the default on KDE**
  (proposed; to be confirmed in review). It looks closer to akregator and
  costs little memory on KDE, but it needs Qt at build time, roughly
  doubles startup, and makes scrolling 60 % more expensive (winit) or
  noticeably less smooth (Qt backend). If it is used, it should be with
  winit and the software renderer (`qt-software`): the Qt backend scrolls
  worse and would also rule out the GPU frame path for Servo in S4.

## Consequences

- S1b is done; next is S2. M5 builds the real window from these
  components on top of the store.
- The `style-qt` feature stays but isn't built in CI (it needs Qt 6). If
  the Qt style becomes a supported option, CI gets a job that installs
  Qt 6 and builds it.
- `scripts/measure.sh` (`just measure`) measures the Qt variants too.
- New open issues:
  - Menu mnemonics (Alt+letter): watch Slint releases; until then
    shortcuts only.
  - Select the row on right click before showing its context menu.
  - Update feed tree rows in place instead of rebuilding the model on every
    unread count change (M5).
  - Accessibility of our custom components when enabling AccessKit (M8,
    with the open issue from 0001).
  - The scroll frame rate differs between S1 and S1b runs on the same
    machine; make `--autoscroll` frame-driven (already an open issue from
    0001) before relying on frame rates in M10.
  - A window with the Qt backend opens shorter than its preferred height
    (720 instead of 800 logical pixels).
