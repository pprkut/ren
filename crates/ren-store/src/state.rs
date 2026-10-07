// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the sync remembers between runs.

use rusqlite::{OptionalExtension, params};

use crate::{Result, Store};

/// The `lastModified` up to which the server's changes are stored.
const CURSOR: &str = "cursor";

impl Store {
    /// The sync cursor: the `lastModified` (unix seconds) up to which the
    /// server's changes are stored. `None` before the first sync.
    pub fn sync_cursor(&self) -> Result<Option<i64>> {
        Ok(self
            .conn
            .prepare_cached("SELECT value FROM sync_state WHERE key = ?1")?
            .query_row([CURSOR], |row| row.get(0))
            .optional()?)
    }

    pub fn set_sync_cursor(&mut self, last_modified: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sync_state (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![CURSOR, last_modified],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor() {
        let mut store = Store::open_in_memory().unwrap();
        assert_eq!(store.sync_cursor().unwrap(), None);
        store.set_sync_cursor(1_367_273_003).unwrap();
        store.set_sync_cursor(1_367_280_000).unwrap();
        assert_eq!(store.sync_cursor().unwrap(), Some(1_367_280_000));
    }
}
