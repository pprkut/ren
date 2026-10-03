// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! View model of the item list: which items are shown, in which order.
//!
//! Sorting and filtering happen here on dummy data for now; with the store
//! (M2) they move into its queries and this keeps only the state.

use std::cmp::Ordering;

use crate::dummy::ItemSummary;

/// The columns of the item table.
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub column: Column,
    pub ascending: bool,
}

impl Default for Sort {
    /// Newest first.
    fn default() -> Self {
        Self {
            column: Column::Date,
            ascending: false,
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

/// The items of the selected feed or folder, filtered by the search text and
/// sorted.
#[derive(Default)]
pub struct ItemList {
    /// All item ids of the selected feed or folder.
    source: Vec<u32>,
    /// The ids shown, in display order.
    shown: Vec<u32>,
    sort: Sort,
    search: String,
}

impl ItemList {
    pub fn ids(&self) -> &[u32] {
        &self.shown
    }

    pub fn id(&self, row: usize) -> Option<u32> {
        self.shown.get(row).copied()
    }

    pub fn row_of(&self, id: u32) -> Option<usize> {
        self.shown.iter().position(|&i| i == id)
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    /// Shows the given items.
    pub fn set_source(&mut self, ids: Vec<u32>, item: impl Fn(u32) -> ItemSummary) {
        self.source = ids;
        self.refresh(item);
    }

    /// Sorts by `column`; ascending first for text columns, newest first for
    /// the date. Selecting the current column again reverses the order.
    pub fn sort_by(&mut self, column: Column, item: impl Fn(u32) -> ItemSummary) {
        self.sort = if self.sort.column == column {
            Sort {
                column,
                ascending: !self.sort.ascending,
            }
        } else {
            Sort {
                column,
                ascending: column != Column::Date,
            }
        };
        self.refresh(item);
    }

    /// Shows only items whose title or author contains `text`, ignoring case.
    pub fn set_search(&mut self, text: &str, item: impl Fn(u32) -> ItemSummary) {
        self.search = text.trim().to_lowercase();
        self.refresh(item);
    }

    fn refresh(&mut self, item: impl Fn(u32) -> ItemSummary) {
        // The sort keys are computed once per item, not once per comparison.
        let mut rows: Vec<(u32, ItemSummary)> = self
            .source
            .iter()
            .map(|&id| (id, item(id)))
            .filter(|(_, it)| {
                self.search.is_empty()
                    || it.title.to_lowercase().contains(&self.search)
                    || it.author.to_lowercase().contains(&self.search)
            })
            .collect();
        let sort = self.sort;
        rows.sort_by(|(_, a), (_, b)| {
            let order = compare(a, b, sort.column).then(b.pub_date.cmp(&a.pub_date));
            if sort.ascending {
                order
            } else {
                order.reverse()
            }
        });
        self.shown = rows.into_iter().map(|(id, _)| id).collect();
    }
}

fn compare(a: &ItemSummary, b: &ItemSummary, column: Column) -> Ordering {
    match column {
        Column::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        // Feed ids are in title order in the dummy data; the store will sort
        // by title.
        Column::Feed => a.feed_id.cmp(&b.feed_id),
        Column::Author => a.author.to_lowercase().cmp(&b.author.to_lowercase()),
        Column::Date => a.pub_date.cmp(&b.pub_date),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dummy::Status;

    fn item(id: u32) -> ItemSummary {
        let titles = ["banana split", "Apple pie", "cherry tart", "apple crumble"];
        let authors = ["Zoe", "Yann", "Xia", "Wim"];
        ItemSummary {
            id,
            feed_id: 3 - id,
            title: titles[id as usize].to_owned(),
            author: authors[id as usize].to_owned(),
            pub_date: 1_000 - i64::from(id),
            status: Status::Unread,
            starred: false,
        }
    }

    fn list() -> ItemList {
        let mut list = ItemList::default();
        list.set_source(vec![0, 1, 2, 3], item);
        list
    }

    #[test]
    fn newest_first_by_default() {
        assert_eq!(list().ids(), [0, 1, 2, 3]);
    }

    #[test]
    fn sort_by_title_ignores_case_and_toggles() {
        let mut list = list();
        list.sort_by(Column::Title, item);
        assert_eq!(list.ids(), [3, 1, 0, 2]);
        assert!(list.sort().ascending);
        list.sort_by(Column::Title, item);
        assert_eq!(list.ids(), [2, 0, 1, 3]);
    }

    #[test]
    fn sort_by_other_columns() {
        let mut list = list();
        list.sort_by(Column::Author, item);
        assert_eq!(list.ids(), [3, 2, 1, 0]);
        list.sort_by(Column::Feed, item);
        assert_eq!(list.ids(), [3, 2, 1, 0]);
        list.sort_by(Column::Date, item);
        assert_eq!(list.ids(), [0, 1, 2, 3], "newest first again");
        list.sort_by(Column::Date, item);
        assert_eq!(list.ids(), [3, 2, 1, 0]);
    }

    #[test]
    fn search_title_and_author() {
        let mut list = list();
        list.set_search("APPLE", item);
        assert_eq!(list.ids(), [1, 3]);
        list.set_search(" xia ", item);
        assert_eq!(list.ids(), [2]);
        list.set_search("", item);
        assert_eq!(list.ids().len(), 4);
    }

    #[test]
    fn rows_and_ids() {
        let mut list = list();
        list.sort_by(Column::Title, item);
        assert_eq!(list.id(0), Some(3));
        assert_eq!(list.row_of(2), Some(3));
        assert_eq!(list.id(9), None);
    }

    #[test]
    fn column_indices() {
        for (i, column) in Column::ALL.into_iter().enumerate() {
            assert_eq!(column.index(), i);
            assert_eq!(Column::from_index(i), Some(column));
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
}
