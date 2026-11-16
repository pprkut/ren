// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Blocking client for the Nextcloud News API v1-3.
//!
//! No UI, no storage: [`Client`] turns [`Endpoint`]s (reading) and
//! [`Update`]s (writing) into HTTP requests and decodes the responses into
//! the [`types`]. Item lists are decoded while they are received and handed
//! to a callback item by item, so a response is never held in memory as a
//! whole. Paging through items is driven by the caller with a [`Pager`], so
//! it can store each page before fetching the next.
//!
//! [`LoginFlow`] logs in with Nextcloud's Login Flow v2, which creates an
//! app password; [`revoke_app_password`] removes one again.
//!
//! ## The API
//!
//! The base URL is `<server>/index.php/apps/news/api/v1-3/` ([`api_base`]),
//! which works with and without pretty URLs, with HTTP basic auth. The
//! reference is `docs/development/api/api-v1-3.md` in `nextcloud/news`;
//! where the server does something else, it is what the server does that
//! counts:
//!
//! - **Paging** (`GET /items`) goes down by item id: `offset` is the lowest
//!   id of the previous page, and the next page has the items below it
//!   (exclusive, although the documentation says "lower than or equal").
//!   `batchSize=-1` (all at once) fails on large accounts, so the sync
//!   always pages. 1000 per page is the trade-off measured in
//!   `docs/decisions/0003-nextcloud-client.md`.
//! - **`GET /items/updated`** returns the items with a `lastModified` at
//!   or after the given time, including changes of the read and starred
//!   state, and isn't paged, so its response can be large.
//! - **Times** (`lastModified`) are Unix seconds.
//! - **Writes** are the v1-3 `POST` forms with `itemIds` ([`Update`]); the
//!   `PUT` variants in the documentation's "How To Sync" section are v1-2.
//!   The server marks the items one by one, and some databases behind
//!   Nextcloud reject long lists of values in one query, so callers keep
//!   requests to about 1000 ids.
//! - **"Mark all as read"** of the feeds outside of folders fails
//!   (`/folders/0/read`, HTTP 500): they are marked feed by feed.
//! - **Not sanitised:** titles, authors, URLs, enclosure and media fields.
//!   The `body` is.
//! - **Nullable:** most fields, also where the documentation shows a value
//!   (a new feed's unread count, say); unknown fields are ignored. A feed's
//!   `folderId` of 0 (older News versions) means no folder.
//! - **Deleting a folder** leaves its feeds behind on the server (News
//!   28.7, 29.0), so a caller deletes them first.
//! - **There is no call to change a feed's URL:** a new address is a new
//!   feed, and the old one's articles go with the old one.

mod client;
mod decode;
mod error;
mod login;
mod request;
pub mod types;

pub use client::{Client, Config, Credentials, Edited, api_base};
pub use decode::{decode, decode_items};
pub use error::Error;
pub use login::{LOGIN_TIMEOUT, Login, LoginFlow, revoke_app_password};
pub use request::{
    Edit, Endpoint, ItemAction, ItemQuery, Method, PageInfo, Pager, ReadScope, Selection, Update,
};
