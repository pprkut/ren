// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the sync needs from the server, as a trait: the HTTP client in the
//! app, a fake in the tests.

use nextcloud_news::types::{Feeds, Folder, Item};
use nextcloud_news::{Client, Error, ItemQuery, Selection, Update};

/// The requests the sync makes.
pub trait NewsApi {
    /// `GET /folders`
    fn folders(&self) -> Result<Vec<Folder>, Error>;

    /// `GET /feeds`
    fn feeds(&self) -> Result<Feeds, Error>;

    /// One page of `GET /items`, handed to `f` item by item while it is
    /// received. An error from `f` stops the request and is returned as it
    /// is.
    fn items<E: From<Error>>(
        &self,
        query: ItemQuery,
        f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<usize, E>;

    /// `GET /items/updated` for all items changed since `last_modified`
    /// (unix seconds, inclusive), handed to `f` item by item. Not paged.
    fn updated_items<E: From<Error>>(
        &self,
        last_modified: i64,
        f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<usize, E>;

    /// A write request.
    fn update(&self, update: &Update<'_>) -> Result<(), Error>;

    /// The server's clock at its latest response, unix seconds, if known.
    fn server_time(&self) -> Option<i64>;
}

impl NewsApi for Client {
    fn folders(&self) -> Result<Vec<Folder>, Error> {
        Client::folders(self)
    }

    fn feeds(&self) -> Result<Feeds, Error> {
        Client::feeds(self)
    }

    fn items<E: From<Error>>(
        &self,
        query: ItemQuery,
        f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<usize, E> {
        Client::items(self, query, f)
    }

    fn updated_items<E: From<Error>>(
        &self,
        last_modified: i64,
        f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<usize, E> {
        Client::updated_items(self, last_modified, Selection::All, f)
    }

    fn update(&self, update: &Update<'_>) -> Result<(), Error> {
        Client::update(self, update)
    }

    fn server_time(&self) -> Option<i64> {
        Client::server_time(self)
    }
}
