// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fetching the server's state: folders, feeds and items.
//!
//! ## The cursor
//!
//! `/items/updated` returns the items whose `lastModified` is at or after
//! the cursor. After fetching the changes, the cursor moves to the newest
//! `lastModified` among them: the response is one snapshot, so anything
//! that changes later gets a later time.
//!
//! A full sync takes minutes on a large account, and items change on the
//! server while it pages through them. Its cursor is the server's time
//! when it started (from the `Date` header of the feeds request, else the
//! local clock), so the next sync fetches whatever changed meanwhile. The
//! newest `lastModified` of the listed items would not do: it can be far
//! in the past (after "mark all as read" on the server, say, when no item
//! is unread), and then every following sync would fetch everything
//! changed since then.

use std::ops::ControlFlow;

use nextcloud_news::types::{Feeds, Item};
use nextcloud_news::{ItemQuery, Pager};
use ren_store::{Listed, Store};

use crate::{Clock, Error, Kind, NewsApi, Progress, Run};

/// The local time when the last sync was completed.
pub(crate) const LAST_SYNC: &str = "last_sync";

/// The progress of a full sync, see [`FullSync`].
const FULL_SINCE: &str = "full_sync.since";
const FULL_STARRED: &str = "full_sync.starred";
const FULL_OFFSET: &str = "full_sync.offset";
const FULL_MARK_NEW: &str = "full_sync.mark_new";

/// How far below the server's time the cursor of a full sync starts: an
/// allowance for a `Date` set by a proxy on another machine. Fetching a
/// minute of changes again costs little.
const CURSOR_MARGIN: i64 = 60;

/// A full sync in progress: paging through the unread items, then the
/// starred ones. Stored after each page, so that an interrupted full sync
/// resumes with the page that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FullSync {
    /// The cursor once it is done.
    since: i64,
    /// Paging through the starred items (else the unread ones).
    starred: bool,
    /// The next page has the items below this id; 0 for the first page.
    offset: u64,
    /// Mark items not stored before as new (not in the first sync).
    mark_new: bool,
}

impl FullSync {
    fn load(store: &Store) -> Result<Option<Self>, Error> {
        let Some(since) = store.sync_value(FULL_SINCE)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            since,
            starred: store.sync_value(FULL_STARRED)? == Some(1),
            offset: store
                .sync_value(FULL_OFFSET)?
                .and_then(|offset| u64::try_from(offset).ok())
                .unwrap_or(0),
            mark_new: store.sync_value(FULL_MARK_NEW)? == Some(1),
        }))
    }

    fn save(self, store: &mut Store) -> Result<(), Error> {
        Ok(store.set_sync_values(&[
            (FULL_SINCE, Some(self.since)),
            (FULL_STARRED, Some(self.starred.into())),
            (FULL_OFFSET, Some(self.offset.cast_signed())),
            (FULL_MARK_NEW, Some(self.mark_new.into())),
        ])?)
    }

    /// Stores the cursor and forgets the progress.
    fn finish(self, store: &mut Store) -> Result<(), Error> {
        store.set_sync_cursor(self.since)?;
        Ok(store.set_sync_values(&[
            (FULL_SINCE, None),
            (FULL_STARRED, None),
            (FULL_OFFSET, None),
            (FULL_MARK_NEW, None),
        ])?)
    }
}

impl<A: NewsApi, C: Clock, P: FnMut(Progress) -> ControlFlow<()>> Run<'_, A, C, P> {
    /// Fetches and stores folders and feeds.
    pub(crate) fn feeds(&mut self) -> Result<Feeds, Error> {
        let folders = self.api.folders()?;
        let feeds = self.api.feeds()?;
        self.store.replace_folders(&folders)?;
        self.report.feeds = self.store.replace_feeds(&feeds.feeds)?;
        Ok(feeds)
    }

    /// Brings the items up to date: a full sync the first time or to
    /// resume one, else the changes, with a resync if they fail or the last
    /// sync was too long ago.
    pub(crate) fn items(&mut self, feeds: &Feeds) -> Result<(), Error> {
        if let Some(full) = FullSync::load(self.store)? {
            self.report.kind = if full.mark_new {
                Kind::Resync
            } else {
                Kind::Initial
            };
            self.report.resumed = true;
            return self.full_sync(full, feeds);
        }
        // The feeds request was the latest.
        let now = self.api.server_time().unwrap_or_else(|| self.clock.now());
        let mut full = FullSync {
            since: now.saturating_sub(CURSOR_MARGIN),
            starred: false,
            offset: 0,
            mark_new: false,
        };
        let Some(cursor) = self.store.sync_cursor()? else {
            self.report.kind = Kind::Initial;
            return self.full_sync(full, feeds);
        };

        // Items that arrived in the previous sync are no longer new.
        self.store.clear_new()?;
        self.mark_new = true;
        full.mark_new = true;
        let resync_after = i64::try_from(self.options.resync_after.as_secs()).unwrap_or(i64::MAX);
        let stale = self
            .store
            .sync_value(LAST_SYNC)?
            .is_some_and(|last| self.clock.now().saturating_sub(last) >= resync_after);
        if !stale {
            match self.changes(cursor) {
                Ok(()) => return Ok(()),
                // The same credentials would fail the resync too.
                Err(Error::Api(nextcloud_news::Error::Unauthorized)) => {
                    return Err(Error::Api(nextcloud_news::Error::Unauthorized));
                }
                Err(Error::Api(err)) => self.report.fallback = Some(err),
                Err(err) => return Err(err),
            }
        }
        self.report.kind = Kind::Resync;
        self.full_sync(full, feeds)
    }

    /// Fetches the items changed since `cursor` and advances the cursor.
    fn changes(&mut self, cursor: i64) -> Result<(), Error> {
        let api = self.api;
        let mut newest = cursor;
        let result = api
            .updated_items(cursor, |item: Item| {
                newest = newest.max(item.last_modified.unwrap_or(cursor));
                self.take(item)
            })
            .and_then(|_| self.write_chunk());
        if result.is_err() {
            self.chunk.clear();
        }
        result?;
        self.store.set_sync_cursor(newest)?;
        Ok(())
    }

    /// Pages through the unread items, then the starred ones, from where
    /// `full` stands, correcting the state of the items that aren't listed.
    fn full_sync(&mut self, mut full: FullSync, feeds: &Feeds) -> Result<(), Error> {
        self.mark_new = full.mark_new;
        let unread: u64 = feeds.feeds.iter().map(|feed| feed.unread_count).sum();
        self.expected = Some(unread + feeds.starred_count);
        full.save(self.store)?;

        let api = self.api;
        let batch_size = Some(self.options.batch_size.max(1));
        for listed in [Listed::Unread, Listed::Starred] {
            let starred = listed == Listed::Starred;
            if full.starred && !starred {
                continue;
            }
            let query = if starred {
                ItemQuery::starred(batch_size)
            } else {
                ItemQuery::unread(batch_size)
            };
            let mut pager = Pager::new(ItemQuery {
                offset: full.offset,
                ..query
            });
            let mut next = pager.start_page();
            while let Some(query) = next {
                let mut ids = Vec::new();
                let result = api
                    .items(query, |item: Item| {
                        if !pager.accept(&item) {
                            return Ok(());
                        }
                        ids.push(item.id);
                        self.take(item)
                    })
                    .and_then(|_| self.write_chunk());
                if result.is_err() {
                    self.chunk.clear();
                }
                result?;
                pager.finish_page();
                next = pager.start_page();

                // The page listed everything from its lowest id up to
                // where it started; the last page everything below.
                let low = next.map_or(0, |next| next.offset);
                let high = if query.offset == 0 {
                    u64::MAX
                } else {
                    query.offset
                };
                self.report.reconciled += self.store.reconcile(listed, low..high, &ids)?;
                full = match next {
                    Some(_) => FullSync {
                        offset: low,
                        ..full
                    },
                    None => FullSync {
                        starred: true,
                        offset: 0,
                        ..full
                    },
                };
                full.save(self.store)?;
            }
        }
        full.finish(self.store)
    }
}
