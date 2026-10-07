// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Local changes of the read and starred state, and the queue that takes
//! them to the server.
//!
//! A change is written to the item and queued in one transaction, so the UI
//! shows it at once and the sync sends it later. The queue holds the latest
//! value per item and field: marking an item read and unread again before
//! the sync sends only "unread". Entries are removed after the server has
//! accepted them, and only if the value is still the one that was sent.

use nextcloud_news::ItemAction;
use rusqlite::params;

use crate::{Result, Store};

/// `pending_changes.field` of the read state; the value is `items.unread`.
pub(crate) const UNREAD: i64 = 0;
/// `pending_changes.field` of the starred state; the value is
/// `items.starred`.
pub(crate) const STARRED: i64 = 1;

/// The field and value in the queue for what an action does.
fn field_value(action: ItemAction) -> (i64, bool) {
    match action {
        ItemAction::Read => (UNREAD, false),
        ItemAction::Unread => (UNREAD, true),
        ItemAction::Star => (STARRED, true),
        ItemAction::Unstar => (STARRED, false),
    }
}

impl Store {
    /// Marks items as read or unread, and queues the change for the server.
    /// Returns the number of items that changed; unknown items and items
    /// already in that state are skipped.
    pub fn set_unread(&mut self, ids: &[u64], unread: bool) -> Result<usize> {
        self.set_flag(ids, UNREAD, unread)
    }

    /// Stars or unstars items, and queues the change for the server.
    /// Returns the number of items that changed; unknown items and items
    /// already in that state are skipped.
    pub fn set_starred(&mut self, ids: &[u64], starred: bool) -> Result<usize> {
        self.set_flag(ids, STARRED, starred)
    }

    fn set_flag(&mut self, ids: &[u64], field: i64, value: bool) -> Result<usize> {
        let column = if field == UNREAD { "unread" } else { "starred" };
        let tx = self.conn.transaction()?;
        let mut changed = 0;
        {
            let mut update = tx.prepare_cached(&format!(
                "UPDATE items SET {column} = ?2 WHERE id = ?1 AND {column} != ?2"
            ))?;
            let mut enqueue = tx.prepare_cached(
                "INSERT INTO pending_changes (item_id, field, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT (item_id, field) DO UPDATE SET value = excluded.value",
            )?;
            for &id in ids {
                if update.execute(params![id, value])? == 1 {
                    enqueue.execute(params![id, field, value])?;
                    changed += 1;
                }
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Up to `limit` items with a pending change that `action` sends to the
    /// server, by id.
    pub fn pending(&self, action: ItemAction, limit: usize) -> Result<Vec<u64>> {
        let (field, value) = field_value(action);
        let mut stmt = self.conn.prepare_cached(
            "SELECT item_id FROM pending_changes WHERE field = ?1 AND value = ?2
             ORDER BY item_id LIMIT ?3",
        )?;
        let ids = stmt
            .query_map(params![field, value, limit], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// Removes the pending changes `action` has sent to the server for
    /// these items. Items changed again since are kept in the queue.
    pub fn remove_pending(&mut self, action: ItemAction, ids: &[u64]) -> Result<()> {
        let (field, value) = field_value(action);
        let tx = self.conn.transaction()?;
        {
            let mut delete = tx.prepare_cached(
                "DELETE FROM pending_changes WHERE item_id = ?1 AND field = ?2 AND value = ?3",
            )?;
            for &id in ids {
                delete.execute(params![id, field, value])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The number of changes waiting to be sent.
    pub fn pending_count(&self) -> Result<u64> {
        Ok(self
            .conn
            .prepare_cached("SELECT count(*) FROM pending_changes")?
            .query_row([], |row| row.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Status;
    use crate::test_util::api_item;
    use nextcloud_news::types;

    fn store() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        let read = types::Item {
            unread: false,
            ..api_item(3, 1)
        };
        store
            .upsert_items(&[api_item(1, 1), api_item(2, 1), read], false)
            .unwrap();
        store
    }

    fn status(store: &Store, id: u64) -> (Status, bool) {
        let item = store.item(id).unwrap().unwrap().summary;
        (item.status, item.starred)
    }

    #[test]
    fn changes_are_applied_and_queued() {
        let mut store = store();
        // Item 3 is read already, 9 doesn't exist.
        assert_eq!(store.set_unread(&[1, 3, 9], false).unwrap(), 1);
        assert_eq!(store.set_starred(&[2, 3], true).unwrap(), 2);
        assert_eq!(status(&store, 1), (Status::Read, false));
        assert_eq!(status(&store, 2), (Status::Unread, true));
        assert_eq!(status(&store, 3), (Status::Read, true));
        assert_eq!(store.pending(ItemAction::Read, 10).unwrap(), [1]);
        assert_eq!(store.pending(ItemAction::Star, 10).unwrap(), [2, 3]);
        assert!(store.pending(ItemAction::Unread, 10).unwrap().is_empty());
        assert!(store.pending(ItemAction::Unstar, 10).unwrap().is_empty());
        assert_eq!(store.pending_count().unwrap(), 3);
        assert_eq!(store.pending(ItemAction::Star, 1).unwrap(), [2]);
    }

    #[test]
    fn the_latest_change_is_queued() {
        let mut store = store();
        store.set_unread(&[1], false).unwrap();
        store.set_unread(&[1], true).unwrap();
        assert!(store.pending(ItemAction::Read, 10).unwrap().is_empty());
        assert_eq!(store.pending(ItemAction::Unread, 10).unwrap(), [1]);
        assert_eq!(store.pending_count().unwrap(), 1);
    }

    #[test]
    fn sent_changes_are_removed() {
        let mut store = store();
        store.set_unread(&[1, 2], false).unwrap();
        let sent = store.pending(ItemAction::Read, 10).unwrap();
        // Item 2 is marked unread again while the request is underway.
        store.set_unread(&[2], true).unwrap();
        store.remove_pending(ItemAction::Read, &sent).unwrap();
        assert!(store.pending(ItemAction::Read, 10).unwrap().is_empty());
        assert_eq!(store.pending(ItemAction::Unread, 10).unwrap(), [2]);
    }

    #[test]
    fn pending_changes_survive_updates_from_the_server() {
        let mut store = store();
        store.set_unread(&[1], false).unwrap();
        store.set_starred(&[2], true).unwrap();
        // The server still has the old state, and other changes.
        let one = types::Item {
            title: Some("New title".into()),
            starred: true,
            ..api_item(1, 1)
        };
        let two = types::Item {
            unread: false,
            ..api_item(2, 1)
        };
        store.upsert_items(&[one, two], false).unwrap();
        assert_eq!(status(&store, 1), (Status::Read, true));
        assert_eq!(store.item(1).unwrap().unwrap().summary.title, "New title");
        assert_eq!(status(&store, 2), (Status::Read, true));

        // Once sent, the server's state counts again.
        store.remove_pending(ItemAction::Read, &[1]).unwrap();
        store.upsert_items(&[api_item(1, 1)], false).unwrap();
        assert_eq!(status(&store, 1), (Status::Unread, false));
    }
}
