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
///
/// ```no_run
/// # fn f(client: &nextcloud_news::Client) -> Result<(), nextcloud_news::Error> {
/// use nextcloud_news::{ItemQuery, Pager};
///
/// let mut pager = Pager::new(ItemQuery::unread(Some(1000)));
/// while let Some(query) = pager.start_page() {
///     client.items(query, |item| {
///         if pager.accept(&item) {
///             // Store the item.
///         }
///         Ok::<_, nextcloud_news::Error>(())
///     })?;
///     pager.finish_page();
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Pager {
    query: ItemQuery,
    done: bool,
    /// The current page so far.
    page: PageInfo,
    lowest: Option<u64>,
}

/// What a page contained.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageInfo {
    /// Items not seen on an earlier page.
    pub new: usize,
    /// Items with an id at or above the offset, i.e. already seen. The API
    /// documentation is ambiguous on whether the offset is inclusive.
    pub repeated: usize,
}

impl Pager {
    pub fn new(query: ItemQuery) -> Self {
        Self {
            query,
            done: false,
            page: PageInfo::default(),
            lowest: None,
        }
    }

    /// Starts the next page and returns its query, or `None` when all pages
    /// were fetched. Starting a page again, e.g. after a failed request,
    /// forgets what was accepted from it.
    pub fn start_page(&mut self) -> Option<ItemQuery> {
        self.page = PageInfo::default();
        self.lowest = None;
        (!self.done).then_some(self.query)
    }

    /// Takes an item of the current page. Returns `false` for an item that
    /// an earlier page already contained, which the caller should skip.
    pub fn accept(&mut self, item: &Item) -> bool {
        let offset = self.query.offset;
        if offset > 0 && item.id >= offset {
            self.page.repeated += 1;
            return false;
        }
        self.page.new += 1;
        self.lowest = Some(self.lowest.map_or(item.id, |lowest| lowest.min(item.id)));
        true
    }

    /// Ends the current page and advances to the next one. Paging ends with
    /// a page shorter than the batch size, or one without new items.
    pub fn finish_page(&mut self) -> PageInfo {
        let total = self.page.new + self.page.repeated;
        match (self.lowest, self.query.batch_size) {
            (Some(lowest), Some(size)) if total >= size as usize => self.query.offset = lowest,
            _ => self.done = true,
        }
        self.page
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

    /// Feeds a page through the pager; returns the accepted ids.
    fn take(pager: &mut Pager, ids: &[u64]) -> (Vec<u64>, PageInfo) {
        assert!(pager.start_page().is_some());
        let accepted = page(ids)
            .iter()
            .filter(|item| pager.accept(item))
            .map(|item| item.id)
            .collect();
        (accepted, pager.finish_page())
    }

    #[test]
    fn paging_until_short_page() {
        let mut pager = Pager::new(ItemQuery::unread(Some(3)));
        assert_eq!(pager.start_page().unwrap().offset, 0);

        assert_eq!(
            take(&mut pager, &[90, 80, 70]),
            (
                vec![90, 80, 70],
                PageInfo {
                    new: 3,
                    repeated: 0
                }
            )
        );
        assert_eq!(pager.start_page().unwrap().offset, 70);

        take(&mut pager, &[60, 50]);
        assert_eq!(pager.start_page(), None);
    }

    #[test]
    fn inclusive_offset_does_not_loop() {
        let mut pager = Pager::new(ItemQuery::unread(Some(2)));
        take(&mut pager, &[90, 80]);
        // The server includes the offset item again.
        assert_eq!(
            take(&mut pager, &[80, 70]),
            (
                vec![70],
                PageInfo {
                    new: 1,
                    repeated: 1
                }
            )
        );
        assert_eq!(pager.start_page().unwrap().offset, 70);
        // Only the offset item comes back: nothing new, stop.
        assert_eq!(
            take(&mut pager, &[70, 70]),
            (
                vec![],
                PageInfo {
                    new: 0,
                    repeated: 2
                }
            )
        );
        assert_eq!(pager.start_page(), None);
    }

    #[test]
    fn restarted_page_forgets_partial_items() {
        let mut pager = Pager::new(ItemQuery::unread(Some(3)));
        take(&mut pager, &[90, 80, 70]);
        // The request for the second page fails after one item.
        pager.start_page();
        assert!(pager.accept(&item(10)));
        // Retried: the lowest id must not be the 10 from the failed try.
        let (_, info) = take(&mut pager, &[60, 50, 40]);
        assert_eq!(info.new, 3);
        assert_eq!(pager.start_page().unwrap().offset, 40);
    }

    #[test]
    fn unpaged_query_is_one_page() {
        let mut pager = Pager::new(ItemQuery::unread(None));
        take(&mut pager, &[3, 2, 1]);
        assert_eq!(pager.start_page(), None);
    }

    #[test]
    fn empty_page_ends_paging() {
        let mut pager = Pager::new(ItemQuery::unread(Some(10)));
        take(&mut pager, &[]);
        assert_eq!(pager.start_page(), None);
    }
}
