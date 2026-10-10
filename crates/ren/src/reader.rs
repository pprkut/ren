// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! View model of the main window: the feed tree, the item list and the
//! article shown, on top of the store.
//!
//! Without Slint types, so that it is tested on a store in memory. Every
//! change goes to the store first (and is queued there for the server);
//! the operations say what the window has to update ([`Changes`]).

use std::collections::{HashMap, HashSet};

use ren_store::{
    ItemDetails, ItemQuery, ItemSummary, MarkReadScope, Result, Selection, Sort, Status, Store,
};

use crate::article::Article;
use crate::feed_tree::{FeedTree, Key, Node, Row};
use crate::item_list::{Column, ItemList, format_date};

/// What the window has to update after an operation.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// The counts in the feed tree.
    pub counts: bool,
    /// Rows of the item list whose content changed.
    pub rows: Vec<usize>,
    /// The item list as a whole: other items, another order, or many
    /// rows changed.
    pub list: bool,
}

/// Rows fetched at a time when looking for the next unread item.
const SCAN_CHUNK: usize = 100;

pub struct Reader {
    store: Store,
    tree: FeedTree,
    selected: Node,
    list: ItemList,
    /// The item shown in the article pane.
    current: Option<u64>,
    /// The feed of the item shown.
    current_feed: Option<u64>,
    /// Feed titles by id, for the item rows.
    feed_titles: HashMap<u64, String>,
    /// Feeds whose articles show their web page instead.
    full_page: HashSet<u64>,
}

impl Reader {
    /// Shows all items of the store.
    pub fn new(store: Store) -> Result<Self> {
        let mut reader = Self {
            store,
            tree: FeedTree::new(&[], &[], &HashSet::new()),
            selected: Node::All,
            list: ItemList::default(),
            current: None,
            current_feed: None,
            feed_titles: HashMap::new(),
            full_page: HashSet::new(),
        };
        reader.reload()?;
        Ok(reader)
    }

    /// Reads folders, feeds, counts and the list again, after a sync
    /// changed them. Folders stay collapsed; if the selected feed or
    /// folder is gone, all items are shown.
    pub fn reload(&mut self) -> Result<()> {
        let folders = self.store.folders()?;
        let feeds = self.store.feeds()?;
        self.tree = FeedTree::new(&folders, &feeds, &self.tree.collapsed());
        self.feed_titles = feeds
            .into_iter()
            .map(|feed| {
                let title = if feed.title.is_empty() {
                    feed.url
                } else {
                    feed.title
                };
                (feed.id, title)
            })
            .collect();
        self.full_page = self.store.full_page_feeds()?.into_iter().collect();
        if !self.tree.contains(self.selected) {
            self.selected = Node::All;
            self.list.set_selection(Selection::All);
        }
        self.load_counts()?;
        self.list.load(&self.store)
    }

    fn load_counts(&mut self) -> Result<()> {
        let unread = self.store.unread_counts()?;
        let starred = self.store.starred_count()?;
        self.tree.set_counts(&unread, starred);
        Ok(())
    }

    pub fn tree_rows(&self) -> Vec<Row> {
        self.tree.rows()
    }

    pub fn selected(&self) -> Node {
        self.selected
    }

    /// The number of unread items.
    pub fn unread(&self) -> u64 {
        self.tree.unread()
    }

    /// Selects a feed, a folder, "All items" or "Starred", and lists its
    /// items, with nothing selected in the list.
    pub fn select(&mut self, node: Node) -> Result<()> {
        self.selected = node;
        self.current = None;
        self.list.set_selection(node.selection());
        self.list.load(&self.store)
    }

    pub fn toggle_folder(&mut self, folder_id: u64) {
        self.tree.toggle(folder_id);
    }

    /// Handles a navigation key in the tree; returns the node to select,
    /// if the selection moves.
    pub fn navigate(&mut self, key: Key) -> Option<Node> {
        self.tree.navigate(self.selected, key)
    }

    pub fn len(&self) -> usize {
        self.list.ids().len()
    }

    pub fn id(&self, row: usize) -> Option<u64> {
        self.list.id(row)
    }

    /// The item shown in the article pane.
    pub fn current(&self) -> Option<u64> {
        self.current
    }

    /// The row of the item shown, if the list has it.
    pub fn current_row(&self) -> Option<usize> {
        self.current.and_then(|id| self.list.row_of(id))
    }

    /// The fields of a row, `None` if the row or its item is gone.
    pub fn row(&self, row: usize) -> Result<Option<ItemSummary>> {
        let Some(id) = self.list.id(row) else {
            return Ok(None);
        };
        Ok(self.store.item_summaries(&[id])?.pop())
    }

    pub fn feed_title(&self, feed_id: u64) -> &str {
        self.feed_titles.get(&feed_id).map_or("", String::as_str)
    }

    pub fn sort(&self) -> Sort {
        self.list.sort()
    }

    pub fn sort_by(&mut self, column: Column) -> Result<()> {
        self.list.sort_by(column);
        self.list.load(&self.store)
    }

    pub fn search(&mut self, text: &str) -> Result<()> {
        self.list.set_search(text);
        self.list.load(&self.store)
    }

    /// Shows the item in a row: it becomes the current item and is marked
    /// read. `None` if the row or its item is gone.
    pub fn open(&mut self, row: usize) -> Result<Option<(Article, Changes)>> {
        let Some(id) = self.list.id(row) else {
            return Ok(None);
        };
        self.current = Some(id);
        self.current_feed = None;
        let Some(item) = self.store.item(id)? else {
            return Ok(None);
        };
        self.current_feed = Some(item.summary.feed_id);
        let mut changes = Changes::default();
        if item.summary.status != Status::Read && self.store.set_unread(&[id], false)? > 0 {
            self.load_counts()?;
            changes.counts = true;
            changes.rows.push(row);
        }
        Ok(Some((self.article(item), changes)))
    }

    fn article(&self, item: ItemDetails) -> Article {
        let summary = item.summary;
        let mut meta = self.feed_title(summary.feed_id).to_owned();
        if !summary.author.is_empty() {
            if !meta.is_empty() {
                meta.push_str(" · ");
            }
            meta.push_str(&summary.author);
        }
        if !meta.is_empty() {
            meta.push_str(" · ");
        }
        meta.push_str(&format_date(summary.pub_date));
        Article {
            title: summary.title,
            meta,
            url: item.url,
            body: item.body.unwrap_or_default(),
        }
    }

    /// Whether the item shown is of a feed set to show the full page
    /// instead of the article.
    pub fn current_full_page(&self) -> bool {
        self.current_feed
            .is_some_and(|feed| self.full_page.contains(&feed))
    }

    /// Whether a feed shows the full page instead of the article.
    pub fn is_full_page(&self, feed_id: u64) -> bool {
        self.full_page.contains(&feed_id)
    }

    /// Sets whether a feed shows the full page instead of the article.
    pub fn set_full_page(&mut self, feed_id: u64, full_page: bool) -> Result<()> {
        let settings = ren_store::FeedSettings {
            open_full_page: full_page,
        };
        self.store.set_feed_settings(feed_id, &settings)?;
        if full_page {
            self.full_page.insert(feed_id);
        } else {
            self.full_page.remove(&feed_id);
        }
        Ok(())
    }

    /// The link to an item's web page, if it has one.
    pub fn url(&self, id: u64) -> Result<Option<String>> {
        Ok(self.store.item(id)?.and_then(|item| item.url))
    }

    /// Whether an item is starred.
    pub fn is_starred(&self, id: u64) -> Result<bool> {
        Ok(self
            .store
            .item_summaries(&[id])?
            .first()
            .is_some_and(|item| item.starred))
    }

    /// Marks an item read or unread.
    pub fn set_read(&mut self, id: u64, read: bool) -> Result<Changes> {
        let changed = self.store.set_unread(&[id], !read)? > 0;
        self.changed(id, changed)
    }

    /// Stars an unstarred item, or unstars a starred one. In the list of
    /// starred items, an unstarred item stays until the list is selected
    /// again, so that the rows don't move under the pointer.
    pub fn toggle_star(&mut self, id: u64) -> Result<Changes> {
        let starred = self.is_starred(id)?;
        let changed = self.store.set_starred(&[id], !starred)? > 0;
        self.changed(id, changed)
    }

    /// The counts and the row of an item that changed.
    fn changed(&mut self, id: u64, changed: bool) -> Result<Changes> {
        let mut changes = Changes::default();
        if changed {
            self.load_counts()?;
            changes.counts = true;
            changes.rows.extend(self.list.row_of(id));
        }
        Ok(changes)
    }

    /// Marks all items of a feed, a folder or everything as read, up to
    /// the newest one stored, so that items arriving meanwhile stay
    /// unread. For "Starred", which the server can't mark as a whole, the
    /// items are marked one by one. Returns the number of items marked.
    pub fn mark_all_read(&mut self, node: Node) -> Result<(usize, Changes)> {
        let query = ItemQuery {
            selection: node.selection(),
            unread_only: true,
            search: String::new(),
        };
        let ids = self.store.item_ids(&query, Sort::default())?;
        let marked = match node {
            Node::Starred => self.store.set_unread(&ids, false)?,
            Node::All | Node::Folder(_) | Node::Feed(_) => match ids.iter().max() {
                Some(&newest) => {
                    let scope = match node {
                        Node::Folder(id) => MarkReadScope::Folder(id),
                        Node::Feed(id) => MarkReadScope::Feed(id),
                        _ => MarkReadScope::All,
                    };
                    self.store.mark_all_read(scope, newest)?
                }
                None => 0,
            },
        };
        let mut changes = Changes::default();
        if marked > 0 {
            self.load_counts()?;
            changes.counts = true;
            changes.list = true;
        }
        Ok((marked, changes))
    }

    /// The row of the next unread item after the current one.
    pub fn next_unread(&self) -> Result<Option<usize>> {
        let start = self.current_row().map_or(0, |row| row + 1);
        let ids = self.list.ids().get(start..).unwrap_or_default();
        for (chunk_index, chunk) in ids.chunks(SCAN_CHUNK).enumerate() {
            // Summaries skip items that are gone, so match them by id.
            let unread: HashSet<u64> = self
                .store
                .item_summaries(chunk)?
                .into_iter()
                .filter(|item| item.status != Status::Read)
                .map(|item| item.id)
                .collect();
            if let Some(offset) = chunk.iter().position(|id| unread.contains(id)) {
                return Ok(Some(start + chunk_index * SCAN_CHUNK + offset));
            }
        }
        Ok(None)
    }

    /// The store, for tests.
    #[cfg(test)]
    fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }
}

#[cfg(test)]
mod tests {
    use nextcloud_news::types;
    use ren_store::SortColumn;

    use super::*;
    use crate::item_list::tests::{api_feed, api_item};

    /// Feeds 1 and 2 in folder 7, feed 3 outside; items 1–6, two per feed,
    /// all unread; item 2 starred.
    fn reader() -> Reader {
        let mut store = Store::open_in_memory().unwrap();
        store
            .replace_folders(&[types::Folder {
                id: 7,
                name: "Tech".into(),
            }])
            .unwrap();
        store
            .replace_feeds(&[
                api_feed(1, Some(7), "One"),
                api_feed(2, Some(7), "Two"),
                api_feed(3, None, ""),
            ])
            .unwrap();
        let mut items: Vec<_> = (1..=6)
            .map(|id| api_item(id, id.div_ceil(2), &format!("Item {id}"), "Ada"))
            .collect();
        items[1].starred = true;
        store.upsert_items(&items, false).unwrap();
        Reader::new(store).unwrap()
    }

    fn counts(reader: &Reader) -> Vec<(Node, u64)> {
        reader
            .tree_rows()
            .into_iter()
            .map(|row| (row.node, row.count))
            .collect()
    }

    fn ids(reader: &Reader) -> Vec<u64> {
        (0..reader.len()).filter_map(|row| reader.id(row)).collect()
    }

    #[test]
    fn starts_with_all_items() {
        let reader = reader();
        assert_eq!(reader.selected(), Node::All);
        assert_eq!(ids(&reader), [6, 5, 4, 3, 2, 1]);
        assert_eq!(
            counts(&reader),
            [
                (Node::All, 6),
                (Node::Starred, 1),
                (Node::Folder(7), 4),
                (Node::Feed(1), 2),
                (Node::Feed(2), 2),
                (Node::Feed(3), 2),
            ]
        );
        assert_eq!(reader.unread(), 6);
        assert_eq!(reader.current(), None);
        // A feed without a title goes by its URL.
        assert_eq!(reader.feed_title(3), "https://example.org/3/feed");
    }

    #[test]
    fn full_page_feeds() {
        let mut reader = reader();
        assert!(!reader.is_full_page(3));
        reader.set_full_page(3, true).unwrap();
        assert!(reader.is_full_page(3));
        // Item 6 is of feed 3, item 4 of feed 2.
        reader.open(0).unwrap();
        assert!(reader.current_full_page());
        reader.open(2).unwrap();
        assert!(!reader.current_full_page());
        // Kept in the store.
        reader.reload().unwrap();
        assert!(reader.is_full_page(3));
        reader.set_full_page(3, false).unwrap();
        reader.reload().unwrap();
        assert!(!reader.is_full_page(3));
        assert!(reader.set_full_page(99, true).is_err());
    }

    #[test]
    fn select_lists() {
        let mut reader = reader();
        reader.select(Node::Folder(7)).unwrap();
        assert_eq!(ids(&reader), [4, 3, 2, 1]);
        reader.select(Node::Feed(3)).unwrap();
        assert_eq!(ids(&reader), [6, 5]);
        reader.select(Node::Starred).unwrap();
        assert_eq!(ids(&reader), [2]);
    }

    #[test]
    fn opening_marks_read() {
        let mut reader = reader();
        let (article, changes) = reader.open(1).unwrap().unwrap();
        assert_eq!(article.title, "Item 5");
        assert_eq!(
            article.meta,
            "https://example.org/3/feed · Ada · 1970-01-01 00:08"
        );
        assert_eq!(article.url.as_deref(), Some("https://example.org/3/5"));
        assert_eq!(article.body, "<p>Body 5</p>");
        assert_eq!(
            changes,
            Changes {
                counts: true,
                rows: vec![1],
                list: false
            }
        );
        assert_eq!(reader.current(), Some(5));
        assert_eq!(reader.current_row(), Some(1));
        assert_eq!(reader.row(1).unwrap().unwrap().status, Status::Read);
        assert_eq!(reader.unread(), 5);
        assert_eq!(counts(&reader)[5], (Node::Feed(3), 1));

        // Opened again, nothing changes.
        let (_, changes) = reader.open(1).unwrap().unwrap();
        assert_eq!(changes, Changes::default());
        assert!(reader.open(6).unwrap().is_none());
    }

    #[test]
    fn read_and_unread() {
        let mut reader = reader();
        let changes = reader.set_read(3, true).unwrap();
        assert_eq!(changes.rows, [3]);
        assert!(changes.counts);
        assert_eq!(reader.unread(), 5);
        assert_eq!(reader.set_read(3, true).unwrap(), Changes::default());
        reader.set_read(3, false).unwrap();
        assert_eq!(reader.unread(), 6);
        // Queued for the server.
        assert_eq!(
            reader
                .store_mut()
                .pending(nextcloud_news::ItemAction::Unread, 10)
                .unwrap(),
            [3]
        );
    }

    #[test]
    fn star_toggle() {
        let mut reader = reader();
        reader.select(Node::Starred).unwrap();
        let changes = reader.toggle_star(2).unwrap();
        assert_eq!(changes.rows, [0]);
        assert!(!reader.is_starred(2).unwrap());
        assert_eq!(counts(&reader)[1], (Node::Starred, 0));
        // The row stays until the list is selected again.
        assert_eq!(ids(&reader), [2]);
        reader.select(Node::Starred).unwrap();
        assert!(ids(&reader).is_empty());

        reader.select(Node::All).unwrap();
        let changes = reader.toggle_star(6).unwrap();
        assert_eq!(changes.rows, [0]);
        assert!(reader.is_starred(6).unwrap());
        assert_eq!(counts(&reader)[1], (Node::Starred, 1));
    }

    #[test]
    fn mark_all_read() {
        let mut reader = reader();
        let (marked, changes) = reader.mark_all_read(Node::Feed(1)).unwrap();
        assert_eq!(marked, 2);
        assert!(changes.counts && changes.list);
        assert_eq!(reader.unread(), 4);
        assert_eq!(
            reader.store_mut().pending_mark_read().unwrap()[0].scope,
            MarkReadScope::Feed(1)
        );

        let (marked, _) = reader.mark_all_read(Node::Folder(7)).unwrap();
        assert_eq!(marked, 2);
        assert_eq!(counts(&reader)[2], (Node::Folder(7), 0));

        // Nothing left to mark: nothing is queued.
        let (marked, changes) = reader.mark_all_read(Node::Folder(7)).unwrap();
        assert_eq!((marked, changes), (0, Changes::default()));
        assert_eq!(reader.store_mut().pending_mark_read().unwrap().len(), 2);

        let (marked, _) = reader.mark_all_read(Node::All).unwrap();
        assert_eq!(marked, 2);
        assert_eq!(reader.unread(), 0);
    }

    #[test]
    fn mark_starred_read() {
        let mut reader = reader();
        let (marked, _) = reader.mark_all_read(Node::Starred).unwrap();
        assert_eq!(marked, 1);
        assert_eq!(reader.unread(), 5);
        assert!(reader.store_mut().pending_mark_read().unwrap().is_empty());
        assert_eq!(
            reader
                .store_mut()
                .pending(nextcloud_news::ItemAction::Read, 10)
                .unwrap(),
            [2]
        );
    }

    #[test]
    fn next_unread() {
        let mut reader = reader();
        assert_eq!(reader.next_unread().unwrap(), Some(0));
        reader.set_read(5, true).unwrap();
        reader.set_read(4, true).unwrap();
        reader.open(0).unwrap();
        assert_eq!(reader.next_unread().unwrap(), Some(3));
        reader.open(5).unwrap();
        assert_eq!(reader.next_unread().unwrap(), None);
    }

    #[test]
    fn next_unread_beyond_a_chunk() {
        let mut store = Store::open_in_memory().unwrap();
        store.replace_feeds(&[api_feed(1, None, "One")]).unwrap();
        let count = SCAN_CHUNK as u64 * 2 + 5;
        let items: Vec<_> = (1..=count)
            .map(|id| {
                let mut item = api_item(id, 1, "Item", "");
                item.unread = id == 2;
                item
            })
            .collect();
        store.upsert_items(&items, false).unwrap();
        let reader = Reader::new(store).unwrap();
        assert_eq!(
            reader.next_unread().unwrap(),
            Some(usize::try_from(count).unwrap() - 2)
        );
    }

    #[test]
    fn sort_and_search_keep_the_current_item() {
        let mut reader = reader();
        reader.open(0).unwrap();
        reader.sort_by(Column::Date).unwrap();
        assert_eq!(reader.sort().column, SortColumn::Date);
        assert!(reader.sort().ascending);
        assert_eq!(ids(&reader), [1, 2, 3, 4, 5, 6]);
        assert_eq!(reader.current_row(), Some(5));
        reader.search("item 6").unwrap();
        assert_eq!(ids(&reader), [6]);
        reader.search("item 1").unwrap();
        assert_eq!(reader.current_row(), None);
        assert_eq!(reader.current(), Some(6));
    }

    #[test]
    fn reload_after_a_sync() {
        let mut reader = reader();
        reader.toggle_folder(7);
        reader.select(Node::Feed(1)).unwrap();

        // The sync removes feed 1 and adds a folder and items.
        let store = reader.store_mut();
        store
            .replace_folders(&[
                types::Folder {
                    id: 7,
                    name: "Tech".into(),
                },
                types::Folder {
                    id: 8,
                    name: "News".into(),
                },
            ])
            .unwrap();
        store
            .replace_feeds(&[api_feed(2, Some(7), "Two"), api_feed(3, Some(8), "Three")])
            .unwrap();
        store
            .upsert_items(&[api_item(7, 3, "Item 7", "")], true)
            .unwrap();
        reader.reload().unwrap();

        assert_eq!(reader.selected(), Node::All);
        assert_eq!(ids(&reader), [7, 6, 5, 4, 3]);
        assert_eq!(reader.feed_title(3), "Three");
        assert_eq!(reader.row(0).unwrap().unwrap().status, Status::New);
        // Folder 7 stays collapsed, the new one is expanded.
        assert_eq!(
            counts(&reader),
            [
                (Node::All, 5),
                (Node::Starred, 0),
                (Node::Folder(8), 3),
                (Node::Feed(3), 3),
                (Node::Folder(7), 2),
            ]
        );
    }

    #[test]
    fn reload_keeps_the_selection() {
        let mut reader = reader();
        reader.select(Node::Feed(3)).unwrap();
        reader.reload().unwrap();
        assert_eq!(reader.selected(), Node::Feed(3));
        assert_eq!(ids(&reader), [6, 5]);
    }
}
