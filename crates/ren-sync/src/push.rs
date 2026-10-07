// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sending the queued local changes.
//!
//! A refused request (an HTTP error status) stays queued and is reported,
//! and the sync goes on, so that one change the server keeps refusing
//! doesn't stop all syncing. Anything else (no connection, wrong
//! credentials) stops the sync: the requests after it would fail too.

use std::ops::ControlFlow;

use nextcloud_news::{ItemAction, ReadScope, Update};
use ren_store::{MarkReadScope, PendingMarkRead};

use crate::{Clock, Error, NewsApi, Progress, Run};

/// What came of a request.
enum Sent {
    Done,
    /// Refused by the server; kept in the queue.
    Refused,
}

impl<A: NewsApi, C: Clock, P: FnMut(Progress) -> ControlFlow<()>> Run<'_, A, C, P> {
    pub(crate) fn push(&mut self) -> Result<(), Error> {
        let mut mark_read_refused = false;
        for pending in self.store.pending_mark_read()? {
            match self.send_mark_read(&pending)? {
                Sent::Done => {
                    self.store.remove_pending_mark_read(pending.id)?;
                    self.report.marked_read += 1;
                }
                Sent::Refused => mark_read_refused = true,
            }
        }
        for action in [
            ItemAction::Read,
            ItemAction::Unread,
            ItemAction::Star,
            ItemAction::Unstar,
        ] {
            // Single changes of the read state are newer than the "mark
            // all as read" before them; sent first, they would be undone
            // by it when it goes through later.
            if mark_read_refused && matches!(action, ItemAction::Read | ItemAction::Unread) {
                continue;
            }
            self.push_items(action)?;
        }
        Ok(())
    }

    /// Sends the queued changes of one kind, a batch at a time.
    fn push_items(&mut self, action: ItemAction) -> Result<(), Error> {
        let limit = self.options.push_batch.max(1);
        loop {
            let ids = self.store.pending(action, limit)?;
            if ids.is_empty() {
                return Ok(());
            }
            let sent = self.send(&Update::Items { action, ids: &ids })?;
            if let Sent::Refused = sent {
                return Ok(());
            }
            self.store.remove_pending(action, &ids)?;
            self.report.pushed += ids.len();
            if ids.len() < limit {
                return Ok(());
            }
        }
    }

    /// Sends a "mark all as read". The feeds outside of folders are marked
    /// one by one: the server fails on folder 0. An unknown feed or folder
    /// (HTTP 404) was deleted on the server, so there is nothing left to
    /// mark.
    fn send_mark_read(&mut self, pending: &PendingMarkRead) -> Result<Sent, Error> {
        let scopes = match pending.scope {
            MarkReadScope::All => vec![ReadScope::All],
            MarkReadScope::Folder(id) => vec![ReadScope::Folder(id)],
            MarkReadScope::Feed(id) => vec![ReadScope::Feed(id)],
            MarkReadScope::OutsideFolders => self
                .store
                .feeds()?
                .into_iter()
                .filter(|feed| feed.folder_id.is_none())
                .map(|feed| ReadScope::Feed(feed.id))
                .collect(),
        };
        let mut result = Sent::Done;
        for scope in scopes {
            let update = Update::MarkRead {
                scope,
                newest_item_id: pending.newest_item_id,
            };
            match self.api.update(&update) {
                Err(nextcloud_news::Error::Status { code: 404, .. }) => {}
                other => {
                    if let Sent::Refused = self.outcome(other)? {
                        result = Sent::Refused;
                    }
                }
            }
        }
        Ok(result)
    }

    fn send(&mut self, update: &Update<'_>) -> Result<Sent, Error> {
        let result = self.api.update(update);
        self.outcome(result)
    }

    fn outcome(&mut self, result: Result<(), nextcloud_news::Error>) -> Result<Sent, Error> {
        match result {
            Ok(()) => Ok(Sent::Done),
            Err(err @ nextcloud_news::Error::Status { .. }) => {
                self.report.push_errors.push(err);
                Ok(Sent::Refused)
            }
            Err(err) => Err(err.into()),
        }
    }
}
