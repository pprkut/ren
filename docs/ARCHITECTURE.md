<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Architecture

How Ren is built and why. Why each choice was made, and the measurements
behind it, are in the decision records (`docs/decisions/`, listed in its
`README.md`). Details of a crate are in its documentation (`cargo doc`).

## Design principles

- **Small memory footprint.** With a few thousand items synced, the idle
  app reading feed articles should stay well below 100 MB RSS. The browser
  engine's cost is only paid while a web page is open in a tab.
- **No CPU spikes when fetching.** Syncing is incremental, bounded in size
  per request, and done off the UI thread at a low priority.
- **Native UI, no webview shell.** No Tauri or Electron, no system
  webview.
- **Offline first.** The UI always reads from a local database; only the
  sync engine uses the network.

Any new dependency is weighed against the first two: an async runtime, a
GPU stack or another heavy dependency needs a reason.

## Technology

| Concern          | Choice | Decision |
|------------------|--------|----------|
| UI toolkit       | **Slint**, with its software renderer by default; its own tree, table, splitter and tool bar components | [0001](decisions/0001-ui-toolkit.md), [0002](decisions/0002-ui-capabilities.md), [0016](decisions/0016-performance.md) |
| Article view     | **Blitz** (`blitz-dom`, `blitz-html`, `blitz-paint`), painted on the CPU (`anyrender_vello_cpu`) into a Slint image | [0004](decisions/0004-article-view.md), [0010](decisions/0010-articles.md) |
| Web pages        | **Servo**, in a helper program of its own (`ren-servo`), started with the first tab | [0005](decisions/0005-web-tabs.md), [0011](decisions/0011-tabs.md) |
| Content blocking | **adblock** (Brave's adblock-rust) in the helper: network rules of EasyList, EasyPrivacy and uBlock's lists | [0012](decisions/0012-content-blocking.md) |
| HTTP             | **ureq 3**, blocking, with rustls | [0003](decisions/0003-nextcloud-client.md) |
| JSON             | serde and serde_json, decoded while the response is received | [0003](decisions/0003-nextcloud-client.md) |
| Storage          | **SQLite** through rusqlite (`bundled`), in write-ahead log mode | [0006](decisions/0006-store.md) |
| Credentials      | Secret Service through `keyring-core` and its zbus store, or a `password-command` | [0008](decisions/0008-settings.md) |
| Settings         | TOML, edited in place with `toml_edit` | [0008](decisions/0008-settings.md), [0014](decisions/0014-settings-dialog.md) |

Rejected along the way: GPUI (Zed-driven, sparsely documented, Servo only
through a CPU readback), libcosmic (git-only, tied to COSMIC's look,
heavier through iced and wgpu), iced (higher idle memory than Slint),
litehtml (kept as the fallback for the article view, not needed), Servo
for the article view too (a whole browser for sanitised feed HTML),
WebKitGTK and system webviews (multi-process and heavy), and Slint's GPU
renderers (2–5 times the window's CPU per page frame and about 50 MiB
more, 0016).

## Crates

```
crates/
  nextcloud-news/   News API v1-3 client: types and a blocking HTTP client,
                    Login Flow v2. No UI, no database.
  ren-store/        SQLite schema, migrations, queries, the queue of local
                    changes for the server.
  ren-sync/         Sync engine: pushes local changes, pulls the server's.
  ren-settings/     Settings model and file, UI state file, app password
                    (Secret Service, password-command), file locations.
                    No UI, no server.
  ren-tabs/         Web page tabs: the protocol between the window and the
                    Servo helper (messages, shared frame buffer, smooth
                    scrolling).
  ren/              The application (Slint UI, Blitz article view, the
                    window's side of the tabs, glue).
  ren-servo/        Servo's helper program, a workspace of its own.
```

`ren-servo` has its own workspace and `Cargo.lock`: Servo and Blitz are
built on different versions of stylo, which can't be in one build
(stylo's `links` key). So the helper follows Servo's releases, ren
follows Blitz's, and `just build` builds both.

Inside `ren`, Slint code stays in `ui/` (the `.slint` files and their
Rust side), Blitz code in `article/`, the window's side of the tabs in
`tabs/`. The logic in between (`reader.rs`, `feed_tree.rs`,
`item_list.rs`, `settings_dialog.rs` and the like) is plain Rust without
Slint types.

### Cargo features of `ren`

- `html-view` (default): the article view with Blitz. Without it, a
  plain-text rendering of the body is shown.
- `servo` (default): web pages in tabs. This is only the window's side;
  Servo itself is in the helper. Without it, pages open in the system
  browser.
- `renderer-software` (default), `renderer-femtovg`, `renderer-skia`:
  Slint's renderers.
- `style-qt`: Slint's native Qt style and its Qt backend; needs the Qt 6
  development files.

Everything but the article view builds and tests without `html-view`,
and nothing in ren's workspace builds Servo.

## Data flow

The UI only reads from the store. Marking an item read or starred writes
it to the store and queues the change for the server, and the UI updates
at once. The sync runs on a thread of its own at `nice` 10: it pushes the
queue, pulls the server's changes into the store, and tells the UI which
ids changed, so the UI re-reads only what is shown. Lists are read as ids
in order, with the visible rows fetched by id, so only those rows are in
memory. The sync algorithm is in `ren-sync`'s documentation, the News
API's quirks in `nextcloud-news`'s.

## Two tiers of rendering

1. **The article view: always there, cheap.** Blitz renders the item's
   body with a small stylesheet of Ren's (light or dark like the theme),
   without JavaScript. Images are fetched by Ren, not by Blitz, into a
   disk cache, refused above 25 megapixels from their header and scaled to
   the content's width before Blitz decodes them. Each article gets a new
   renderer, and the heap is trimmed after article switches, so memory
   doesn't grow over many articles (0010). The view and its frames are
   dropped while a page is shown, and while the window is hidden in the
   tray. Link clicks open `http` and `https` links in a tab and `mailto`
   links on the desktop; other schemes are ignored.
2. **Web pages in tabs: on demand, expensive.** "Open page" (or a link in
   an article) opens a tab. The first tab starts the helper; closing the
   last one stops it, without waiting on the UI thread. The helper is a
   separate process so that its memory really goes when it exits, and so
   that Servo can be on a different stylo than Blitz. Feeds that only
   publish teasers can show their pages instead of their articles (a
   setting per feed).

### The helper

`ren-tabs`' documentation describes the protocol: one Unix socket pair as
the helper's standard input, length-prefixed JSON messages, frames in a
sealed memfd made by the window and passed with `SCM_RIGHTS`. The helper
reads each frame back from the GPU asynchronously (a pixel buffer object
and a fence), paints the next while the window copies the last, and sends
a frame only after the window took the previous one. It dies with the
window (`PR_SET_PDEATHSIG`) and has 3 s to shut down.

The window doesn't trust what the helper sends: messages and frames are
bounded. Tabs only navigate to `http` and `https`; pages may open new
tabs only within 5 s of a click or key press in them, once per gesture
and at most 10 at a time. Drop-downs, context menus, colour pickers,
`alert`/`confirm`/`prompt` and file choosers (the XDG portal) are shown by
the window. Site data is kept in a Servo profile unless
`pages.keep-site-data` is off.

Content blocking runs in the helper, in Servo's `load_web_resource`,
which every request passes through. Ren downloads the filter lists when a
tab opens and they're older than four days, resolving uBlock's
`!#include` and `!#if`; the helper builds its engine on a thread and saves
it for the next start.

## Settings, state and secrets

Three kinds of data, kept apart (`ren-settings`):

- **Settings**, curated by the user, by hand or through the settings
  dialog. The file only holds overrides; the defaults are in one table of
  setting descriptors in Rust (key, title, description, category, kind of
  control, default, validation), which also drives the dialog. A change
  edits that one value in the text with `toml_edit`, so comments and
  formatting survive; nothing serialises the whole file. The file is
  watched with inotify (editors save by renaming a new file over the old
  one) and read again 100 ms after it was saved; a file with errors keeps
  the last good settings and shows the error with line and column. The
  settings themselves are in [`settings.md`](settings.md).
- **UI state**, written by Ren alone when the window closes: the list
  shown, sort order, collapsed folders, the item shown, pane sizes and
  column widths per arrangement, the window's geometry. Never mixed into
  the settings file.
- **The app password**, never in a file: from the Secret Service, or from
  `password-command`. Without either it lasts until Ren quits.

Logging in uses Nextcloud's Login Flow v2 (the browser does the login, so
two-factor authentication and single sign-on work), or an app password
made by hand. Logging out revokes the app password on the server.

### Files

| What | Where |
|------|-------|
| Settings | `$XDG_CONFIG_HOME/ren/settings.toml` |
| UI state | `$XDG_STATE_HOME/ren/state.toml` |
| Database | `$XDG_DATA_HOME/ren/ren.db` |
| Web site data | `$XDG_DATA_HOME/ren/servo` |
| Article images | `$XDG_CACHE_HOME/ren/images` |
| Feed icons (32 px PNGs) | `$XDG_CACHE_HOME/ren/favicons` |
| Filter lists and the engine | `$XDG_CACHE_HOME/ren/filters` |
| The running instance's socket | `$XDG_RUNTIME_DIR/ren/instance` |

A second start of Ren hands over to the running one through the socket
and exits. Measurement and test modes (`--dump`, `--measure-*`, a
`--database` of their own) stay independent of it.

## Memory

glibc's mmap threshold is fixed at 1 MiB, in the window and the helper,
and the heap is trimmed (`malloc_trim`) after syncs, article switches and
loaded images: glibc otherwise keeps freed memory. What a hidden window
doesn't need goes while it's hidden (0013). What things cost is in
[`performance.md`](performance.md).

## Designing for testability

- **Logic lives in library crates; the binary is glue.** The library
  crates know nothing about the UI. Within `ren`, toolkit and engine code
  stays in its modules, so changing Slint or Blitz touches only those.
- **View models in plain Rust.** Selection, list paging, mark-read-on-open,
  unread counts and the settings dialog's rows live in structs without
  Slint types; Slint only binds properties and forwards callbacks. They
  are tested without a window, and a few UI flows run on Slint's testing
  backend (`ui/flows.rs`).
- **Seams at the boundaries, as small traits**, only where a second
  implementation exists: `NewsApi` (the client, or a fake server in the
  sync's tests), `Clock`, `SecretStore` (the Secret Service, or one in
  memory).
- **No hidden global state or I/O.** The API client takes its base URL
  and credentials as parameters, so tests point it at a mock server; the
  store opens in memory; file paths are passed in, not looked up deep in
  library code; settings are parsed from and written to strings.
- **Sync is a function; threading is the caller's job.** `ren_sync::sync`
  runs synchronously; the app runs it on a thread, tests call it.
- **Real data as fixtures**, anonymised (`just anonymise-dump`).
- **The end-to-end test** (`just test-nextcloud`) runs against Nextcloud
  with News in a container: login, syncs, changes both ways, feeds and
  folders, logging out.
- **Measurements are scripted** (`scripts/`), so numbers can be
  reproduced and compared after changes.
