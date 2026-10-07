// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Items: writing what the server sent, and the paged list queries.
//!
//! Lists are queried a page at a time with a count, or as the ids in
//! order with the rows fetched by id, so a list view only ever holds the
//! rows it shows. Filtered items (keyword filters of News
//! 28.4) are stored but never listed, as in the News web interface.

use nextcloud_news::types;
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, Row, ToSql, params, params_from_iter};

use crate::changes::{STARRED, UNREAD};
use crate::text::{collapse, key, search_key};
use crate::{Result, Store};

/// Read state of an item, as shown in the item list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Unread, and arrived in the latest sync.
    New,
    Unread,
    Read,
}

impl Status {
    fn new(unread: bool, is_new: bool) -> Self {
        match (unread, is_new) {
            (false, _) => Status::Read,
            (true, true) => Status::New,
            (true, false) => Status::Unread,
        }
    }
}

/// The fields of an item shown in the item list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemSummary {
    pub id: u64,
    pub feed_id: u64,
    /// With whitespace collapsed; empty if the item has none. Not
    /// sanitised.
    pub title: String,
    /// With whitespace collapsed; empty if the item has none. Not
    /// sanitised.
    pub author: String,
    /// Publication date, unix seconds.
    pub pub_date: i64,
    pub status: Status,
    pub starred: bool,
}

/// An item with everything needed to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemDetails {
    pub summary: ItemSummary,
    /// The article's page. Not sanitised.
    pub url: Option<String>,
    /// Sanitised HTML.
    pub body: Option<String>,
    pub enclosure_mime: Option<String>,
    pub enclosure_link: Option<String>,
    pub media_thumbnail: Option<String>,
    pub media_description: Option<String>,
    /// Right-to-left text.
    pub rtl: bool,
    pub filtered: bool,
    /// The same fingerprint means the same article in several feeds.
    pub fingerprint: Option<String>,
}

/// Which items a list shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Selection {
    #[default]
    All,
    Starred,
    Folder(u64),
    Feed(u64),
}

/// The items of a list: a selection, narrowed down.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemQuery {
    pub selection: Selection,
    pub unread_only: bool,
    /// Only items whose title or author contains this text, ignoring case.
    /// Empty for all.
    pub search: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Title,
    Feed,
    Author,
    Date,
}

/// The order of a list. Items that are equal in the sort column are
/// newest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub column: SortColumn,
    pub ascending: bool,
}

impl Default for Sort {
    /// Newest first.
    fn default() -> Self {
        Self {
            column: SortColumn::Date,
            ascending: false,
        }
    }
}

const SUMMARY_COLUMNS: &str =
    "i.id, i.feed_id, i.title, i.author, i.pub_date, i.unread, i.is_new, i.starred";

/// The `FROM` and `WHERE` clauses of a list and their parameters. Only items
/// of known feeds are listed.
///
/// Sorted by feed, the feeds are ranked by title first (`r.rank`), so that
/// the items are sorted by a number instead of comparing feed titles with
/// the collation for every pair of items (a third faster).
fn list_filter(query: &ItemQuery, sort: Sort) -> (String, Vec<Value>) {
    let mut sql = String::from(" FROM items i JOIN feeds f ON f.id = i.feed_id");
    if sort.column == SortColumn::Feed {
        sql.push_str(
            " JOIN (SELECT id, row_number() OVER (ORDER BY title COLLATE caseless, id) AS rank
                 FROM feeds) r ON r.id = i.feed_id",
        );
    }
    sql.push_str(" WHERE i.filtered = 0");
    let mut params = Vec::new();
    match query.selection {
        Selection::All => {}
        Selection::Starred => sql.push_str(" AND i.starred = 1"),
        Selection::Folder(id) => {
            sql.push_str(" AND f.folder_id = ?");
            params.push(Value::Integer(id.cast_signed()));
        }
        Selection::Feed(id) => {
            sql.push_str(" AND i.feed_id = ?");
            params.push(Value::Integer(id.cast_signed()));
        }
    }
    if query.unread_only {
        sql.push_str(" AND i.unread = 1");
    }
    let search = search_key(&query.search);
    if !search.is_empty() {
        sql.push_str(" AND (instr(i.title_key, ?) > 0 OR instr(i.author_key, ?) > 0)");
        params.push(Value::Text(search.clone()));
        params.push(Value::Text(search));
    }
    (sql, params)
}

/// The `ORDER BY` clause of a list.
fn order_by(sort: Sort) -> String {
    let dir = if sort.ascending { "ASC" } else { "DESC" };
    let ties = "i.pub_date DESC, i.id DESC";
    match sort.column {
        SortColumn::Date => format!(" ORDER BY i.pub_date {dir}, i.id {dir}"),
        SortColumn::Title => format!(" ORDER BY i.title_key {dir}, {ties}"),
        SortColumn::Author => format!(" ORDER BY i.author_key {dir}, {ties}"),
        SortColumn::Feed => format!(" ORDER BY r.rank {dir}, {ties}"),
    }
}

fn summary_from_row(row: &Row<'_>) -> rusqlite::Result<ItemSummary> {
    Ok(ItemSummary {
        id: row.get(0)?,
        feed_id: row.get(1)?,
        title: row.get(2)?,
        author: row.get(3)?,
        pub_date: row.get(4)?,
        status: Status::new(row.get(5)?, row.get(6)?),
        starred: row.get(7)?,
    })
}

impl Store {
    /// Writes items as the server sent them, in one transaction: adds new
    /// ones and updates known ones. Returns the number of new items.
    ///
    /// Items that weren't stored before are marked as new if `mark_new` is
    /// set (not in the initial sync, where everything is new). The read and
    /// starred state of an item with a pending local change of that state
    /// is kept: the local change is newer and still has to reach the
    /// server.
    pub fn upsert_items(&mut self, items: &[types::Item], mark_new: bool) -> Result<usize> {
        let tx = self.write_transaction()?;
        let mut added = 0;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO items (id, feed_id, title, author, pub_date, url, enclosure_mime,
                     enclosure_link, media_thumbnail, fingerprint, last_modified, unread,
                     starred, filtered, rtl, title_key, author_key, is_new)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, ?18)
                 ON CONFLICT (id) DO NOTHING",
            )?;
            let mut update = tx.prepare_cached(&format!(
                "UPDATE items SET feed_id = ?2, title = ?3, author = ?4, pub_date = ?5,
                     url = ?6, enclosure_mime = ?7, enclosure_link = ?8,
                     media_thumbnail = ?9, fingerprint = ?10, last_modified = ?11,
                     unread = CASE WHEN EXISTS (SELECT 1 FROM pending_changes
                         WHERE item_id = ?1 AND field = {UNREAD}) THEN unread ELSE ?12 END,
                     starred = CASE WHEN EXISTS (SELECT 1 FROM pending_changes
                         WHERE item_id = ?1 AND field = {STARRED}) THEN starred ELSE ?13 END,
                     filtered = ?14, rtl = ?15, title_key = ?16, author_key = ?17
                 WHERE id = ?1"
            ))?;
            let mut content = tx.prepare_cached(
                "INSERT INTO item_contents (item_id, body, media_description) VALUES (?1, ?2, ?3)
                 ON CONFLICT (item_id) DO UPDATE SET
                     body = excluded.body, media_description = excluded.media_description",
            )?;
            for item in items {
                let title = collapse(item.title.as_deref().unwrap_or_default());
                let author = collapse(item.author.as_deref().unwrap_or_default());
                // The News app sets a date for every item; just in case.
                let pub_date = item.pub_date.or(item.last_modified).unwrap_or(0);
                let title_key = key(&title);
                let author_key = key(&author);
                let values: [&dyn ToSql; 18] = [
                    &item.id,
                    &item.feed_id,
                    &title,
                    &author,
                    &pub_date,
                    &item.url,
                    &item.enclosure_mime,
                    &item.enclosure_link,
                    &item.media_thumbnail,
                    &item.fingerprint,
                    &item.last_modified,
                    &item.unread,
                    &item.starred,
                    &item.filtered,
                    &item.rtl,
                    &title_key,
                    &author_key,
                    &mark_new,
                ];
                if insert.execute(&values[..])? == 1 {
                    added += 1;
                } else {
                    update.execute(&values[..17])?;
                }
                content.execute(params![item.id, item.body, item.media_description])?;
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// An item with its body.
    pub fn item(&self, id: u64) -> Result<Option<ItemDetails>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {SUMMARY_COLUMNS}, i.url, c.body, i.enclosure_mime, i.enclosure_link,
                 i.media_thumbnail, c.media_description, i.rtl, i.filtered, i.fingerprint
             FROM items i LEFT JOIN item_contents c ON c.item_id = i.id
             WHERE i.id = ?1"
        ))?;
        let item = stmt
            .query_row([id], |row| {
                Ok(ItemDetails {
                    summary: summary_from_row(row)?,
                    url: row.get(8)?,
                    body: row.get(9)?,
                    enclosure_mime: row.get(10)?,
                    enclosure_link: row.get(11)?,
                    media_thumbnail: row.get(12)?,
                    media_description: row.get(13)?,
                    rtl: row.get(14)?,
                    filtered: row.get(15)?,
                    fingerprint: row.get(16)?,
                })
            })
            .optional()?;
        Ok(item)
    }

    /// The number of items in a list.
    pub fn item_count(&self, query: &ItemQuery) -> Result<u64> {
        let (filter, params) = list_filter(query, Sort::default());
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT count(*){filter}"))?;
        Ok(stmt.query_row(params_from_iter(params), |row| row.get(0))?)
    }

    /// The rows `offset..offset + limit` of a list.
    pub fn item_page(
        &self,
        query: &ItemQuery,
        sort: Sort,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<ItemSummary>> {
        let (filter, mut params) = list_filter(query, sort);
        let order = order_by(sort);
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {SUMMARY_COLUMNS}{filter}{order} LIMIT ? OFFSET ?"
        ))?;
        params.push(Value::Integer(limit.try_into().unwrap_or(i64::MAX)));
        params.push(Value::Integer(offset.try_into().unwrap_or(i64::MAX)));
        let rows = stmt
            .query_map(params_from_iter(params), summary_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// The ids of all items in a list, in order.
    ///
    /// The other way to show a long list: sort it once (8 bytes per item)
    /// and fetch the rows shown with [`item_summaries`](Self::item_summaries).
    /// Pages from the middle of a list sorted by anything but the date cost
    /// a sort each with [`item_page`](Self::item_page).
    pub fn item_ids(&self, query: &ItemQuery, sort: Sort) -> Result<Vec<u64>> {
        let (filter, params) = list_filter(query, sort);
        let order = order_by(sort);
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT i.id{filter}{order}"))?;
        let ids = stmt
            .query_map(params_from_iter(params), |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// The list rows of these items, in this order. Unknown ids are skipped.
    pub fn item_summaries(&self, ids: &[u64]) -> Result<Vec<ItemSummary>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {SUMMARY_COLUMNS} FROM items i WHERE i.id = ?1"
        ))?;
        let mut rows = Vec::with_capacity(ids.len());
        for &id in ids {
            if let Some(row) = stmt.query_row([id], summary_from_row).optional()? {
                rows.push(row);
            }
        }
        Ok(rows)
    }

    /// The row of an item in a list, `None` if the list doesn't contain it.
    /// For keeping the selection when the list changes.
    pub fn item_position(&self, query: &ItemQuery, sort: Sort, id: u64) -> Result<Option<u64>> {
        let (filter, mut params) = list_filter(query, sort);
        // The item's sort keys, if it is in the list.
        let rank = if sort.column == SortColumn::Feed {
            "r.rank"
        } else {
            "0"
        };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT i.title_key, i.author_key, i.pub_date, {rank}{filter} AND i.id = ?"
        ))?;
        let mut key_params = params.clone();
        key_params.push(Value::Integer(id.cast_signed()));
        let Some((title, author, pub_date, rank)) = stmt
            .query_row(params_from_iter(key_params), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .optional()?
        else {
            return Ok(None);
        };
        // Counting the rows before it takes one pass over the list, where
        // numbering the sorted list would sort it first.
        let lt = if sort.ascending { "<" } else { ">" };
        let id = id.cast_signed();
        let ties = "(i.pub_date, i.id) > (?, ?)";
        let (before, keys) = match sort.column {
            SortColumn::Date => (
                format!("(i.pub_date, i.id) {lt} (?, ?)"),
                vec![pub_date.into(), id.into()],
            ),
            SortColumn::Title | SortColumn::Author => {
                let (column, value) = match sort.column {
                    SortColumn::Title => ("i.title_key", title),
                    _ => ("i.author_key", author),
                };
                (
                    format!("{column} {lt} ? OR ({column} = ? AND {ties})"),
                    vec![
                        value.clone().into(),
                        value.into(),
                        pub_date.into(),
                        id.into(),
                    ],
                )
            }
            SortColumn::Feed => (
                format!("r.rank {lt} ? OR (r.rank = ? AND {ties})"),
                vec![rank.into(), rank.into(), pub_date.into(), id.into()],
            ),
        };
        params.extend(keys);
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT count(*){filter} AND ({before})"))?;
        Ok(Some(
            stmt.query_row(params_from_iter(params), |row| row.get(0))?,
        ))
    }

    /// Clears the "new" marker of all items, when a sync starts. Returns
    /// the number of items that had it.
    pub fn clear_new(&mut self) -> Result<usize> {
        Ok(self
            .conn
            .execute("UPDATE items SET is_new = 0 WHERE is_new = 1", [])?)
    }

    /// Deletes read and unstarred items that haven't changed on the server
    /// since `before` (unix seconds), unless they have a pending local
    /// change. Returns the number of items deleted.
    ///
    /// The server updates an item's `lastModified` when it is marked read,
    /// so this is about the time since an item was read; items without one
    /// go by their publication date.
    pub fn purge(&mut self, before: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM items
             WHERE unread = 0 AND starred = 0 AND coalesce(last_modified, pub_date) < ?1
                 AND NOT EXISTS (SELECT 1 FROM pending_changes p WHERE p.item_id = items.id)",
            [before],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{api_feed, api_item};

    fn ids(rows: &[ItemSummary]) -> Vec<u64> {
        rows.iter().map(|r| r.id).collect()
    }

    #[test]
    fn items_are_stored_as_sent() {
        let mut store = Store::open_in_memory().unwrap();
        store.replace_feeds(&[api_feed(67, None, "Feed")]).unwrap();
        let item = types::Item {
            id: 3443,
            guid: Some("http://example.org/?p=76".into()),
            guid_hash: Some("3059047a".into()),
            url: Some("http://example.org/post/".into()),
            title: Some("  A\n post ".into()),
            author: Some("Ada\n\n  Lovelace".into()),
            pub_date: Some(1_367_270_544),
            updated_date: None,
            body: Some("<p>Text</p>".into()),
            enclosure_mime: Some("audio/ogg".into()),
            enclosure_link: Some("http://example.org/a.ogg".into()),
            media_thumbnail: Some("http://example.org/t.jpg".into()),
            media_description: Some("A thumbnail".into()),
            feed_id: 67,
            unread: true,
            starred: true,
            filtered: false,
            rtl: true,
            last_modified: Some(1_367_273_003),
            fingerprint: Some("aeaae2123".into()),
            content_hash: Some("abc".into()),
        };
        assert_eq!(store.upsert_items(&[item], true).unwrap(), 1);
        let details = store.item(3443).unwrap().unwrap();
        assert_eq!(
            details,
            ItemDetails {
                summary: ItemSummary {
                    id: 3443,
                    feed_id: 67,
                    title: "A post".into(),
                    author: "Ada Lovelace".into(),
                    pub_date: 1_367_270_544,
                    status: Status::New,
                    starred: true,
                },
                url: Some("http://example.org/post/".into()),
                body: Some("<p>Text</p>".into()),
                enclosure_mime: Some("audio/ogg".into()),
                enclosure_link: Some("http://example.org/a.ogg".into()),
                media_thumbnail: Some("http://example.org/t.jpg".into()),
                media_description: Some("A thumbnail".into()),
                rtl: true,
                filtered: false,
                fingerprint: Some("aeaae2123".into()),
            }
        );
        assert_eq!(store.item(1).unwrap(), None);
    }

    #[test]
    fn missing_fields() {
        let mut store = Store::open_in_memory().unwrap();
        let mut item: types::Item = types::Item {
            title: None,
            author: None,
            pub_date: None,
            body: None,
            ..api_item(1, 1)
        };
        item.last_modified = Some(77);
        let mut undated = api_item(2, 1);
        undated.pub_date = None;
        undated.last_modified = None;
        store.upsert_items(&[item, undated], false).unwrap();
        let details = store.item(1).unwrap().unwrap();
        assert_eq!(details.summary.title, "");
        assert_eq!(details.summary.author, "");
        assert_eq!(details.summary.pub_date, 77);
        assert_eq!(details.body, None);
        assert_eq!(store.item(2).unwrap().unwrap().summary.pub_date, 0);
    }

    #[test]
    fn updates_replace_items() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_items(&[api_item(1, 1)], false).unwrap();
        let mut changed = api_item(1, 2);
        changed.title = Some("Changed".into());
        changed.body = Some("<p>New body</p>".into());
        changed.unread = false;
        changed.starred = true;
        changed.filtered = true;
        assert_eq!(store.upsert_items(&[changed], true).unwrap(), 0);
        let details = store.item(1).unwrap().unwrap();
        assert_eq!(details.summary.feed_id, 2);
        assert_eq!(details.summary.title, "Changed");
        assert_eq!(details.summary.status, Status::Read);
        assert!(details.summary.starred);
        assert!(details.filtered);
        assert_eq!(details.body.as_deref(), Some("<p>New body</p>"));
    }

    #[test]
    fn new_marker() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_items(&[api_item(1, 1)], false).unwrap();
        assert_eq!(
            store.item(1).unwrap().unwrap().summary.status,
            Status::Unread
        );

        assert_eq!(store.clear_new().unwrap(), 0);
        // An update doesn't make an item new.
        store
            .upsert_items(&[api_item(1, 1), api_item(2, 1)], true)
            .unwrap();
        assert_eq!(
            store.item(1).unwrap().unwrap().summary.status,
            Status::Unread
        );
        assert_eq!(store.item(2).unwrap().unwrap().summary.status, Status::New);
        // Read items are read, new or not.
        store.set_unread(&[2], false).unwrap();
        assert_eq!(store.item(2).unwrap().unwrap().summary.status, Status::Read);
        store.set_unread(&[2], true).unwrap();
        assert_eq!(store.item(2).unwrap().unwrap().summary.status, Status::New);

        assert_eq!(store.clear_new().unwrap(), 1);
        assert_eq!(
            store.item(2).unwrap().unwrap().summary.status,
            Status::Unread
        );
    }

    /// Feeds 1 ("Beta") and 2 ("alpha") in folder 7, feed 3 ("Gamma") at the
    /// top level. Items: id, feed, title, author, date, unread, starred.
    fn list_store() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .replace_feeds(&[
                api_feed(1, Some(7), "Beta"),
                api_feed(2, Some(7), "alpha"),
                api_feed(3, None, "Gamma"),
            ])
            .unwrap();
        let rows = [
            (1, 1, "banana split", "Zoe", 500, true, false),
            (2, 2, "Apple pie", "yann", 400, false, true),
            (3, 3, "cherry tart", "Xia", 300, true, false),
            (4, 1, "apple crumble", "Wim", 200, false, false),
            (5, 2, "Éclair", "Ünal", 400, true, true),
        ];
        let items: Vec<_> = rows
            .iter()
            .map(
                |&(id, feed, title, author, date, unread, starred)| types::Item {
                    title: Some(title.into()),
                    author: Some(author.into()),
                    pub_date: Some(date),
                    unread,
                    starred,
                    ..api_item(id, feed)
                },
            )
            .collect();
        store.upsert_items(&items, false).unwrap();
        store
    }

    fn query(selection: Selection) -> ItemQuery {
        ItemQuery {
            selection,
            ..ItemQuery::default()
        }
    }

    fn sort(column: SortColumn, ascending: bool) -> Sort {
        Sort { column, ascending }
    }

    fn page(store: &Store, query: &ItemQuery, sort: Sort) -> Vec<u64> {
        ids(&store.item_page(query, sort, 0, 100).unwrap())
    }

    #[test]
    fn selections() {
        let store = list_store();
        let all = query(Selection::All);
        assert_eq!(page(&store, &all, Sort::default()), [1, 5, 2, 3, 4]);
        assert_eq!(store.item_count(&all).unwrap(), 5);
        let starred = query(Selection::Starred);
        assert_eq!(page(&store, &starred, Sort::default()), [5, 2]);
        let folder = query(Selection::Folder(7));
        assert_eq!(page(&store, &folder, Sort::default()), [1, 5, 2, 4]);
        let feed = query(Selection::Feed(2));
        assert_eq!(page(&store, &feed, Sort::default()), [5, 2]);
        assert_eq!(store.item_count(&feed).unwrap(), 2);
        let unread = ItemQuery {
            unread_only: true,
            ..query(Selection::Folder(7))
        };
        assert_eq!(page(&store, &unread, Sort::default()), [1, 5]);
        assert_eq!(store.item_count(&query(Selection::Feed(9))).unwrap(), 0);
    }

    #[test]
    fn search_ignores_case() {
        let store = list_store();
        let search = |text: &str| ItemQuery {
            search: text.into(),
            ..ItemQuery::default()
        };
        assert_eq!(page(&store, &search("APPLE"), Sort::default()), [2, 4]);
        // Authors too, and beyond ASCII.
        assert_eq!(page(&store, &search(" ünal "), Sort::default()), [5]);
        assert_eq!(page(&store, &search("éCLAIR"), Sort::default()), [5]);
        assert_eq!(
            page(&store, &search("  "), Sort::default()),
            [1, 5, 2, 3, 4]
        );
        assert_eq!(store.item_count(&search("tart")).unwrap(), 1);
        // Wildcards of LIKE are plain text.
        assert_eq!(store.item_count(&search("%")).unwrap(), 0);
        // Whitespace is collapsed, as in the titles.
        assert_eq!(page(&store, &search("Apple \n Pie"), Sort::default()), [2]);
    }

    #[test]
    fn sorting() {
        let store = list_store();
        let all = query(Selection::All);
        let order = |column, ascending| page(&store, &all, sort(column, ascending));
        assert_eq!(order(SortColumn::Date, true), [4, 3, 2, 5, 1]);
        assert_eq!(order(SortColumn::Date, false), [1, 5, 2, 3, 4]);
        // Ignoring case; same titles newest first.
        assert_eq!(order(SortColumn::Title, true), [4, 2, 1, 3, 5]);
        assert_eq!(order(SortColumn::Title, false), [5, 3, 1, 2, 4]);
        assert_eq!(order(SortColumn::Author, true), [4, 3, 2, 1, 5]);
        // By feed title (alpha, Beta, Gamma), within a feed newest first.
        assert_eq!(order(SortColumn::Feed, true), [5, 2, 1, 4, 3]);
        assert_eq!(order(SortColumn::Feed, false), [3, 1, 4, 5, 2]);
    }

    #[test]
    fn paging_and_positions() {
        let store = list_store();
        let all = query(Selection::All);
        let by_title = sort(SortColumn::Title, true);
        assert_eq!(ids(&store.item_page(&all, by_title, 0, 2).unwrap()), [4, 2]);
        assert_eq!(ids(&store.item_page(&all, by_title, 2, 2).unwrap()), [1, 3]);
        assert_eq!(ids(&store.item_page(&all, by_title, 4, 2).unwrap()), [5]);
        assert!(store.item_page(&all, by_title, 5, 2).unwrap().is_empty());
        for (row, id) in [4, 2, 1, 3, 5].into_iter().enumerate() {
            assert_eq!(
                store.item_position(&all, by_title, id).unwrap(),
                Some(row as u64)
            );
        }
        assert_eq!(store.item_ids(&all, by_title).unwrap(), [4, 2, 1, 3, 5]);
        let summaries = store.item_summaries(&[3, 9, 1]).unwrap();
        assert_eq!(ids(&summaries), [3, 1]);
        assert_eq!(summaries[0].title, "cherry tart");
        let feed = query(Selection::Feed(2));
        assert_eq!(store.item_ids(&feed, Sort::default()).unwrap(), [5, 2]);
        assert_eq!(
            store.item_position(&feed, Sort::default(), 2).unwrap(),
            Some(1)
        );
        assert_eq!(
            store.item_position(&feed, Sort::default(), 1).unwrap(),
            None
        );
    }

    #[test]
    fn lists_hide_filtered_items_and_unknown_feeds() {
        let mut store = list_store();
        let mut filtered = api_item(6, 1);
        filtered.filtered = true;
        filtered.starred = true;
        // Feed 9 isn't known (yet).
        store
            .upsert_items(&[filtered, api_item(7, 9)], false)
            .unwrap();
        let all = query(Selection::All);
        assert_eq!(page(&store, &all, Sort::default()), [1, 5, 2, 3, 4]);
        assert_eq!(
            page(&store, &query(Selection::Starred), Sort::default()),
            [5, 2]
        );
        assert_eq!(store.item_position(&all, Sort::default(), 6).unwrap(), None);
        // They are kept, though.
        assert!(store.item(6).unwrap().unwrap().filtered);
        store
            .replace_feeds(&[api_feed(1, Some(7), "Beta"), api_feed(9, None, "Late")])
            .unwrap();
        assert_eq!(
            page(&store, &query(Selection::Feed(9)), Sort::default()),
            [7]
        );
    }

    #[test]
    fn purge() {
        let mut store = Store::open_in_memory().unwrap();
        let item = |id, unread, starred, last_modified| types::Item {
            unread,
            starred,
            pub_date: Some(10),
            last_modified,
            ..api_item(id, 1)
        };
        store
            .upsert_items(
                &[
                    item(1, false, false, Some(100)),
                    item(2, false, false, Some(300)),
                    item(3, true, false, Some(100)),
                    item(4, false, true, Some(100)),
                    item(5, false, false, None),
                    item(6, true, false, Some(100)),
                    item(7, false, false, Some(100)),
                ],
                false,
            )
            .unwrap();
        // Read locally, not yet sent.
        store.set_unread(&[6], false).unwrap();
        // Unread locally, not yet sent.
        store.set_unread(&[7], true).unwrap();
        store.set_unread(&[7], false).unwrap();
        assert_eq!(store.purge(200).unwrap(), 2);
        let left: Vec<u64> = (1..=7)
            .filter(|&id| store.item(id).unwrap().is_some())
            .collect();
        assert_eq!(left, [2, 3, 4, 6, 7]);
        let contents: u64 = store
            .conn
            .query_row("SELECT count(*) FROM item_contents", [], |row| row.get(0))
            .unwrap();
        assert_eq!(contents, 5);
    }

    /// The query plan of a list query, one step per line.
    fn plan(store: &Store, sql: &str, params: Vec<Value>) -> String {
        let mut stmt = store
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let steps: Vec<String> = stmt
            .query_map(params_from_iter(params), |row| row.get(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        steps.join("\n")
    }

    fn page_plan(store: &Store, query: &ItemQuery) -> String {
        let (filter, mut params) = list_filter(query, Sort::default());
        params.extend([Value::Integer(50), Value::Integer(0)]);
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS}{filter}{} LIMIT ? OFFSET ?",
            order_by(Sort::default())
        );
        plan(store, &sql, params)
    }

    #[test]
    fn list_queries_use_the_indices() {
        let store = list_store();
        let all = page_plan(&store, &query(Selection::All));
        assert!(all.contains("USING INDEX items_date"), "{all}");
        assert!(!all.contains("TEMP B-TREE"), "{all}");
        let starred = page_plan(&store, &query(Selection::Starred));
        assert!(starred.contains("USING INDEX items_starred"), "{starred}");
        assert!(!starred.contains("TEMP B-TREE"), "{starred}");
        let feed = page_plan(&store, &query(Selection::Feed(1)));
        assert!(feed.contains("USING INDEX items_feed"), "{feed}");
        assert!(!feed.contains("TEMP B-TREE"), "{feed}");
        let counts = plan(
            &store,
            "SELECT feed_id, count(*) FROM items WHERE unread = 1 AND filtered = 0
             GROUP BY feed_id",
            Vec::new(),
        );
        assert!(counts.contains("items_unread"), "{counts}");
        let new = plan(
            &store,
            "UPDATE items SET is_new = 0 WHERE is_new = 1",
            Vec::new(),
        );
        assert!(new.contains("items_new"), "{new}");
    }
}
