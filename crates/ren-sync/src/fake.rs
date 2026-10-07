// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! An in-memory News server for the tests: answers the requests the sync
//! makes from its own folders, feeds and items, logs them, and fails them
//! on request.

use std::cell::RefCell;
use std::collections::BTreeMap;

use nextcloud_news::types::{Feed, Feeds, Folder, Item};
use nextcloud_news::{Error, ItemAction, ItemQuery, ReadScope, Selection, Update};

use crate::NewsApi;

/// A request as the fake received it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Folders,
    Feeds,
    Items(ItemQuery),
    Updated(i64),
    Write(ItemAction, Vec<u64>),
    MarkRead(ReadScope, u64),
}

/// How a request fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fail {
    /// HTTP status, after handing over this many items.
    Status(u16, usize),
    /// The connection is lost after this many items.
    Network(usize),
    Unauthorized,
}

impl Fail {
    fn error(self) -> Error {
        match self {
            Fail::Status(code, _) => Error::Status {
                code,
                message: None,
            },
            Fail::Network(_) => Error::Network(ureq::Error::ConnectionFailed),
            Fail::Unauthorized => Error::Unauthorized,
        }
    }

    fn after(self) -> usize {
        match self {
            Fail::Status(_, after) | Fail::Network(after) => after,
            Fail::Unauthorized => 0,
        }
    }
}

type FailWhen = Box<dyn FnMut(&Request) -> Option<Fail>>;

pub struct State {
    pub folders: Vec<Folder>,
    pub feeds: Vec<Feed>,
    pub items: BTreeMap<u64, Item>,
    /// The server's clock: the `lastModified` of changes, and its `Date`.
    pub now: i64,
    /// Whether responses carry a date.
    pub sends_date: bool,
    pub requests: Vec<Request>,
    /// Items handed over by the current or last item request.
    pub delivered: usize,
    fail: Option<FailWhen>,
}

pub struct FakeServer(RefCell<State>);

impl FakeServer {
    pub fn new(now: i64) -> Self {
        Self(RefCell::new(State {
            folders: Vec::new(),
            feeds: Vec::new(),
            items: BTreeMap::new(),
            now,
            sends_date: true,
            requests: Vec::new(),
            delivered: 0,
            fail: None,
        }))
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.0.borrow_mut())
    }

    pub fn add_folder(&self, id: u64) {
        self.with(|s| {
            s.folders.push(Folder {
                id,
                name: format!("Folder {id}"),
            });
        });
    }

    pub fn add_feed(&self, id: u64, folder_id: Option<u64>) {
        self.with(|s| s.feeds.push(feed(id, folder_id)));
    }

    /// Adds or replaces an item, changed now.
    pub fn put(&self, item: Item) {
        self.with(|s| {
            let item = Item {
                last_modified: Some(s.now),
                ..item
            };
            s.items.insert(item.id, item);
        });
    }

    /// Changes an item now.
    pub fn change(&self, id: u64, f: impl FnOnce(&mut Item)) {
        self.with(|s| {
            let item = s.items.get_mut(&id).expect("no such item");
            f(item);
            item.last_modified = Some(s.now);
        });
    }

    pub fn item(&self, id: u64) -> Item {
        self.with(|s| s.items[&id].clone())
    }

    pub fn advance(&self, seconds: i64) {
        self.with(|s| s.now += seconds);
    }

    /// Fails the requests for which `when` returns a failure.
    pub fn fail_when(&self, when: impl FnMut(&Request) -> Option<Fail> + 'static) {
        self.with(|s| s.fail = Some(Box::new(when)));
    }

    pub fn stop_failing(&self) {
        self.with(|s| s.fail = None);
    }

    /// Takes the requests received so far.
    pub fn requests(&self) -> Vec<Request> {
        self.with(|s| std::mem::take(&mut s.requests))
    }

    pub fn delivered(&self) -> usize {
        self.with(|s| s.delivered)
    }

    /// Logs a request and decides whether it fails.
    fn receive(&self, request: Request) -> Option<Fail> {
        self.with(|s| {
            let fail = s.fail.as_mut().and_then(|when| when(&request));
            s.requests.push(request);
            fail
        })
    }

    /// Hands `items` to `f` one by one, as the client does while a
    /// response is received, or fails after some of them.
    fn deliver<E: From<Error>>(
        &self,
        items: Vec<Item>,
        fail: Option<Fail>,
        mut f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<usize, E> {
        self.with(|s| s.delivered = 0);
        let count = items.len();
        for (n, item) in items.into_iter().enumerate() {
            if fail.is_some_and(|fail| fail.after() == n) {
                break;
            }
            // Not borrowed while the sync works with the item.
            self.with(|s| s.delivered += 1);
            f(item)?;
        }
        match fail {
            Some(fail) => Err(fail.error().into()),
            None => Ok(count),
        }
    }

    fn folder_of(&self, feed_id: u64) -> Option<u64> {
        self.with(|s| {
            s.feeds
                .iter()
                .find(|feed| feed.id == feed_id)
                .and_then(|feed| feed.folder_id)
        })
    }
}

pub fn feed(id: u64, folder_id: Option<u64>) -> Feed {
    Feed {
        id,
        url: format!("https://example.org/{id}/feed"),
        title: Some(format!("Feed {id}")),
        favicon_link: None,
        added: Some(1_700_000_000),
        folder_id,
        unread_count: 0,
        ordering: 0,
        link: None,
        pinned: false,
        update_error_count: 0,
        last_update_error: None,
    }
}

/// An unread, unstarred item.
pub fn item(id: u64, feed_id: u64) -> Item {
    Item {
        id,
        guid: Some(format!("guid-{id}")),
        guid_hash: None,
        url: Some(format!("https://example.org/{feed_id}/{id}")),
        title: Some(format!("Item {id}")),
        author: None,
        pub_date: Some(1_700_000_000 + id.cast_signed()),
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
        last_modified: None,
        fingerprint: None,
        content_hash: None,
    }
}

impl NewsApi for FakeServer {
    fn folders(&self) -> Result<Vec<Folder>, Error> {
        if let Some(fail) = self.receive(Request::Folders) {
            return Err(fail.error());
        }
        Ok(self.with(|s| s.folders.clone()))
    }

    fn feeds(&self) -> Result<Feeds, Error> {
        if let Some(fail) = self.receive(Request::Feeds) {
            return Err(fail.error());
        }
        Ok(self.with(|s| {
            let unread = |id| {
                s.items
                    .values()
                    .filter(|item| item.feed_id == id && item.unread)
                    .count() as u64
            };
            Feeds {
                feeds: s
                    .feeds
                    .iter()
                    .map(|feed| Feed {
                        unread_count: unread(feed.id),
                        ..feed.clone()
                    })
                    .collect(),
                starred_count: s.items.values().filter(|item| item.starred).count() as u64,
                newest_item_id: s.items.keys().next_back().copied(),
            }
        }))
    }

    fn items<E: From<Error>>(
        &self,
        query: ItemQuery,
        f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<usize, E> {
        let fail = self.receive(Request::Items(query));
        if fail == Some(Fail::Unauthorized) {
            return Err(Error::Unauthorized.into());
        }
        let all: Vec<Item> = self.with(|s| s.items.values().rev().cloned().collect());
        let items: Vec<Item> = all
            .into_iter()
            .filter(|item| match query.selection {
                Selection::All => true,
                Selection::Starred => item.starred,
                Selection::Feed(id) => item.feed_id == id,
                Selection::Folder(id) => self.folder_of(item.feed_id) == Some(id),
            })
            .filter(|item| query.get_read || item.unread)
            .filter(|item| query.offset == 0 || item.id < query.offset)
            .take(query.batch_size.map_or(usize::MAX, |size| size as usize))
            .collect();
        self.deliver(items, fail, f)
    }

    fn updated_items<E: From<Error>>(
        &self,
        last_modified: i64,
        f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<usize, E> {
        let fail = self.receive(Request::Updated(last_modified));
        if fail == Some(Fail::Unauthorized) {
            return Err(Error::Unauthorized.into());
        }
        let items = self.with(|s| {
            s.items
                .values()
                .filter(|item| item.last_modified.is_some_and(|lm| lm >= last_modified))
                .cloned()
                .collect()
        });
        self.deliver(items, fail, f)
    }

    fn update(&self, update: &Update<'_>) -> Result<(), Error> {
        let request = match *update {
            Update::Items { action, ids } => Request::Write(action, ids.to_vec()),
            Update::MarkRead {
                scope,
                newest_item_id,
            } => Request::MarkRead(scope, newest_item_id),
        };
        if let Some(fail) = self.receive(request) {
            return Err(fail.error());
        }
        self.with(|s| {
            let now = s.now;
            match *update {
                Update::Items { action, ids } => {
                    for id in ids {
                        // Unknown ids are skipped.
                        if let Some(item) = s.items.get_mut(id) {
                            match action {
                                ItemAction::Read => item.unread = false,
                                ItemAction::Unread => item.unread = true,
                                ItemAction::Star => item.starred = true,
                                ItemAction::Unstar => item.starred = false,
                            }
                            item.last_modified = Some(now);
                        }
                    }
                    Ok(())
                }
                Update::MarkRead {
                    scope,
                    newest_item_id,
                } => {
                    let feeds: Vec<u64> = match scope {
                        ReadScope::All => s.feeds.iter().map(|feed| feed.id).collect(),
                        // News 28.7 fails on folder 0.
                        ReadScope::Folder(0) => {
                            return Err(Error::Status {
                                code: 500,
                                message: None,
                            });
                        }
                        ReadScope::Folder(id) if s.folders.iter().any(|f| f.id == id) => s
                            .feeds
                            .iter()
                            .filter(|feed| feed.folder_id == Some(id))
                            .map(|feed| feed.id)
                            .collect(),
                        ReadScope::Feed(id) if s.feeds.iter().any(|f| f.id == id) => vec![id],
                        _ => {
                            return Err(Error::Status {
                                code: 404,
                                message: Some("not found".into()),
                            });
                        }
                    };
                    for item in s.items.values_mut() {
                        if item.id <= newest_item_id && item.unread && feeds.contains(&item.feed_id)
                        {
                            item.unread = false;
                            item.last_modified = Some(now);
                        }
                    }
                    Ok(())
                }
            }
        })
    }

    fn server_time(&self) -> Option<i64> {
        self.with(|s| s.sends_date.then_some(s.now))
    }
}
