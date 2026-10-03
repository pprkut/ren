<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0003 — Nextcloud News client: ureq + serde (spike S2)

**Status:** draft; waiting for measurements against the real server.

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
  for the fetch and peak RSS. `--batch-size <n|all>` (default 200).
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

To be run against the real server:

```sh
ren --check
ren --dump-items ~/ren-dump        # outside the repository
just measure-fetch                 # or: scripts/measure-fetch.sh 50 200 1000 all
```

*Results: pending.*

## Decision

*Pending the measurements:* ureq + serde approach, batch size.

## Consequences

*Pending.*
