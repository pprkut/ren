<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# ren — implementation plan

A native Nextcloud News reader written in Rust. It syncs feeds, folders,
items and read/starred state with the Nextcloud News app. Articles are
rendered with the lightweight Blitz HTML engine; the full web page behind an
article can be opened in an in-app tab rendered by Servo, which is only
loaded on demand (akregator-style).

## Goals

- **Small memory footprint.** The idle app, with a few thousand items synced,
  should stay well below 100 MB RSS while reading feed articles. The browser
  engine's cost is only paid while a full-page tab is open.
- **No CPU spikes when fetching.** Syncing must be incremental, bounded in
  size per request, and done off the UI thread at low priority.
- **Native UI, no webview shell.** No Tauri/Electron, no system webview.
- **Offline first.** The UI always reads from a local database; the network
  is only used by the sync engine.

## Technology choices

These are initial picks, not commitments. Phase 1 (below) validates each one
with a spike; if one fails, we pivot and record why in `docs/decisions/`.

| Concern        | Choice                               | Why |
|----------------|--------------------------------------|-----|
| UI toolkit     | **Slint** (confirmed in S1, software renderer by default: [0001](decisions/0001-ui-toolkit.md)) | Designed for low-resource targets; software, FemtoVG and Skia renderers; virtualised `ListView`; maintained Servo embedding (`examples/servo` in the Slint repo, GPU texture sharing on Linux via Vulkan external memory). Published on crates.io. |
| Article view   | **Blitz** (`blitz-dom`, `blitz-html`, `blitz-paint`) | HTML/CSS engine built on Servo's Stylo, without a JS engine or networking stack. Painted on the CPU (`anyrender_vello_cpu`) into a Slint image, so no GPU stack is needed for reading articles. Feed bodies are already sanitised and must not run JS anyway. |
| Full-page tabs | **Servo** (`servo` crate), in a helper program (`ren-servo`, [0011](decisions/0011-tabs.md)) | Opening the original page of an article in an in-app tab. Created on demand, torn down when the last tab closes. |
| Content blocking | **adblock** (Brave's adblock-rust) in the helper, with EasyList, EasyPrivacy and uBlock's lists ([0012](decisions/0012-content-blocking.md)) | Ads and trackers make heavy pages big and busy. Network rules only; lists downloaded by ren. |
| HTTP           | **ureq 3** (blocking, rustls)        | No async runtime needed for a single background sync thread; small dependency tree. |
| JSON           | serde / serde_json                   | Streaming deserialisation from the response reader. |
| Storage        | **SQLite** via rusqlite (`bundled`)  | Paged queries for the list views, so only visible rows are ever in memory. |
| Credentials    | Secret Service via `keyring` crate    | No passwords in files. A Nextcloud app password from Login Flow v2, never the account password. See "Settings, state and credentials". |
| Settings       | TOML in `$XDG_CONFIG_HOME/ren/`, edited in place with `toml_edit` | Hand-editable file as the source of truth, plus a Zed-style settings dialog on top of it. |

Alternatives considered:

- **GPUI** (Zed): very fast, but Zed-driven and sparsely documented. Servo can
  only be embedded via a CPU readback per frame today.
- **libcosmic**: git-only dependency, tied to the COSMIC desktop look, and
  built on iced/wgpu, which is heavier in memory than Slint's renderers.
- **iced**: a reasonable second choice; Servo integration works via its shader
  widget, but idle memory is higher than Slint.

- **litehtml** (C++ with Rust bindings): lightweight and mature fallback for
  the article view if Blitz stalls.
- **Servo for the article view too**: rejected; it would load a full browser
  (JS engine, networking, multi-threaded pipeline) just to show sanitised
  feed HTML.
- **WebKitGTK / system webviews**: rejected; multi-process and heavy, the
  same trade-off as Tauri.

### Two-tier rendering

1. **Article pane (always available, cheap).** The item body is rendered by
   Blitz with a small built-in stylesheet (readable typography, light/dark
   following the Slint theme). No JavaScript. Images are fetched by the app
   (not by Blitz's own net provider) through a small on-disk cache so they
   can be shown offline and loaded off the UI thread. Link clicks open the
   page in a Servo tab (any button, with or without modifiers); the link's
   context menu also offers the system browser. Without the `servo`
   feature, clicks go to the system browser.
2. **Full-page tabs (on demand, expensive).** "Open page" on an item opens
   its `url` in a new tab in the main window, rendered by Servo with
   JavaScript enabled. Rules:
   - The Servo instance is created when the first tab opens and dropped when
     the last one closes. Measure whether RSS actually drops after teardown
     (allocator fragmentation); if not, move Servo into a helper process that
     renders offscreen and shares frames with the UI process.
   - One webview per tab; background tabs are throttled/hidden.
   - Per-feed setting "open full page instead of article" for feeds that
     only publish teasers (akregator has the same option).
   - Tabs are not persisted across restarts (at most as a list of URLs).

Cargo features, so the rest of the app builds and tests without the heavy
engines (this also keeps CI and cloud builds fast):

- `html-view` (default on): Blitz article pane. Without it, a plain-text
  rendering of the body is shown.
- `servo` (default on): full-page tabs. Without it, "Open page" hands the URL
  to the system browser. Since M7 the feature is only the window's side;
  Servo itself is in the helper `ren-servo` (`crates/ren-servo`, a
  workspace of its own), which ren starts with the first tab.

Open questions for the prototypes (spikes S3 and S4):

- **Renderer interplay.** Blitz's CPU path works with any Slint renderer,
  but Servo's zero-copy path needs Slint's wgpu-based renderer, which costs
  idle memory even when no tab is open. Alternative: Servo renders into a
  software/offscreen context and frames are read back into a Slint image
  (a GPU→CPU copy per frame; acceptable for reading, less smooth scrolling).
  Measure both and pick; possibly make it a setting. *S4: readback, from a
  Servo helper process; the wgpu renderer cost 60 MiB more and wasn't
  faster (0005).*
- **Dependency overlap.** Blitz and Servo both depend on `stylo` and related
  crates. Check that the versions can be unified; two copies would inflate
  the binary and compile time (not RSS, as unused code pages are not loaded).
  *S4: unified with Servo 0.5 and Blitz 0.3.0-beta.2 (stylo 0.20); Servo
  0.6 already needs stylo 0.21, so upgrades go together (0005). M7: two
  stylo versions can't be in one build (its `links` key), so Servo moved
  into a helper program of its own and follows its own releases (0011).*

## Architecture

```
crates/
  nextcloud-news/   API v1-3 client: types + blocking HTTP client. No UI, no DB.
  ren-store/        SQLite schema, migrations, queries, pending-change queue.
  ren-sync/         Sync engine: pushes local changes, pulls remote changes.
  ren-settings/     Settings model and file, UI state file, app password
                    (Secret Service, password-command). No UI, no server.
  ren-tabs/         Web page tabs: the protocol between the window and the
                    Servo helper (messages, shared frame buffer).
  ren/              The application binary (Slint UI, Blitz article view,
                    the window's side of the tabs, glue).
  ren-servo/        Servo's helper program, a workspace of its own (M7).
```

During phase 1 only `nextcloud-news` and `ren` exist; `ren-store`,
`ren-sync` and `ren-settings` are added in phase 2.

Data flow: UI → `ren-store` (mark read locally, enqueue change) → UI updates
immediately. The sync thread drains the queue to the server and pulls
updates into the store, then notifies the UI with the ids that changed
(the UI re-queries only what is visible).

### Nextcloud News API notes (v1-3)

Base URL `https://<host>/index.php/apps/news/api/v1-3/`, HTTP basic auth.
Reference: `docs/development/api/api-v1-3.md` in `nextcloud/news`.

- `GET /version`, `GET /status` (show the cron warning if set)
- `GET /folders`, `GET /feeds` (`feeds`, `starredCount`, `newestItemId`)
- `GET /items?type=&id=&getRead=&batchSize=&offset=&oldestFirst=`
  - type: 0 feed, 1 folder, 2 starred, 3 all; `offset` = item id, returns
    items with lower ids (paging; exclusive in practice, although the
    documentation says "lower than equal").
- `GET /items/updated?lastModified=<unix seconds>&type=3&id=0` — items with
  `lastModified >=` the given value, including read/star state changes.
  No paging on this endpoint, so the response size limit must be generous.
- `POST /items/{read,unread,star,unstar}/multiple` with `{"itemIds": [...]}`
  (the News app's `appinfo/routes.php`; the `PUT` variants with `items` or
  `feedId`/`guidHash` in the docs' "How To Sync" section are API v1-2 and
  only kept for compatibility)
- `POST /items/read`, `/feeds/{id}/read`, `/folders/{id}/read` with
  `{"newestItemId": n}` for "mark all read".
- Item `lastModified` is an int in **seconds**. `title`, `author`, `url`,
  enclosure and media fields are **not sanitised**. `body` is.
- Treat unknown fields as ignorable and most fields as nullable.

### Sync algorithm

Initial sync:
1. `GET /folders`, `GET /feeds`.
2. Page unread items (`type=3, getRead=false, batchSize=1000, offset=<lowest id>`)
   until a page comes back short; then starred items the same way.
   Items are written in chunks of 200, one transaction each, while a
   page is received. Never `batchSize=-1`: the server fails on large
   accounts (S2). The progress is stored after each page, so an
   interrupted sync resumes there.
3. Store the server's time when paging started (its `Date`, a minute
   earlier) as the sync cursor, so that changes made while paging are
   fetched next time (M3, `0007`).

Incremental sync:
1. Push the pending-change queue: queued "mark all as read" requests
   first, then read/unread/star/unstar, batched per action. Remove
   entries only after a 2xx response; a refused change stays queued and
   the sync goes on.
2. `GET /folders`, `GET /feeds` — upsert, delete what disappeared.
3. `GET /items/updated?lastModified=<cursor>` — upsert, but do not overwrite
   read/starred state for items that still have a pending local change.
   This endpoint is not paged; if it fails or the last sync was 30 days
   ago or more, fall back to a paged resync of unread and starred items,
   which also marks local items no longer in these lists read or
   unstarred, page by page.
4. Advance the cursor to the max `lastModified` seen. Re-fetching items
   from the same second is harmless because upserts are idempotent.
5. Purge locally: read, unstarred items older than N days.

Resource rules for the sync thread:
- Lower its priority (`nice`) and never run it on the UI thread.
- Deserialise directly from the response body reader.
- Don't pre-process article bodies during sync; sanitise/strip lazily when
  displayed (and cache a short plain-text excerpt only if the list needs it).
- Timer-based (default 15 min) plus manual refresh; skip if one is running.

### Settings, state and credentials

Three kinds of data, kept apart:

- **Settings**: `$XDG_CONFIG_HOME/ren/settings.toml`, curated by the user,
  by hand or through the settings dialog. Only overrides are written;
  defaults live in the code. Resetting a setting removes its key.
- **UI state**: `$XDG_STATE_HOME/ren/state.toml`, written by ren alone:
  pane sizes, arrangement-specific column widths, sort order, expanded
  folders, last selection. Never mixed into the settings file.
- **Secrets**: the app password, in the Secret Service (GNOME Keyring,
  KWallet) through the `keyring` crate. Never in a file.

```toml
# settings.toml
[account]
server = "https://cloud.example.org"
user = "heinz"

[sync]
interval-minutes = 15
```

**Settings model.** One table of setting descriptors in Rust (key, title,
description, category, control kind: switch, choice, number, text, colour;
default, validation) drives loading, validation, the settings dialog and,
optionally, a generated JSON Schema that TOML editors (Taplo, Tombi) can use
for completion. The dialog renders rows generically from the descriptors;
only special pages (the account) are hand-made.

**Writing.** `toml_edit` changes single values in the parsed document, so
comments, ordering and formatting of a hand-edited file survive. The
settings model never serialises the whole file through serde.

**Reloading.** The settings file is watched (inotify, or a modification
time check when the window gains focus). If it doesn't parse or a value is
invalid, ren keeps the last good settings and shows the error with line and
column.

**Account and login.** The server URL is stored bare; the API base is
built as `<server>/index.php/apps/news/api/v1-3/`, which works with and
without pretty URLs.

- Login uses Nextcloud's **Login Flow v2**: `POST /index.php/login/v2`,
  open the returned login URL in the browser, poll the returned endpoint
  with the token until it returns `server`, `loginName` and `appPassword`
  (the token is valid for 20 minutes). 2FA and SSO work because the browser
  handles them. The app password is listed as its own device in the
  user's Nextcloud security settings and can be revoked there.
- Fallback: entering an app password created in Nextcloud by hand.
- Logging out revokes the app password on the server
  (`DELETE /ocs/v2.php/core/apppassword`, with `OCS-APIRequest: true`) and
  removes it from the Secret Service.
- Without a Secret Service: a `password-command` setting (e.g.
  `pass show nextcloud/ren`), else asking at every start. Never a plaintext
  password file.

**Testability.** Settings are parsed from and written to strings; file
paths are passed in. The secret store is a small trait with an in-memory
fake. The login flow is plain HTTP and is covered by the mock server.

## Designing for testability

Tests are deliberately light until the technology choices are locked down
(phase 1), but the code is structured from the first line so that full test
coverage is cheap to add later and pivots stay local:

- **Logic lives in library crates; the binary is glue.** `nextcloud-news`,
  `ren-store` and `ren-sync` know nothing about the UI. Inside `ren`,
  toolkit-specific code stays in `ui/`, rendering-engine code in
  `article/` (Blitz) and `tabs/` (Servo). Swapping Slint or Blitz touches
  only that module.
- **View models in plain Rust.** Selection, list paging, mark-read-on-open,
  unread counts, etc. live in structs without Slint types; Slint only binds
  properties and forwards callbacks. They are unit-testable without a window.
- **Seams at the boundaries, as small traits** — only where a second
  implementation (fake or alternative) actually exists:
  - `NewsApi`: the HTTP client in production, an in-memory fake in sync tests.
  - `Clock`: for sync cursors and purge ages.
  - `ArticleRenderer`: plain text / Blitz (/ whatever we pivot to).
  - `PageOpener`: Servo tab / system browser.
  - `SecretStore`: the Secret Service, an in-memory fake in tests.
- **No hidden global state or I/O.** The API client takes its base URL and
  credentials as parameters (so tests can point it at a local mock server);
  the store can be opened in memory; config paths are passed in, not looked
  up deep inside library code.
- **Sync is a function, threading is the caller's job.** `sync(api, store,
  clock)` runs synchronously; the app runs it on a background thread. Tests
  call it directly.
- **Real data as fixtures.** Responses recorded during spike S2 are anonymised
  and become `tests/fixtures/` for the API and sync tests.
- **Measurements are scripted** (`scripts/`), so numbers in decision records
  can be reproduced and compared after changes.

## Milestones

Work on **one milestone at a time**. Each milestone ends with a short summary
(what was done, measurements, open issues) and a review before the next one
starts. Within a milestone, keep semantically different changes in separate
commits.

### Phase 0 — Scaffold *(done)*

Workspace, hello-world `ren` binary, this plan.

### Phase 1 — Spikes: lock down the technology choices

Goal: answer the open questions with working code and numbers, as cheaply as
possible. Tests are optional here, but the code must respect the module
boundaries above so it can be kept and hardened later instead of rewritten.
Each spike ends with a decision record `docs/decisions/NNNN-<topic>.md`
(context, what was tried, measurements, decision, consequences).

Measurements that need a display or the real Nextcloud server are run
locally by the user; the code and `scripts/` for them are prepared so that
this is a single command.

- **S1 — Slint window.** *(done: Slint yes, software renderer by default
  until S4; see `docs/decisions/0001-ui-toolkit.md`)* Main window with the
  three-pane layout as placeholders: feed tree, item list backed by ~10k dummy rows (virtualised
  `ListView`), article pane showing plain text. Add `scripts/measure.sh`
  (RSS, CPU over time from `/proc`). Update CI along with the first real
  dependency: install the system libraries Slint builds against, add
  `deny.toml` and a `cargo deny check licenses` job.
  *Done when:* idle RSS, startup time and idle CPU are recorded for the
  software, FemtoVG and Skia renderers, and scrolling the 10k list is smooth.
  *Decides:* Slint yes/no, default renderer.
- **S1b — UI capabilities.** *(done: Slint stays, custom tree, table,
  splitter and tool bar components; the Qt style is an opt-in build
  feature, `style-qt`, which needs the Qt 6 development files (set
  `QMAKE=qmake6` if there is no `qmake`); see
  `docs/decisions/0002-ui-capabilities.md`)* Before building on Slint, check that it can
  carry the desktop UI we want, using akregator as the reference. Still on
  dummy data; keep presentation in `ui/` and the logic in plain-Rust view
  models (tree guides, sorting, column state) so it survives into M5.
  - *Feed tree:* real tree visualisation (branch lines, expand/collapse
    chevrons, folder and feed icons as placeholders for favicons),
    keyboard navigation (up/down, left/right to collapse/expand).
  - *Item list as a table:* header with Title, Author and Date columns,
    resizable columns, sorting by clicking a header; alternating row
    backgrounds; text colour by state: new (arrived in the latest sync),
    unread, read. The dummy data gets authors and a "new" flag.
  - *Selection:* closer to a native (Breeze) look: highlight with the accent
    colour, a different look when the list doesn't have focus, hover
    feedback; keyboard navigation in the item list.
  - *Panes:* draggable splitters between the panes. Two arrangements, the
    current one (list beside the article) and akregator's (list above the
    article), switchable at runtime from the View menu. Persisting the
    choice is M8b.
  - *Menu bar, tool bar, context menus:* a skeleton menu bar (File, Edit,
    View, Go, Feed, Article, Settings, Help) with keyboard shortcuts, a tool
    bar with icon buttons, context menus on feeds and items, and a search
    field above the item list (filtering the dummy titles is enough). Icons
    come from the system icon theme (freedesktop lookup) with a small
    bundled fallback set; check how both look in light and dark mode.
  - *Native Qt style:* build a variant with Slint's `qt` style (behind a
    cargo feature, e.g. `style-qt`, so the default build and CI don't need
    Qt). It draws the standard widgets (scroll bars, line edits, buttons,
    table headers, tabs) and takes the palette through Qt's `QStyle`, so on
    KDE it looks like Breeze; our custom tree and table rows are still drawn
    by us, with the native palette. Find out: whether it needs Slint's Qt
    backend (Qt drawing the window instead of winit + our renderer), how the
    menu bar looks with it, what it costs (RSS, but especially PSS: on a KDE
    desktop the Qt libraries are already shared with other processes;
    startup, idle and scrolling CPU) compared to the software renderer, the
    build-time requirements (Qt 6 development files), and what it means for
    S3 (a Blitz image works with any backend) and S4 (no wgpu texture
    sharing with the Qt backend, so Servo would need CPU readback).
  - Re-run `just measure` to see what the richer UI costs.
  Text selection and links in the article pane are not part of S1b: Blitz
  handles both itself (selection with copy through a clipboard hook the app
  implements, link clicks through a navigation hook, cursor changes), and S3
  verifies that.
  *Done when:* screenshots of both arrangements in light and dark mode, with
  the default and the Qt style, a list of what worked, what needed
  workarounds and what didn't, and the measurements are in
  `docs/decisions/0002-ui-capabilities.md`.
  *Decides:* whether Slint stays (or what to pivot to), which custom widgets
  (tree, table, splitter) we maintain ourselves, and whether the Qt style
  becomes an option (or the default on KDE).
- **S2 — Talk to Nextcloud.** *(done: ureq + serde, batch size 1000; see
  `docs/decisions/0003-nextcloud-client.md`)* Minimal `nextcloud-news`
  types and client (version, folders, feeds, paged items, updated items).
  `ren --check` prints server version, folder/feed/unread counts;
  `ren --dump-items <dir>` stores raw responses outside the repo for later
  spikes and fixtures.
  Server and user come from `[account]` in the settings file (parsed
  minimally); the app password from `password-command` or the
  `REN_APP_PASSWORD` environment variable, so S2 doesn't wait for the
  Secret Service and login work of M4/M8b.
  *Done when:* works against the real server; time, peak RSS and CPU of
  fetching all unread items are recorded for a few batch sizes.
  *Decides:* ureq + serde approach, batch size.
- **S3 — Article view with Blitz.** *(done: Blitz 0.3 (pre-release) stays;
  memory with images is the open condition for M6; see
  `docs/decisions/0004-article-view.md`)* Render the items dumped in S2
  into the article pane of the S1 window: scrolling, link clicks (just logged),
  text selection with copy to the clipboard (Blitz's clipboard hook,
  implemented e.g. with `arboard`, since Slint has no public clipboard API),
  mouse cursor changes over links and text, images, light/dark stylesheet.
  Use the released Blitz version; selection support is recent, so note
  if it needs a newer, unreleased one.
  *Done when:* selecting and copying text and clicking links work on a
  varied set of real items, and RSS delta, render time per item and CSS
  coverage are recorded.
  *Decides:* Blitz vs. litehtml vs. native rich text.
- **S4 — Full page in a Servo tab.** *(done: Servo 0.5 in a helper process
  with CPU readback, software renderer stays; see
  `docs/decisions/0005-web-tabs.md`)* A button opens an item's URL in a tab;
  closing the last tab drops Servo. Try both frame paths (wgpu texture
  sharing, CPU readback) and check `stylo` unification with Blitz.
  *Done when:* RSS before opening, with one and three tabs, and after
  closing all tabs is recorded for both paths, plus a note on scrolling
  smoothness.
  *Decides:* frame path and Slint renderer, in-process vs. helper process.

Order rationale: S1 first because S3 and S4 need a window; S2 before S3 so
the article view is tried on real content. S4 is the riskiest and most
expensive to build, so it comes last with everything else in place.

### Phase 2 — Core, with tests

From here on every milestone ships with tests for what it adds.

- **M1 — API client.** *(done; see "M1" in
  `docs/decisions/0003-nextcloud-client.md`)*
  Harden the S2 code: all endpoints listed above
  (including the `*/multiple` and mark-all-read writes), typed errors (auth,
  HTTP status, network, decode), user agent, timeouts. The write
  endpoints are the v1-3 `POST` forms with `itemIds` (see the API notes);
  the open question in 0003 about the `PUT` variants is settled.
  Streaming decode for item lists: the `items` array is decoded item by
  item and handed to a callback, instead of into a `Vec<Item>`, so memory
  stays flat however large a response is. This matters most for the
  unpaged `GET /items/updated` (S2: about 3× the JSON size in memory when
  decoded at once). Tests: fixture deserialisation (nulls, unknown
  fields), request building, streaming decode, a local mock HTTP server.
- **M2 — Store.** *(done; see `docs/decisions/0006-store.md`)*
  `ren-store`: schema and migrations (folders, feeds, items,
  pending_changes, sync_state, per-feed settings), indices for the list
  queries, upserts, paged queries, purge, and a "new" marker for items
  that arrived in the latest sync (cleared when the next sync starts), for
  the item list colours. A feed `folderId` of 0 (older News versions)
  is stored as no folder. Items keep the `filtered` flag (News 28.4
  keyword filters per feed). Single-line fields (folder names, feed titles,
  item titles and authors) are stored with whitespace collapsed: real
  feeds put line breaks into authors and spaces around titles (S3).
  Tests on an in-memory database.
  *Result:* rusqlite pinned to 0.38 (Servo links the same SQLite). The
  upsert already keeps the read and starred state of items with a
  pending local change (M3's conflict rule). Lists come as count and
  pages or as all ids in order plus rows by id; M5 picks (the ids suit
  the item table: pages from the middle of a list sorted by feed or
  title cost up to about 140 ms each, all ids 40–65 ms once).
- **M3 — Sync engine.** *(done; see `docs/decisions/0007-sync.md`)*
  `ren-sync`: initial and incremental sync and the
  pending-change queue as described above. Items from the streaming
  decode are written in chunks (one transaction per chunk), so neither a
  page nor a large `/items/updated` response is ever held in memory as a
  whole. The `NewsApi` trait is defined here, after what the sync needs
  from `Client` (M1 left it out). The server marks items read or starred
  one by one per `*/multiple` request, so the queue is pushed in
  batches of at most 1,000 ids per request: some databases behind
  Nextcloud reject longer value lists in one query. Not verified against
  a server; real use (e.g. marking thousands of items read) will show
  whether it holds. "Mark all read" for feeds outside of folders goes feed
  by feed (`/folders/0/read` fails with HTTP 500, see 0003). Unit tests
  with a fake `NewsApi`
  (including conflicts: local pending change vs. remote update), one
  integration test with the real client against the mock server.
  *Result:* `sync(api, store, clock, options, progress)` with a fake
  server in the tests. "Mark all as read" is queued as one request. The
  cursor after a full sync is the server's time when it started (the
  newest `lastModified` can be far in the past, e.g. after "mark all
  read" on the web). Full syncs resume after an interruption; the
  resync corrects states page by page. A refused change stays queued
  without stopping the sync. Memory stays flat at about 6 MiB above the
  start for 62,000 items. On the real server (63,247 items): the initial
  sync and a resync take 2 minutes (the server's time; 1.9 s of CPU),
  an incremental one 4.6 s for three requests; peak RSS 20–21 MiB,
  18 MiB after any sync (TLS, the open database); `malloc_trim` after a
  full sync gives back up to 2.2 MiB (M5).
- **M4 — Settings & credentials.** *(done; see
  `docs/decisions/0008-settings.md`)* As described in "Settings, state and
  credentials": the setting descriptor table, loading with defaults and
  validation, writing single values with `toml_edit` (comments and order
  preserved), the state file, reloading on change with error reporting,
  the `SecretStore` trait with the Secret Service implementation, and
  `password-command`. Check which `keyring` backend reuses the zbus that
  Slint already brings, and that KWallet answers the Secret Service API on
  Plasma. Tests for parsing, defaults, validation, round-trips that keep
  comments, and the secret store fake.
  *Result:* a new crate, `ren-settings`. The table has the account and
  two sync settings so far; unknown keys are warnings, invalid values
  errors with line and column that keep the last good settings. The
  file is reloaded when its stamp (time, size, inode) changed, checked
  when the window gains focus (M5), not watched. The Secret Service goes
  through `keyring-core` and its zbus store, on Slint's zbus 5 and
  async-io (the other store needs libdbus); five new small crates.
  Lookup order: `REN_APP_PASSWORD`, `password-command`, Secret Service
  (`REN_APP_PASSWORD` removed in M8b).
  `ren --set-password` and `ren --check-secret-service` until M8b's
  account page; the Secret Service tests run against GNOME Keyring on a
  private bus (`just test-secret-service`, also in CI). Connecting costs
  about 7.5 ms and 1–1.5 MiB RSS while the password is fetched; the
  binary grows by 1.1 MiB. KWallet answers on Plasma (as `ksecretd`).

### Phase 3 — The application

- **M5 — UI wired to real data.** *(done; see `docs/decisions/0009-app.md`)*
  The S1 window on top of store and sync:
  feed tree with unread counts, item list, plain-text article pane, mark read
  on open, star toggle, manual refresh, sync status, background sync thread
  (`slint::invoke_from_event_loop`). View models unit-tested.
  Filtered items (`filtered`) are hidden, as in the News web interface.
  *Result:* the window shows `$XDG_DATA_HOME/ren/ren.db` through a view
  model in plain Rust (`reader.rs`, tested on a store in memory); the
  item list holds the ids in order and fetches visible rows by id; a
  "Starred" entry; tree rows updated in place. A thread per sync at
  `nice` 10, which fetches the app password, refuses a database of
  another account and trims the heap after each sync; progress,
  results and cancelling in the status and tool bar; the lists fill up
  during the first sync. Settings are read at start, on window
  activation and before each sync. `--items`/`--dump` go through a
  temporary database. On the desktop with the real account (63,510
  items): idle RSS 48 MiB, 4.4 MiB above S1b's dummy data at the same
  item count (the code, not the data); startup 20 ms longer, another
  20–30 ms with 63,000 items; scrolling as in S1b (47 % of a core with
  the real data, 41 % with generated items, 48 % in S1b). A full sync in
  the window takes the server's 2 minutes and 5.5 s of CPU, adds 7 MiB,
  and the lists are read again in 42 ms on average while it runs. Freed
  memory is trimmed after every sync (the incremental one kept 3 MiB).
- **M6 — Article view.** *(done; see `docs/decisions/0010-articles.md`)*
  The S3 result made production-ready behind the
  `html-view` feature, including the image cache. From S3: freed image
  memory returned to the system (glibc keeps it; `malloc_trim` or the
  mmap threshold), images decoded at display size, the heap growth over
  many articles found and capped, reading with images under the memory
  goal (re-measured with `just measure-articles`), author colours in dark
  mode, a scroll position indicator, media elements as poster or link, and
  a setting to load remote images only on request. From the S3 review:
  - A pixel limit for images, checked from the image header before
    decoding (refuse or downscale above it): the 20 MiB download limit
    doesn't stop a small file that decodes to gigabytes. Find out which
    limits Blitz passes to the `image` crate.
  - Link clicks open only `http`, `https` and `mailto` URLs; anything
    else (`javascript:`, `file:`, …) is ignored. (Done in S4.)
  - A selection colour derived from the accent, readable in dark mode
    (`::selection`, if Blitz supports it).
  - Free the two frame buffers while no article is shown.

  From use (M5):
  - **Jump links within an article** (`#section`) do nothing: the News
    app's sanitiser strips `id` and `name` attributes from bodies, so
    Blitz's fragment navigation (`scroll_to_fragment`, used for links to
    the same document, also absolute ones to the item's URL) finds no
    target. Example: "GSoC 2026 Final Update - Jenkins Email
    Notifications using Outlook SMTP with OAuth", whose table of contents
    links to its headings. Their text still matches the fragments as
    slugs (lower case, punctuation removed, words joined by hyphens) in
    18 of 18 cases there. So: when a fragment has no target, scroll to the
    heading whose slug matches; failing that, open the original page at
    the fragment (tab or browser).

  *Result:* images go through a disk cache (100 MiB), are refused above
  25 MP from their header and scaled to the content's width before Blitz
  decodes them (large JPEGs decoded at a fraction with `jpeg-decoder`),
  one at a time. glibc's mmap threshold is fixed at 1 MiB, the heap is
  trimmed after article switches and loaded images, and each article
  gets a new renderer: the "heap Blitz keeps" was vello_cpu's glyph
  outline cache. `articles.load-images` (a bar and Article → Load
  Images), author colours replaced in dark mode, the selection in the
  accent, a scroll indicator, video and audio as poster and link, jump
  links by heading slugs. In the container with synthetic articles:
  after 30 articles with large images 50 instead of 192 MiB RSS, peak 86
  instead of 718 MiB; after 300 articles of text 50 instead of 80 MiB,
  first frames as fast. On the desktop with 100 real articles: 76.4 MiB
  after them with images, peak 90.3 MiB (S3: 108.4 and 122.9), 26 MiB
  above plain text (S3: 60); creating the view still costs 16 MiB.
- **M7 — Full-page tabs.** *(done; see `docs/decisions/0011-tabs.md`)*
  The S4 result made production-ready behind the
  `servo` feature: tab bar, "Open page", on-demand lifecycle, per-feed "open
  full page instead of article" setting. Remove the spike-only variants
  (`--tabs in-process`, `servo-wgpu`). From the S4 decision record (0005):
  frames through shared memory, the UI's frame buffers given back after
  the last tab closes, popups, navigation buttons. From the S4 review:
  - **Keep Servo current**, instead of pinning it to the version that
    shares stylo with Blitz: Servo runs in the helper and Blitz in the UI
    process, so sharing stylo saves build time and binary size but no
    memory, while a pinned web engine misses security fixes. Accept two
    stylo versions while their releases differ.
  - **Restrict navigation inside tabs** to `http`/`https` with Servo's
    `request_navigation` hook, so pages can't navigate to `file:` and
    other schemes (link clicks from articles are already filtered).
  - **Decide on a Servo profile:** without one, cookies and site data are
    lost when the helper exits, so consent banners and logins come back
    every time. Likely a profile in `$XDG_DATA_HOME/ren/servo` with a
    setting to clear it.
  - **Pass the helper an inherited socket pair** instead of a socket path,
    which also removes the fallback to the temporary directory.
  - **Don't wait for the helper on the UI thread** when the last tab
    closes (up to 3 s for a hung helper).
  - **Let the helper die with the UI:** `PR_SET_PDEATHSIG`, and a bound
    on its own shutdown (Servo's can hang once the UI is gone; an
    orphaned helper ran on for 10 minutes).
  - **glibc's mmap threshold in the helper:** the UI process fixes it at
    1 MiB since M6 (`0010`), the helper keeps glibc's dynamic one; check
    with the tab measurements whether fixing it helps there too.

  Scrolling heavy pages (0005, "Scrolling heavy pages"); check each with
  `REN_TAB_STATS=1`:
  - **Asynchronous readback:** read frames into pixel buffer objects and
    fetch the previous one, instead of `glReadPixels` waiting for the GPU
    (4–22 ms per frame on heise, up to 160 ms).
  - **Frame pacing:** the helper sends a frame only when the UI has taken
    the previous one, always the newest; frames the UI couldn't show are
    not read back at all.
  - **Shared memory** for frames instead of the socket (see above): no
    transfer, no receiving thread in the UI.
  - **Smooth wheel scrolling:** spread each wheel step over a few frames.
  - **Content blocking (optional):** filter ad and tracker requests
    through Servo's request interception with a filter list (e.g. the
    `adblock` crate, MPL-2.0). Ads are what makes these pages repaint all
    the time; also a privacy feature. As a setting. *Moved to M7b, a
    milestone of its own.*

  *Result:* Servo can't share a build with Blitz on another stylo
  (stylo's `links` key), so the helper became a program of its own,
  `ren-servo`, in its own workspace on Servo 0.7.0, following Servo's
  monthly releases; ren's binary has no Servo, and `ren-tabs` is the
  protocol. One socket pair (the helper's stdin), frames through a
  memfd sealed by ren and passed with `SCM_RIGHTS`, frame pacing,
  asynchronous readback with a pixel buffer object and a fence, smooth
  wheel scrolling, a death signal and a 3 s bound on the helper's
  shutdown, no waiting on the UI thread when the last tab closes. Tabs
  only navigate to web pages (mail links go to the desktop); popups open
  tabs; back, forward, reload and the address in a navigation bar; a
  Servo profile in `$XDG_DATA_HOME/ren/servo` (`pages.keep-site-data`,
  Settings → Clear Web Site Data); per feed, "Show Full Pages Instead of
  Articles" loads its articles' pages in one tab. Spike variants
  removed. On the desktop: 48.0 MiB before the first tab (S4: 56.9),
  248.4 with one (288.5), 311.9 with three (418.0); the helper 30–90
  MiB smaller; starting 0.9 ms, stopping 1.2 ms without waiting on the
  UI thread; frames 3.9 ms to the window (6.9), 133 frames while
  scrolling (127), Ars Technica 162. The window kept one frame after the
  last tab closed (Slint keeps the image of a hidden element); fixed,
  1.0 MiB above the start afterwards. The helper keeps the fixed mmap
  threshold (on heise 100–117 MiB less). Heavy pages cost 160–470 MiB
  per tab (heise after its consent: 470) and scroll at about 25 frames
  per second on heise, with one frame under way at a time. Content
  blocking isn't done (M7b). Scrolling a page in a tab felt smooth, but
  the window took 60–80 % of a core: Slint's software renderer drawing
  each frame. RGB frames and no background under the frame cut a third
  of that per frame; the rest is M10's.

- **M7b — Content blocking.** *(done; see
  `docs/decisions/0012-content-blocking.md`)* Block ads and trackers in the web page
  tabs: they make heavy pages big (heise: 470 MiB per tab once its
  consent is accepted, M7) and repaint all the time, and they are a
  privacy matter. Starts with research, recorded in a decision record:
  - *Engines:* Brave's `adblock` crate (MPL-2.0; network rules, cosmetic
    rules, its memory and CPU cost with the usual lists), what Servo
    offers itself (request interception through `load_web_resource`, any
    content-blocking work upstream), simpler host lists (e.g. hosts files
    or a domain set), and how other Servo and Rust embedders do it.
  - *Lists:* which ones (EasyList, EasyPrivacy, uBlock's, regional ones),
    their licences (downloaded, not shipped, unless one allows it), how
    and how often they are updated (by the helper, by the sync, a size
    and age limit), where they are kept.
  - *Cosmetic filtering:* hiding elements needs CSS or scripts injected
    into pages (Servo's user content manager); worth it or network
    blocking only.
  - *Cost:* RSS of the helper with the engine and lists loaded against
    what blocking saves on heavy pages, the time per request, startup
    time of the helper.
  Then the implementation in the helper, a setting to turn it off
  (default on), and per-site exceptions if pages break. Measured with
  `just measure-tabs` on the heavy pages of M7.
  Also from M7 (0011): **popups** need a user gesture. Pages can open
  tabs by themselves now, up to the limit of 50 tabs, at 200–470 MiB
  each; allow a new tab only right after a click or key press in the
  page, with a much lower limit for the tabs pages open themselves.

  *Result:* Brave's `adblock` crate in the helper checks every request
  of a page in Servo's `load_web_resource` (which all requests already
  pass through, one at a time) against the network rules of EasyList,
  EasyPrivacy and uBlock's filters, Privacy and Unbreak by default;
  network blocking only, cosmetic filtering went to the backlog. ren
  downloads the lists when a tab opens and they are older than four
  days (into `$XDG_CACHE_HOME/ren/filters`, at `nice` 10), resolving
  uBlock's `!#include` and `!#if`, which the engine doesn't; the helper
  makes its engine on a thread and saves it. Settings
  `pages.block-content`, `pages.filter-lists`, `pages.unblocked-sites`
  (ren-settings got list settings), "Allow Ads Here" in the navigation
  bar, `ren --update-filters`. Pages open a tab only within 5 s of a
  click or key press in them, once per gesture, at most 10 at a time. In
  the container: about 11 MiB for the helper with the default lists
  (once, not per tab), 10 µs per request on average, the engine made in
  a sixth of a second and loaded in 40 ms next time. On the desktop:
  the helper with one Ars Technica tab 96 MiB smaller (263 instead of
  359), with three 224 MiB (439 instead of 663); heise 59 MiB smaller
  with one tab, 116 with three, and it scrolls at 134 frames in 3 s
  instead of 60, the helper at 36 % of a core instead of 155 %. Light
  pages cost 7–9 MiB more. Ars Technica scrolled slowly with and
  without blocking (48 and 38 frames, 162 in 0011): M10.
- **M8 — Polish.** *(done; see `docs/decisions/0013-polish.md`)*
  Keyboard navigation (j/k, s, m, o), mark-all-read,
  periodic sync, purge settings, favicons (cached on disk), persisted pane
  sizes, column widths and sort order (state file).
  - **Window size, position and maximised state** in the state file too
    (M4's `State` has no fields for them yet), restored at start and
    saved when the window closes. Logical pixels, so a change of scale
    factor keeps the size; clamp to the current screen so a disconnected
    monitor doesn't leave the window off-screen. Wayland doesn't let
    applications place their windows, so there only the size and the
    maximised state apply (the compositor, e.g. KWin's window rules,
    handles position).
  - **System tray:** an icon with the unread count (tooltip, or a badge
    if the tray supports it) and a menu (Show/Hide, Fetch All Feeds,
    Quit), and a setting to close or minimise to the tray instead of
    quitting, so the periodic sync goes on in the background. Slint 1.18
    has a `SystemTrayIcon` element (its `system-tray` feature,
    StatusNotifierItem over D-Bus through `ksni` on the async-io runtime
    ren already has; works on Plasma, needs the AppIndicator extension on
    GNOME). `ksni` is under the Unlicense, which isn't on the licence
    list yet: decide on it like the `webpki-roots` exception. Measure RSS
    with the window hidden; free what a hidden window doesn't need (the
    article's frame buffers, the Blitz view, Servo tabs).
  - **Shortcuts while a page has the focus** (M7, 0011): keys with
    modifiers go to the page and to the window's shortcuts (Ctrl+F in a
    page also focuses ren's search). Decide which keys stay ren's (tab
    and window shortcuts) and send the rest only to the page.
  - **Drop-downs and the context menu in pages** (M7, 0011): Servo asks
    the embedder to show a `<select>`'s options and a page's context
    menu. The helper passes the request (options or menu entries, and
    where) to ren, which shows them as a Slint popup and sends the
    choice back. The context menu: open the link in a new tab, copy the
    link, copy the selection, back/forward/reload.
  - **Item list** (0009): select the row on right click before its
    context menu opens; "Copy Link Address" in the item menu (with the
    article view's clipboard, M6); dates in local time, not UTC.
  - **Article view** (0004, 0010): select all (Ctrl+A); smooth scrolling
    for the keyboard; transparent images with dark lines (diagrams,
    logos) readable in dark mode, e.g. on a light backdrop.
  - **Enclosures and thumbnails** (0010): podcast episodes
    (`enclosureLink`) and the items' media thumbnails.

  *Result:* the window's state (list, order, collapsed folders, the
  item shown, pane sizes and column widths per arrangement, the window's
  size, position and maximised state) in `state.toml`, written when the
  window closes, only with the default database; a window off the
  screens is centred on the primary one (winit through Slint's
  `unstable-winit-030`). j/k/s/m/o from the tree, list and article; Mark
  All Feeds as Read; a sync at start and every interval (purging was
  done in M4/M5). Favicons downloaded after a sync, kept as 32 pixel PNGs
  in `$XDG_CACHE_HOME/ren/favicons` (`sync.feed-icons`). The item list:
  right click selects, Copy Link Address (Slint's clipboard, through a
  hidden text input), local dates. In tabs, the window keeps its menu's
  shortcuts and the page gets the other keys (F5 and Ctrl+R reload the
  page); Servo's drop-downs and context menus are shown by the window.
  The article: select all, smooth scrolling by keyboard, a light
  backdrop for images in dark mode, enclosures and thumbnails. A tray
  icon (Slint's `SystemTrayIcon`, ksni; its Unlicense is an approved
  exception) with the unread count and a menu; `window.close-to-tray`
  hides the window there, only where the desktop shows tray icons, and
  drops the article's view and the tabs while hidden: on the desktop
  with an article and two pages, 350 MiB shown, 62 hidden (`just
  measure-hidden`). The desktop found the maximised state lost under
  KWin (winit asks before the window manager manages the window) and
  drop-downs placed by the box Servo reports, which is relative to the
  document; both fixed.
- **M8b — Settings dialog and account setup.** *(done; see
  `docs/decisions/0014-settings-dialog.md`)* A Zed-style settings
  dialog in ren's own style: search field, category list, rows of title,
  description and control generated from the setting descriptors, live
  apply, an "Open settings file" action, and reset to default per setting.
  Settings for the pane arrangement and the item status colours. The
  account page: Login Flow v2 with a "waiting for the browser" state, the
  manual app password fallback, and log out (revoking the app password).
  First run opens the account page. A setting for the image cache's size
  (fixed at 100 MiB, 0010). Edits to the file by hand apply when
  it's saved, as in Zed: a thread sleeps on inotify for the config
  directory (editors save by renaming a new file over the old one) and
  wakes the UI to reload. That replaces the reload on window activation,
  which never happens while the search field has the focus (its
  `LineEdit` doesn't forward `focus-gained`, see 0009); the sync still
  reads the file when it starts. Also Help → About ren: version, the
  GPL notice, a link to the source, credits (Slint, Blitz, Servo, the
  icons) and a "Third-party licenses" view showing the notices from M11
  (rendered like an article), embedded in the binary.

  *Result:* the dialog is a window of its own, made when it opens: a
  search, the categories, rows generated from the descriptors
  (`settings_dialog.rs`, tested) with a switch, drop-down, text field
  or list editor and Reset to Default; every change written to the
  file and applied at once, invalid input kept with its error, a file
  with errors shown and left alone. New settings `list.arrangement`
  (the View menu changes it), `list.new-colour` and
  `list.unread-colour` (unset: the theme's), `articles.image-cache-mib`.
  The file is watched with inotify (`ren_settings::watch`, a thread on
  libc) and read 100 ms after it was saved; the reload on activation is
  gone. The account page: Login Flow v2 (`nextcloud_news::LoginFlow`;
  the login page and poll endpoint must be https or on the server),
  an app password by hand, Log Out revoking it; logins on a thread of
  their own; without a Secret Service the password lasts until ren
  quits and the page opens at the next start. It also opens on the first
  run. About ren with "Third-Party Licenses"; the notices file is a
  placeholder until M11. In the container: the dialog
  3.2–3.5 MiB while open, 1.0 MiB after closing it; About 1 MiB, given
  back. Not yet checked on the desktop (`just measure-settings`, a real
  login with KWallet). After the review: settings that need a switch
  are disabled while it's off (`Setting::requires`), the category list
  counts and narrows search results, colour pickers of ren's own, and
  the arrangement chosen by pictures; after the second, the controls
  flush right, lists below their description, and the licenses on a
  page of About ren (drawn by Slint, `notices.rs`); after the third,
  units inside the number fields and no keys in the rows. On the desktop the
  dialog costs 10.3–10.7 MiB while open, 0.9–1.3 after closing it;
  About 4.1–4.5 MiB, given back.

### Phase 4 — Hardening

- **M9 — Test coverage.** *(done; see `docs/decisions/0015-tests.md`)*
  Measure coverage (`cargo llvm-cov`), close gaps,
  add a few UI flow tests with Slint's testing backend, and an end-to-end
  test against a Nextcloud + News container.

  *Result:* `just coverage` (cargo-llvm-cov, without the engines; CI's
  full job runs its tests with coverage and shows it in its summary):
  79.9 % of the lines instead of 67.6 %, the library crates at 93–98 %,
  `ren` outside `ui/` 89.7 % (80.1), `ui/` 40 % (2.9); with the article
  view 78.5 %. `just
  test-nextcloud` runs an end-to-end test against Nextcloud with SQLite
  in a container (docker or podman): Login Flow v2 with the browser's
  part done by the test, the initial sync in pages, pushed and pulled
  changes, a resync, a feed deleted on the server, log out revoking the
  app password; it serves its own feeds. CI runs it with Nextcloud
  32/News 28.7.0 (the desktop's) and 33/29.0.0; nothing needed a change
  for either. Seven UI flows on Slint's testing backend (`ui/flows.rs`,
  without a display): reading with clicks and keys, the tree, search,
  sorting, marking read, the settings dialog, About ren; they found
  Mark Feed as Read panicking since M8 (fixed). Tests for the command
  line modes, the sync thread, the downloads and the frame statistics.
  Not covered: the tabs' window side (needs the Servo helper), the
  account page's login in the window (needs the Secret Service passed
  in), the window's geometry (winit only), the tray and the measurement
  modes.
- **M10 — Performance.** *(for review; see `docs/decisions/0016-performance.md`;
  measured on the desktop)*
  Re-run the measurement scripts: idle, reading, with
  tabs open and after closing them, during initial and incremental sync of a
  large account. Profile and fix hotspots. Compare the UI renderers again
  for scrolling web pages after M7's frame path work: GPU renderers halve
  the UI's drawing cost on heavy pages but cost 35–60 MiB for the whole
  session (0001, 0005); if one still feels clearly better, offer it as a
  setting. From M7:
  - **The window's CPU while a page scrolls** (60–80 % of a core on the
    desktop before RGB frames; 0011): Slint's software renderer drawing
    every frame. Compare with the GPU renderers, and whether the software
    renderer can draw an opaque image without its per-pixel path.
  - **Ars Technica scrolls slowly** (0012): 38–48 frames in 3 s with
    and without content blocking, against 162 in 0011; the other pages
    are as fast as before. The GPU takes 45–74 ms per frame (the
    readback waits for it; Servo's paint is 2–3 ms). Find out what on
    the page costs that (compare with servoshell 0.7 and WebRender's
    profiler), and whether it's Servo's or Mesa's.
  - **Two frames under way** in the tabs (two slots in the shared
    buffer), so the GPU's work on the next frame overlaps the window's
    copy of the last; heise scrolled at about 25 frames per second with
    one.
  - **Smooth wheel scrolling in the article view,** as in the tabs:
    scrolling articles feels less smooth than pages.
  - **The article view's last frame** is probably kept while a page is
    shown (Slint keeps the image of an element it doesn't draw, 0011).
  From M6 (0010):
  - **Very large PNGs** are decoded at full size before they are scaled
    (an 800×12,000 PNG: 29 MiB for a moment); JPEGs are decoded at a
    fraction already. Decode PNGs row by row into the scaled image if
    real articles show such peaks.
  - **Very tall images** (over 4 million pixels at the content's width)
    are shown narrower than the content.

  *Result:* the article's frames and renderer go while a page is shown;
  smooth wheel scrolling in the article view (the helper's, moved to
  `ren-tabs`); very tall images keep their width; the helper paints the
  next frame while the window copies the last (in the container 111
  instead of 101 frames in 3 s; no second slot needed). Profiled: the
  window's CPU while a page scrolls is Slint's software renderer (47 %
  drawing the image pixel by pixel, 15 % filling the background under
  it) and ren's copy of the frame (14 %); a fast path in Slint for
  unscaled opaque images saves a quarter of it (tried on a copy, to be
  decided: upstream or a patch). No hotspots in ren's own code for the
  list, the articles or the store. New tools: `pattern-helper` (a
  stand-in for Servo's helper), `REN_SERVO_ONE_FRAME`,
  `REN_SERVO_WR_PROFILER` (WebRender's profiler, for Ars Technica), the
  largest image decoded in `--cycle-articles`, and `just measure-all`,
  which runs every measurement (with the GPU renderers and the synthetic
  page in `measure-tabs`) into one report. On the desktop: the GPU
  renderers take 2–5 times the window's CPU per page frame and 50 MiB
  more, so there is no renderer setting; the largest image decoded
  before scaling over 100 real articles took 5.9 MiB, so no row-by-row
  PNG decoding; the window with the article and two pages takes 74
  instead of 86 MiB; Ars Technica scrolls at 57 frames per second on
  some loads and at 16–18 on others: the same requests every time, but
  the integrated GPU at its lowest clock while the CPU is busy (found
  with `REN_SERVO_REQUEST_LOG` and reproduced under CPU load), so page
  measurements need an idle machine; on the fast ones the new pacing
  gives 14 % more frames in half the time to the window. Open: the
  article peak with images went above 100 MiB (101–104, M6: 88–90), and
  Ars Technica's corrupted images.
- **M10b — Features for the first release.** *(done; see
  `docs/decisions/0017-release-features.md`)* In this order: each item
  uses what the ones before it made (the logo is also M11's app icon).
  - **"Ren" in what users see:** window titles, menus, dialogs, About,
    the tray's tooltip, status and settings texts, the welcome page, and
    the `.desktop` file's name (M11). Not the executable, crate names,
    paths (`$XDG_*/ren`), application and D-Bus ids, or the `ren:`
    prefix of messages on stderr.
  - **A logo:** a wire-frame lotus (蓮, *ren*) as an SVG of ren's own,
    licensed like the code (or CC-BY-SA), at the sizes a desktop needs;
    it becomes the app icon of the `.desktop` file and AppStream (M11).
  - **The logo in the tray:** it replaces the feed icon on an orange
    square (`icons/tray.svg`, M8), with a setting for a monochrome
    version that fits Plasma's symbolic tray icons (drawn in the panel's
    text colour, light or dark), next to the coloured one.
  - **A welcome page** instead of "No article selected" at the start,
    as KDE's applications have: ren's name and version, the unread
    count and what the last sync did, a few shortcuts and links (log
    in, add a feed, the settings), over the logo as a faint background.
    Shown in the article view, so it costs nothing beyond what the
    first article costs anyway.
  - **One instance:** starting ren while it runs brings the running
    one's window to the front (also from the tray) instead of a second
    window on the same database. A well-known name on the session bus
    (zbus is in the build already) or a socket in `$XDG_RUNTIME_DIR`;
    the second process hands over and exits. Measurement and test modes
    (`--dump`, `--measure-*`, a `--database` copy) stay independent.
  - **Broken feeds:** the store has the server's `updateErrorCount` and
    `lastUpdateError` per feed (M2, synced already); show them: a mark
    on the feed in the tree with the error as its tooltip, the feed's
    last successful update if the server tells it, and a way to list
    all broken feeds (a filter or a "Broken feeds" node in the tree).
  - **Adding feeds, and fixing their URLs:** Add Feed (now "not
    implemented yet") with the URL and a folder (`POST /feeds`; the
    server finds the feed from a page's URL and reports what fails).
    The News API v1-3 can create, delete, move and rename feeds but has
    no call to change a feed's URL, so "Change URL" is adding the new
    feed and deleting the old one, whose articles (starred ones
    included) go with it: ren says so before, and keeps the name and the
    folder. Also renaming, moving and deleting feeds and folders (0009's
    "not implemented yet" list).
  - **The rest of Servo's embedder controls** (M7, 0011): file pickers
    (uploads, through the desktop's portal or a Slint dialog), colour
    pickers, input methods (IME, for Chinese, Japanese or Korean text),
    and the `alert`/`confirm`/`prompt` dialogs, which are dismissed now.
    Drop-downs and the context menu came in M8. The largest item, and
    Servo's: split off if it would hold up the release.

  *Result:* "Ren" in titles, menus, dialogs and texts; the logo, a
  wire-frame lotus (`icons/ren.svg`; a small one-pixel-line version
  without the wires on Breeze's 22-pixel grid and a one-colour one), the
  small one as the windows' icon, the large one in About Ren; in the tray
  in colour or in one colour (`window.tray-icon-style`: following the
  colour scheme, light or dark), drawn at 44 pixels. A welcome page while
  no article is shown, drawn by Slint rather than Blitz so the start
  doesn't make the article view: version, counts, the last sync, a
  button to log in without an account, the logo faint in the middle. A
  find bar (Ctrl+F) for the article or the page shown, after the review;
  the article list's search moved to Alt+Q. One instance through a
  socket in `$XDG_RUNTIME_DIR` (a second start hands over; the window
  comes forward as a pager asks on X11, with the launcher's token on
  Wayland, or shown anew without one). Broken feeds (more than eight
  failed updates, as News' web interface) with a mark and the
  error as tooltip, "Broken feeds" in the tree; the API has no last
  successful update. Add Feed, editing (name, address, folder) and
  deleting feeds, adding, renaming and deleting folders, on the sync
  thread before a sync; a new address adds the feed anew and deletes the
  old one, saying so first. The end-to-end test covers them on both
  server versions and found News answering a new feed with a null count
  and leaving a deleted folder's feeds behind (Ren deletes them first).
  Pages' `alert`/`confirm`/`prompt` over the page, headed by its site;
  colour inputs with Ren's picker; uploads through the desktop's file
  chooser (XDG portal). Input methods split off to the backlog (Slint
  gives IME only to its own text inputs). 2.2–2.6 MiB more RSS at the
  start in the container (0.6 of it the welcome page's faint logo,
  within the runs' spread after the review made it smaller).
  Not yet checked on the desktop.
- **M11 — Release and packaging.** *(done; see
  `docs/decisions/0018-release.md`)*
  - **Third-party notices** generated with `cargo-about` (all features, so
    one file covers every build) into a committed
    `THIRD-PARTY-NOTICES.html`, regenerated with `just notices`; CI checks
    that it matches `Cargo.lock`. Content that isn't a crate is added by
    hand: the bundled Lucide icons, and libraries vendored inside crates
    (SpiderMonkey's in `mozjs_sys`, e.g. ICU and zlib). The About dialog
    (M8b) embeds the file; packages install it as documentation.
  - A `.desktop` file, the app icon (M10b's logo), AppStream metadata,
    the install layout (binary, the Servo helper in `libexec/ren/`,
    notices, icons), and release tarballs for offline builds by
    packages. The notices cover both workspaces (ren's and the
    helper's).

  *Result:* the application's id `io.github.pprkut.ren`, of the
  `.desktop` file, the AppStream metadata (both validated) and the
  windows (`slint::set_xdg_app_id`); the logo in hicolor as SVG and as
  PNGs from 16 to 512 (`just icons`). `just notices` writes
  THIRD-PARTY-NOTICES.html from cargo-about's licences of both
  workspaces, merged with jq (offline, without test and build-only
  crates: 790 crates, 174 texts), SpiderMonkey's libraries in mozjs_sys
  (ICU 78's licence in `notices/`) and `notices/header.html`; CI checks
  it with the same cargo-about. 740 KiB, 1.8 MiB while About Ren shows
  it. `just install` (bin, libexec/ren/ren-servo, applications, metainfo, icons,
  doc; `prefix` and `rootdir` as in COSMIC's justfiles), `just vendor`
  (both workspaces' crates, for offline builds with `just
  build-vendored`: 141 MiB), a README, `rust-version` 1.92 (Slint's).
  Distributions' build files stay out of the repository; packagers
  build from the tag's source and `vendor.tar.xz`.
- **Performance tracking in CI.** *(built, for review; see "Performance
  tracking" in `docs/decisions/0019-ci.md`)* Side work, like the CI
  rework. The measurements that are steady on GitHub's runners run for
  every pull request (also from forks) and every push to `master`, and
  their history is kept in [Bencher](https://bencher.dev), which
  reports regressions on the pull request. Bencher's machines aren't
  used; the results are uploaded from CI.
  - The measurement scripts (and `measure-store`) write their results
    as Bencher Metric Format JSON next to `summary.md`; `just
    measure-all`'s are merged into one file, so the desktop's runs can
    be uploaded too, as a testbed of their own.
  - In CI, under Xvfb with the software renderer: the window's memory
    at each stage of `measure-hidden` and `measure-settings`, the tabs'
    with `pattern-helper` (no Servo), `measure-store`, and ren's release
    binary's size, stripped and unstripped. Time, CPU and frames are
    noise on shared runners: they stay on the desktop's testbed.
  - Two workflows, so pull requests from forks are measured without
    getting the Bencher token: `performance.yml` measures without
    secrets and keeps the results as an artifact, `performance-upload.yml`
    (`workflow_run`) uploads them and never runs the pull request's code.
    Pull requests get a branch `pr-<number>` in Bencher, compared with
    `master`; archived when the pull request closes.
  - Reporting only at first: no alert fails CI until the thresholds have
    settled on real data.

  *Result:* `scripts/bmf.sh` turns the scripts' values into BMF (runs
  averaged, lowest and highest as bounds); `measure-all`'s scripts and
  the store's example write `results.json`, `measure-all` (now with the
  store) merges them, `just measure-upload` uploads them as the
  `desktop` testbed. `just measure-ci` builds the default features
  once, unstripped, strips a copy, and runs `measure-hidden`,
  `measure-settings`, `measure-tabs` (synthetic) with generated items
  and `pattern-helper`, and `measure-store`, under `xvfb-run` without a
  display. `performance.yml` (on `ubuntu-24.04`, the testbed),
  `performance-upload.yml` and `performance-closed.yml` as planned;
  thresholds a share above `master`'s mean: anonymous memory 2 %, RSS
  5 %, size 1 %. In the container (without Blitz) 3.5 minutes after the
  build; anonymous memory agrees within 4–32 KiB over three runs but
  after the window is shown again with a page (up to 1.1 MiB), RSS within
  0.3–1.3 MiB. Open: Bencher's project, key and measures (set up
  by the maintainer), the first runs on GitHub's runners, which only
  start on `master` (`workflow_run`).

### Backlog

Not planned into a milestone; picked up when articles or users need them.

- **Input methods in pages** (0017): Servo asks for an input method
  where an editable element is and takes the composition as `ImeEvent`s,
  but Slint hands input methods only to its own `TextInput`. Try a
  hidden `TextInput` at the element that passes on what is composed, on
  a desktop with an input method (Chinese, Japanese, Korean text).

- **Cosmetic filtering in the tabs** (0012): hiding elements and
  uBlock's scriptlets, through a user content manager per web view
  updated when the main frame navigates; consent banners and empty ad
  frames stay without it. Try once M7b's network blocking is measured.

- **Text in SVG images is missing** in the article view: the shapes are
  drawn, the labels aren't (PlanetScale's "When to choose x86-64 vs
  aarch64", 2026-09-25: `data:` and remote SVGs alike). Blitz renders
  SVGs with usvg and its own `fontdb` (`FONT_DB` in `blitz-dom`'s
  `util.rs`: `load_system_fonts()` only), whose generic families are
  fontdb's defaults (`monospace` = Courier New, `sans-serif` = Arial,
  `serif` = Times New Roman). Without those fonts (as on the desktop)
  a `font-family` list that ends in a generic family matches nothing,
  and usvg leaves the text out ("No match for … font-family"). A fix
  belongs in Blitz: set the generic families from the system's
  (fontconfig, or Parley's font collection, which knows them already).
  Report upstream first; until then nothing in ren can reach
  `FONT_DB`.
- **Servo drops the text of one SVG on the same page** (in a tab):
  the remote diagram (`vector-widths-…svg` from imgix, light and dark
  versions) shows its boxes without any text, while the two `data:`
  SVGs render with theirs. All three use the same `font-family` list;
  usvg 0.48 alone renders the text of all three (so not the `&apos;`
  in the remote file's attribute, nor the system's fonts). Servo
  resolves SVG fonts through its own font system (`select_font` in
  `servo-net`'s `image_cache.rs`, an empty `fontdb` filled on demand),
  so the difference is likely there. Opened on its own ("Open Image in
  New Tab") the same SVG (the dark version, which the page shows in
  dark mode) renders with its text, so it depends on the
  page around it: the `<picture>` with a `prefers-color-scheme` source,
  the size it's drawn at, or the timing among the page's other images
  (the page has no web fonts). Check with servoshell 0.7 and report to
  Servo.
- **`srcset` and `<picture>`** in the article view, to fetch the smaller
  image a page offers (0.7 % and 0.2 % of the articles in S3; 0010).
- **AVIF** in the article view (13 articles in S3): the `image` crate's
  decoder needs dav1d (0010).

## Conventions

- Keep semantically different changes in separate commits.
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` must pass
  before each commit, and `cargo test` too once tests exist (mandatory from
  phase 2 on).
- CI (`.github/workflows/`) is split by when the checks run, and its jobs
  run the `justfile`'s recipes, so `just check` runs what CI runs (Servo's
  helper: `just check-servo`):
  - `general.yml`, on every push: REUSE, typos, EditorConfig,
    shellcheck, zizmor (the workflows), the `.desktop` file and the
    AppStream metadata.
  - `dependencies.yml`, on every push and weekly: both workspaces'
    licences and RustSec advisories (cargo-deny, policy in `deny.toml`),
    unused dependencies (cargo-machete), the third-party notices.
  - `code-tests.yml`, on every push and weekly: `cargo fmt`, clippy and
    the tests without Blitz and Servo, the Secret Service test.
  - `full-tests.yml`, for pull requests, `master`, weekly and on demand:
    clippy and the tests with all default features (under cargo-nextest,
    with coverage, M9; both uploaded to Codecov), Servo's helper, both workspaces with their `rust-version`, the
    end-to-end test against Nextcloud.
  - `performance.yml`, for pull requests and `master`: the measurements
    that are steady on GitHub's runners (`just measure-ci`), uploaded to
    Bencher by `performance-upload.yml`; `performance-closed.yml`
    archives a closed pull request's branch there (0019).

  Keep it green. Default CI builds must stay fast: the builds with the
  engines are only in `full-tests.yml` and `performance.yml`. So that it also checks work on
  feature branches, each milestone is developed on a branch with a draft
  pull request, opened with the first push and marked ready when the
  milestone is done. A new check gets a recipe of its own, which CI runs.
  Actions are pinned by commit (dependabot updates them weekly), jobs ask
  for their permissions, Rust jobs set up with `.github/actions/setup-rust`
  (build caches are saved on `master` only), and zizmor must pass. If
  `-D warnings` gets in the way of rough spike code, it may be relaxed for
  phase 1 only.
- Task automation, when needed, uses [`just`](https://just.systems/) with a
  `justfile` at the repository root (no Makefiles, `cargo xtask` or ad-hoc
  wrapper scripts). Cargo stays the build system; `just` recipes only wrap
  multi-step tasks such as running measurements or the full CI checks
  locally. Scripts too long for a recipe live in `scripts/` and are invoked
  from one.
- Prefer small, well-maintained dependencies; justify every new heavy one
  (anything pulling an async runtime, a GPU stack, etc.) in the commit
  message.

## Licensing

The project is licensed under **GPL-3.0-or-later** (trivial files under
**CC0-1.0**) and follows the
[REUSE specification](https://reuse.software/).

- Every file carries SPDX headers in this form (comment syntax adapted to
  the file type; copy them from an existing file):

  ```
  # SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
  # SPDX-License-Identifier: GPL-3.0-or-later
  ```

  Trivial files (build and tool configuration such as `Cargo.toml`,
  `.gitignore`, `REUSE.toml`, CI workflows, and generated files like
  `Cargo.lock`) use `CC0-1.0` instead.
  Files that cannot carry a header (generated files, binary assets) are
  annotated in `REUSE.toml`. Third-party assets keep their original
  license; its text goes to `LICENSES/`.
- `reuse lint` must pass before each commit.
- Dependencies must be compatible with GPL-3.0-or-later. Acceptable:
  MIT, Apache-2.0 (also with LLVM-exception), BSD-2-Clause, BSD-3-Clause,
  ISC, Zlib, BSL-1.0, Unicode-3.0, CC0-1.0, MPL-2.0 (unless marked
  "Incompatible With Secondary Licenses"), LGPL-2.1-or-later, LGPL-3.0,
  GPL-3.0. Slint is used under its `GPL-3.0-only` option, so the combined
  binary is distributed under GPLv3. Not acceptable: GPL-2.0-only,
  proprietary or non-commercial licenses, and anything unclear.
  Exceptions are decided one crate at a time and listed in `deny.toml`:
  `webpki-roots` (CDLA-Permissive-2.0, Mozilla's CA certificates as data,
  pulled in by Servo; approved in the S4 review, its text ships with the
  third-party notices).
- With the first real dependency (spike S1), add a `deny.toml` for
  `cargo deny check licenses` encoding the list above, and run it whenever
  dependencies change.
