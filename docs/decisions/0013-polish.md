<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0013 — Polish (M8)

**Status:** done (2026-10-11), for review; measured and checked on the
desktop on 2026-10-11, which found two bugs, fixed (see "On the
desktop").

## Context

M5–M7b made the window work on real data; M8 collects what using it
showed missing: keyboard navigation, mark all as read, periodic sync,
favicons, the window's state between runs (pane sizes, columns, sort
order, window size and position), a system tray icon, the open points
of the tabs (keys while a page has the focus, drop-downs and context
menus), of the item list and of the article view (`0009`, `0010`,
`0011`), and enclosures and thumbnails.

## What was built

### The window's state

`$XDG_STATE_HOME/ren/state.toml` (M4's `State`, `0008`), now with the
window's size, position and maximised state. The window keeps, in
`window_state.rs` (tested without Slint):

- the list selected, its order and the collapsed folders (the reader's
  `View`), and the item shown, which is shown again at start without
  marking it read and without loading the page of a full-page feed
  (that would start Servo at start);
- the tree's width, the list's width beside or height above the article,
  and the table's column widths, per arrangement (switching the
  arrangement in the View menu keeps each one's sizes);
- the window's size in logical pixels (so a change of scale factor
  keeps it), its position, and whether it is maximised. A maximised
  window keeps the size it had before. On Wayland, where applications
  can't place their windows, winit returns no position, and none is
  kept.

**Written when the window closes,** not on every change: dragging a
splitter would write the file at every step, and nothing else writes it.
A crash loses the run's changes.

**Only with the default database:** the ids are that database's, and
the measurement scripts, which use copies (`--database`) or demo data,
start alike.

**Maximised, and window managers:** winit asks to maximise a window
right after mapping it, before the window manager manages it, and KWin
and Openbox drop the request, so the window came back unmaximised. Once
the window exists, ren moves it to its saved position again and then
asks to maximise it. The size and position before maximising are kept
a moment (300 ms, once the window manager updated the maximised state)
after every move or resize while the window isn't maximised; at first
they were kept only when the window closed, so a window closed
maximised had none. A window closed maximised on another screen keeps
its normal size and position moved onto that screen, so it comes back
there. Checked under Xvfb with Openbox (one screen: Xvfb's RandR
monitors aren't seen by winit).

**Off the screen:** Slint can't list the screens. Its
`unstable-winit-030` feature gives the winit window (winit 0.30 is
Slint's backend already, no new crate): once it exists, ren compares
its rectangle with the monitors and, if less than 100×50 pixels of it
are on one (a monitor that is gone), centres it on the primary monitor,
shrunk to fit. Tested by hand under Xvfb with a position of 3000 on a
1920 pixels wide screen.

The arrangement itself isn't kept: it becomes a setting in M8b.

### Keys and actions

- **j/k, s, m, o:** next and previous article, star, read or unread,
  open the link (in a tab; without Servo in the browser). A focus scope
  around the window's content that never takes the focus
  (`enabled: false`) hears the keys the focused element rejects, so they
  work from the tree, the list and the article, and not in the search
  field or pages, which take all keys.
- **Feed → Mark All Feeds as Read** (Ctrl+Shift+R), next to Mark Feed as
  Read.
- **Item list:** a right click selects the row (and shows its article,
  as in akregator) before the menu opens; **Copy Link Address** works;
  dates are in local time (`localtime_r` through `libc`, already in the
  build; TZ, else `/etc/localtime`).
- **The clipboard** is Slint's (the platform's), reached through a hidden
  `TextInput`'s `copy()`: Slint has no other way to it. Until now only
  the HTML view had a clipboard, and only after the first article.

### Periodic sync

A sync 3 s after the start, then one `sync.interval-minutes` (15) after
the end of the last one, manual ones included; 0 turns both off.
Automatic syncs are skipped quietly while one runs or without an
account, never with demo data, and not while measuring. A changed
interval applies when the settings are read again.

**Purge settings** were done in M4/M5: `sync.keep-read-days` (30) is
read by every sync. Nothing was added.

### Favicons

After a sync, the icons the News app found on the feeds' sites
(`faviconLink`) that are missing or a month old are downloaded on a
thread of low priority (`nice` 10, as the sync), decoded (ICO, PNG,
JPEG, with the `image` crate, which Slint brings already; ICO is a
feature of it), scaled down to 32 pixels and kept as PNG in
`$XDG_CACHE_HOME/ren/favicons`, named by a hash of the link; the tree
loads them from there. A failed download is marked and tried again a
day later. Downloads are limited to 512 KiB and images to 1024 pixels a
side, checked from the header. `sync.feed-icons` (on) turns it off: the
icons come from the feeds' sites, which learn that the reader follows
them. Checked under Xvfb against a mock server with an ICO, a PNG, a
missing icon and a feed without one.

### Web pages in tabs

**Keys (open point of `0011`).** With a page focused, keys with
modifiers went to the page and to the window's shortcuts. Now the
window keeps its menu's shortcuts (Ctrl+W, Ctrl+Q, Ctrl+F, Ctrl+L,
Ctrl+U, Ctrl+Shift+A/R/N/Up/Down, Alt+Left/Right) and the page gets all
others, editing keys (Ctrl+C, Ctrl+A, Ctrl+Z, …) and Ctrl+Return (forms)
included. F5 and Ctrl+R reload the page, as in a browser, instead of
fetching the feeds and marking the feed read.

**Drop-downs and context menus.** Servo 0.7 asks the embedder to show a
`<select>`'s options and a page's context menu
(`WebViewDelegate::show_embedder_control`, `EmbedderControl::SelectElement`
and `ContextMenu`). The helper passes them to the window
(`Event::Select`, `Event::ContextMenu`: the entries, where, and the
link and image under the pointer) and applies the answer
(`Command::Select` with option ids, as servoshell does;
`Command::ContextMenu`). The window shows:

- the context menu: open the link or image in a new tab, the link in
  the browser, copy their addresses, cut, copy, paste, select all, back,
  forward, reload (enabled as Servo says), and the page in the browser.
  Links and images are opened and copied by the window, not Servo: its
  own tabs, and a clipboard that outlives the helper. Servo only
  dismisses its menu for those;
- a drop-down's options in a popup below the element (above it without
  room), a list that scrolls, with group headings and disabled options.
  Servo 0.7 reports a `<select>`'s box in CSS pixels relative to the
  document, not in device pixels relative to the view (a FIXME in its
  script crate), so the popup first opened elsewhere on a scrolled page.
  The box is scaled, and moved to the click that opened the drop-down
  where it doesn't contain it. Context menus get the click's point.

Controls of hidden tabs are dismissed, and a new one replaces the tab's
older ones: Slint doesn't tell when a menu was closed without a choice.
Labels (200 characters) and entries (2,000) are limited on both sides.
Colour and file pickers, input methods and dialogs are still dismissed
(M12). The window's side was checked under Xvfb with a stand-in for the
helper (a script speaking `ren-tabs`' protocol, not in the repository);
the helper's side compiles in CI's Servo job but hasn't run with Servo
yet.

### The article view

- **Select all** (Ctrl+A, the article's menu): from the start of the
  first block of text to the end of the last, found through the layout's
  children, which include Blitz's anonymous blocks (Blitz selects all
  only in text inputs).
- **Smooth scrolling by keyboard:** arrows, Page Up/Down, Space, Home and
  End move a target, and the view eases towards it in 150 ms (cubic
  ease-out, a frame every 16 ms); another key moves on from the target.
  Smooth wheel scrolling stays in M10.
- **Transparent images in dark mode** get a light backdrop (the text
  colour mixed with a tenth of the background); opaque ones cover it.
- **Enclosures and thumbnails:** a podcast's episode or another file is
  linked at the top of the article ("Audio: episode.mp3 (audio/mpeg)"),
  or shown if it is an image, unless the body links to it already; the
  item's `media:thumbnail` is shown if the body has no image. The plain
  text starts with the file's link. These fields aren't sanitised by the
  News app: only web links are used, escaped.

### The system tray

Slint 1.18's `SystemTrayIcon` (its `system-tray` feature: ksni, a
StatusNotifierItem over D-Bus, on the zbus and async-io ren has already;
new crates: ksni, pastey, task-local). The icon is Lucide's feed icon on
an orange square (`icons/tray.svg`), which shows on light and dark
panels; the tooltip and title are the unread count (SNI has no badge
that Slint exposes); the menu shows or hides the window, fetches the
feeds and quits. Settings in a new category, Window:

- `window.tray-icon` (on): the icon.
- `window.close-to-tray` (off): closing the window hides it in the tray,
  and the syncs go on; the event loop runs until ren quits
  (`run_event_loop_until_quit`).

**Off by default, and checked:** Slint creates the icon even where
nothing shows it (GNOME without the AppIndicator extension, a session
without D-Bus): it prints an error and the component exists. A window
closed to such a tray couldn't come back. So ren asks the session bus
whether a StatusNotifierWatcher has a host
(`IsStatusNotifierHostRegistered`, with zbus directly) when the window
closes, and quits without one. Checked under Xvfb without D-Bus.

**A hidden window drops what only a shown one needs:** the article's
Blitz view and frame buffers (made again from the article when the
window comes back; the scroll position is lost), and the tabs, which end
Servo's helper and are opened again (and load again) when the window
comes back. The heap is trimmed a second later. `--measure-hidden`
(`just measure-hidden`) measures it.

**Licence:** ksni is under the Unlicense, which isn't on the plan's
list. It dedicates the code to the public domain, with a permissive
licence (MIT-like, no conditions) as fallback, and the FSF lists it as
compatible with the GPL. `deny.toml` has an exception for this crate
only, approved in the M8 review like `webpki-roots`' in S4.

## Testing

- `window_state.rs`: the view's round trip through the state, the empty
  state, saving only with a file and changes, screens (left alone, gone,
  barely visible, too large, none known).
- `reader.rs`: restoring a view (also of a feed that is gone), showing
  without marking read, read toggle, favicon links.
- `favicons.rs`: converting PNG and a 32-bit ICO, large ones scaled, too
  large and non-images refused; which icons are due (missing, a month
  old, failed a day ago, only web links, each once).
- `item_list.rs`: dates in UTC and local time (any TZ).
- `tabs/controls.rs` (window) and `ren-servo/src/controls.rs` (helper):
  drop-down rows, choosing one and several options, limits; menu
  entries, what each choice does, Servo's actions.
- `article`: enclosure kinds and names, web links only, the attachments
  in the document (escaped, not twice, an image enclosure instead of a
  thumbnail), the dark backdrop; select all with Blitz; the glide's
  easing.
- `ren-settings`: the new settings, the window's geometry in the state
  file; a test of the filter lists that raced the file's time (CI on
  7b440e4) no longer does.
- By hand in the container under Xvfb: the state written and restored
  (selection, collapsed folder, splitter, size, position), an off-screen
  position centred, j/s/m, right click and Copy Link Address (read back
  with `xclip`), local dates with `TZ`, the sync at start and a minute
  later against a mock server, favicons from it, the page's context menu
  and drop-down with the stand-in helper (answers and clipboard read
  back), hiding and showing the window, closing without a tray host.

## On the desktop

By the user, at d4e1d5d (KDE on X11), release build with the default
features. `just measure-hidden` with a dump and two pages in tabs:

| run | at the start | article and pages shown | hidden | shown again |
|---|---|---|---|---|
| 1 | 49.1 | 346.2 (85.6 + 260.6) | 61.6 | 349.4 (86.0 + 263.4) |
| 2 | 50.3 | 352.7 (86.4 + 266.3) | 62.4 | 350.3 (86.6 + 263.7) |

RSS in MiB, total (UI + Servo's helper). Hidden in the tray, ren gives
back 285–290 MiB: the helper ends with the tabs, and the window drops
the article's view and frames. It stays 12 MiB above the start (what
the window keeps of the article, the reader and Slint's caches, not
broken down), and comes back to the same size when shown again.

By hand:

- Closing to the tray on Plasma works; the licence of ksni is approved.
- A drop-down on a real page (pecl.php.net) works, but opened in the
  wrong place: the box Servo reports (above), fixed in 42f3394. No page
  with `<optgroup>`s was at hand.
- The window's size, position and maximised state weren't restored:
  closed maximised, it came back unmaximised; moved to another screen,
  it came back on the primary one (the maximised state, and the missing
  normal size, above), fixed in e399b7b. To check again on KWin.
- Startup with a restored article costs what opening an article costs
  (`0010`: 16 MiB for the view); not measured on its own.

## Open

- **The window's geometry on KWin** after the fix, and with two screens:
  checked only under Openbox on one screen.
- A drop-down opened by keyboard on a scrolled page still opens where
  Servo's box says (there is no click to place it by).
- A context menu or drop-down closed without a choice isn't reported to
  Servo until the next one replaces it (Slint has no closed callback for
  menus and popups).
- A drop-down of several options (`<select multiple>`) toggles one option
  per opening.
- A refreshed favicon shows after a restart: Slint keeps images loaded
  from a path.
- The tabs load again after the window comes back from the tray, and
  the article's scroll position is lost.
- The arrangement isn't kept between runs (a setting in M8b).
