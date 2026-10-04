// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding responses in the form the News app sends them.
//!
//! `tests/fixtures/handwritten/` is written after the News app's source
//! (`toAPI()` of items, feeds and folders; Nextcloud's `JSONResponse`
//! escaping). Other directories under `tests/fixtures/` are anonymised
//! server responses (`scripts/anonymise-dump.py`) and get the generic
//! checks.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use nextcloud_news::types::{Feeds, Folders, Item, Items, Status, Version};
use nextcloud_news::{Error, decode, decode_items};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let file = fs::File::open(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    decode(file).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// The items of a file, decoded item by item; checked against decoding
/// the file at once.
fn items(path: &Path) -> Vec<Item> {
    let mut streamed = Vec::new();
    let count = decode_items(fs::File::open(path).unwrap(), |item| {
        streamed.push(item);
        Ok::<_, Error>(())
    })
    .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    assert_eq!(count, streamed.len());
    let whole: Items = read(path);
    assert_eq!(streamed, whole.items, "{}", path.display());
    streamed
}

fn item_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            let name = path.file_name().unwrap().to_string_lossy();
            ["items", "starred", "updated"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .collect();
    files.sort();
    files
}

/// What every set of responses must satisfy.
fn check_dir(dir: &Path) {
    let version: Version = read(&dir.join("version.json"));
    let status: Status = read(&dir.join("status.json"));
    assert_eq!(version.version, status.version);

    let folders: Folders = read(&dir.join("folders.json"));
    let folder_ids: HashSet<_> = folders.folders.iter().map(|f| f.id).collect();
    let feeds: Feeds = read(&dir.join("feeds.json"));
    let feed_ids: HashSet<_> = feeds.feeds.iter().map(|f| f.id).collect();
    for feed in &feeds.feeds {
        if let Some(folder) = feed.folder_id.filter(|&id| id != 0) {
            assert!(folder_ids.contains(&folder), "feed {}", feed.id);
        }
    }

    let files = item_files(dir);
    assert!(!files.is_empty(), "no item files in {}", dir.display());
    for path in files {
        for item in items(&path) {
            assert!(feed_ids.contains(&item.feed_id), "item {}", item.id);
            assert!(item.last_modified.is_some(), "item {}", item.id);
        }
    }
}

#[test]
fn all_fixture_sets() {
    let mut dirs = 0;
    for entry in fs::read_dir(fixtures()).unwrap() {
        let dir = entry.unwrap().path();
        if dir.is_dir() {
            check_dir(&dir);
            dirs += 1;
        }
    }
    assert!(dirs >= 1);
}

fn handwritten() -> PathBuf {
    fixtures().join("handwritten")
}

#[test]
fn folders_and_feeds() {
    let folders: Folders = read(&handwritten().join("folders.json"));
    assert_eq!(folders.folders[1].name, "Nachrichten über Ärger");

    let feeds: Feeds = read(&handwritten().join("feeds.json"));
    assert_eq!(feeds.starred_count, 2);
    assert_eq!(feeds.newest_item_id, Some(227_491));
    let [blog, news, rtl] = &feeds.feeds[..] else {
        panic!("expected three feeds");
    };
    assert_eq!(blog.url, "https://blog.example.org/feed/");
    assert_eq!(blog.folder_id, Some(4));
    assert!(blog.pinned);
    assert_eq!(news.url, "https://news.example.com/rss?lang=en&full=1");
    assert_eq!(news.title.as_deref(), Some("  Example News\n"));
    assert_eq!(news.folder_id, None);
    assert_eq!(news.favicon_link, None);
    assert_eq!(news.ordering, 2);
    assert_eq!(news.update_error_count, 15);
    assert!(
        news.last_update_error
            .as_deref()
            .unwrap()
            .contains("timed out")
    );
    assert_eq!(rtl.link, None);
    assert_eq!(rtl.title.as_deref(), Some("مثال – Beispiel"));
}

#[test]
fn item_fields() {
    let items = items(&handwritten().join("items.json"));
    let ids: Vec<_> = items.iter().map(|item| item.id).collect();
    assert_eq!(ids, [227_491, 227_490, 227_488, 227_400]);

    let full = &items[0];
    assert_eq!(
        full.title.as_deref(),
        Some("A post with <em>markup</em> in the title")
    );
    assert_eq!(
        full.url.as_deref(),
        Some("https://blog.example.org/2026/01/02/a-post/")
    );
    assert!(
        full.body
            .as_deref()
            .unwrap()
            .starts_with("<p>First paragraph")
    );
    assert!(full.body.as_deref().unwrap().contains("a=1&amp;b=2"));
    assert_eq!(full.enclosure_mime.as_deref(), Some("audio/mpeg"));
    assert_eq!(
        full.media_description.as_deref(),
        Some("Episode 12: “Quotes” & more")
    );
    assert_eq!(full.pub_date, Some(1_767_355_200));
    assert_eq!(full.last_modified, Some(1_767_355_800));
    assert_eq!(full.updated_date, None);
    assert!(full.unread && !full.starred && !full.filtered && !full.rtl);
    assert_eq!(full.guid_hash.as_deref().map(str::len), Some(32));

    let rtl = &items[1];
    assert!(rtl.rtl);
    assert_eq!(rtl.author, None);
    assert_eq!(rtl.body.as_deref(), Some("<p dir=\"rtl\">مرحبا 😀</p>"));

    let filtered = &items[2];
    assert!(filtered.filtered);
    assert_eq!(filtered.title.as_deref(), Some(" Fish & Chips <3 \n"));
    assert_eq!(filtered.author.as_deref(), Some("Someone,\n  Someone Else"));
    assert_eq!(filtered.body.as_deref(), Some(""));

    let bare = &items[3];
    assert_eq!(bare.guid, None);
    assert_eq!(bare.guid_hash, None);
    assert_eq!(bare.url, None);
    assert_eq!(bare.body, None);
    assert_eq!(bare.fingerprint, None);
    assert_eq!(bare.author.as_deref(), Some(""));
}

#[test]
fn starred_and_updated_items() {
    let starred = items(&handwritten().join("starred.json"));
    assert!(starred.iter().all(|item| item.starred));
    assert!(!starred[0].unread && starred[1].unread);

    let updated = items(&handwritten().join("updated.json"));
    let state: Vec<_> = updated
        .iter()
        .map(|item| (item.id, item.unread, item.starred))
        .collect();
    assert_eq!(
        state,
        [
            (227_491, false, false),
            (226_990, true, false),
            (227_492, true, false)
        ]
    );
}
