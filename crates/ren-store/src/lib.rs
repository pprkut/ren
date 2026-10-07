// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Local storage of ren: folders, feeds and items in SQLite.
//!
//! The UI reads only from the store, the sync engine writes the server's
//! state into it, and local changes (read, starred) are written here first
//! and queued for the server. Lists are queried page by page, so only the
//! visible rows are ever in memory.
//!
//! A [`Store`] is one connection to the database. It can be moved to
//! another thread but not shared: the UI and the sync thread each open
//! their own. The database uses SQLite's write-ahead log, so reading is not
//! blocked while the sync writes.

mod changes;
mod error;
mod feeds;
mod items;
mod schema;
mod state;
mod text;

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

pub use error::Error;
pub use feeds::{Feed, FeedChanges, FeedSettings, Folder};
pub use items::{ItemDetails, ItemQuery, ItemSummary, Selection, Sort, SortColumn, Status};

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// How long a write waits for another connection's transaction to end.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The local database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens the database at `path`, creating it if it doesn't exist, and
    /// brings its schema up to date. The directory must exist.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        // The write-ahead log lets the UI read while the sync writes, and
        // with it `NORMAL` is safe against corruption (a power loss can
        // lose the last transactions, which the next sync fetches again).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::setup(conn)
    }

    /// A database in memory, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::setup(Connection::open_in_memory()?)
    }

    fn setup(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        text::register(&conn)?;
        schema::migrate(&mut conn)?;
        Ok(Self { conn })
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nextcloud_news::types;

    pub fn api_folder(id: u64, name: &str) -> types::Folder {
        types::Folder {
            id,
            name: name.into(),
        }
    }

    pub fn api_feed(id: u64, folder_id: Option<u64>, title: &str) -> types::Feed {
        types::Feed {
            id,
            url: format!("https://example.org/{id}/feed"),
            title: Some(title.into()),
            favicon_link: None,
            added: Some(1_700_000_000),
            folder_id,
            unread_count: 0,
            ordering: 0,
            link: None,
            pinned: false,
            update_error_count: 0,
            last_update_error: None,
        }
    }

    /// An unread item, published at `id * 100`.
    pub fn api_item(id: u64, feed_id: u64) -> types::Item {
        types::Item {
            id,
            guid: Some(format!("guid-{id}")),
            guid_hash: None,
            url: Some(format!("https://example.org/{feed_id}/{id}")),
            title: Some(format!("Item {id}")),
            author: Some("Author".into()),
            pub_date: Some(id.cast_signed() * 100),
            updated_date: None,
            body: Some(format!("<p>Body {id}</p>")),
            enclosure_mime: None,
            enclosure_link: None,
            media_thumbnail: None,
            media_description: None,
            feed_id,
            unread: true,
            starred: false,
            filtered: false,
            rtl: false,
            last_modified: Some(1_700_000_000),
            fingerprint: None,
            content_hash: None,
        }
    }

    /// A database file in the temporary directory, removed when dropped.
    pub struct TempDb(pub PathBuf);

    impl TempDb {
        pub fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let name = format!(
                "ren-store-test-{}-{}.db",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            Self(std::env::temp_dir().join(name))
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let mut path = self.0.clone().into_os_string();
                path.push(suffix);
                let _ = std::fs::remove_file(path);
            }
        }
    }
}
