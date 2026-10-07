// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Measures the store with a synthetic account: writing an initial sync,
//! the size of the database, the list queries, an incremental sync, marking
//! items read and purging. Run with `just measure-store [ITEMS]`, which
//! puts the database into `target/measure` (on disk; the temporary
//! directory, the default without a second argument, may be in memory).
//!
//! The account looks like the one measured in S2: 49 feeds in 11 folders,
//! about 2.4 KiB of body per item, unevenly spread over the feeds.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nextcloud_news::types;
use ren_store::{ItemQuery, Selection, Sort, SortColumn, Store};

const FEEDS: u64 = 49;
const FOLDERS: u64 = 11;
const CHUNK: usize = 1000;
const PAGE: u64 = 50;
const RUNS: usize = 5;

const WORDS: &[&str] = &[
    "release",
    "kernel",
    "memory",
    "renderer",
    "update",
    "security",
    "browser",
    "compiler",
    "desktop",
    "network",
    "storage",
    "research",
    "climate",
    "election",
    "report",
    "study",
    "announces",
    "improves",
    "breaks",
    "fixes",
    "introduces",
    "removes",
    "faster",
    "smaller",
    "quantum",
    "planet",
    "graphics",
    "driver",
    "audio",
    "font",
    "layout",
    "engine",
    "Über",
    "Ärger",
    "toolkit",
    "community",
    "project",
    "native",
    "portable",
    "stable",
];

/// xorshift64*, so runs are comparable.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn words(&mut self, n: usize) -> String {
        (0..n)
            .map(|_| WORDS[self.below(WORDS.len() as u64) as usize])
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Newest item: 2026-09-01 00:00:00 UTC; one item every 10 minutes before.
const NEWEST: i64 = 1_788_220_800;

fn item(rng: &mut Rng, id: u64, count: u64) -> types::Item {
    // Lower feed ids publish more.
    let feed_id = rng.below(FEEDS).min(rng.below(FEEDS)) + 1;
    let age = (count - id) as i64 * 600;
    let mut body = String::from("<p>");
    let body_len = 500 + rng.below(4000) as usize;
    while body.len() < body_len {
        body.push_str(&rng.words(12));
        body.push_str(". ");
    }
    body.push_str("</p>");
    let title_words = 4 + rng.below(6) as usize;
    types::Item {
        id,
        guid: Some(format!("https://example.org/{feed_id}/{id}")),
        guid_hash: Some(format!("{id:032x}")),
        url: Some(format!("https://example.org/{feed_id}/{id}")),
        title: Some(rng.words(title_words)),
        author: Some(rng.words(2)),
        pub_date: Some(NEWEST - age),
        updated_date: None,
        body: Some(body),
        enclosure_mime: None,
        enclosure_link: None,
        media_thumbnail: None,
        media_description: None,
        feed_id,
        unread: true,
        starred: rng.below(1000) == 0,
        filtered: false,
        rtl: false,
        last_modified: Some(NEWEST - age + 60),
        fingerprint: Some(format!("{:016x}", rng.next())),
        content_hash: None,
    }
}

fn status_kib(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            let line = s.lines().find(|l| l.starts_with(field))?.to_owned();
            line.split_whitespace().nth(1)?.parse().ok()
        })
        .unwrap_or(0)
}

fn cpu() -> Duration {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let ticks: u64 = stat
        .rsplit_once(") ")
        .map(|(_, rest)| {
            rest.split_whitespace()
                .skip(11)
                .take(2)
                .filter_map(|t| t.parse::<u64>().ok())
                .sum()
        })
        .unwrap_or(0);
    Duration::from_millis(ticks * 10)
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

fn ms(d: Duration) -> String {
    format!("{:.2}", d.as_secs_f64() * 1000.0)
}

/// The median time of a query over a few runs.
fn time<T>(mut f: impl FnMut() -> T) -> Duration {
    let mut times: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            std::hint::black_box(f());
            start.elapsed()
        })
        .collect();
    times.sort();
    times[RUNS / 2]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let count: u64 = args
        .next()
        .map(|n| n.parse().expect("ITEMS must be a number"))
        .unwrap_or(63_000);
    let dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = dir.join(format!("ren-store-measure-{}.db", std::process::id()));
    let rss_start = status_kib("VmRSS:");

    let mut store = Store::open(&path).unwrap();
    let folders: Vec<_> = (1..=FOLDERS)
        .map(|id| types::Folder {
            id,
            name: format!("Folder {id}"),
        })
        .collect();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let feeds: Vec<_> = (1..=FEEDS)
        .map(|id| types::Feed {
            id,
            url: format!("https://example.org/{id}/feed"),
            title: Some(rng.words(2)),
            favicon_link: None,
            added: Some(NEWEST),
            folder_id: (id % (FOLDERS + 3) <= FOLDERS).then_some(id % (FOLDERS + 3)),
            unread_count: 0,
            ordering: 0,
            link: None,
            pinned: false,
            update_error_count: 0,
            last_update_error: None,
        })
        .collect();
    store.replace_folders(&folders).unwrap();
    store.replace_feeds(&feeds).unwrap();

    // Initial sync: pages of 1000, newest first, as the server sends them.
    let mut body_bytes = 0;
    let mut write_time = Duration::ZERO;
    let cpu_start = cpu();
    let mut id = count;
    while id > 0 {
        let chunk: Vec<_> = (id.saturating_sub(CHUNK as u64) + 1..=id)
            .rev()
            .map(|id| item(&mut rng, id, count))
            .collect();
        body_bytes += chunk
            .iter()
            .map(|i| i.body.as_ref().map_or(0, String::len) as u64)
            .sum::<u64>();
        let start = Instant::now();
        store.upsert_items(&chunk, false).unwrap();
        write_time += start.elapsed();
        id = id.saturating_sub(CHUNK as u64);
    }
    let write_cpu = cpu() - cpu_start;
    let db_after_write = file_size(&path);
    let wal_after_write = file_size(&path.with_extension("db-wal"));
    let rss_after_write = status_kib("VmRSS:");

    println!("## Store with {count} items in {FEEDS} feeds\n");
    println!("Bodies: {:.1} MiB.\n", mib(body_bytes));
    println!("| step | time ms | notes |");
    println!("|---|---|---|");
    println!(
        "| initial sync, {} chunks of {CHUNK} | {} | CPU {} ms; {:.0} µs per item |",
        count.div_ceil(CHUNK as u64),
        ms(write_time),
        ms(write_cpu),
        write_time.as_secs_f64() * 1e6 / count as f64
    );

    // The UI's connection, as it would be opened next to the sync's.
    let ui = Store::open(&path).unwrap();
    let all = ItemQuery::default();
    let by_date = Sort::default();
    let by_title = Sort {
        column: SortColumn::Title,
        ascending: true,
    };
    let by_feed = Sort {
        column: SortColumn::Feed,
        ascending: true,
    };
    let middle = count / 2;
    let feed = ItemQuery {
        selection: Selection::Feed(1),
        ..ItemQuery::default()
    };
    let folder = ItemQuery {
        selection: Selection::Folder(3),
        ..ItemQuery::default()
    };
    let unread = ItemQuery {
        unread_only: true,
        ..ItemQuery::default()
    };
    let search = ItemQuery {
        search: "über kernel".into(),
        ..ItemQuery::default()
    };
    let feed_count = ui.item_count(&feed).unwrap();
    let page_ids: Vec<u64> = (middle..middle + PAGE).collect();
    let search_count = ui.item_count(&search).unwrap();
    let queries: Vec<(String, Duration)> = vec![
        (
            "unread counts per feed".into(),
            time(|| ui.unread_counts().unwrap()),
        ),
        (
            "count, all items".into(),
            time(|| ui.item_count(&all).unwrap()),
        ),
        (
            format!("page of {PAGE}, all, by date, first"),
            time(|| ui.item_page(&all, by_date, 0, PAGE).unwrap()),
        ),
        (
            format!("page of {PAGE}, all, by date, middle"),
            time(|| ui.item_page(&all, by_date, middle, PAGE).unwrap()),
        ),
        (
            format!("page of {PAGE}, all, by title, first"),
            time(|| ui.item_page(&all, by_title, 0, PAGE).unwrap()),
        ),
        (
            format!("page of {PAGE}, all, by feed, middle"),
            time(|| ui.item_page(&all, by_feed, middle, PAGE).unwrap()),
        ),
        (
            format!("page of {PAGE}, all unread, by date, middle"),
            time(|| ui.item_page(&unread, by_date, middle, PAGE).unwrap()),
        ),
        (
            format!("page of {PAGE}, largest feed ({feed_count} items), by date"),
            time(|| ui.item_page(&feed, by_date, 0, PAGE).unwrap()),
        ),
        (
            format!("page of {PAGE}, a folder, by date"),
            time(|| ui.item_page(&folder, by_date, 0, PAGE).unwrap()),
        ),
        (
            "all ids, by date".into(),
            time(|| ui.item_ids(&all, by_date).unwrap()),
        ),
        (
            "all ids, by title".into(),
            time(|| ui.item_ids(&all, by_title).unwrap()),
        ),
        (
            "all ids, by feed".into(),
            time(|| ui.item_ids(&all, by_feed).unwrap()),
        ),
        (
            format!("{PAGE} rows by id"),
            time(|| ui.item_summaries(&page_ids).unwrap()),
        ),
        (
            "position of an item, all, by date".into(),
            time(|| ui.item_position(&all, by_date, middle).unwrap()),
        ),
        (
            "position of an item, all, by title".into(),
            time(|| ui.item_position(&all, by_title, middle).unwrap()),
        ),
        (
            format!("search, count ({search_count} found)"),
            time(|| ui.item_count(&search).unwrap()),
        ),
        (
            "one item with body".into(),
            time(|| ui.item(middle).unwrap()),
        ),
    ];
    for (name, t) in &queries {
        println!("| {name} | {} | median of {RUNS} |", ms(*t));
    }

    // Incremental sync: 1000 changed items (read on the server), 200 new.
    let mut updated: Vec<_> = (1..=1000)
        .map(|n| {
            let mut item = item(&mut rng, count - n * 7 % count, count);
            item.unread = false;
            item
        })
        .collect();
    updated.extend((1..=200).map(|n| item(&mut rng, count + n, count + 200)));
    let start = Instant::now();
    let added = store.upsert_items(&updated, true).unwrap();
    let incremental = start.elapsed();
    // When the next sync starts.
    let start = Instant::now();
    let new_marked = store.clear_new().unwrap();
    let clear_time = start.elapsed();
    println!(
        "| incremental sync, 1200 items | {} | {added} new |",
        ms(incremental)
    );
    println!(
        "| clear the new marker | {} | {new_marked} items had it |",
        ms(clear_time)
    );

    // Marking 1000 items read at once (e.g. a feed), then the sync's
    // reading of the queue and its removal.
    let ids: Vec<u64> = (1..=1000).map(|n| n * 31 % count + 1).collect();
    let start = Instant::now();
    let changed = store.set_unread(&ids, false).unwrap();
    let mark = start.elapsed();
    let start = Instant::now();
    let pending = store
        .pending(nextcloud_news::ItemAction::Read, 1000)
        .unwrap();
    store
        .remove_pending(nextcloud_news::ItemAction::Read, &pending)
        .unwrap();
    let queue = start.elapsed();
    println!(
        "| mark 1000 items read | {} | {changed} changed |",
        ms(mark)
    );
    println!("| take and clear the queue | {} | |", ms(queue));

    // Purge: a third of all items read long ago.
    let old: Vec<u64> = (1..=count / 3).collect();
    store.set_unread(&old, false).unwrap();
    loop {
        let pending = store
            .pending(nextcloud_news::ItemAction::Read, CHUNK)
            .unwrap();
        if pending.is_empty() {
            break;
        }
        store
            .remove_pending(nextcloud_news::ItemAction::Read, &pending)
            .unwrap();
    }
    let cutoff = NEWEST - (count as i64 * 2 / 3) * 600;
    let start = Instant::now();
    let purged = store.purge(cutoff).unwrap();
    let purge = start.elapsed();
    println!("| purge | {} | {purged} items deleted |", ms(purge));

    drop(ui);
    drop(store);
    let db = file_size(&path);
    println!();
    println!(
        "Database: {:.1} MiB and {:.1} MiB write-ahead log after the initial sync, \
         {:.1} MiB after the purge (closed).",
        mib(db_after_write),
        mib(wal_after_write),
        mib(db)
    );
    println!(
        "RSS: {:.1} MiB at the start, {:.1} MiB after the initial sync, {:.1} MiB peak.",
        rss_start as f64 / 1024.0,
        rss_after_write as f64 / 1024.0,
        status_kib("VmHWM:") as f64 / 1024.0
    );
    for suffix in ["", "-wal", "-shm"] {
        let mut file = path.clone().into_os_string();
        file.push(suffix);
        let _ = std::fs::remove_file(file);
    }
}
