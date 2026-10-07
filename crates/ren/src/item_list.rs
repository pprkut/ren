// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! View model of the item list: which items are shown, in which order.
//!
//! The store filters and sorts; the list keeps the ids of all its items in
//! order (8 bytes each), and the rows shown are fetched by id when the
//! list view asks for them (`docs/decisions/0006-store.md`).

use ren_store::{ItemQuery, Selection, Sort, SortColumn, Store};

/// The columns of the item table, in the order of their indices in the
/// UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Title,
    Feed,
    Author,
    Date,
}

impl Column {
    pub const ALL: [Column; 4] = [Column::Title, Column::Feed, Column::Author, Column::Date];

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&c| c == self).unwrap_or(0)
    }

    fn sort_column(self) -> SortColumn {
        match self {
            Column::Title => SortColumn::Title,
            Column::Feed => SortColumn::Feed,
            Column::Author => SortColumn::Author,
            Column::Date => SortColumn::Date,
        }
    }

    pub fn of(column: SortColumn) -> Self {
        match column {
            SortColumn::Title => Column::Title,
            SortColumn::Feed => Column::Feed,
            SortColumn::Author => Column::Author,
            SortColumn::Date => Column::Date,
        }
    }
}

/// Keys that move the selection in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

/// Where the selection goes from `current` (`None`: nothing selected) in a
/// list of `len` rows, `page` rows per page. `None` if the list is empty.
pub fn step(current: Option<usize>, key: Key, len: usize, page: usize) -> Option<usize> {
    let last = len.checked_sub(1)?;
    let page = page.max(1);
    let next = match (key, current) {
        (Key::Home, _) | (Key::Down | Key::PageDown, None) => 0,
        (Key::End, _) | (Key::Up | Key::PageUp, None) => last,
        (Key::Up, Some(i)) => i.saturating_sub(1),
        (Key::Down, Some(i)) => i + 1,
        (Key::PageUp, Some(i)) => i.saturating_sub(page),
        (Key::PageDown, Some(i)) => i + page,
    };
    Some(next.min(last))
}

/// The items of the selected list, filtered by the search text and
/// sorted.
#[derive(Debug, Default)]
pub struct ItemList {
    query: ItemQuery,
    sort: Sort,
    /// The ids shown, in display order.
    ids: Vec<u64>,
}

impl ItemList {
    pub fn ids(&self) -> &[u64] {
        &self.ids
    }

    pub fn id(&self, row: usize) -> Option<u64> {
        self.ids.get(row).copied()
    }

    pub fn row_of(&self, id: u64) -> Option<usize> {
        self.ids.iter().position(|&i| i == id)
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    /// Shows the items of another list; [`load`](Self::load) fetches them.
    pub fn set_selection(&mut self, selection: Selection) {
        self.query.selection = selection;
    }

    /// Shows only items whose title or author contains `text`, ignoring
    /// case; [`load`](Self::load) fetches them.
    pub fn set_search(&mut self, text: &str) {
        text.trim().clone_into(&mut self.query.search);
    }

    /// Sorts by `column`; ascending first for text columns, newest first for
    /// the date. Selecting the current column again reverses the order.
    /// [`load`](Self::load) fetches the items in the new order.
    pub fn sort_by(&mut self, column: Column) {
        let column = column.sort_column();
        self.sort = if self.sort.column == column {
            Sort {
                column,
                ascending: !self.sort.ascending,
            }
        } else {
            Sort {
                column,
                ascending: column != SortColumn::Date,
            }
        };
    }

    /// Fetches the ids of the items to show.
    pub fn load(&mut self, store: &Store) -> ren_store::Result<()> {
        self.ids = store.item_ids(&self.query, self.sort)?;
        Ok(())
    }
}

/// Formats unix seconds as `YYYY-MM-DD HH:MM` (UTC).
pub fn format_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rem / 3_600,
        rem % 3_600 / 60
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use nextcloud_news::types;

    use super::*;

    pub fn api_feed(id: u64, folder_id: Option<u64>, title: &str) -> types::Feed {
        types::Feed {
            id,
            url: format!("https://example.org/{id}/feed"),
            title: Some(title.into()),
            favicon_link: None,
            added: None,
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
    pub fn api_item(id: u64, feed_id: u64, title: &str, author: &str) -> types::Item {
        types::Item {
            id,
            guid: None,
            guid_hash: None,
            url: Some(format!("https://example.org/{feed_id}/{id}")),
            title: Some(title.into()),
            author: Some(author.into()),
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

    fn store() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .replace_feeds(&[api_feed(1, None, "B feed"), api_feed(2, None, "A feed")])
            .unwrap();
        store
            .upsert_items(
                &[
                    api_item(1, 1, "banana split", "Zoe"),
                    api_item(2, 2, "Apple pie", "Yann"),
                    api_item(3, 1, "cherry tart", "Xia"),
                    api_item(4, 2, "apple crumble", "Wim"),
                ],
                false,
            )
            .unwrap();
        store
    }

    fn list(store: &Store) -> ItemList {
        let mut list = ItemList::default();
        list.load(store).unwrap();
        list
    }

    #[test]
    fn newest_first_by_default() {
        assert_eq!(list(&store()).ids(), [4, 3, 2, 1]);
    }

    #[test]
    fn sort_by_title_ignores_case_and_toggles() {
        let store = store();
        let mut list = list(&store);
        list.sort_by(Column::Title);
        list.load(&store).unwrap();
        assert_eq!(list.ids(), [4, 2, 1, 3]);
        assert!(list.sort().ascending);
        list.sort_by(Column::Title);
        list.load(&store).unwrap();
        assert_eq!(list.ids(), [3, 1, 2, 4]);
    }

    #[test]
    fn sort_by_other_columns() {
        let store = store();
        let mut list = list(&store);
        let mut sorted = |column| {
            list.sort_by(column);
            list.load(&store).unwrap();
            list.ids().to_vec()
        };
        assert_eq!(sorted(Column::Author), [4, 3, 2, 1]);
        // By feed title, then newest first.
        assert_eq!(sorted(Column::Feed), [4, 2, 3, 1]);
        assert_eq!(sorted(Column::Date), [4, 3, 2, 1], "newest first again");
        assert_eq!(sorted(Column::Date), [1, 2, 3, 4]);
    }

    #[test]
    fn search_title_and_author() {
        let store = store();
        let mut list = list(&store);
        list.set_search("APPLE");
        list.load(&store).unwrap();
        assert_eq!(list.ids(), [4, 2]);
        list.set_search(" xia ");
        list.load(&store).unwrap();
        assert_eq!(list.ids(), [3]);
        list.set_search("");
        list.load(&store).unwrap();
        assert_eq!(list.ids().len(), 4);
    }

    #[test]
    fn selection() {
        let store = store();
        let mut list = list(&store);
        list.set_selection(Selection::Feed(1));
        list.load(&store).unwrap();
        assert_eq!(list.ids(), [3, 1]);
    }

    #[test]
    fn rows_and_ids() {
        let list = list(&store());
        assert_eq!(list.id(0), Some(4));
        assert_eq!(list.row_of(2), Some(2));
        assert_eq!(list.row_of(9), None);
        assert_eq!(list.id(9), None);
    }

    #[test]
    fn column_indices() {
        for (i, column) in Column::ALL.into_iter().enumerate() {
            assert_eq!(column.index(), i);
            assert_eq!(Column::from_index(i), Some(column));
            assert_eq!(Column::of(column.sort_column()), column);
        }
        assert_eq!(Column::from_index(4), None);
    }

    #[test]
    fn keyboard_steps() {
        assert_eq!(step(None, Key::Down, 0, 10), None);
        assert_eq!(step(None, Key::Down, 5, 10), Some(0));
        assert_eq!(step(None, Key::Up, 5, 10), Some(4));
        assert_eq!(step(Some(0), Key::Up, 5, 10), Some(0));
        assert_eq!(step(Some(4), Key::Down, 5, 10), Some(4));
        assert_eq!(step(Some(2), Key::PageDown, 50, 10), Some(12));
        assert_eq!(step(Some(45), Key::PageDown, 50, 10), Some(49));
        assert_eq!(step(Some(5), Key::PageUp, 50, 10), Some(0));
        assert_eq!(step(Some(5), Key::Home, 50, 10), Some(0));
        assert_eq!(step(Some(5), Key::End, 50, 10), Some(49));
    }

    #[test]
    fn date_formatting() {
        assert_eq!(format_date(0), "1970-01-01 00:00");
        assert_eq!(format_date(1_788_220_800), "2026-09-01 00:00");
        assert_eq!(format_date(951_827_696), "2000-02-29 12:34");
    }
}
