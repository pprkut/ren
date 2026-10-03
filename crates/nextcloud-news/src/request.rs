// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The read endpoints, as paths and query parameters relative to the API
//! base URL, and the paging logic for `GET /items`.

use crate::types::Item;

/// Which items `GET /items` and `GET /items/updated` return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Feed(u64),
    Folder(u64),
    Starred,
    All,
}

impl Selection {
    /// The `type` and `id` parameters.
    fn params(self) -> (u8, u64) {
        match self {
            Selection::Feed(id) => (0, id),
            Selection::Folder(id) => (1, id),
            Selection::Starred => (2, 0),
            Selection::All => (3, 0),
        }
    }
}

/// Parameters of `GET /items`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemQuery {
    pub selection: Selection,
    /// Include read items.
    pub get_read: bool,
    /// Items per page; `None` for all at once.
    pub batch_size: Option<u32>,
    /// Only items with an id below this one; 0 starts with the newest.
    pub offset: u64,
}

impl ItemQuery {
    /// Unread items of all feeds, newest first.
    pub fn unread(batch_size: Option<u32>) -> Self {
        Self {
            selection: Selection::All,
            get_read: false,
            batch_size,
            offset: 0,
        }
    }

    /// Starred items, read or not, newest first.
    pub fn starred(batch_size: Option<u32>) -> Self {
        Self {
            selection: Selection::Starred,
            get_read: true,
            batch_size,
            offset: 0,
        }
    }
}

/// A read request to the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Version,
    Status,
    Folders,
    Feeds,
    Items(ItemQuery),
    /// Items changed since `last_modified` (unix seconds, inclusive),
    /// including read and starred state changes. Not paged.
    UpdatedItems {
        last_modified: i64,
        selection: Selection,
    },
}

impl Endpoint {
    /// Path relative to the API base URL.
    pub fn path(&self) -> &'static str {
        match self {
            Endpoint::Version => "version",
            Endpoint::Status => "status",
            Endpoint::Folders => "folders",
            Endpoint::Feeds => "feeds",
            Endpoint::Items(_) => "items",
            Endpoint::UpdatedItems { .. } => "items/updated",
        }
    }

    pub fn query(&self) -> Vec<(&'static str, String)> {
        match *self {
            Endpoint::Items(q) => {
                let (kind, id) = q.selection.params();
                vec![
                    ("type", kind.to_string()),
                    ("id", id.to_string()),
                    ("getRead", q.get_read.to_string()),
                    ("batchSize", q.batch_size.map_or(-1, i64::from).to_string()),
                    ("offset", q.offset.to_string()),
                ]
            }
            Endpoint::UpdatedItems {
                last_modified,
                selection,
            } => {
                let (kind, id) = selection.params();
                vec![
                    ("lastModified", last_modified.to_string()),
                    ("type", kind.to_string()),
                    ("id", id.to_string()),
                ]
            }
            _ => Vec::new(),
        }
    }
}

/// Paging through `GET /items`: the next page starts below the lowest id of
/// the previous one.
#[derive(Debug, Clone)]
pub struct Pager {
    query: ItemQuery,
    done: bool,
}

/// What a page contained, after dropping items already seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageInfo {
    /// Items not seen on an earlier page.
    pub new: usize,
    /// Items with an id at or above the offset, i.e. already seen. The API
    /// documentation is ambiguous on whether the offset is inclusive.
    pub repeated: usize,
}

impl Pager {
    pub fn new(query: ItemQuery) -> Self {
        Self { query, done: false }
    }

    /// The query for the next page, or `None` when all pages were fetched.
    pub fn next_query(&self) -> Option<ItemQuery> {
        (!self.done).then_some(self.query)
    }

    /// Takes a fetched page: removes items that an earlier page already
    /// contained and advances to the next page. Paging ends with a page
    /// shorter than the batch size, or one without new items.
    pub fn advance(&mut self, page: &mut Vec<Item>) -> PageInfo {
        let total = page.len();
        if self.query.offset > 0 {
            let offset = self.query.offset;
            page.retain(|item| item.id < offset);
        }
        let info = PageInfo {
            new: page.len(),
            repeated: total - page.len(),
        };
        let lowest = page.iter().map(|item| item.id).min();
        match (lowest, self.query.batch_size) {
            (Some(lowest), Some(size)) if total >= size as usize => self.query.offset = lowest,
            _ => self.done = true,
        }
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64) -> Item {
        serde_json::from_str(&format!(r#"{{"id": {id}, "feedId": 1}}"#)).unwrap()
    }

    fn page(ids: &[u64]) -> Vec<Item> {
        ids.iter().copied().map(item).collect()
    }

    #[test]
    fn item_query_parameters() {
        let q = Endpoint::Items(ItemQuery {
            offset: 43,
            ..ItemQuery::unread(Some(200))
        });
        assert_eq!(q.path(), "items");
        assert_eq!(
            q.query(),
            [
                ("type", "3".to_owned()),
                ("id", "0".to_owned()),
                ("getRead", "false".to_owned()),
                ("batchSize", "200".to_owned()),
                ("offset", "43".to_owned()),
            ]
        );
        let starred = Endpoint::Items(ItemQuery::starred(None)).query();
        assert!(starred.contains(&("type", "2".to_owned())));
        assert!(starred.contains(&("getRead", "true".to_owned())));
        assert!(starred.contains(&("batchSize", "-1".to_owned())));
    }

    #[test]
    fn updated_items_parameters() {
        let q = Endpoint::UpdatedItems {
            last_modified: 1_367_273_003,
            selection: Selection::Feed(12),
        };
        assert_eq!(q.path(), "items/updated");
        assert_eq!(
            q.query(),
            [
                ("lastModified", "1367273003".to_owned()),
                ("type", "0".to_owned()),
                ("id", "12".to_owned()),
            ]
        );
        assert!(Endpoint::Feeds.query().is_empty());
    }

    #[test]
    fn paging_until_short_page() {
        let mut pager = Pager::new(ItemQuery::unread(Some(3)));
        assert_eq!(pager.next_query().unwrap().offset, 0);

        let mut p = page(&[90, 80, 70]);
        assert_eq!(
            pager.advance(&mut p),
            PageInfo {
                new: 3,
                repeated: 0
            }
        );
        assert_eq!(pager.next_query().unwrap().offset, 70);

        let mut p = page(&[60, 50]);
        pager.advance(&mut p);
        assert_eq!(pager.next_query(), None);
    }

    #[test]
    fn inclusive_offset_does_not_loop() {
        let mut pager = Pager::new(ItemQuery::unread(Some(2)));
        pager.advance(&mut page(&[90, 80]));
        // The server includes the offset item again.
        let mut p = page(&[80, 70]);
        assert_eq!(
            pager.advance(&mut p),
            PageInfo {
                new: 1,
                repeated: 1
            }
        );
        assert_eq!(p, page(&[70]));
        assert_eq!(pager.next_query().unwrap().offset, 70);
        // Only the offset item comes back: nothing new, stop.
        let mut p = page(&[70, 70]);
        assert_eq!(
            pager.advance(&mut p),
            PageInfo {
                new: 0,
                repeated: 2
            }
        );
        assert_eq!(pager.next_query(), None);
    }

    #[test]
    fn unpaged_query_is_one_page() {
        let mut pager = Pager::new(ItemQuery::unread(None));
        pager.advance(&mut page(&[3, 2, 1]));
        assert_eq!(pager.next_query(), None);
    }

    #[test]
    fn empty_page_ends_paging() {
        let mut pager = Pager::new(ItemQuery::unread(Some(10)));
        pager.advance(&mut Vec::new());
        assert_eq!(pager.next_query(), None);
    }
}
