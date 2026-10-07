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

use rusqlite::{Connection, Transaction, TransactionBehavior};

pub use changes::{MarkReadScope, PendingMarkRead};
pub use error::Error;
pub use feeds::{Feed, FeedChanges, FeedSettings, Folder};
pub use items::{ItemDetails, ItemQuery, ItemSummary, Listed, Selection, Sort, SortColumn, Status};

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// How long a write waits for another connection's transaction to end.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Switches the database to the write-ahead log. Switching a new database
/// fails at once, without waiting for the busy timeout, while another
/// connection switches it too; the mode is stored in the file, so trying
/// again succeeds once the other connection is done.
fn set_wal(conn: &Connection) -> Result<()> {
    let started = std::time::Instant::now();
    loop {
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::DatabaseBusy
                    && started.elapsed() < BUSY_TIMEOUT =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            result => return Ok(result?),
        }
    }
}

/// The local database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens the database at `path`, creating it if it doesn't exist, and
    /// brings its schema up to date. The directory must exist.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        // Before anything else, so that opening a new database while the
        // other connection sets it up waits instead of failing.
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // The write-ahead log lets the UI read while the sync writes, and
        // with it `NORMAL` is safe against corruption (a power loss can
        // lose the last transactions, which the next sync fetches again).
        set_wal(&conn)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::setup(conn)
    }

    /// A database in memory, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::setup(Connection::open_in_memory()?)
    }

    /// Starts a transaction that writes. It takes the write lock at once
    /// (`BEGIN IMMEDIATE`): a transaction that reads first and writes later
    /// fails at once, without waiting, if the other connection wrote in
    /// between, since its snapshot is stale then. Taking the lock first
    /// makes the other connection's writes wait instead.
    fn write_transaction(&mut self) -> Result<Transaction<'_>> {
        Ok(self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?)
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
mod tests {
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use super::*;
    use crate::test_util::{TempDb, api_item};

    #[test]
    fn a_write_transaction_holds_the_lock_from_its_start() {
        let db = TempDb::new();
        let mut sync = Store::open(&db.0).unwrap();
        sync.upsert_items(&[api_item(1, 1)], false).unwrap();
        let mut ui = Store::open(&db.0).unwrap();
        ui.conn.busy_timeout(Duration::from_millis(20)).unwrap();

        // The sync reads first, as replace_feeds does. With a deferred
        // transaction, the UI's write would get through here, and the
        // sync's own write would then fail at once.
        let tx = sync.write_transaction().unwrap();
        let _: u64 = tx
            .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
            .unwrap();
        assert!(
            ui.set_unread(&[1], false).is_err(),
            "the UI wrote in between"
        );
        tx.execute("UPDATE items SET title = 'x' WHERE id = 1", [])
            .unwrap();
        tx.commit().unwrap();

        // Once the sync is done, the UI's write goes through.
        assert_eq!(ui.set_unread(&[1], false).unwrap(), 1);
    }

    #[test]
    fn two_connections_can_create_the_database_together() {
        for _ in 0..10 {
            let db = Arc::new(TempDb::new());
            let barrier = Arc::new(Barrier::new(2));
            let threads: Vec<_> = (0..2)
                .map(|_| {
                    let (db, barrier) = (db.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        Store::open(&db.0).map(|_| ())
                    })
                })
                .collect();
            for thread in threads {
                thread.join().unwrap().unwrap();
            }
        }
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
