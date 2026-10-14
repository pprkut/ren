// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The endpoints, as paths, query parameters and request bodies relative to
//! the API base URL, and the paging logic for `GET /items`.

use serde::Serialize;

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

/// What [`Update::Items`] does to the items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemAction {
    Read,
    Unread,
    Star,
    Unstar,
}

/// Which items [`Update::MarkRead`] marks as read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadScope {
    All,
    Feed(u64),
    /// A folder. Feeds outside of folders have to be marked one by one:
    /// folder 0 fails with HTTP 500 (News 28.7 passes a null id where it
    /// expects a number).
    Folder(u64),
}

/// A write request to the API: a `POST` with a JSON body (API v1-3; the
/// `PUT` variants in the documentation's "How To Sync" section are v1-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Update<'a> {
    /// `POST /items/{read,unread,star,unstar}/multiple` with
    /// `{"itemIds": [...]}`. The server skips ids it doesn't know.
    Items { action: ItemAction, ids: &'a [u64] },
    /// "Mark all as read": the items up to and including `newest_item_id`,
    /// so that items which arrived after the user's last look stay unread.
    /// `POST /items/read`, `/feeds/{id}/read` or `/folders/{id}/read` with
    /// `{"newestItemId": n}`.
    MarkRead {
        scope: ReadScope,
        newest_item_id: u64,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ItemIds<'a> {
    item_ids: &'a [u64],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NewestItemId {
    newest_item_id: u64,
}

impl Update<'_> {
    /// Path relative to the API base URL.
    pub fn path(&self) -> String {
        match *self {
            Update::Items { action, .. } => {
                let action = match action {
                    ItemAction::Read => "read",
                    ItemAction::Unread => "unread",
                    ItemAction::Star => "star",
                    ItemAction::Unstar => "unstar",
                };
                format!("items/{action}/multiple")
            }
            Update::MarkRead { scope, .. } => match scope {
                ReadScope::All => "items/read".to_owned(),
                ReadScope::Feed(id) => format!("feeds/{id}/read"),
                ReadScope::Folder(id) => format!("folders/{id}/read"),
            },
        }
    }

    /// The JSON request body.
    pub fn body(&self) -> Vec<u8> {
        let body = match *self {
            Update::Items { ids, .. } => serde_json::to_vec(&ItemIds { item_ids: ids }),
            Update::MarkRead { newest_item_id, .. } => {
                serde_json::to_vec(&NewestItemId { newest_item_id })
            }
        };
        body.expect("serialising numbers can't fail")
    }
}

/// A change to the feeds and folders on the server. Adding a feed or a
/// folder answers with it ([`Edited`](crate::Edited)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit<'a> {
    /// `POST /feeds` with `{"url": …, "folderId": …}`: the server finds
    /// the feed (also from a web page's address) and fetches it. Folder 0
    /// is none.
    AddFeed {
        url: &'a str,
        folder_id: Option<u64>,
    },
    /// `POST /feeds/{id}/rename` with `{"feedTitle": …}`.
    RenameFeed { id: u64, title: &'a str },
    /// `POST /feeds/{id}/move` with `{"folderId": …}`; 0 is no folder.
    MoveFeed { id: u64, folder_id: Option<u64> },
    /// `DELETE /feeds/{id}`, with its articles.
    DeleteFeed { id: u64 },
    /// `POST /folders` with `{"name": …}`.
    AddFolder { name: &'a str },
    /// `PUT /folders/{id}` with `{"name": …}`.
    RenameFolder { id: u64, name: &'a str },
    /// `DELETE /folders/{id}`, with its feeds and their articles.
    DeleteFolder { id: u64 },
}

/// The HTTP methods of [`Edit`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Post,
    Put,
    Delete,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NewFeed<'a> {
    url: &'a str,
    folder_id: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeedTitle<'a> {
    feed_title: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderId {
    folder_id: u64,
}

#[derive(Serialize)]
struct FolderName<'a> {
    name: &'a str,
}

impl Edit<'_> {
    pub fn method(&self) -> Method {
        match self {
            Edit::AddFeed { .. }
            | Edit::RenameFeed { .. }
            | Edit::MoveFeed { .. }
            | Edit::AddFolder { .. } => Method::Post,
            Edit::RenameFolder { .. } => Method::Put,
            Edit::DeleteFeed { .. } | Edit::DeleteFolder { .. } => Method::Delete,
        }
    }

    /// Path relative to the API base URL.
    pub fn path(&self) -> String {
        match *self {
            Edit::AddFeed { .. } => "feeds".to_owned(),
            Edit::RenameFeed { id, .. } => format!("feeds/{id}/rename"),
            Edit::MoveFeed { id, .. } => format!("feeds/{id}/move"),
            Edit::DeleteFeed { id } => format!("feeds/{id}"),
            Edit::AddFolder { .. } => "folders".to_owned(),
            Edit::RenameFolder { id, .. } | Edit::DeleteFolder { id } => format!("folders/{id}"),
        }
    }

    /// The JSON request body; none for deletions.
    pub fn body(&self) -> Option<Vec<u8>> {
        let body = match *self {
            Edit::AddFeed { url, folder_id } => serde_json::to_vec(&NewFeed {
                url,
                folder_id: folder_id.unwrap_or(0),
            }),
            Edit::RenameFeed { title, .. } => serde_json::to_vec(&FeedTitle { feed_title: title }),
            Edit::MoveFeed { folder_id, .. } => serde_json::to_vec(&FolderId {
                folder_id: folder_id.unwrap_or(0),
            }),
            Edit::AddFolder { name } | Edit::RenameFolder { name, .. } => {
                serde_json::to_vec(&FolderName { name })
            }
            Edit::DeleteFeed { .. } | Edit::DeleteFolder { .. } => return None,
        };
        Some(body.expect("serialising strings and numbers can't fail"))
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

    #[test]
    fn item_updates() {
        let ids = [3, 1, 2];
        for (action, path) in [
            (ItemAction::Read, "items/read/multiple"),
            (ItemAction::Unread, "items/unread/multiple"),
            (ItemAction::Star, "items/star/multiple"),
            (ItemAction::Unstar, "items/unstar/multiple"),
        ] {
            let update = Update::Items { action, ids: &ids };
            assert_eq!(update.path(), path);
            assert_eq!(update.body(), br#"{"itemIds":[3,1,2]}"#);
        }
    }

    #[test]
    fn mark_read_updates() {
        for (scope, path) in [
            (ReadScope::All, "items/read"),
            (ReadScope::Feed(12), "feeds/12/read"),
            (ReadScope::Folder(4), "folders/4/read"),
            (ReadScope::Folder(0), "folders/0/read"),
        ] {
            let update = Update::MarkRead {
                scope,
                newest_item_id: 227_491,
            };
            assert_eq!(update.path(), path);
            assert_eq!(update.body(), br#"{"newestItemId":227491}"#);
        }
    }

    #[test]
    fn edits() {
        let body = |edit: Edit<'_>| edit.body().map(|body| String::from_utf8(body).unwrap());
        let cases = [
            (
                Edit::AddFeed {
                    url: "https://example.org/\"feed\"",
                    folder_id: Some(3),
                },
                Method::Post,
                "feeds",
                Some(r#"{"url":"https://example.org/\"feed\"","folderId":3}"#),
            ),
            (
                Edit::AddFeed {
                    url: "example.org",
                    folder_id: None,
                },
                Method::Post,
                "feeds",
                Some(r#"{"url":"example.org","folderId":0}"#),
            ),
            (
                Edit::RenameFeed { id: 4, title: "Ä" },
                Method::Post,
                "feeds/4/rename",
                Some(r#"{"feedTitle":"Ä"}"#),
            ),
            (
                Edit::MoveFeed {
                    id: 4,
                    folder_id: None,
                },
                Method::Post,
                "feeds/4/move",
                Some(r#"{"folderId":0}"#),
            ),
            (Edit::DeleteFeed { id: 4 }, Method::Delete, "feeds/4", None),
            (
                Edit::AddFolder { name: "News" },
                Method::Post,
                "folders",
                Some(r#"{"name":"News"}"#),
            ),
            (
                Edit::RenameFolder { id: 2, name: "Old" },
                Method::Put,
                "folders/2",
                Some(r#"{"name":"Old"}"#),
            ),
            (
                Edit::DeleteFolder { id: 2 },
                Method::Delete,
                "folders/2",
                None,
            ),
        ];
        for (edit, method, path, expected) in cases {
            assert_eq!(edit.method(), method, "{edit:?}");
            assert_eq!(edit.path(), path, "{edit:?}");
            assert_eq!(body(edit).as_deref(), expected, "{edit:?}");
        }
    }
}
