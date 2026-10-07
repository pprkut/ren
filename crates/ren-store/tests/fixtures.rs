// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The store filled with the anonymised responses recorded from a real
//! server (nextcloud-news's `tests/fixtures/recorded`), as a sync would.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;

use nextcloud_news::types::{Feeds, Folders, Item};
use nextcloud_news::{decode, decode_items};
use ren_store::{ItemQuery, Selection, Sort, SortColumn, Status, Store};

fn fixture(name: &str) -> File {
    let path = format!(
        "{}/../nextcloud-news/tests/fixtures/recorded/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    File::open(&path).unwrap_or_else(|err| panic!("{path}: {err}"))
}

fn items(name: &str) -> Vec<Item> {
    let mut items = Vec::new();
    decode_items(fixture(name), |item| {
        items.push(item);
        Ok::<_, nextcloud_news::Error>(())
    })
    .unwrap();
    items
}

/// The items of `/items/updated` that the initial sync leaves out, as if
/// they had arrived on the server in between.
fn arrived_later() -> HashSet<u64> {
    items("updated").iter().take(5).map(|i| i.id).collect()
}

/// The store after an initial sync (folders, feeds, unread and starred
/// items) and an incremental one (`/items/updated`), and all items sent.
fn synced() -> (Store, Vec<Item>) {
    let mut store = Store::open_in_memory().unwrap();
    let folders: Folders = decode(fixture("folders")).unwrap();
    let feeds: Feeds = decode(fixture("feeds")).unwrap();
    store.replace_folders(&folders.folders).unwrap();
    store.replace_feeds(&feeds.feeds).unwrap();
    let later = arrived_later();
    let mut sent = Vec::new();
    for name in ["items", "starred"] {
        let mut page = items(name);
        page.retain(|i| !later.contains(&i.id));
        store.upsert_items(&page, false).unwrap();
        sent.extend(page);
    }
    store.clear_new().unwrap();
    let updated = items("updated");
    store.upsert_items(&updated, true).unwrap();
    sent.extend(updated);
    (store, sent)
}

/// The latest state of each item sent, by id.
fn latest(sent: &[Item]) -> BTreeMap<u64, &Item> {
    sent.iter().map(|item| (item.id, item)).collect()
}

#[test]
fn single_line_fields_are_collapsed() {
    let (store, _) = synced();
    let clean = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ") == s;
    let feeds = store.feeds().unwrap();
    assert_eq!(feeds.len(), 49);
    assert!(feeds.iter().all(|f| clean(&f.title)));
    // The recorded titles do have stray spaces.
    let raw: Feeds = decode(fixture("feeds")).unwrap();
    assert!(
        raw.feeds
            .iter()
            .any(|f| !clean(f.title.as_deref().unwrap_or_default()))
    );
    assert!(store.folders().unwrap().iter().all(|f| clean(&f.name)));
    let all = store
        .item_page(&ItemQuery::default(), Sort::default(), 0, 1000)
        .unwrap();
    assert!(all.iter().all(|i| clean(&i.title) && clean(&i.author)));
}

#[test]
fn counts_match_what_was_sent() {
    let (store, sent) = synced();
    let latest = latest(&sent);
    let feeds: HashSet<u64> = store.feeds().unwrap().iter().map(|f| f.id).collect();
    let listed: Vec<&Item> = latest
        .values()
        .copied()
        .filter(|i| !i.filtered && feeds.contains(&i.feed_id))
        .collect();

    let all = ItemQuery::default();
    assert_eq!(store.item_count(&all).unwrap(), listed.len() as u64);
    let unread: u64 = store.unread_counts().unwrap().iter().map(|(_, n)| n).sum();
    assert_eq!(unread, listed.iter().filter(|i| i.unread).count() as u64);
    assert_eq!(
        store.starred_count().unwrap(),
        listed.iter().filter(|i| i.starred).count() as u64
    );
    let starred = ItemQuery {
        selection: Selection::Starred,
        ..ItemQuery::default()
    };
    assert_eq!(
        store.item_count(&starred).unwrap(),
        store.starred_count().unwrap()
    );

    // The unread items that arrived with `/items/updated` are new.
    let new: HashSet<u64> = arrived_later()
        .into_iter()
        .filter(|id| latest[id].unread)
        .collect();
    assert!(!new.is_empty());
    let page = store.item_page(&all, Sort::default(), 0, 1000).unwrap();
    let shown_new: HashSet<u64> = page
        .iter()
        .filter(|i| i.status == Status::New)
        .map(|i| i.id)
        .collect();
    assert_eq!(shown_new, new);
}

#[test]
fn pages_make_up_the_list() {
    let (store, _) = synced();
    let all = ItemQuery::default();
    let total = store.item_count(&all).unwrap();
    for column in [
        SortColumn::Date,
        SortColumn::Title,
        SortColumn::Feed,
        SortColumn::Author,
    ] {
        for ascending in [true, false] {
            let sort = Sort { column, ascending };
            let whole = store.item_page(&all, sort, 0, total).unwrap();
            let mut paged = Vec::new();
            let mut offset = 0;
            loop {
                let page = store.item_page(&all, sort, offset, 7).unwrap();
                if page.is_empty() {
                    break;
                }
                offset += page.len() as u64;
                paged.extend(page);
            }
            assert_eq!(paged, whole, "{sort:?}");
            let ids = store.item_ids(&all, sort).unwrap();
            assert_eq!(store.item_summaries(&ids).unwrap(), whole, "{sort:?}");
            for (row, item) in whole.iter().enumerate() {
                assert_eq!(
                    store.item_position(&all, sort, item.id).unwrap(),
                    Some(row as u64),
                    "{sort:?}"
                );
            }
        }
    }
}

#[test]
fn bodies_are_kept() {
    let (store, sent) = synced();
    for item in latest(&sent).values() {
        let stored = store.item(item.id).unwrap().unwrap();
        assert_eq!(stored.body, item.body);
        assert_eq!(stored.url, item.url);
        assert_eq!(stored.summary.starred, item.starred);
    }
}
