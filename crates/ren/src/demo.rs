// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Data for the window without the server, for trying it out and for the
//! measurements: generated items (`--items`) or the items of a
//! `--dump-items` directory (`--dump`), written into a temporary database
//! that is removed when ren exits. The window never syncs it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use nextcloud_news::types::{Feed, Feeds, Folder, Folders, Item, Items};
use ren_store::Store;

/// Items written per transaction.
const CHUNK: usize = 500;

const FOLDERS: &[&str] = &["Technology", "Science", "News", "Comics"];

/// Feed titles; the first `FOLDER_SIZES.iter().sum()` go into folders in
/// order, the rest are top-level.
const FEEDS: &[&str] = &[
    "LWN.net",
    "Rust Blog",
    "This Week in Rust",
    "Hacker News",
    "Phoronix",
    "KDE Planet",
    "Quanta Magazine",
    "Nature News",
    "Ars Technica Science",
    "The Guardian",
    "BBC World",
    "Tagesschau",
    "NOS Nieuws",
    "Deutsche Welle",
    "xkcd",
    "Saturday Morning Breakfast Cereal",
    "Nextcloud Blog",
    "Planet GNOME",
    "Servo Blog",
];

const FOLDER_SIZES: &[usize] = &[6, 3, 5, 2];

const AUTHORS: &[&str] = &[
    "Ada Lovelace",
    "Alan Turing",
    "Barbara Liskov",
    "Dennis Ritchie",
    "Edsger Dijkstra",
    "Frances Allen",
    "Grace Hopper",
    "Hedy Lamarr",
    "Ken Thompson",
    "Margaret Hamilton",
    "Radia Perlman",
    "Sophie Wilson",
];

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
    "new",
    "old",
    "open",
    "native",
    "portable",
    "stable",
    "experimental",
    "quantum",
    "planet",
    "graphics",
    "driver",
    "audio",
    "font",
    "layout",
    "engine",
    "toolkit",
    "community",
    "project",
];

const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod \
tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud \
exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in \
reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint \
occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.";

/// Newest item date: 2026-09-01 00:00:00 UTC.
const NEWEST: i64 = 1_788_220_800;

/// Share of the items that arrived in the "latest sync": the newest 2 %.
const NEW_SHARE: usize = 50;

/// Cheap deterministic hash (splitmix64) so every field of an item can be
/// derived from its index.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn pick<T: Copy>(list: &[T], seed: u64) -> T {
    list[(seed % list.len() as u64) as usize]
}

/// The generated folders, with ids from 1.
fn folders() -> Vec<Folder> {
    FOLDERS
        .iter()
        .zip(1..)
        .map(|(name, id)| Folder {
            id,
            name: (*name).to_owned(),
        })
        .collect()
}

/// The generated feeds, with ids from 1.
fn feeds() -> Vec<Feed> {
    let mut folder_of = Vec::with_capacity(FEEDS.len());
    for (folder, &size) in (1..).zip(FOLDER_SIZES) {
        folder_of.extend(std::iter::repeat_n(Some(folder), size));
    }
    folder_of.resize(FEEDS.len(), None);
    FEEDS
        .iter()
        .zip(folder_of)
        .zip(1..)
        .map(|((title, folder_id), id)| Feed {
            id,
            url: format!("https://example.org/feeds/{id}"),
            title: Some((*title).to_owned()),
            favicon_link: None,
            added: None,
            folder_id,
            unread_count: 0,
            ordering: 0,
            link: None,
            pinned: false,
            update_error_count: 0,
            last_update_error: None,
        })
        .collect()
}

/// The generated item `index` of `count`; index 0 is the newest and has
/// the highest id, as on the server. The newest items are new and
/// unread, of the others about a third is unread.
fn item(index: u64, count: u64) -> Item {
    let seed = mix(index);
    let words = 4 + (seed % 7) as usize;
    let mut title = String::new();
    for w in 0..words {
        if w > 0 {
            title.push(' ');
        }
        title.push_str(pick(WORDS, mix(seed ^ w as u64)));
    }
    if let Some(first) = title.get_mut(0..1) {
        first.make_ascii_uppercase();
    }

    let paragraphs = 2 + (mix(seed ^ 0xb0d7) % 5) as usize;
    let mut body = String::with_capacity(paragraphs * (LOREM.len() + 7));
    for _ in 0..paragraphs {
        body.push_str("<p>");
        body.push_str(LOREM);
        body.push_str("</p>");
    }

    let new = index < (count as usize).div_ceil(NEW_SHARE) as u64;
    Item {
        id: count - index,
        guid: None,
        guid_hash: None,
        url: None,
        title: Some(title),
        author: Some(pick(AUTHORS, mix(seed ^ 0xa07)).to_owned()),
        // Items are between 5 and 60 minutes apart.
        pub_date: Some(NEWEST - index.cast_signed() * 1_950 - (seed % 600).cast_signed()),
        updated_date: None,
        body: Some(body),
        enclosure_mime: None,
        enclosure_link: None,
        media_thumbnail: None,
        media_description: None,
        feed_id: mix(index ^ 0xfeed) % FEEDS.len() as u64 + 1,
        unread: new || mix(index).is_multiple_of(3),
        starred: seed.is_multiple_of(50),
        filtered: false,
        rtl: false,
        last_modified: None,
        fingerprint: None,
        content_hash: None,
    }
}

/// A database in the temporary directory, removed when dropped.
pub struct DemoDb {
    path: PathBuf,
}

impl DemoDb {
    fn create() -> Result<(Self, Store), String> {
        // Numbered, for tests running at the same time.
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let name = format!(
            "ren-demo-{}-{}.db",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        let db = Self { path };
        db.remove();
        let store = Store::open(&db.path).map_err(|err| format!("{}: {err}", db.path.display()))?;
        Ok((db, store))
    }

    /// `count` generated items in 19 feeds, 16 of them in 4 folders.
    pub fn generated(count: usize) -> Result<Self, String> {
        let (db, mut store) = Self::create()?;
        let error = |err: ren_store::Error| format!("{}: {err}", db.path.display());
        store.replace_folders(&folders()).map_err(error)?;
        store.replace_feeds(&feeds()).map_err(error)?;
        let count = count as u64;
        let new = count.div_ceil(NEW_SHARE as u64);
        let mut start = 0;
        while start < count {
            let end = (start + CHUNK as u64).min(count);
            let items: Vec<Item> = (start..end).map(|index| item(index, count)).collect();
            // Item by item, as the "new" marker is for the whole chunk.
            let (fresh, old): (Vec<Item>, Vec<Item>) =
                items.into_iter().partition(|item| count - item.id < new);
            store.upsert_items(&fresh, true).map_err(error)?;
            store.upsert_items(&old, false).map_err(error)?;
            start = end;
        }
        Ok(db)
    }

    /// The folders, feeds and at most `max_items` items of a `--dump-items`
    /// directory: the unread items first, newest first, then the starred
    /// ones.
    pub fn from_dump(dir: &Path, max_items: usize) -> Result<Self, String> {
        let folders: Folders = read(&dir.join("folders.json"))?;
        let feeds: Feeds = read(&dir.join("feeds.json"))?;
        let (db, mut store) = Self::create()?;
        let error = |err: ren_store::Error| format!("{}: {err}", db.path.display());
        store.replace_folders(&folders.folders).map_err(error)?;
        store.replace_feeds(&feeds.feeds).map_err(error)?;
        let known: HashSet<u64> = feeds.feeds.iter().map(|feed| feed.id).collect();

        let mut seen = HashSet::new();
        for path in item_files(dir)? {
            let page: Items = read(&path)?;
            let remaining = max_items.saturating_sub(seen.len());
            // `take` stops pulling items once it has enough, so only the
            // items taken are marked as seen.
            let items: Vec<Item> = page
                .items
                .into_iter()
                .filter(|item| known.contains(&item.feed_id) && seen.insert(item.id))
                .take(remaining)
                .collect();
            for chunk in items.chunks(CHUNK) {
                store.upsert_items(chunk, false).map_err(error)?;
            }
            if seen.len() >= max_items {
                break;
            }
        }
        if seen.is_empty() {
            return Err(format!("{}: no items found", dir.display()));
        }
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn remove(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.path.clone().into_os_string();
            path.push(suffix);
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Drop for DemoDb {
    fn drop(&mut self) {
        self.remove();
    }
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = std::fs::File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    serde_json::from_reader(std::io::BufReader::new(file))
        .map_err(|err| format!("{}: {err}", path.display()))
}

/// The item pages of a dump: unread first, then starred, each in page order.
fn item_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|err| format!("{}: {err}", dir.display()))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| {
            (name.starts_with("unread-") || name.starts_with("starred-")) && name.ends_with(".json")
        })
        .collect();
    names.sort_by_key(|name| (name.starts_with("starred-"), name.clone()));
    Ok(names.into_iter().map(|name| dir.join(name)).collect())
}

#[cfg(test)]
mod tests {
    use ren_store::{ItemQuery, Selection, Sort, Status};

    use super::*;

    fn ids(store: &Store, selection: Selection) -> Vec<u64> {
        let query = ItemQuery {
            selection,
            ..ItemQuery::default()
        };
        store.item_ids(&query, Sort::default()).unwrap()
    }

    #[test]
    fn generated_items() {
        let db = DemoDb::generated(1_000).unwrap();
        let store = Store::open(db.path()).unwrap();
        assert_eq!(store.folders().unwrap().len(), FOLDERS.len());
        let feeds = store.feeds().unwrap();
        assert_eq!(feeds.len(), FEEDS.len());
        let in_folders = feeds.iter().filter(|f| f.folder_id.is_some()).count();
        assert_eq!(in_folders, FOLDER_SIZES.iter().sum::<usize>());

        // Newest first is highest id first.
        let all = ids(&store, Selection::All);
        assert_eq!(all.len(), 1_000);
        assert_eq!(all[..3], [1_000, 999, 998]);
        let rows = store.item_summaries(&all[..30]).unwrap();
        assert!(rows[..20].iter().all(|r| r.status == Status::New));
        assert!(rows[20..].iter().all(|r| r.status != Status::New));
        assert!(!ids(&store, Selection::Starred).is_empty());
        let unread: u64 = store.unread_counts().unwrap().iter().map(|(_, n)| n).sum();
        assert!((300..400).contains(&unread), "{unread}");

        let item = store.item(1_000).unwrap().unwrap();
        assert!(item.body.unwrap().starts_with("<p>Lorem ipsum"));

        let path = db.path().to_owned();
        drop(store);
        drop(db);
        assert!(!path.exists());
    }

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(item(42, 100), item(42, 100));
        assert!(item(1, 100).pub_date < item(0, 100).pub_date);
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("ren-dump-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, name: &str, json: &str) {
            std::fs::write(self.0.join(name), json).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dump_item(id: u64, feed: u64, unread: bool, starred: bool) -> String {
        format!(
            r#"{{"id": {id}, "feedId": {feed}, "title": "Item {id}", "url": "https://e/{id}",
                "pubDate": {id}, "body": "<p>Body {id}</p>", "unread": {unread},
                "starred": {starred}}}"#
        )
    }

    fn dump(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        dir.write(
            "folders.json",
            r#"{"folders": [{"id": 7, "name": "Tech"}]}"#,
        );
        dir.write(
            "feeds.json",
            r#"{"feeds": [
                {"id": 10, "url": "https://a/feed", "title": "A", "folderId": 7},
                {"id": 11, "url": "https://b/feed", "folderId": 0}
            ]}"#,
        );
        let page = |items: &[String]| format!(r#"{{"items": [{}]}}"#, items.join(","));
        dir.write(
            "unread-0001.json",
            &page(&[dump_item(5, 10, true, false), dump_item(4, 11, true, true)]),
        );
        dir.write("unread-0002.json", &page(&[dump_item(3, 99, true, false)]));
        dir.write(
            "starred-0001.json",
            &page(&[dump_item(4, 11, true, true), dump_item(1, 10, false, true)]),
        );
        dir.write("version.json", r#"{"version": "28.7.0"}"#);
        dir
    }

    #[test]
    fn dump_items() {
        let dir = dump("load");
        let db = DemoDb::from_dump(&dir.0, 100).unwrap();
        let store = Store::open(db.path()).unwrap();
        assert_eq!(store.folders().unwrap().len(), 1);
        // By title; feed 11 has none. A folder id of 0 is no folder.
        let folders: Vec<_> = store
            .feeds()
            .unwrap()
            .iter()
            .map(|f| (f.id, f.folder_id))
            .collect();
        assert_eq!(folders, [(11, None), (10, Some(7))]);
        // Item 3 has an unknown feed, item 4 is in two files.
        assert_eq!(ids(&store, Selection::All), [5, 4, 1]);
        assert_eq!(ids(&store, Selection::Starred), [4, 1]);
        let item = store.item(1).unwrap().unwrap();
        assert_eq!(item.summary.status, Status::Read);
        assert_eq!(item.body.as_deref(), Some("<p>Body 1</p>"));
    }

    #[test]
    fn dump_limit() {
        let dir = dump("limit");
        let db = DemoDb::from_dump(&dir.0, 2).unwrap();
        let store = Store::open(db.path()).unwrap();
        assert_eq!(ids(&store, Selection::All), [5, 4]);
    }

    #[test]
    fn dump_without_files() {
        let dir = TempDir::new("missing");
        let Err(err) = DemoDb::from_dump(&dir.0, 10) else {
            panic!("loaded an empty directory");
        };
        assert!(err.contains("folders.json"));
    }
}
