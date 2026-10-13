<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0015 — Test coverage (M9)

**Status:** done (2026-10-13), for review. Measured in the container and
in CI; nothing here needs the desktop.

## Context

Tests have been mandatory since phase 2, and the library crates were
written against fakes and a mock server from the start ("Designing for
testability" in the plan). What had no tests was the glue: the window
(`ui/`), the command line modes, the threads and downloads of the `ren`
crate, and nothing had run against a real Nextcloud since S2/M1, except
by hand on the desktop's server. The plan's M9 asks for the coverage
measured (`cargo llvm-cov`), the gaps closed, a few UI flow tests with
Slint's testing backend, and an end-to-end test against a Nextcloud +
News container.

## Coverage

Measured with cargo-llvm-cov 0.9, `just coverage` (all crates of the
workspace, without the rendering engines:
`--no-default-features --features renderer-software,servo`), lines:

| | before M9 | after M9 |
|---|---|---|
| nextcloud-news | 97.5 % of 853 | 97.9 % of 853 |
| ren-store | 98.5 % of 1,621 | 98.1 % of 1,621 |
| ren-sync | 94.0 % of 650 | 94.0 % of 650 |
| ren-settings | 92.9 % of 1,742 | 93.3 % of 1,742 |
| ren-tabs | 81.8 % of 407 | 95.9 % of 434 |
| ren, without `ui/` | 80.1 % of 6,091 | 89.7 % of 6,441 |
| ren, `ui/` | 2.9 % of 3,363 | 40.0 % of 3,688 |
| **all** | **67.6 % of 14,727** | **79.9 % of 15,429** |

The tests run in 3.2 s (328 tests, plus the two that need a service:
the Secret Service and the end-to-end test). Those two aren't in these
numbers: `ren-settings`'s `secret.rs` (69.7 %) is mostly the Secret
Service code that `just test-secret-service` tests.

With the article view (Blitz, all default features), the numbers come
from CI: the full job runs its tests under cargo-llvm-cov and shows the
table in the job's summary, so Blitz isn't built locally for them (see
"In CI" for the first numbers). Servo's helper (`crates/ren-servo`, a
workspace of its own) is tested in its CI job, but its coverage isn't
measured: that would be another build of Servo (12–24 minutes),
instrumented, for the helper's glue around it.

## What was added

### End to end against Nextcloud

`just test-nextcloud` (`scripts/test-nextcloud.sh`) starts Nextcloud with
SQLite in a container (docker or podman), installs a News release, and
runs `crates/ren-sync/tests/nextcloud.rs` against it, which:

- logs in with **Login Flow v2** as ren does (`nextcloud_news::LoginFlow`),
  doing the browser's part itself: it opens the login URL, follows the
  redirects with the session's cookies and grants the flow through the
  login page's "app password" form (Nextcloud 32 has the state token in
  the form, 33 draws the page with scripts and has it in a base64
  initial state; the request token is on the page in both);
- serves its own feeds (the mock server of `nextcloud-news`'s tests, on
  all addresses, which the container reaches as `host.docker.internal`;
  `allow_local_remote_servers` lets Nextcloud fetch from there) and adds
  them through the API: a folder with one feed, one feed outside;
- runs the initial sync in pages of two items, local changes (read,
  starred, the feeds outside of folders marked read, which goes feed by
  feed), changes on the server as another client makes them (read state,
  a star, a new item fetched with the admin's `feeds/update`), a resync,
  a folder marked read and a feed deleted on the server;
- and logs out: the app password is revoked, the next request is
  refused (401), and revoking again too.

It checks the store after each step and the server's state after the
push. Titles and authors with line breaks come back collapsed, a body's
`<script>` is gone (the server sanitises bodies, as the API notes say).
The test refuses an account that has feeds or folders of its own, since
it deletes them.

The script makes an **app password per run** through the OCS API
(`core/getapppassword` with the admin's password): the login flow
granted with an app password hands back that same password (granted
after a login with the account's password, it would make a new one), so
the test's log out revokes it. `occ user:auth-tokens:add` fails on
Nextcloud 33 (it reads a `--login-name` option it doesn't define). The
container and its volume (Nextcloud's `/var/www/html`, about 1 GB) are
removed afterwards; `--keep` keeps them for the next run, which saves
Nextcloud's install (a run with a new container took 25 s here, 4 s
with a kept one).

CI runs it for the release on the desktop's server and the newest:
**Nextcloud 32 with News 28.7.0** and **Nextcloud 33 with News 29.0.0**
(News 29 needs Nextcloud 33 to 35), for pull requests, master and
weekly, like the full job: it downloads Nextcloud's image and News's
release. Both pass, the script in 42–52 s per job in CI (with the
build of ren-sync's test), the test itself in about 4 s. In the container, without Docker Hub's rate limit and GitHub's
release downloads, the same images came from `mirror.gcr.io` and News
was built from its tag (`NEXTCLOUD_IMAGE`, `NEWS_APP`).

Nothing in the client, the sync or the login needed a change for either
version.

### UI flow tests

`crates/ren/src/ui/flows.rs` makes the main window over a small
database (a folder with one feed, a feed outside, four items) on
**Slint's testing backend** (`i-slint-backend-testing`, at Slint's exact
version, a dev-dependency already in the lock file), without a display,
and clicks and types at it like a reader:

- reading: a click on a row shows the article and marks it read, j/k
  and the arrow keys move, s stars, m toggles read, and the changes are
  queued for the sync;
- the feed tree: clicks on a feed, a folder and Starred, the keyboard
  (down, left closes a folder, right opens it);
- typing in the search field, and backspace;
- sorting by a click on the header (the list above the article; beside
  it, the rows have no header);
- Mark Feed as Read and Mark All Feeds as Read, the status message and
  its timeout (mock time);
- the settings dialog: the arrangement chosen by its picture, written to
  the file and applied; a number out of range kept with its error and
  the file untouched; Reset to Default; closing the dialog drops it;
- About ren opened and closed.

The window is made by `build_window` (split from `run_window`, which
shows it and runs the event loop). Articles are plain text, so the flows
are the same with and without `html-view`. The testing backend runs
without an event loop: timers fire when the test advances mock time;
`invoke_from_event_loop` fails, so nothing in the flows waits for a
thread (no syncs, no login).

Two things were needed:

- The element lookup (`ElementHandle`, by type name such as
  `ItemTableRow`) needs Slint's **debug info** in the generated code;
  `build.rs` asks for it in debug builds (`cargo build`, the tests), not
  in release builds. Ren's Slint files have no accessibility labels,
  which the lookup could use instead.
- The testing backend's `internal` feature, which has `mock_elapsed_time`
  and the keyboard helpers, embeds fonts from Slint's repository that
  aren't in the published crate, so it doesn't build from crates.io;
  `mock_elapsed_time` is public in its `testing_backend` module, and keys
  go through `Window::dispatch_event`.

The flows found a bug: **Feed → Mark Feed as Read** (Ctrl+R, and its
tool bar button) panicked with "RefCell already borrowed", since M8's
feed icons (2fc4ec9): the selection was passed while the reader was
still borrowed. Fixed (0c56380); the feed tree's menu was fine.

### The gaps in the `ren` crate

With the mock server and the recorded responses (`testing.rs`, shared
by the modules' tests, with temporary directories and a settings file
whose `password-command` gives the app password):

- the command line modes: `--check`, `--fetch-unread`, `--dump-items`
  (and its refusal of a directory with files), `--check-writes` against
  a server with one item (also one that ignores a request: the check
  fails and the item is restored), `--sync` and `--resync`;
- the sync thread: the password fetched there and handed on, an initial
  and an incremental sync, cancelling, another account's database, a
  revoked password, a failing `password-command`;
- the downloads: feed icons (scaled, failures marked), filter lists
  with includes (a failing include keeps the old list, too many
  includes, not UTF-8), `http::fetch`'s limit;
- `ren-tabs`'s frame statistics (made testable without
  `REN_TAB_STATS`).

**ureq refuses a body of exactly the limit's size**, one byte less than
the limit says; ren's limits (512 KiB for icons, 16 MiB for filter
lists, 20 MiB for images) are far from anything fetched, so it stays.

## Not covered, and why

- **Web page tabs** (`ui/pages.rs`, `tabs/helper.rs`, 5–19 %): they need
  the Servo helper, a 12–24 minute build; the protocol (`ren-tabs`) and
  the tab list are tested, and the helper's own tests run in its CI job.
  A fake helper (a small program speaking the protocol) would cover the
  window's side; not done.
- **The account page's login and log out in the window**
  (`ui/account.rs`, 21 %): `login::start` connects to the real Secret
  Service, and a test on a desktop must never write into the user's
  keyring. The logic is tested in `login.rs` with the in-memory store,
  the protocol in the end-to-end test. Covering the window's side needs
  the store passed in (a seam in `App`); not done.
- **The window's geometry** (`ui/geometry.rs`, 5 %): winit only; the
  testing backend has no winit window.
- **The tray**, measurement modes (`--measure-*`, `--autoscroll`,
  `--cycle-articles`), `main.rs`, `--set-password` and
  `--check-secret-service`: they need D-Bus, a display or a terminal, and
  the measurements are checked by running them.

No coverage threshold fails CI: the numbers move with the UI code, and
the review sees them in the full job's summary.

## In CI

The first full job with coverage (all default features): see the pull
request's job summary.

## Open

- A fake Servo helper for the window's side of the tabs.
- The Secret Service passed into the window, for flow tests of the
  account page.
- Accessibility labels in ren's Slint files would help screen readers,
  and the tests could look elements up by them instead of by type.
