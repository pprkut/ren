// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Real items from a `--dump-items` directory, as data for the window
//! (spike S3), until the store exists.
//!
//! Folders, feeds and the item list fields are kept in memory; bodies are
//! read from the dump files when an item is opened, so memory use is close
//! to what the store-backed app will need.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use nextcloud_news::types::{Feeds, Folders, Items};

use crate::dummy::{Feed, Folder};

/// The list fields of a dumped item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpItem {
    pub feed_id: u32,
    pub title: String,
    pub author: String,
    pub url: Option<String>,
    pub pub_date: i64,
    pub unread: bool,
    pub starred: bool,
    /// Where the body is: file number and position in its `items` array.
    file: u16,
    index: u32,
}

pub struct Dump {
    pub folders: Vec<Folder>,
    pub feeds: Vec<Feed>,
    pub items: Vec<DumpItem>,
    files: Vec<PathBuf>,
    /// The bodies of the last file read.
    cache: RefCell<Option<(u16, Vec<Option<String>>)>>,
}

/// Text for a single line: leading and trailing whitespace removed, runs
/// of whitespace (including line breaks) collapsed to one space. Feeds put
/// line breaks into authors and spaces around titles.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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

impl Dump {
    /// Loads at most `max_items` items, newest unread ones first.
    pub fn load(dir: &Path, max_items: usize) -> Result<Self, String> {
        let server_folders: Folders = read(&dir.join("folders.json"))?;
        let server_feeds: Feeds = read(&dir.join("feeds.json"))?;

        let mut folder_index = HashMap::new();
        let folders = server_folders
            .folders
            .iter()
            .enumerate()
            .map(|(i, folder)| {
                folder_index.insert(folder.id, i as u32);
                Folder {
                    id: i as u32,
                    name: one_line(&folder.name),
                }
            })
            .collect();

        let mut feed_index = HashMap::new();
        let feeds = server_feeds
            .feeds
            .iter()
            .enumerate()
            .map(|(i, feed)| {
                feed_index.insert(feed.id, i as u32);
                Feed {
                    id: i as u32,
                    // 0 is "no folder" in older News versions.
                    folder_id: feed.folder_id.and_then(|id| folder_index.get(&id).copied()),
                    title: feed
                        .title
                        .as_deref()
                        .map(one_line)
                        .filter(|title| !title.is_empty())
                        .unwrap_or_else(|| feed.url.clone()),
                }
            })
            .collect();

        let files = item_files(dir)?;
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        'files: for (file, path) in files.iter().enumerate() {
            let page: Items = read(path)?;
            for (index, item) in page.items.into_iter().enumerate() {
                if items.len() >= max_items {
                    break 'files;
                }
                let Some(&feed_id) = feed_index.get(&item.feed_id) else {
                    continue;
                };
                if !seen.insert(item.id) {
                    continue;
                }
                items.push(DumpItem {
                    feed_id,
                    title: item.title.as_deref().map(one_line).unwrap_or_default(),
                    author: item.author.as_deref().map(one_line).unwrap_or_default(),
                    url: item.url,
                    pub_date: item.pub_date.unwrap_or_default(),
                    unread: item.unread,
                    starred: item.starred,
                    file: file as u16,
                    index: index as u32,
                });
            }
        }
        if items.is_empty() {
            return Err(format!("{}: no items found", dir.display()));
        }

        Ok(Self {
            folders,
            feeds,
            items,
            files,
            cache: RefCell::new(None),
        })
    }

    /// The HTML body of an item, read from its dump file.
    pub fn body(&self, item: &DumpItem) -> Option<String> {
        let mut cache = self.cache.borrow_mut();
        if cache.as_ref().is_none_or(|(file, _)| *file != item.file) {
            let page: Items = match read(&self.files[usize::from(item.file)]) {
                Ok(page) => page,
                Err(err) => {
                    eprintln!("ren: {err}");
                    return None;
                }
            };
            let bodies = page.items.into_iter().map(|item| item.body).collect();
            *cache = Some((item.file, bodies));
        }
        let (_, bodies) = cache.as_ref()?;
        bodies.get(item.index as usize)?.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn item(id: u64, feed: u64, unread: bool, starred: bool) -> String {
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
            &page(&[item(5, 10, true, false), item(4, 11, true, true)]),
        );
        dir.write("unread-0002.json", &page(&[item(3, 99, true, false)]));
        dir.write(
            "starred-0001.json",
            &page(&[item(4, 11, true, true), item(1, 10, false, true)]),
        );
        dir.write("version.json", r#"{"version": "28.7.0"}"#);
        dir
    }

    #[test]
    fn load_and_read_bodies() {
        let dir = dump("load");
        let dump = Dump::load(&dir.0, 100).unwrap();
        assert_eq!(dump.folders.len(), 1);
        assert_eq!(dump.feeds[0].folder_id, Some(0));
        assert_eq!(dump.feeds[1].folder_id, None);
        assert_eq!(dump.feeds[1].title, "https://b/feed");

        // Item 3 has an unknown feed, item 4 is in two files.
        let ids: Vec<_> = dump.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(ids, ["Item 5", "Item 4", "Item 1"]);
        assert!(!dump.items[2].unread && dump.items[2].starred);

        assert_eq!(dump.body(&dump.items[1]).as_deref(), Some("<p>Body 4</p>"));
        assert_eq!(dump.body(&dump.items[2]).as_deref(), Some("<p>Body 1</p>"));
        assert_eq!(dump.body(&dump.items[0]).as_deref(), Some("<p>Body 5</p>"));
    }

    #[test]
    fn single_line_fields() {
        assert_eq!(one_line("  A\n  B\t C \r\n"), "A B C");
        assert_eq!(one_line(" \n "), "");

        let dir = dump("lines");
        dir.write(
            "folders.json",
            r#"{"folders": [{"id": 7, "name": " Tech\n"}]}"#,
        );
        dir.write(
            "feeds.json",
            r#"{"feeds": [{"id": 10, "url": "https://a/feed", "title": " A ", "folderId": 7},
                          {"id": 11, "url": "https://b/feed", "title": " ", "folderId": 0}]}"#,
        );
        dir.write(
            "unread-0001.json",
            r#"{"items": [{"id": 1, "feedId": 10, "title": "Two\n lines",
                           "author": "Someone,\n Someone Else"}]}"#,
        );
        dir.write("unread-0002.json", r#"{"items": []}"#);
        dir.write("starred-0001.json", r#"{"items": []}"#);
        let dump = Dump::load(&dir.0, 10).unwrap();
        assert_eq!(dump.folders[0].name, "Tech");
        assert_eq!(dump.feeds[0].title, "A");
        assert_eq!(dump.feeds[1].title, "https://b/feed");
        assert_eq!(dump.items[0].title, "Two lines");
        assert_eq!(dump.items[0].author, "Someone, Someone Else");
    }

    #[test]
    fn limit() {
        let dir = dump("limit");
        assert_eq!(Dump::load(&dir.0, 1).unwrap().items.len(), 1);
    }

    #[test]
    fn missing_files() {
        let dir = TempDir::new("missing");
        let Err(err) = Dump::load(&dir.0, 10) else {
            panic!("loaded an empty directory");
        };
        assert!(err.contains("folders.json"));
    }
}
