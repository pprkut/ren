// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sync engine of ren: sends local changes to the server and stores the
//! server's state.
//!
//! [`sync`] runs one sync synchronously; running it on a background thread
//! at a low priority, on a timer or on request, is the caller's job.
//!
//! 1. **Push:** the queued "mark all as read" requests, then the read and
//!    starred changes of single items, in batches of at most
//!    [`Options::push_batch`] ids. Entries leave the queue only after the
//!    server accepted them.
//! 2. **Folders and feeds:** replaced as a whole; the items of removed feeds
//!    go with them.
//! 3. **Items:**
//!    - The first sync pages through all unread items, then all starred
//!      ones ("full sync").
//!    - Later syncs fetch the items changed since the cursor
//!      (`/items/updated`), which is not paged. If that fails, or the last
//!      sync was longer ago than [`Options::resync_after`], they page
//!      through the unread and starred items instead ("resync") and correct
//!      the state of the items that are no longer in these lists.
//!
//!    Items are written in chunks of [`Options::chunk_size`], one
//!    transaction each, while they are received, so neither a page nor a
//!    large `/items/updated` response is held in memory. The server's state
//!    doesn't overwrite local changes that are still queued (`ren-store`).
//!    A full sync records its progress after each page and resumes there
//!    if it was interrupted.
//! 4. **Purge:** read, unstarred items older than [`Options::purge_after`].

mod api;
mod clock;
mod error;
#[cfg(test)]
mod fake;
mod pull;
mod push;
#[cfg(test)]
mod tests;

use std::ops::ControlFlow;
use std::time::Duration;

use nextcloud_news::types::Item;
use ren_store::{FeedChanges, Store};

pub use api::NewsApi;
pub use clock::{Clock, SystemClock};
pub use error::Error;

/// Settings of a sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Items per page of a full sync. 1000 is the trade-off measured in S2
    /// (`docs/decisions/0003-nextcloud-client.md`); the server fails on
    /// all items at once for large accounts.
    pub batch_size: u32,
    /// Items written per transaction while they are received. Bounds the
    /// items held in memory, and how long the UI's writes wait for the
    /// sync's.
    pub chunk_size: usize,
    /// Item ids per write request. The server marks the items one by one,
    /// and some databases behind Nextcloud reject longer lists of values
    /// in one query.
    pub push_batch: usize,
    /// Resync the unread and starred items instead of fetching the changes
    /// if the last sync was this long ago or longer. Zero always resyncs.
    pub resync_after: Duration,
    /// Delete read, unstarred items that haven't changed for this long
    /// (usually: read this long ago). `None` keeps them.
    pub purge_after: Option<Duration>,
}

impl Default for Options {
    fn default() -> Self {
        const DAY: u64 = 24 * 60 * 60;
        Self {
            batch_size: 1000,
            chunk_size: 200,
            push_batch: 1000,
            resync_after: Duration::from_secs(30 * DAY),
            purge_after: Some(Duration::from_secs(30 * DAY)),
        }
    }
}

/// How the items were brought up to date.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Kind {
    /// The changes since the last sync.
    #[default]
    Incremental,
    /// The first sync: all unread and starred items.
    Initial,
    /// All unread and starred items again, instead of the changes: the last
    /// sync was too long ago, or fetching the changes failed
    /// ([`Report::fallback`]).
    Resync,
}

/// What a sync did.
#[derive(Debug, Default)]
pub struct Report {
    pub kind: Kind,
    /// A full sync was resumed where an earlier one stopped.
    pub resumed: bool,
    /// Why fetching the changes failed, if a resync replaced it.
    pub fallback: Option<nextcloud_news::Error>,
    /// Item changes sent (one per item and state).
    pub pushed: usize,
    /// "Mark all as read" requests sent.
    pub marked_read: usize,
    /// Changes the server refused; they stay queued for the next sync.
    pub push_errors: Vec<nextcloud_news::Error>,
    pub feeds: FeedChanges,
    /// Items received and stored.
    pub items: usize,
    /// Items that weren't stored before (marked as new, except in the
    /// first sync).
    pub added: usize,
    /// Items whose read or starred state a resync corrected.
    pub reconciled: usize,
    /// Items deleted by the purge.
    pub purged: usize,
}

/// How far a sync is, for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// The local changes were sent.
    Pushed,
    /// Folders and feeds are stored.
    Feeds,
    /// Another chunk of items is stored: `stored` so far, of about
    /// `expected` (in a full sync, from the server's unread and starred
    /// counts).
    Items {
        stored: usize,
        expected: Option<u64>,
    },
}

/// Runs one sync. `progress` is called as the sync goes on; returning
/// [`ControlFlow::Break`] stops it with [`Error::Cancelled`], with
/// everything stored so far kept.
pub fn sync(
    api: &impl NewsApi,
    store: &mut Store,
    clock: &impl Clock,
    options: &Options,
    progress: impl FnMut(Progress) -> ControlFlow<()>,
) -> Result<Report, Error> {
    let mut run = Run {
        api,
        store,
        clock,
        options,
        progress,
        report: Report::default(),
        chunk: Vec::with_capacity(options.chunk_size),
        mark_new: false,
        expected: None,
    };
    run.push()?;
    run.notify(Progress::Pushed)?;
    let feeds = run.feeds()?;
    run.notify(Progress::Feeds)?;
    run.items(&feeds)?;
    run.purge()?;
    run.store
        .set_sync_values(&[(pull::LAST_SYNC, Some(clock.now()))])?;
    Ok(run.report)
}

/// One sync in progress.
struct Run<'a, A, C, P> {
    api: &'a A,
    store: &'a mut Store,
    clock: &'a C,
    options: &'a Options,
    progress: P,
    report: Report,
    /// Items received and not yet written.
    chunk: Vec<Item>,
    /// Mark items not stored before as new.
    mark_new: bool,
    /// The number of items a full sync expects.
    expected: Option<u64>,
}

impl<A: NewsApi, C: Clock, P: FnMut(Progress) -> ControlFlow<()>> Run<'_, A, C, P> {
    fn notify(&mut self, progress: Progress) -> Result<(), Error> {
        match (self.progress)(progress) {
            ControlFlow::Continue(()) => Ok(()),
            ControlFlow::Break(()) => Err(Error::Cancelled),
        }
    }

    /// Takes a received item, and writes the chunk when it is full.
    fn take(&mut self, item: Item) -> Result<(), Error> {
        self.chunk.push(item);
        if self.chunk.len() >= self.options.chunk_size {
            self.write_chunk()?;
        }
        Ok(())
    }

    /// Writes the items received so far.
    fn write_chunk(&mut self) -> Result<(), Error> {
        if self.chunk.is_empty() {
            return Ok(());
        }
        self.report.added += self.store.upsert_items(&self.chunk, self.mark_new)?;
        self.report.items += self.chunk.len();
        self.chunk.clear();
        self.notify(Progress::Items {
            stored: self.report.items,
            expected: self.expected,
        })
    }

    fn purge(&mut self) -> Result<(), Error> {
        if let Some(age) = self.options.purge_after {
            let age = i64::try_from(age.as_secs()).unwrap_or(i64::MAX);
            let before = self.clock.now().saturating_sub(age);
            self.report.purged = self.store.purge(before)?;
        }
        Ok(())
    }
}
