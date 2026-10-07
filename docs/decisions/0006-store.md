<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0006 — Local store: SQLite through rusqlite (M2)

**Status:** done (2026-10-07), for review.

## Context

`docs/PLAN.md` keeps everything the UI shows in a local SQLite database
(rusqlite, `bundled`), queried page by page so that only the visible rows
are in memory. M2 builds the `ren-store` crate: schema and migrations,
writing what the server sends, the list queries, local changes with
their queue for the server, the "new" marker and purging.

## What was built

`ren-store` depends on `nextcloud-news` for its types (the sync hands
decoded items straight to the store) and on rusqlite. A `Store` is one
connection; the UI and the sync thread each open their own.

- **Opening:** WAL journal (the UI reads while the sync writes),
  `synchronous = NORMAL` (safe with WAL; a power loss can lose the last
  transactions, which the next sync fetches again), foreign keys on, 5 s
  busy timeout. `Store::open_in_memory` for tests.
- **Migrations:** `PRAGMA user_version` counts the migrations applied;
  each runs in a transaction with its version, and a database from a
  newer ren is refused (`Error::TooNew`) instead of being misread.
- **Schema** (version 1):
  - `folders`, `feeds` with the server's fields; a `folderId` of 0 is
    stored as `NULL`.
  - `items` with the fields lists and the article view need, and the
    `filtered` flag; `item_contents` with the body and media description.
  - `feed_settings` (for now `open_full_page`, M7), deleted with the feed.
  - `pending_changes`: one row per item and field (read state, starred
    state) with the latest local value.
  - `sync_state`: key–value; for now the sync cursor.
  - Indices: items by date, by feed and date, unread per feed (partial,
    for the counts), starred by date (partial), new (partial).
- **Writing:** `replace_folders` and `replace_feeds` (upsert, delete what
  the server no longer has; a removed feed takes its items, settings and
  pending changes along) and `upsert_items` (one transaction per call,
  for the sync's chunks). Folder names, feed titles, item titles and
  authors are stored with whitespace collapsed.
- **Reading:** folders and feeds sorted by name, unread counts per feed,
  the starred count, one item with its body, and lists: a `Selection`
  (all, starred, folder, feed), unread only, a search in title and author,
  sorted by title, feed, author or date in either direction (ties newest
  first), as a count plus pages (`item_count`, `item_page`), or as all
  ids in order plus rows by id (`item_ids`, `item_summaries`), and the
  position of an item in a list (`item_position`, for keeping the
  selection). Filtered items and items of unknown feeds are not listed or
  counted.
- **Local changes:** `set_unread` and `set_starred` change the items and
  queue the change in one transaction; `pending(action, limit)` and
  `remove_pending(action, ids)` are what M3's push needs (batches of up to
  1,000 ids, removed after a 2xx).
- **New marker:** `upsert_items(items, mark_new)` marks items it hadn't
  stored before (the initial sync passes `false`: everything would be
  new); `clear_new` clears the marker when a sync starts. An unread item
  with the marker has `Status::New`.
- **Purge:** `purge(before)` deletes read, unstarred items without a
  pending change whose `lastModified` is older (the server updates it
  when an item is read, so this is the time since reading; without one,
  the publication date). The caller computes `before` from its clock.
- `just measure-store [ITEMS]` (`examples/measure.rs`) fills a database
  with a synthetic account and times the operations.

### Choices and findings

- **rusqlite is pinned to 0.38.** Servo 0.5 (`servo-storage`) depends on
  rusqlite 0.38 with the bundled SQLite, and only one crate in a build may
  link `sqlite3` (`libsqlite3-sys` 0.36). Servo 0.6 and 0.7 are still on
  0.38, so the pin doesn't hold back the Servo upgrade planned for M7;
  if a later Servo moves, ren-store moves with it. The SQLite is the same
  one Servo builds, so the full build compiles it only once.
- **No foreign key from items to feeds.** The server can send an item
  before its feed (a feed added between `GET /feeds` and
  `GET /items/updated`); with a foreign key the item would be rejected,
  and the cursor would move past it, so it would be lost until a full
  resync. Such items are stored and only listed once their feed is known.
- **Bodies in a table of their own.** With a body of a few KiB in the item
  row, each row would take most of a 4 KiB page, and every list scan and
  count would read all bodies (150 MiB at 63,000 items) along with the
  rows.
- **The server's state doesn't overwrite pending local changes.** The
  upsert keeps the read or starred state of an item that has a pending
  change of that state. This is the plan's conflict rule for M3, but it
  belongs to the write itself, so it is here and tested here.
- **Case-insensitive sorting and searching beyond ASCII.** SQLite's
  `NOCASE` and `LIKE` only fold ASCII ("Über" and "über" sort apart). Items
  store the lower case of title and author (`title_key`, `author_key`,
  about 8 MiB for 63,000 items) and are sorted and searched by those as
  plain bytes. A collation calling into Rust took 204 ms to sort all ids
  by title, the keys 55–65 ms. Folders and feeds use such a collation
  (`caseless`), being few. The order is by code point of the lower case,
  not by a language's rules ("Äpfel" after "Zebra").
- **Sorting by feed** ranks the feeds by title in a subquery and sorts
  the items by that number (135 → 95 ms for a page from the middle,
  in the same run).
- **Positions are counted**, as the rows sorting before the item, in one
  pass over the list; numbering the sorted list with `row_number()` took
  290 ms by title, counting about 20 ms.
- **Two ways of showing a list.** Pages with `LIMIT`/`OFFSET` are cheap
  sorted by date (the index gives the order) and for the first page in any
  order, but every page from the middle of a list sorted otherwise sorts
  the whole list again. Fetching all ids in order once (8 bytes per item,
  0.5 MiB for 63,000) and then the shown rows by id (0.1–0.2 ms per 50
  rows) makes scrolling cost nothing. M5 decides; the ids are probably the
  better fit for the item table, which already works that way on the
  dummy data.

## Measurements

`just measure-store` at 3799c1a in a cloud container (4 vCPUs, Xeon at
2.1 GHz, shared, so times vary by up to a third between runs; not the
user's machine): a synthetic account like the one in S2, 63,000 items in
49 feeds and 11 folders, 153.5 MiB of bodies (2.4 KiB per item on
average), unevenly spread over the feeds; release build, database on
disk; median of 5 runs per query, with a second connection as the UI's.

| step | time ms | notes |
|---|---|---|
| initial sync, 63 chunks of 1000 | 2137.25 | CPU 1510.00 ms; 34 µs per item |
| unread counts per feed | 2.95 | median of 5 |
| count, all items | 21.73 | median of 5 |
| page of 50, all, by date, first | 0.06 | median of 5 |
| page of 50, all, by date, middle | 14.39 | median of 5 |
| page of 50, all, by title, first | 21.95 | median of 5 |
| page of 50, all, by feed, middle | 138.09 | median of 5 |
| page of 50, all unread, by date, middle | 19.53 | median of 5 |
| page of 50, largest feed (2556 items), by date | 0.09 | median of 5 |
| page of 50, a folder, by date | 0.76 | median of 5 |
| all ids, by date | 41.39 | median of 5 |
| all ids, by title | 64.55 | median of 5 |
| all ids, by feed | 63.39 | median of 5 |
| 50 rows by id | 0.17 | median of 5 |
| position of an item, all, by date | 12.92 | median of 5 |
| position of an item, all, by title | 21.13 | median of 5 |
| search, count (258 found) | 26.60 | median of 5 |
| one item with body | 0.01 | median of 5 |
| incremental sync, 1200 items | 101.96 | 200 new |
| clear the new marker | 0.70 | 200 items had it |
| mark 1000 items read | 54.33 | 1000 changed |
| take and clear the queue | 1.79 | |
| purge | 231.50 | 20980 items deleted |

- **Database:** 229.0 MiB plus 7.7 MiB write-ahead log after the initial
  sync, for 153.5 MiB of bodies; 230.7 MiB after purging a third of the
  items (the free pages are reused, see "Open").
- **Memory:** 2.6 MiB at the start, 11.1 MiB after the initial sync,
  14.9 MiB at the peak, including the synthetic data being generated.
  SQLite's page cache is the default 2 MiB per connection.
- **Writing is not the bottleneck:** 34 µs per item, 2.1 s and 1.5 s of
  CPU for the whole initial sync, next to about 2 minutes of waiting for
  the server (0003). One chunk of 1,000 items takes about 35 ms, so the
  UI's writes (marking read) wait at most that long for the sync's
  transaction.
- **Lists:** the first page in any order and every page sorted by date
  take at most about 20 ms; sorting all ids takes 40–65 ms, after which
  50 rows by id take 0.2 ms. A page from the middle of the whole list
  sorted by feed is the slow case at about 100–140 ms.

Re-run with `just measure-store [ITEMS]` on the target machine.

## Review (2026-10-07)

- **Write transactions take the write lock when they start**
  (`BEGIN IMMEDIATE`). `replace_feeds` began a deferred transaction with a
  read; if the UI wrote before its first write, the sync's write failed at
  once with "database is locked": SQLite doesn't wait for the busy
  timeout when a transaction's snapshot is stale. Reproduced with SQLite
  directly, fixed and tested with two connections. Migrations take the
  lock too and read the schema version inside their transaction.
- **Opening a new database from two connections at once** failed for one
  of them (19 of 40 tries): switching to the write-ahead log doesn't wait
  for the busy timeout either. The switch is retried for up to the busy
  timeout, and the busy timeout is set first.
- `just measure-store` on the user's machine (i7-1185G7): initial sync
  1.6 s (25 µs per item), pages up to 20 ms, the slowest a page from the
  middle sorted by feed at 102 ms, 50 rows by id 0.1 ms, marking 1,000
  items read 26 ms, purge 196 ms; peak RSS 15.8 MiB.

## Open

- **Purge age and when to purge** are M8's settings; `purge` takes the
  cut-off. The database file doesn't shrink after a purge: SQLite reuses
  the free pages for new items. A `VACUUM` (manual, or rare) could give
  the space back, if that matters.
- **Mark all read** (by `newestItemId` per feed, folder or everything)
  isn't queued yet: M3 or M8 decide whether it goes into the queue as
  such or as the item ids it marks.
- **More sync state** (e.g. resuming an interrupted initial sync) goes
  into `sync_state` with M3, as it needs it.
- **Locale-aware sorting** ("Ä" with "A") would need ICU or a collation
  crate; not planned.
- The write-ahead log stays at its peak size on disk after the initial
  sync (about 8 MiB); harmless, `journal_size_limit` could cap it.
- Counting all items and fetching all ids join every item with its feed
  (to hide unknown feeds), about 20 and 40 ms at 63,000 items; fine for
  a selection change, to revisit if it shows in M5.
