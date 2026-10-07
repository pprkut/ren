// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The schema and its migrations.
//!
//! `PRAGMA user_version` holds the number of migrations applied. Each
//! migration runs in a transaction with the version update, so a database
//! is never left half migrated. Migrations are only ever appended.

use rusqlite::{Connection, TransactionBehavior};

use crate::{Error, Result};

/// Notes on the schema:
///
/// - Ids are the server's. A `folder_id` of 0 (older News versions) is
///   stored as `NULL`, like a feed outside of folders.
/// - `items.feed_id` and `feeds.folder_id` have no foreign keys: the
///   server can send an item before the feed it belongs to (a feed added
///   between `GET /feeds` and `GET /items/updated`), and the item must not
///   be lost then, since the sync cursor moves past it. Lists only show
///   items of known feeds; the sync deletes the items of removed feeds.
/// - Bodies are kept apart from the item rows (`item_contents`), so the
///   rows scanned for lists and counts stay small: a body of a few KiB in
///   the row would take a page of its own per item.
/// - `pending_changes` holds the read and starred states changed locally
///   and not yet sent to the server, one row per item and field (0: unread,
///   1: starred) with the new value.
/// - `items.title_key` and `author_key` are the lower case of title and
///   author, for sorting and searching (see `text`).
/// - `items.is_new` marks items that arrived in the latest sync.
/// - The indices serve the list queries (by date, of a feed, starred), the
///   unread counts and clearing the "new" marker.
///
/// Version 2:
///
/// - `pending_mark_read` holds the "mark all as read" requests not yet sent
///   to the server, in the order they were made (`id`): the items up to
///   `newest_item_id` of everything (`scope` 0), a folder (1, `scope_id`),
///   the feeds outside of folders (2) or a feed (3, `scope_id`).
const MIGRATIONS: &[&str] = &[
    r"
CREATE TABLE folders (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL
);

CREATE TABLE feeds (
    id INTEGER PRIMARY KEY,
    folder_id INTEGER,
    title TEXT NOT NULL,
    url TEXT NOT NULL,
    link TEXT,
    favicon_link TEXT,
    added INTEGER,
    ordering INTEGER NOT NULL,
    pinned INTEGER NOT NULL,
    update_error_count INTEGER NOT NULL,
    last_update_error TEXT
);

CREATE TABLE feed_settings (
    feed_id INTEGER PRIMARY KEY REFERENCES feeds (id) ON DELETE CASCADE,
    open_full_page INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE items (
    id INTEGER PRIMARY KEY,
    feed_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    author TEXT NOT NULL,
    title_key TEXT NOT NULL,
    author_key TEXT NOT NULL,
    pub_date INTEGER NOT NULL,
    url TEXT,
    enclosure_mime TEXT,
    enclosure_link TEXT,
    media_thumbnail TEXT,
    fingerprint TEXT,
    last_modified INTEGER,
    unread INTEGER NOT NULL,
    starred INTEGER NOT NULL,
    filtered INTEGER NOT NULL,
    rtl INTEGER NOT NULL,
    is_new INTEGER NOT NULL
);

CREATE TABLE item_contents (
    item_id INTEGER PRIMARY KEY REFERENCES items (id) ON DELETE CASCADE,
    body TEXT,
    media_description TEXT
);

CREATE TABLE pending_changes (
    item_id INTEGER NOT NULL REFERENCES items (id) ON DELETE CASCADE,
    field INTEGER NOT NULL,
    value INTEGER NOT NULL,
    PRIMARY KEY (item_id, field)
) WITHOUT ROWID;

CREATE TABLE sync_state (
    key TEXT PRIMARY KEY,
    value
) WITHOUT ROWID;

CREATE INDEX items_date ON items (pub_date);
CREATE INDEX items_feed ON items (feed_id, pub_date);
CREATE INDEX items_unread ON items (feed_id) WHERE unread = 1 AND filtered = 0;
CREATE INDEX items_starred ON items (pub_date) WHERE starred = 1;
CREATE INDEX items_new ON items (id) WHERE is_new = 1;
",
    r"
CREATE TABLE pending_mark_read (
    id INTEGER PRIMARY KEY,
    scope INTEGER NOT NULL,
    scope_id INTEGER,
    newest_item_id INTEGER NOT NULL
);
",
];

/// The schema version this build creates and understands.
#[cfg(test)]
const VERSION: u32 = MIGRATIONS.len() as u32;

pub fn migrate(conn: &mut Connection) -> Result<()> {
    migrate_with(conn, MIGRATIONS)
}

/// Applies the missing migrations, one transaction each. The version is
/// read inside the transaction, which holds the write lock from the start:
/// when two connections open a new database at the same time (the UI and
/// the sync thread), the second waits for the first and then finds its
/// migrations applied.
fn migrate_with(conn: &mut Connection, migrations: &[&str]) -> Result<()> {
    let supported = migrations.len() as u32;
    loop {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: u32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > supported {
            return Err(Error::TooNew { version, supported });
        }
        let Some(sql) = migrations.get(version as usize) else {
            return Ok(());
        };
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version + 1)?;
        tx.commit()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    use crate::test_util::TempDb;

    fn version(conn: &Connection) -> u32 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn creates_the_schema() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(version(&store.conn), VERSION);
        let tables: Vec<String> = store
            .conn
            .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            tables,
            [
                "feed_settings",
                "feeds",
                "folders",
                "item_contents",
                "items",
                "pending_changes",
                "pending_mark_read",
                "sync_state"
            ]
        );
    }

    #[test]
    fn reopening_keeps_the_data() {
        let db = TempDb::new();
        {
            let store = Store::open(&db.0).unwrap();
            store
                .conn
                .execute("INSERT INTO folders (id, name) VALUES (1, 'News')", [])
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        assert_eq!(version(&store.conn), VERSION);
        let name: String = store
            .conn
            .query_row("SELECT name FROM folders WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "News");
        let mode: String = store
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn applies_only_missing_migrations() {
        let mut conn = Connection::open_in_memory().unwrap();
        let first = ["CREATE TABLE a (x)"];
        migrate_with(&mut conn, &first).unwrap();
        assert_eq!(version(&conn), 1);
        // Running the first migration again would fail: the table exists.
        let both = ["CREATE TABLE a (x)", "ALTER TABLE a ADD COLUMN y"];
        migrate_with(&mut conn, &both).unwrap();
        assert_eq!(version(&conn), 2);
        conn.execute("INSERT INTO a (x, y) VALUES (1, 2)", [])
            .unwrap();
    }

    #[test]
    fn a_failed_migration_changes_nothing() {
        let mut conn = Connection::open_in_memory().unwrap();
        let migrations = ["CREATE TABLE a (x); CREATE TABLE a (y)"];
        assert!(migrate_with(&mut conn, &migrations).is_err());
        assert_eq!(version(&conn), 0);
        let tables: u32 = conn
            .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tables, 0);
    }

    #[test]
    fn refuses_a_newer_schema() {
        let db = TempDb::new();
        {
            let conn = Connection::open(&db.0).unwrap();
            conn.pragma_update(None, "user_version", VERSION + 1)
                .unwrap();
        }
        match Store::open(&db.0) {
            Err(Error::TooNew { version, supported }) => {
                assert_eq!(version, VERSION + 1);
                assert_eq!(supported, VERSION);
            }
            Err(err) => panic!("unexpected error: {err}"),
            Ok(_) => panic!("opened a database with a newer schema"),
        }
    }
}
