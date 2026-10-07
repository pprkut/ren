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
        self.set_sync_values(&[(CURSOR, Some(last_modified))])
    }

    /// A number the sync keeps between runs, e.g. the progress of a resync.
    pub fn sync_value(&self, key: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .prepare_cached("SELECT value FROM sync_state WHERE key = ?1")?
            .query_row([key], |row| row.get(0))
            .optional()?)
    }

    /// Sets numbers the sync keeps between runs (`Some`), or removes them
    /// (`None`), in one transaction.
    pub fn set_sync_values(&mut self, values: &[(&str, Option<i64>)]) -> Result<()> {
        let tx = self.write_transaction()?;
        {
            let mut set = tx.prepare_cached(
                "INSERT INTO sync_state (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            )?;
            let mut remove = tx.prepare_cached("DELETE FROM sync_state WHERE key = ?1")?;
            for &(key, value) in values {
                match value {
                    Some(value) => set.execute(params![key, value])?,
                    None => remove.execute([key])?,
                };
            }
        }
        tx.commit()?;
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

    #[test]
    fn values() {
        let mut store = Store::open_in_memory().unwrap();
        assert_eq!(store.sync_value("a").unwrap(), None);
        store
            .set_sync_values(&[("a", Some(1)), ("b", Some(-2))])
            .unwrap();
        store
            .set_sync_values(&[("a", None), ("b", Some(3)), ("c", None)])
            .unwrap();
        assert_eq!(store.sync_value("a").unwrap(), None);
        assert_eq!(store.sync_value("b").unwrap(), Some(3));
        assert_eq!(store.sync_cursor().unwrap(), None);
    }
}
