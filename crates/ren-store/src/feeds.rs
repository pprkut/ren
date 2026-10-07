// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Folders, feeds and per-feed settings.

use std::collections::HashSet;

use nextcloud_news::types;
use rusqlite::{OptionalExtension, Row, params};

use crate::text::collapse;
use crate::{Result, Store};

/// A folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: u64,
    /// With whitespace collapsed.
    pub name: String,
}

/// A feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feed {
    pub id: u64,
    /// `None` for feeds outside of folders.
    pub folder_id: Option<u64>,
    /// With whitespace collapsed; empty if the feed has none. Not
    /// sanitised.
    pub title: String,
    /// The feed URL.
    pub url: String,
    /// The web site. Not sanitised.
    pub link: Option<String>,
    pub favicon_link: Option<String>,
    /// Unix seconds.
    pub added: Option<i64>,
    /// 0: default, 1: oldest first, 2: newest first.
    pub ordering: u8,
    pub pinned: bool,
    pub update_error_count: u64,
    pub last_update_error: Option<String>,
}

/// Settings of ren's own for a feed, kept locally.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedSettings {
    /// Open the full page in a tab instead of the article, for feeds that
    /// only publish teasers.
    pub open_full_page: bool,
}

/// What [`Store::replace_feeds`] changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedChanges {
    /// Feeds that are gone, with their items.
    pub removed: Vec<u64>,
    /// Feeds that weren't there before.
    pub added: Vec<u64>,
}

/// A folder id of 0 (older News versions) means no folder.
fn folder_id(id: Option<u64>) -> Option<u64> {
    id.filter(|&id| id != 0)
}

impl Store {
    /// Replaces the folders with the server's list: updates and adds, and
    /// deletes the folders that are not in it. Feeds in a deleted folder
    /// are shown at the top level until [`replace_feeds`](Self::replace_feeds)
    /// moves them.
    pub fn replace_folders(&mut self, folders: &[types::Folder]) -> Result<()> {
        let tx = self.write_transaction()?;
        {
            let mut upsert = tx.prepare_cached(
                "INSERT INTO folders (id, name) VALUES (?1, ?2)
                 ON CONFLICT (id) DO UPDATE SET name = excluded.name",
            )?;
            for folder in folders {
                upsert.execute(params![folder.id, collapse(&folder.name)])?;
            }
            let keep: HashSet<u64> = folders.iter().map(|f| f.id).collect();
            let stored: Vec<u64> = tx
                .prepare("SELECT id FROM folders")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for id in stored.into_iter().filter(|id| !keep.contains(id)) {
                tx.execute("DELETE FROM folders WHERE id = ?1", [id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Replaces the feeds with the server's list: updates and adds, and
    /// deletes the feeds that are not in it, with their items, settings and
    /// pending changes.
    pub fn replace_feeds(&mut self, feeds: &[types::Feed]) -> Result<FeedChanges> {
        let tx = self.write_transaction()?;
        let mut changes = FeedChanges::default();
        {
            let known: HashSet<u64> = tx
                .prepare("SELECT id FROM feeds")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut upsert = tx.prepare_cached(
                "INSERT INTO feeds (id, folder_id, title, url, link, favicon_link, added,
                     ordering, pinned, update_error_count, last_update_error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT (id) DO UPDATE SET
                     folder_id = excluded.folder_id, title = excluded.title,
                     url = excluded.url, link = excluded.link,
                     favicon_link = excluded.favicon_link, added = excluded.added,
                     ordering = excluded.ordering, pinned = excluded.pinned,
                     update_error_count = excluded.update_error_count,
                     last_update_error = excluded.last_update_error",
            )?;
            for feed in feeds {
                upsert.execute(params![
                    feed.id,
                    folder_id(feed.folder_id),
                    collapse(feed.title.as_deref().unwrap_or_default()),
                    feed.url,
                    feed.link,
                    feed.favicon_link,
                    feed.added,
                    feed.ordering,
                    feed.pinned,
                    feed.update_error_count,
                    feed.last_update_error,
                ])?;
                if !known.contains(&feed.id) {
                    changes.added.push(feed.id);
                }
            }
            let keep: HashSet<u64> = feeds.iter().map(|f| f.id).collect();
            changes.removed = known.into_iter().filter(|id| !keep.contains(id)).collect();
            changes.removed.sort_unstable();
            // Item contents and pending changes go with the items, settings
            // with the feed (foreign keys).
            for &id in &changes.removed {
                tx.execute("DELETE FROM items WHERE feed_id = ?1", [id])?;
                tx.execute("DELETE FROM feeds WHERE id = ?1", [id])?;
            }
        }
        tx.commit()?;
        Ok(changes)
    }

    /// All folders, sorted by name.
    pub fn folders(&self) -> Result<Vec<Folder>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, name FROM folders ORDER BY name COLLATE caseless, id")?;
        let folders = stmt
            .query_map([], |row| {
                Ok(Folder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(folders)
    }

    /// All feeds, sorted by title.
    pub fn feeds(&self) -> Result<Vec<Feed>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, folder_id, title, url, link, favicon_link, added, ordering, pinned,
                 update_error_count, last_update_error
             FROM feeds ORDER BY title COLLATE caseless, id",
        )?;
        let feeds = stmt
            .query_map([], feed_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(feeds)
    }

    /// The number of unread items per feed, for feeds that have any.
    /// Filtered items are not counted, as they are not shown.
    pub fn unread_counts(&self) -> Result<Vec<(u64, u64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT feed_id, count(*) FROM items WHERE unread = 1 AND filtered = 0
             GROUP BY feed_id",
        )?;
        let counts = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(counts)
    }

    /// The number of starred items, as listed with
    /// [`Selection::Starred`](crate::Selection::Starred).
    pub fn starred_count(&self) -> Result<u64> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT count(*) FROM items i JOIN feeds f ON f.id = i.feed_id
                 WHERE i.starred = 1 AND i.filtered = 0",
            )?
            .query_row([], |row| row.get(0))?)
    }

    /// The settings of a feed; the defaults if none were changed.
    pub fn feed_settings(&self, feed_id: u64) -> Result<FeedSettings> {
        let settings = self
            .conn
            .prepare_cached("SELECT open_full_page FROM feed_settings WHERE feed_id = ?1")?
            .query_row([feed_id], |row| {
                Ok(FeedSettings {
                    open_full_page: row.get(0)?,
                })
            })
            .optional()?;
        Ok(settings.unwrap_or_default())
    }

    /// Changes the settings of a feed. Fails if the feed doesn't exist.
    pub fn set_feed_settings(&mut self, feed_id: u64, settings: &FeedSettings) -> Result<()> {
        if *settings == FeedSettings::default() {
            self.conn
                .execute("DELETE FROM feed_settings WHERE feed_id = ?1", [feed_id])?;
        } else {
            self.conn.execute(
                "INSERT INTO feed_settings (feed_id, open_full_page) VALUES (?1, ?2)
                 ON CONFLICT (feed_id) DO UPDATE SET open_full_page = excluded.open_full_page",
                params![feed_id, settings.open_full_page],
            )?;
        }
        Ok(())
    }
}

fn feed_from_row(row: &Row<'_>) -> rusqlite::Result<Feed> {
    Ok(Feed {
        id: row.get(0)?,
        folder_id: row.get(1)?,
        title: row.get(2)?,
        url: row.get(3)?,
        link: row.get(4)?,
        favicon_link: row.get(5)?,
        added: row.get(6)?,
        ordering: row.get(7)?,
        pinned: row.get(8)?,
        update_error_count: row.get(9)?,
        last_update_error: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{api_feed, api_folder, api_item};

    #[test]
    fn folders_are_replaced() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .replace_folders(&[api_folder(1, "Tech"), api_folder(2, " Science\n")])
            .unwrap();
        store
            .replace_folders(&[api_folder(2, "Science"), api_folder(3, "comics")])
            .unwrap();
        let folders = store.folders().unwrap();
        assert_eq!(
            folders,
            [
                Folder {
                    id: 3,
                    name: "comics".into()
                },
                Folder {
                    id: 2,
                    name: "Science".into()
                },
            ]
        );
    }

    #[test]
    fn feeds_are_stored_as_sent() {
        let mut store = Store::open_in_memory().unwrap();
        let feed = types::Feed {
            id: 39,
            url: "http://example.org/feed".into(),
            title: Some("  Example\n Feed ".into()),
            favicon_link: Some("http://example.org/favicon.ico".into()),
            added: Some(1_367_063_790),
            folder_id: Some(4),
            unread_count: 9,
            ordering: 2,
            link: Some("http://example.org/".into()),
            pinned: true,
            update_error_count: 3,
            last_update_error: Some("timeout".into()),
        };
        store.replace_feeds(&[feed]).unwrap();
        assert_eq!(
            store.feeds().unwrap(),
            [Feed {
                id: 39,
                folder_id: Some(4),
                title: "Example Feed".into(),
                url: "http://example.org/feed".into(),
                link: Some("http://example.org/".into()),
                favicon_link: Some("http://example.org/favicon.ico".into()),
                added: Some(1_367_063_790),
                ordering: 2,
                pinned: true,
                update_error_count: 3,
                last_update_error: Some("timeout".into()),
            }]
        );
    }

    #[test]
    fn folder_zero_is_no_folder() {
        let mut store = Store::open_in_memory().unwrap();
        let mut old = api_feed(1, Some(0), "Old");
        old.title = None;
        store
            .replace_feeds(&[old, api_feed(2, None, "New"), api_feed(3, Some(5), "In")])
            .unwrap();
        let feeds = store.feeds().unwrap();
        let folders: Vec<_> = feeds.iter().map(|f| (f.id, f.folder_id)).collect();
        // Sorted by title; a missing title is empty.
        assert_eq!(folders, [(1, None), (3, Some(5)), (2, None)]);
        assert_eq!(feeds[0].title, "");
    }

    #[test]
    fn feeds_are_replaced_with_their_items() {
        let mut store = Store::open_in_memory().unwrap();
        let changes = store
            .replace_feeds(&[api_feed(1, None, "One"), api_feed(2, None, "Two")])
            .unwrap();
        assert_eq!(changes.added, [1, 2]);
        assert!(changes.removed.is_empty());
        store
            .upsert_items(&[api_item(10, 1), api_item(20, 2), api_item(21, 2)], false)
            .unwrap();
        store.set_starred(&[21], true).unwrap();
        store
            .set_feed_settings(
                2,
                &FeedSettings {
                    open_full_page: true,
                },
            )
            .unwrap();

        let changes = store
            .replace_feeds(&[api_feed(1, None, "Renamed"), api_feed(3, None, "Three")])
            .unwrap();
        assert_eq!(changes.added, [3]);
        assert_eq!(changes.removed, [2]);
        let titles: Vec<_> = store
            .feeds()
            .unwrap()
            .into_iter()
            .map(|f| f.title)
            .collect();
        assert_eq!(titles, ["Renamed", "Three"]);
        assert!(store.item(10).unwrap().is_some());
        assert!(store.item(20).unwrap().is_none());
        assert!(store.item(21).unwrap().is_none());
        assert_eq!(store.pending_count().unwrap(), 0);
        let rows: u64 = store
            .conn
            .query_row(
                "SELECT (SELECT count(*) FROM item_contents)
                     + (SELECT count(*) FROM feed_settings)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn unread_and_starred_counts() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .replace_feeds(&[api_feed(1, None, "One"), api_feed(2, None, "Two")])
            .unwrap();
        let mut read = api_item(3, 1);
        read.unread = false;
        read.starred = true;
        let mut filtered = api_item(4, 1);
        filtered.filtered = true;
        filtered.starred = true;
        store
            .upsert_items(
                &[
                    api_item(1, 1),
                    api_item(2, 1),
                    read,
                    filtered,
                    api_item(5, 2),
                ],
                false,
            )
            .unwrap();
        let mut counts = store.unread_counts().unwrap();
        counts.sort_unstable();
        assert_eq!(counts, [(1, 2), (2, 1)]);
        assert_eq!(store.starred_count().unwrap(), 1);
    }

    #[test]
    fn feed_settings() {
        let mut store = Store::open_in_memory().unwrap();
        store.replace_feeds(&[api_feed(1, None, "One")]).unwrap();
        assert_eq!(store.feed_settings(1).unwrap(), FeedSettings::default());
        let full = FeedSettings {
            open_full_page: true,
        };
        store.set_feed_settings(1, &full).unwrap();
        assert_eq!(store.feed_settings(1).unwrap(), full);
        store
            .set_feed_settings(1, &FeedSettings::default())
            .unwrap();
        assert_eq!(store.feed_settings(1).unwrap(), FeedSettings::default());
        // Unknown feeds have no settings.
        assert!(store.set_feed_settings(9, &full).is_err());
        assert_eq!(store.feed_settings(9).unwrap(), FeedSettings::default());
    }
}
