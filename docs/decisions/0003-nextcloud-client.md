<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0003 — Nextcloud News client: ureq + serde (spike S2)

**Status:** accepted (2026-10-03).

## Context

`docs/PLAN.md` picks ureq 3 (blocking) and serde_json, deserialising
straight from the response body, so that syncing needs no async runtime
and stays small. Spike S2 checks this against the real server and finds a
batch size for paging through items.

## What was tried

**`nextcloud-news` crate** (no UI, no storage):

- `types`: the responses of `/version`, `/status`, `/folders`, `/feeds`,
  `/items` and `/items/updated`. Unknown fields are ignored and almost
  everything is optional.
- `Endpoint`: path and query parameters of each read request, separate
  from the HTTP code, so request building is unit-tested without a server.
- `Pager`: paging through `GET /items`; each page starts below the lowest
  id of the previous one, until a page comes back short. The caller fetches
  and stores each page before asking for the next, so memory stays bounded
  by the batch size.
- `Client`: a ureq `Agent` with basic auth, a user agent, timeouts
  (connect 15 s, response headers 60 s, body 300 s) and a 512 MiB limit
  per response body. Responses are decoded while they are received
  (`serde_json::from_reader` over a `BufReader`). Errors are an enum
  (unauthorised, HTTP status, transport, decode, I/O).

**In `ren`:**

- `--check` prints the News app version, status warnings, and the folder,
  feed, unread and starred counts.
- `--fetch-unread` pages through all unread items without storing them and
  prints requests, items, JSON and body sizes, wall time, its own CPU time
  for the fetch and peak RSS. `--batch-size <n|all>` (default 200 during
  the measurements, 1000 now).
- `--dump-items <dir>` stores the raw responses: version, status, folders,
  feeds, unread and starred item pages, and `/items/updated` for the day
  before the newest change. It refuses directories inside the source tree
  and non-empty ones, so real feed data doesn't end up in the repository.
- The account comes from `[account]` (`server`, `user`,
  `password-command`) in `$XDG_CONFIG_HOME/ren/settings.toml`, the app
  password from `$REN_APP_PASSWORD` or `password-command`. Only `https://`
  server URLs are accepted (plain HTTP only for localhost), since basic
  auth sends the password with every request.
- `scripts/measure-fetch.sh` (`just measure-fetch`) runs `--fetch-unread`
  for several batch sizes, interleaved, after a warm-up run, and prints a
  Markdown table of medians.

### Findings that don't need the server

- **Dependencies.** ureq with `rustls` and the `ring` crypto provider, and
  `rustls-platform-verifier`, which checks certificates against the
  system's trust store, so self-hosted servers with their own CA work as in
  the browser. ureq's default `rustls` feature would bring `webpki-roots`,
  whose CDLA-Permissive-2.0 license is not on our list, and Mozilla's
  roots instead of the system's. `ring` instead of `aws-lc-rs` avoids a
  CMake build and OpenSSL-licensed code. The whole client tree is about 45
  crates, with no async runtime. `gzip` is on, so the server can compress
  responses.
- ureq honours `HTTPS_PROXY`/`ALL_PROXY` from the environment by default.
- **The API documentation is inconsistent in places**, so the code is
  lenient where it matters:
  - Item `lastModified` is listed as `string|null`, but the examples show
    a number. Both are accepted (also `updatedDate`). The dump shows what
    the server sends.
  - The `offset` of `GET /items` is described as "lower than equal that
    id". If it were inclusive, paging by the lowest id would return the
    last item again on every page and never end on a full page. The pager
    drops items at or above the offset and stops on a page without new
    items; `--fetch-unread` reports such items as `repeated`.
  - The "How To Sync" section shows `PUT /items/read/multiple` with
    `{"items": [...]}` and `/items/starred/multiple`, while the endpoint
    reference (and the plan) say `POST /items/read/multiple`,
    `/items/star/multiple` with `{"itemIds": [...]}`. To verify in M1.
- Tested against a local mock server: all three commands, a wrong password
  (HTTP 401), an inclusive-offset server, and the dump guard. TLS was
  checked against a public host through the system trust store.

## Measurements

Against the real server (News app 28.7.0; 11 folders, 49 feeds of which 15
have update errors, 62,811 unread and 20 starred items, newest item id
227,491):

- `--check` and `--dump-items` work. The dump with the default batch size
  of 200 took 315 pages of unread items, 1 of starred items, and 115 items
  in `/items/updated` for the last day.
- A first `just measure-fetch` with batch sizes 50, 200, 1000 and all:
  50 was very slow (1,257 requests), and **`batchSize=-1` (all at once)
  fails with HTTP 500**: the server can't build a response with 62,811
  items. The script now continues after a failing batch size and its
  defaults are 200, 500, 1000 and 2000.

`just measure-fetch` at 99acda3 (i7-1185G7, Linux 7.2.8; 62,817 unread
items; median of 3 runs after a warm-up; CPU is ren's own user + system
time while fetching):

| batch size | requests | items | repeated | JSON MiB | bodies MiB | wall s | ms per request | CPU s (user + sys) | CPU % of wall | RSS before MiB | peak RSS MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 200 | 315 | 62817 | 0 | 215.4 | 147.0 | 501.996 | 1594 | 1.48 | 0 | 8.3 | 14.2 |
| 500 | 126 | 62817 | 0 | 215.4 | 147.0 | 212.448 | 1686 | 1.28 | 1 | 8.4 | 17.0 |
| 1000 | 63 | 62817 | 0 | 215.4 | 147.0 | 116.078 | 1843 | 1.17 | 1 | 8.4 | 21.1 |
| 2000 | 32 | 62817 | 0 | 215.4 | 147.0 | 68.643 | 2145 | 1.16 | 2 | 8.3 | 29.9 |

### Reading the numbers

- **The server's fixed cost per request dominates.** The time per request
  fits 1.53 s + 0.31 ms per item almost exactly for all four batch sizes.
  That points to the News app doing most of its work per request,
  independent of the page size (presumably selecting and sorting all
  62,817 unread items before applying the limit and offset), and it means
  the bytes on the wire are not the bottleneck. The number of requests
  decides the time of an initial sync: 8.4 min at 200, 1.9 min at 1000,
  1.1 min at 2000.
- **The client costs next to nothing.** Decoding 215 MiB of JSON straight
  from the response takes 1.2–1.5 s of CPU, at most 2 % of the wall time,
  so the sync thread will not cause CPU spikes. The CPU time drops
  slightly with fewer requests (less per-request overhead).
- **Memory grows with the page.** Peak RSS above the 8.3 MiB baseline is
  5.9, 8.6, 12.7 and 21.6 MiB for 200, 500, 1000 and 2000 items: about
  3× the JSON of one page (3.5 KiB per item on average) plus a few MiB of
  buffers. Only one page is held at a time, so this doesn't grow with the
  account.
- **The offset is exclusive** on News 28.7.0: no item came back twice
  (`repeated` = 0), despite the ambiguous documentation. The pager's
  protection stays, as it costs nothing.

## Decision

- **ureq + serde: yes.** Blocking requests on one background thread,
  decoding straight from the response body, are fast and small enough; no
  async runtime is needed for syncing.
- **Batch size: 1000** for paging through `GET /items`. It cuts the
  initial sync of this account from 8.4 to 1.9 minutes compared to the
  plan's 200, for 7 MiB more peak memory. 2000 would save another 47 s but
  costs another 9 MiB at the peak, which may stay in the allocator after
  the sync, and a larger response per request for the server to build
  (which already fails for all 63k items at once). The `--fetch-unread`
  and `--dump-items` default is now 1000 as well.

## Consequences

- **`GET /items/updated` is not paged**, and the server already fails on
  one response of 63k items. An incremental sync after a long time offline,
  or after a mass change on the server (e.g. "mark all read" on a large
  account), can hit the same limit. M3 needs a fallback: if
  `/items/updated` fails or the cursor is too old, resynchronise state with
  paged `GET /items` instead.
- The News API documentation recommends `batchSize=-1` for the initial
  sync; at this account size that doesn't work, so ren always pages.
- The initial sync of a large account takes minutes, mostly waiting for
  the server. M3/M5 should store and show each page as it arrives, so the
  list fills up while the sync is still running, and report progress
  (the expected total is the sum of the feeds' unread counts).
- Open issues:
  - Whether RSS returns to the baseline after a sync, or the allocator
    keeps the freed pages (then `malloc_trim` after a sync, or a smaller
    batch size). To check in M3, when the pages go into the store.
    *M3: flat during the sync; on the real server a sync ends at about
    18 MiB whatever its size, a resync 2.5 MiB above that, of which
    `malloc_trim` gives back 2.2 MiB (`0007`).*
  - Whether the server sends `lastModified` as a number or a string; both
    are accepted. The dump answers it, and M1's fixtures should cover the
    form the server uses. *M1: a number (see below).*
  - The write endpoints (`*/multiple`, mark all read) and which of the
    documented variants the server accepts: M1. *M1: the `POST` forms
    (see below).*
  - Timeouts (60 s until the response headers) are far above the 1.5–2 s
    per request seen here, but a slower server or a much larger account
    could need more; make them settings if that happens.

## M1: the client hardened

**Status:** done (2026-10-04), checked against the real server.

What changed in `nextcloud-news`:

- **Write requests:** `Update::Items` (`POST
  /items/{read,unread,star,unstar}/multiple` with `{"itemIds": [...]}`)
  and `Update::MarkRead` (`POST /items/read`, `/feeds/{id}/read`,
  `/folders/{id}/read` with `{"newestItemId": n}`), sent by
  `Client::update`. An empty id list sends nothing.
- **Streaming decode:** `decode_items` decodes `{"items": [...]}` item by
  item and hands each one to a callback; `Client::items`,
  `Client::updated_items` and `Client::next_page` (one page through a
  `Pager`) use it. The callback's own error type stops the request and is
  returned unchanged, so the sync can stop on a store error. The `Pager`
  takes items one at a time (`start_page`, `accept`, `finish_page`); a
  page that is started again after a failed request forgets what it had
  accepted.
- **Typed errors:** `Unauthorized` (401), `Status { code, message }` with
  the News app's `{"message": ...}` if it sent one, `Network` (name
  resolution, connection, TLS, timeouts, a connection lost during the
  body, a body above the limit) and `Decode`. Before, a connection lost
  while the body was decoded came out as a decode error.
- **`Config`:** user agent, the three timeouts and the body limit, with
  the S2 values as defaults.
- **`Item::filtered`:** News 28.4 added keyword filters per feed; matching
  items are sent with `"filtered": true` and hidden by the web interface.
- Tests: the request building, the streaming decode (including that items
  are handed over before the body is complete), fixtures in the server's
  form, and the client against a mock HTTP server on localhost (a small
  `std::net` server in the tests, no new dependency): headers and
  credentials, paging with exclusive and inclusive offsets, every write
  request, 401, 404 with a message, 500 without one, invalid JSON,
  connection refused, response timeout, connection lost mid-body, the body
  limit, and a caller's error.
- `ren --fetch-unread` decodes item by item now; `ren --check-writes`
  checks the write requests against the server (see "Open").

### Findings from the News app's source

Read at `nextcloud/news` master (29.0.0-beta.1, 2026-10-04), since the
documentation is inconsistent:

- **The write endpoints of v1-3 are the `POST` forms with `itemIds`**
  (`appinfo/routes.php`, `ItemApiController::readMultipleByIds` etc.).
  The `PUT` forms with `items` are routed for v1-2 only. The server marks
  the items one by one and skips unknown ids; the controllers return
  nothing (an empty 200 response).
- **`lastModified` is a number**: `Item::toAPI()` sends
  `cropApiLastModified()`, which returns an `int` (seconds; the database
  keeps microseconds). `updatedDate` is always `null`. Strings stay
  accepted.
- **"Mark all read" with `newestItemId` 0 changes nothing:** all three
  select `items.id <= :maxItemId` first. `ren --check-writes` relies on
  that to try them without changing anything.
- **`/folders/0/read` fails** (HTTP 500 on 28.7.0, without a message):
  the controller turns folder 0 into `null` and passes it to
  `FolderServiceV2::read(string $userId, int $id, ...)`, which can't take
  `null` in PHP 8. So feeds outside of folders are marked as read feed by
  feed.
- **Errors** are `{"message": "..."}` with the status (404 for an unknown
  feed or folder, 422, 409). `GET /items` answers `getRead=false` on the
  starred list with HTTP 200 and a message instead of items; the decoder
  reports that message.

### Measurements

Peak memory when decoding one item list, decoded at once vs. item by item
(synthetic: 60,000 items, 197 MiB of JSON with 2.9 KiB bodies; release
build; `VmHWM` after decoding, 2.3 MiB before):

| decoding | peak RSS |
|---|---|
| at once (`decode::<Items>`) | 207 MiB |
| item by item (`decode_items`) | 2.3 MiB |

With real data S2 measured about 3× the JSON size for decoding at once
(more, shorter strings than this synthetic body).

Against the real server, `just measure-fetch 200 2000` at b1c0b58
(i7-1185G7, Linux 7.2.8; News 28.7.0, 62,849 unread items; median of 3
runs after a warm-up), next to S2's numbers, which decoded each page at
once:

| batch size | requests | wall s | CPU s (user + sys) | RSS before MiB | peak RSS MiB | S2 peak RSS MiB |
|---|---|---|---|---|---|---|
| 200 | 315 | 502.356 | 1.31 | 10.0 | 14.8 | 14.2 |
| 2000 | 32 | 68.695 | 1.14 | 10.1 | 14.9 | 29.9 |

The peak no longer depends on the batch size: 4.8 MiB above the start for
both, where S2 needed 5.9 and 21.6 MiB. Time and CPU are unchanged, so the
streaming decode costs nothing. (The start is 1.7 MiB higher than in S2;
the binary grew in between, with the S3/S4 code.)

### Checked against the real server

`ren --check-writes` on News 28.7.0: all four `*/multiple` requests
changed the item as expected and the state was restored; "mark all read"
up to item 0 was accepted for everything, a feed and a folder;
`/folders/0/read` failed with HTTP 500 (see above).

The anonymised fixtures from the S2 dump (`tests/fixtures/recorded/`,
News 28.7.0) have the same form as the hand-written ones: the same keys
in the same order, `lastModified` as a number, and `filtered` already
sent by 28.7.0.

### Open

Left for M3: the `NewsApi` trait (shaped by what the sync needs),
bounding the number of ids per `*/multiple` request (the server updates
them one by one), and whether RSS returns to the baseline after a sync.
*M3: done, see `0007`.*
