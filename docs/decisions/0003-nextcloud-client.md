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
  - Whether the server sends `lastModified` as a number or a string; both
    are accepted. The dump answers it, and M1's fixtures should cover the
    form the server uses.
  - The write endpoints (`*/multiple`, mark all read) and which of the
    documented variants the server accepts: M1.
  - Timeouts (60 s until the response headers) are far above the 1.5–2 s
    per request seen here, but a slower server or a much larger account
    could need more; make them settings if that happens.
