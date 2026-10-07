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
//!
//! "Mark all as read" is queued as such, as one request for the server
//! instead of one id per item, in a queue of its own. It replaces the
//! queued read state of the items it covers, so the sync sends these
//! requests before the changes of single items: whatever is queued for an
//! item is newer than the "mark all as read" covering it.

use nextcloud_news::ItemAction;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter};

use crate::{Result, Store};

/// `pending_changes.field` of the read state; the value is `items.unread`.
pub(crate) const UNREAD: i64 = 0;
/// `pending_changes.field` of the starred state; the value is
/// `items.starred`.
pub(crate) const STARRED: i64 = 1;

/// Which items "mark all as read" marks: those up to a given id in one of
/// these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkReadScope {
    All,
    Folder(u64),
    /// The feeds outside of folders. The server has no request for these
    /// (folder 0 fails), so the sync marks them feed by feed.
    OutsideFolders,
    Feed(u64),
}

impl MarkReadScope {
    /// `pending_mark_read.scope` and `scope_id`.
    fn columns(self) -> (i64, Option<u64>) {
        match self {
            MarkReadScope::All => (0, None),
            MarkReadScope::Folder(id) => (1, Some(id)),
            MarkReadScope::OutsideFolders => (2, None),
            MarkReadScope::Feed(id) => (3, Some(id)),
        }
    }

    fn from_columns(scope: i64, id: Option<u64>) -> rusqlite::Result<Self> {
        match (scope, id) {
            (0, _) => Ok(MarkReadScope::All),
            (1, Some(id)) => Ok(MarkReadScope::Folder(id)),
            (2, _) => Ok(MarkReadScope::OutsideFolders),
            (3, Some(id)) => Ok(MarkReadScope::Feed(id)),
            _ => Err(rusqlite::Error::IntegralValueOutOfRange(1, scope)),
        }
    }

    /// A condition on `feed_id` for the items in this scope, with `?2` for
    /// the scope's id, if it has one.
    fn filter(self) -> &'static str {
        match self {
            MarkReadScope::All => "1",
            MarkReadScope::Folder(_) => "feed_id IN (SELECT id FROM feeds WHERE folder_id = ?2)",
            MarkReadScope::OutsideFolders => {
                "feed_id IN (SELECT id FROM feeds WHERE folder_id IS NULL)"
            }
            MarkReadScope::Feed(_) => "feed_id = ?2",
        }
    }
}

/// A queued "mark all as read".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingMarkRead {
    /// Its place in the queue.
    pub id: u64,
    pub scope: MarkReadScope,
    pub newest_item_id: u64,
}

/// SQL that is true if a queued "mark all as read" covers the item with the
/// id `?1` of the feed `?2`: the server will mark it read.
pub(crate) const MARKED_READ: &str = "EXISTS (SELECT 1 FROM pending_mark_read m
    WHERE ?1 <= m.newest_item_id AND (m.scope = 0
        OR (m.scope = 1 AND EXISTS (SELECT 1 FROM feeds WHERE id = ?2 AND folder_id = m.scope_id))
        OR (m.scope = 2 AND EXISTS (SELECT 1 FROM feeds WHERE id = ?2 AND folder_id IS NULL))
        OR (m.scope = 3 AND m.scope_id = ?2)))";

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
        let tx = self.write_transaction()?;
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
        let tx = self.write_transaction()?;
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

    /// Marks the items up to `newest_item_id` in `scope` as read, and
    /// queues the "mark all as read" for the server. Returns the number of
    /// items that changed.
    ///
    /// `newest_item_id` is the newest item the user has seen, so that items
    /// which arrive in the meantime stay unread. Queued read states of the
    /// items it covers are dropped: the server marks them read.
    pub fn mark_all_read(&mut self, scope: MarkReadScope, newest_item_id: u64) -> Result<usize> {
        let (code, scope_id) = scope.columns();
        let filter = scope.filter();
        let mut params = vec![Value::Integer(newest_item_id.cast_signed())];
        params.extend(scope_id.map(|id| Value::Integer(id.cast_signed())));
        let tx = self.write_transaction()?;
        tx.execute(
            &format!(
                "DELETE FROM pending_changes WHERE field = {UNREAD}
                 AND item_id IN (SELECT id FROM items WHERE id <= ?1 AND {filter})"
            ),
            params_from_iter(&params),
        )?;
        let changed = tx.execute(
            &format!("UPDATE items SET unread = 0 WHERE unread = 1 AND id <= ?1 AND {filter}"),
            params_from_iter(&params),
        )?;
        tx.execute(
            "INSERT INTO pending_mark_read (scope, scope_id, newest_item_id) VALUES (?1, ?2, ?3)",
            params![code, scope_id, newest_item_id],
        )?;
        tx.commit()?;
        Ok(changed)
    }

    /// The queued "mark all as read", oldest first.
    pub fn pending_mark_read(&self) -> Result<Vec<PendingMarkRead>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, scope, scope_id, newest_item_id FROM pending_mark_read ORDER BY id",
        )?;
        let pending = stmt
            .query_map([], |row| {
                Ok(PendingMarkRead {
                    id: row.get(0)?,
                    scope: MarkReadScope::from_columns(row.get(1)?, row.get(2)?)?,
                    newest_item_id: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(pending)
    }

    /// Removes a "mark all as read" from the queue, once the server has
    /// accepted it.
    pub fn remove_pending_mark_read(&mut self, id: u64) -> Result<()> {
        self.conn
            .execute("DELETE FROM pending_mark_read WHERE id = ?1", [id])?;
        Ok(())
    }

    /// The number of changes waiting to be sent: item changes and "mark all
    /// as read".
    pub fn pending_count(&self) -> Result<u64> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT (SELECT count(*) FROM pending_changes)
                     + (SELECT count(*) FROM pending_mark_read)",
            )?
            .query_row([], |row| row.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Status;
    use crate::test_util::{api_feed, api_folder, api_item};
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

    /// Feeds 1 and 2 in folder 10, 3 and 4 outside of folders; items 1–8,
    /// two per feed (feed `(id + 1) / 2`), all unread.
    fn feed_store() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store.replace_folders(&[api_folder(10, "Folder")]).unwrap();
        store
            .replace_feeds(&[
                api_feed(1, Some(10), "One"),
                api_feed(2, Some(10), "Two"),
                api_feed(3, None, "Three"),
                api_feed(4, None, "Four"),
            ])
            .unwrap();
        let items: Vec<_> = (1..=8).map(|id| api_item(id, id.div_ceil(2))).collect();
        store.upsert_items(&items, false).unwrap();
        store
    }

    fn unread(store: &Store) -> Vec<u64> {
        (1..=8)
            .filter(|&id| status(store, id).0 != Status::Read)
            .collect()
    }

    #[test]
    fn mark_all_read_by_scope() {
        let cases = [
            (MarkReadScope::All, 6, vec![7, 8]),
            (MarkReadScope::Folder(10), 8, vec![5, 6, 7, 8]),
            (MarkReadScope::Folder(10), 3, vec![4, 5, 6, 7, 8]),
            (MarkReadScope::OutsideFolders, 8, vec![1, 2, 3, 4]),
            (MarkReadScope::Feed(3), 8, vec![1, 2, 3, 4, 7, 8]),
            (MarkReadScope::Feed(9), 8, vec![1, 2, 3, 4, 5, 6, 7, 8]),
        ];
        for (scope, newest, left) in cases {
            let mut store = feed_store();
            let changed = store.mark_all_read(scope, newest).unwrap();
            assert_eq!(unread(&store), left, "{scope:?} up to {newest}");
            assert_eq!(changed, 8 - left.len());
            // Queued as such, not item by item.
            assert!(store.pending(ItemAction::Read, 10).unwrap().is_empty());
            assert_eq!(
                store.pending_mark_read().unwrap(),
                [PendingMarkRead {
                    id: 1,
                    scope,
                    newest_item_id: newest
                }]
            );
        }
    }

    #[test]
    fn mark_all_read_replaces_queued_read_states() {
        let mut store = feed_store();
        store.set_unread(&[1, 2, 5, 6], false).unwrap();
        store.set_unread(&[1, 5], true).unwrap();
        store.set_starred(&[1], true).unwrap();
        // Items 1 and 2 are covered, 5 and 6 aren't.
        store.mark_all_read(MarkReadScope::Feed(1), 8).unwrap();
        assert_eq!(status(&store, 1), (Status::Read, true));
        assert_eq!(store.pending(ItemAction::Unread, 10).unwrap(), [5]);
        assert_eq!(store.pending(ItemAction::Read, 10).unwrap(), [6]);
        assert_eq!(store.pending(ItemAction::Star, 10).unwrap(), [1]);
        // Changed again afterwards: queued as a change of its own.
        store.set_unread(&[2], true).unwrap();
        assert_eq!(store.pending(ItemAction::Unread, 10).unwrap(), [2, 5]);
        assert_eq!(store.pending_count().unwrap(), 5);
    }

    #[test]
    fn items_marked_read_stay_read_until_sent() {
        let mut store = feed_store();
        store.mark_all_read(MarkReadScope::Folder(10), 3).unwrap();
        store.mark_all_read(MarkReadScope::Feed(4), 20).unwrap();
        // The server still has them unread, and new items: 9 (feed 1) and
        // 11 (feed 2) are newer than the newest item their folder was
        // marked up to, 10 (feed 4) isn't.
        let sent: Vec<_> = (1..=8)
            .map(|id| api_item(id, id.div_ceil(2)))
            .chain([api_item(9, 1), api_item(10, 4), api_item(11, 2)])
            .map(|item| types::Item {
                title: Some("Changed".into()),
                ..item
            })
            .collect();
        store.upsert_items(&sent, true).unwrap();
        let unread: Vec<_> = (1..=11)
            .filter(|&id| status(&store, id).0 != Status::Read)
            .collect();
        assert_eq!(unread, [4, 5, 6, 9, 11]);
        assert_eq!(store.item(1).unwrap().unwrap().summary.title, "Changed");

        // Once sent, the server's state counts again.
        let pending = store.pending_mark_read().unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].scope, MarkReadScope::Folder(10));
        assert_eq!(pending[1].scope, MarkReadScope::Feed(4));
        store.remove_pending_mark_read(pending[0].id).unwrap();
        assert_eq!(store.pending_count().unwrap(), 1);
        store.upsert_items(&sent, true).unwrap();
        let unread: Vec<_> = (1..=11)
            .filter(|&id| status(&store, id).0 != Status::Read)
            .collect();
        assert_eq!(unread, [1, 2, 3, 4, 5, 6, 9, 11]);
    }
}
