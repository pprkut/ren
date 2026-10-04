// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Blocking client for the Nextcloud News API v1-3.
//!
//! No UI, no storage: [`Client`] turns [`Endpoint`]s into HTTP requests and
//! decodes the responses into the [`types`]. Item lists are decoded while
//! they are received and handed to a callback item by item, so a response
//! is never held in memory as a whole. Paging through items is driven by the caller with a [`Pager`], so
//! it can store each page before fetching the next.

mod client;
mod decode;
mod error;
mod request;
pub mod types;

pub use client::{Client, Config, Credentials, api_base};
pub use decode::{decode, decode_items};
pub use error::Error;
pub use request::{Endpoint, ItemQuery, PageInfo, Pager, Selection};
