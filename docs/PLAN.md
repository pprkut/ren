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
| Full-page tabs | **Servo** (`servo` crate)            | Opening the original page of an article in an in-app tab. Created on demand, torn down when the last tab closes. |
| HTTP           | **ureq 3** (blocking, rustls)        | No async runtime needed for a single background sync thread; small dependency tree. |
| JSON           | serde / serde_json                   | Streaming deserialisation from the response reader. |
| Storage        | **SQLite** via rusqlite (`bundled`)  | Paged queries for the list views, so only visible rows are ever in memory. |
| Credentials    | Secret Service via `keyring` crate    | No plaintext passwords in the config file. Nextcloud app passwords (login flow v2) preferred over the account password. |
| Config         | TOML in `$XDG_CONFIG_HOME/ren/`       | |

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
   can be shown offline and loaded off the UI thread. Link clicks go to the
   system browser, or to a Servo tab with a modifier.
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
  to the system browser.

Open questions for the prototypes (spikes S3 and S4):

- **Renderer interplay.** Blitz's CPU path works with any Slint renderer,
  but Servo's zero-copy path needs Slint's wgpu-based renderer, which costs
  idle memory even when no tab is open. Alternative: Servo renders into a
  software/offscreen context and frames are read back into a Slint image
  (a GPU→CPU copy per frame; acceptable for reading, less smooth scrolling).
  Measure both and pick; possibly make it a setting.
- **Dependency overlap.** Blitz and Servo both depend on `stylo` and related
  crates. Check that the versions can be unified; two copies would inflate
  the binary and compile time (not RSS, as unused code pages are not loaded).

## Architecture

```
crates/
  nextcloud-news/   API v1-3 client: types + blocking HTTP client. No UI, no DB.
  ren-store/        SQLite schema, migrations, queries, pending-change queue.
  ren-sync/         Sync engine: pushes local changes, pulls remote changes.
  ren/              The application binary (Slint UI, Blitz article view,
                    Servo tabs, glue).
```

During phase 1 only `nextcloud-news` and `ren` exist; `ren-store` and
`ren-sync` are added in phase 2.

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
    items with lower ids (paging).
- `GET /items/updated?lastModified=<unix seconds>&type=3&id=0` — items with
  `lastModified >=` the given value, including read/star state changes.
  No paging on this endpoint, so the response size limit must be generous.
- `POST /items/{read,unread,star,unstar}/multiple` with `{"itemIds": [...]}`
- `POST /items/read`, `/feeds/{id}/read`, `/folders/{id}/read` with
  `{"newestItemId": n}` for "mark all read".
- Item `lastModified` is an int in **seconds**. `title`, `author`, `url`,
  enclosure and media fields are **not sanitised**. `body` is.
- Treat unknown fields as ignorable and most fields as nullable.

### Sync algorithm

Initial sync:
1. `GET /folders`, `GET /feeds`.
2. Page unread items (`type=3, getRead=false, batchSize=200, offset=<lowest id>`)
   until a page comes back short; then starred items the same way.
   Each page is written in one transaction.
3. Store `max(lastModified)` as the sync cursor.

Incremental sync:
1. Push the pending-change queue (read/unread/star/unstar, batched per
   action). Remove entries only after a 2xx response.
2. `GET /folders`, `GET /feeds` — upsert, delete what disappeared.
3. `GET /items/updated?lastModified=<cursor>` — upsert, but do not overwrite
   read/starred state for items that still have a pending local change.
4. Advance the cursor to the max `lastModified` seen. Re-fetching items
   from the same second is harmless because upserts are idempotent.
5. Purge locally: read, unstarred items older than N days.

Resource rules for the sync thread:
- Lower its priority (`nice`) and never run it on the UI thread.
- Deserialise directly from the response body reader.
- Don't pre-process article bodies during sync; sanitise/strip lazily when
  displayed (and cache a short plain-text excerpt only if the list needs it).
- Timer-based (default 15 min) plus manual refresh; skip if one is running.

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
- **S1b — UI capabilities.** Before building on Slint, check that it can
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
    choice is M8.
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
- **S2 — Talk to Nextcloud.** Minimal `nextcloud-news` types and client
  (version, folders, feeds, paged items, updated items). `ren --check`
  prints server version, folder/feed/unread counts; `ren --dump-items <dir>`
  stores raw responses outside the repo for later spikes and fixtures.
  *Done when:* works against the real server; time, peak RSS and CPU of
  fetching all unread items are recorded for a few batch sizes.
  *Decides:* ureq + serde approach, batch size.
- **S3 — Article view with Blitz.** Render the items dumped in S2 into the
  article pane of the S1 window: scrolling, link clicks (just logged),
  text selection with copy to the clipboard (Blitz's clipboard hook,
  implemented e.g. with `arboard`, since Slint has no public clipboard API),
  mouse cursor changes over links and text, images, light/dark stylesheet.
  Use the released Blitz version; selection support is recent, so note
  if it needs a newer, unreleased one.
  *Done when:* selecting and copying text and clicking links work on a
  varied set of real items, and RSS delta, render time per item and CSS
  coverage are recorded.
  *Decides:* Blitz vs. litehtml vs. native rich text.
- **S4 — Full page in a Servo tab.** A button opens an item's URL in a tab;
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

- **M1 — API client.** Harden the S2 code: all endpoints listed above
  (including the `*/multiple` and mark-all-read writes), typed errors (auth,
  HTTP status, network, decode), user agent, timeouts. Tests: fixture
  deserialisation (nulls, unknown fields), request building, a local mock
  HTTP server.
- **M2 — Store.** `ren-store`: schema and migrations (folders, feeds, items,
  pending_changes, sync_state, per-feed settings), indices for the list
  queries, upserts, paged queries, purge, and a "new" marker for items
  that arrived in the latest sync (cleared when the next sync starts), for
  the item list colours. Tests on an in-memory database.
- **M3 — Sync engine.** `ren-sync`: initial and incremental sync and the
  pending-change queue as described above. Unit tests with a fake `NewsApi`
  (including conflicts: local pending change vs. remote update), one
  integration test with the real client against the mock server.
- **M4 — Config & credentials.** Config file, keyring storage, account
  setup data model. Tests for config parsing and defaults.

### Phase 3 — The application

- **M5 — UI wired to real data.** The S1 window on top of store and sync:
  feed tree with unread counts, item list, plain-text article pane, mark read
  on open, star toggle, manual refresh, sync status, background sync thread
  (`slint::invoke_from_event_loop`). View models unit-tested.
- **M6 — Article view.** The S3 result made production-ready behind the
  `html-view` feature, including the image cache.
- **M7 — Full-page tabs.** The S4 result made production-ready behind the
  `servo` feature: tab bar, "Open page", on-demand lifecycle, per-feed "open
  full page instead of article" setting.
- **M8 — Polish.** Keyboard navigation (j/k, s, m, o), mark-all-read,
  periodic sync, purge settings, favicons (cached on disk), first-run
  account setup UI, settings for the pane arrangement (list beside or above
  the article) and the item state colours, persisted column widths and
  sort order.

### Phase 4 — Hardening

- **M9 — Test coverage.** Measure coverage (`cargo llvm-cov`), close gaps,
  add a few UI flow tests with Slint's testing backend, and an end-to-end
  test against a Nextcloud + News container.
- **M10 — Performance.** Re-run the measurement scripts: idle, reading, with
  tabs open and after closing them, during initial and incremental sync of a
  large account. Profile and fix hotspots.

## Conventions

- Keep semantically different changes in separate commits.
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` must pass
  before each commit, and `cargo test` too once tests exist (mandatory from
  phase 2 on).
- CI (`.github/workflows/ci.yml`) runs `reuse lint`, `cargo fmt --check`,
  `cargo clippy -D warnings` and `cargo test` on every push and pull request.
  Keep it green. Default CI builds must stay fast: once Blitz and Servo
  arrive (S3/S4), the default job builds without the `html-view`/`servo`
  features and full builds get a separate, less frequent job. If
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
- With the first real dependency (spike S1), add a `deny.toml` for
  `cargo deny check licenses` encoding the list above, and run it whenever
  dependencies change.
