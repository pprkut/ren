// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sync against the fake server.

use std::cell::Cell;
use std::ops::ControlFlow;

use nextcloud_news::{ItemAction, ItemQuery, ReadScope, Selection};
use ren_store::{self as store, MarkReadScope, Status, Store};

use crate::fake::{Fail, FakeServer, Request, item};
use crate::{Clock, Error, Kind, Options, Progress, Report, sync};

const NOW: i64 = 1_790_000_000;
const DAY: i64 = 24 * 60 * 60;

struct FakeClock(Cell<i64>);

impl Clock for FakeClock {
    fn now(&self) -> i64 {
        self.0.get()
    }
}

/// A server, an empty store and the local clock, all at [`NOW`].
struct Setup {
    server: FakeServer,
    store: Store,
    clock: FakeClock,
    options: Options,
}

impl Setup {
    /// Folders 10 and 20; feeds 1 and 2 in folder 10, 3 outside of
    /// folders, 4 in folder 20; items 1–20 of feed `id % 4 + 1`, read if
    /// `id` is a multiple of 3, starred if it is a multiple of 5.
    ///
    /// Unread: 1 2 4 5 7 8 10 11 13 14 16 17 19 20; starred: 5 10 15 20.
    /// They were last changed a day ago.
    fn new() -> Self {
        let server = FakeServer::new(NOW - DAY);
        server.add_folder(10);
        server.add_folder(20);
        for (feed, folder) in [(1, Some(10)), (2, Some(10)), (3, None), (4, Some(20))] {
            server.add_feed(feed, folder);
        }
        for id in 1..=20 {
            server.put(nextcloud_news::types::Item {
                unread: id % 3 != 0,
                starred: id % 5 == 0,
                ..item(id, id % 4 + 1)
            });
        }
        server.advance(DAY);
        Self {
            server,
            store: Store::open_in_memory().unwrap(),
            clock: FakeClock(Cell::new(NOW)),
            options: Options {
                batch_size: 4,
                chunk_size: 3,
                push_batch: 2,
                ..Options::default()
            },
        }
    }

    /// After the initial sync, with the requests forgotten.
    fn synced() -> Self {
        let mut setup = Self::new();
        setup.sync().unwrap();
        setup.server.requests();
        setup
    }

    fn sync(&mut self) -> Result<Report, Error> {
        sync(
            &self.server,
            &mut self.store,
            &self.clock,
            &self.options,
            |_| ControlFlow::Continue(()),
        )
    }

    /// Server and local clock move on.
    fn advance(&self, seconds: i64) {
        self.server.advance(seconds);
        self.clock.0.set(self.clock.0.get() + seconds);
    }

    fn local(&self, selection: store::Selection, unread_only: bool) -> Vec<u64> {
        let query = store::ItemQuery {
            selection,
            unread_only,
            search: String::new(),
        };
        let mut ids = self.store.item_ids(&query, store::Sort::default()).unwrap();
        ids.sort_unstable();
        ids
    }

    fn local_unread(&self) -> Vec<u64> {
        self.local(store::Selection::All, true)
    }

    fn local_starred(&self) -> Vec<u64> {
        self.local(store::Selection::Starred, false)
    }

    fn server_ids(&self, f: impl Fn(&nextcloud_news::types::Item) -> bool) -> Vec<u64> {
        self.server
            .with(|s| s.items.values().filter(|i| f(i)).map(|i| i.id).collect())
    }

    fn status(&self, id: u64) -> Status {
        self.store.item(id).unwrap().unwrap().summary.status
    }
}

/// The selection and offset of the item list requests.
fn pages(requests: &[Request]) -> Vec<(Selection, u64)> {
    requests
        .iter()
        .filter_map(|request| match request {
            Request::Items(query) => Some((query.selection, query.offset)),
            _ => None,
        })
        .collect()
}

#[test]
fn initial_sync() {
    let mut setup = Setup::new();
    let mut events = Vec::new();
    let report = sync(
        &setup.server,
        &mut setup.store,
        &setup.clock,
        &setup.options,
        |progress| {
            events.push(progress);
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    assert_eq!(report.kind, Kind::Initial);
    assert!(!report.resumed);
    // 14 unread, 4 starred of which 3 unread.
    assert_eq!((report.items, report.added), (18, 15));
    assert_eq!(report.feeds.added, [1, 2, 3, 4]);
    assert_eq!(setup.local_unread(), setup.server_ids(|i| i.unread));
    assert_eq!(setup.local_starred(), setup.server_ids(|i| i.starred));
    // Read, unstarred items aren't fetched.
    assert_eq!(setup.store.item(3).unwrap(), None);
    assert_eq!(setup.status(1), Status::Unread);
    assert_eq!(setup.store.sync_cursor().unwrap(), Some(NOW - 60));

    let requests = setup.server.requests();
    assert_eq!(requests[..2], [Request::Folders, Request::Feeds]);
    // Unread: 20 19 17 16 | 14 13 11 10 | 8 7 5 4 | 2 1; starred: 20 15 10
    // 5 | (empty).
    assert_eq!(
        pages(&requests),
        [
            (Selection::All, 0),
            (Selection::All, 16),
            (Selection::All, 10),
            (Selection::All, 4),
            (Selection::Starred, 0),
            (Selection::Starred, 5),
        ]
    );
    let Request::Items(query) = requests[2] else {
        panic!("not an item list: {:?}", requests[2]);
    };
    assert_eq!(query, ItemQuery::unread(Some(4)));
    assert_eq!(requests.len(), 8);

    assert_eq!(events[..2], [Progress::Pushed, Progress::Feeds]);
    assert_eq!(
        events.last(),
        Some(&Progress::Items {
            stored: 18,
            expected: Some(18)
        })
    );
}

#[test]
fn items_are_stored_while_they_are_received() {
    let mut setup = Setup::new();
    let server = &setup.server;
    let mut delivered = Vec::new();
    sync(
        server,
        &mut setup.store,
        &setup.clock,
        &setup.options,
        |progress| {
            if let Progress::Items { stored, .. } = progress {
                delivered.push((stored, server.delivered()));
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    // A chunk of 3 is written while the page of 4 is still being received;
    // the rest of the page after it.
    assert_eq!(delivered[..2], [(3, 3), (4, 4)]);
}

#[test]
fn incremental_sync() {
    let mut setup = Setup::synced();
    setup.advance(600);
    let server = &setup.server;
    server.put(item(21, 1));
    server.put(nextcloud_news::types::Item {
        starred: true,
        ..item(22, 3)
    });
    server.change(1, |i| i.unread = false);
    server.change(9, |i| i.starred = true);
    server.change(20, |i| i.title = Some("Changed".into()));

    let report = setup.sync().unwrap();
    assert_eq!(report.kind, Kind::Incremental);
    assert_eq!((report.items, report.added), (5, 3));
    assert_eq!(setup.server.requests()[2..], [Request::Updated(NOW - 60)]);
    assert_eq!(setup.local_unread(), setup.server_ids(|i| i.unread));
    assert_eq!(setup.local_starred(), setup.server_ids(|i| i.starred));
    assert_eq!(setup.status(21), Status::New);
    assert_eq!(setup.status(22), Status::New);
    assert_eq!(setup.status(2), Status::Unread);
    assert_eq!(
        setup.store.item(20).unwrap().unwrap().summary.title,
        "Changed"
    );
    assert_eq!(setup.store.sync_cursor().unwrap(), Some(NOW + 600));

    // Nothing changed: the last second's items come again, and are no
    // longer new.
    setup.advance(600);
    let report = setup.sync().unwrap();
    assert_eq!(setup.server.requests()[2..], [Request::Updated(NOW + 600)]);
    assert_eq!((report.items, report.added), (5, 0));
    assert_eq!(setup.status(21), Status::Unread);
    assert_eq!(setup.store.sync_cursor().unwrap(), Some(NOW + 600));
}

#[test]
fn local_changes_are_pushed_first() {
    let mut setup = Setup::synced();
    let store = &mut setup.store;
    store.set_unread(&[1, 2, 4], false).unwrap();
    store.set_unread(&[15], true).unwrap();
    store.set_starred(&[1, 2, 4, 7, 8], true).unwrap();
    store.set_starred(&[5], false).unwrap();
    // Feed 4: 7 11 15 19; replaces the queued "unread" of 15.
    store.mark_all_read(MarkReadScope::Folder(20), 20).unwrap();

    let report = setup.sync().unwrap();
    assert_eq!((report.pushed, report.marked_read), (9, 1));
    assert!(report.push_errors.is_empty());
    assert_eq!(
        setup.server.requests()[..8],
        [
            Request::MarkRead(ReadScope::Folder(20), 20),
            Request::Write(ItemAction::Read, vec![1, 2]),
            Request::Write(ItemAction::Read, vec![4]),
            Request::Write(ItemAction::Star, vec![1, 2]),
            Request::Write(ItemAction::Star, vec![4, 7]),
            Request::Write(ItemAction::Star, vec![8]),
            Request::Write(ItemAction::Unstar, vec![5]),
            Request::Folders,
        ]
    );
    assert_eq!(
        setup.server_ids(|i| i.unread),
        [5, 8, 10, 13, 14, 16, 17, 20]
    );
    assert_eq!(setup.server_ids(|i| i.starred), [1, 2, 4, 7, 8, 10, 15, 20]);
    assert_eq!(setup.local_unread(), setup.server_ids(|i| i.unread));
    assert_eq!(setup.local_starred(), setup.server_ids(|i| i.starred));
    assert_eq!(setup.store.pending_count().unwrap(), 0);
}

#[test]
fn queued_local_changes_win_over_the_servers_state() {
    let mut setup = Setup::synced();
    setup.store.set_unread(&[1], false).unwrap();
    setup.store.set_starred(&[2], true).unwrap();
    setup.advance(60);
    // Meanwhile on the server, through the web interface.
    setup.server.change(1, |i| {
        i.title = Some("Changed".into());
        i.starred = true;
    });
    setup.server.change(2, |i| i.unread = false);
    setup
        .server
        .fail_when(|request| matches!(request, Request::Write(..)).then_some(Fail::Status(500, 0)));

    // The server refuses the changes: they stay queued, and the server's
    // state doesn't overwrite them.
    let report = setup.sync().unwrap();
    assert_eq!(report.push_errors.len(), 2);
    assert_eq!(report.pushed, 0);
    assert_eq!(setup.store.pending_count().unwrap(), 2);
    let one = setup.store.item(1).unwrap().unwrap().summary;
    assert_eq!(
        (one.title.as_str(), one.status, one.starred),
        ("Changed", Status::Read, true)
    );
    let two = setup.store.item(2).unwrap().unwrap().summary;
    assert_eq!((two.status, two.starred), (Status::Read, true));

    setup.server.stop_failing();
    setup.advance(60);
    let report = setup.sync().unwrap();
    assert_eq!(report.pushed, 2);
    assert_eq!(setup.store.pending_count().unwrap(), 0);
    let one = setup.server.item(1);
    assert_eq!((one.unread, one.starred), (false, true));
    let two = setup.server.item(2);
    assert_eq!((two.unread, two.starred), (false, true));
    assert_eq!(setup.local_unread(), setup.server_ids(|i| i.unread));
    assert_eq!(setup.local_starred(), setup.server_ids(|i| i.starred));
}

#[test]
fn a_refused_mark_all_read_holds_back_later_read_changes() {
    let mut setup = Setup::synced();
    // Feed 1: 4 8 12 16 20.
    setup
        .store
        .mark_all_read(MarkReadScope::Feed(1), 20)
        .unwrap();
    setup.store.set_unread(&[4], true).unwrap();
    setup.store.set_starred(&[1], true).unwrap();
    setup.server.fail_when(|request| {
        matches!(request, Request::MarkRead(..)).then_some(Fail::Status(500, 0))
    });

    let report = setup.sync().unwrap();
    assert_eq!(report.push_errors.len(), 1);
    assert_eq!(
        setup.server.requests()[..3],
        [
            Request::MarkRead(ReadScope::Feed(1), 20),
            Request::Write(ItemAction::Star, vec![1]),
            Request::Folders,
        ]
    );
    assert_eq!(setup.store.pending_count().unwrap(), 2);
    // Still marked read here, although unread on the server.
    assert_eq!(
        setup.local_unread(),
        [1, 2, 4, 5, 7, 10, 11, 13, 14, 17, 19]
    );

    setup.server.stop_failing();
    setup.sync().unwrap();
    assert_eq!(
        setup.server.requests()[..3],
        [
            Request::MarkRead(ReadScope::Feed(1), 20),
            Request::Write(ItemAction::Unread, vec![4]),
            Request::Folders,
        ]
    );
    assert_eq!(setup.store.pending_count().unwrap(), 0);
    assert_eq!(setup.local_unread(), setup.server_ids(|i| i.unread));
    assert_eq!(
        setup.local_unread(),
        [1, 2, 4, 5, 7, 10, 11, 13, 14, 17, 19]
    );
}

#[test]
fn feeds_outside_of_folders_are_marked_read_one_by_one() {
    let mut setup = Setup::new();
    setup.server.add_feed(5, None);
    setup.server.put(item(21, 5));
    setup.sync().unwrap();
    setup.server.requests();
    setup
        .store
        .mark_all_read(MarkReadScope::OutsideFolders, 21)
        .unwrap();
    // Feed 5 is deleted on the server: nothing left to mark there.
    setup.server.with(|s| s.feeds.retain(|feed| feed.id != 5));

    let report = setup.sync().unwrap();
    assert_eq!(report.marked_read, 1);
    assert!(report.push_errors.is_empty());
    assert_eq!(
        setup.server.requests()[..3],
        [
            Request::MarkRead(ReadScope::Feed(3), 21),
            Request::MarkRead(ReadScope::Feed(5), 21),
            Request::Folders,
        ]
    );
    assert_eq!(setup.store.pending_count().unwrap(), 0);
    assert_eq!(report.feeds.removed, [5]);
    // Feed 3: 2 6 10 14 18.
    assert_eq!(
        setup.server_ids(|i| i.unread && i.feed_id == 3),
        [] as [u64; 0]
    );
    assert_eq!(
        setup.local_unread(),
        setup.server_ids(|i| i.unread && i.feed_id != 5)
    );
}

#[test]
fn a_failed_incremental_sync_falls_back_to_a_resync() {
    let mut setup = Setup::synced();
    setup.advance(600);
    setup.server.change(1, |i| i.unread = false);
    // Read: not in the list of unread items either.
    setup.server.change(15, |i| i.starred = false);
    setup.server.put(item(21, 1));
    // E.g. a response too large for the server to build.
    setup.server.fail_when(|request| {
        matches!(request, Request::Updated(_)).then_some(Fail::Status(500, 1))
    });

    let report = setup.sync().unwrap();
    assert_eq!(report.kind, Kind::Resync);
    assert!(matches!(
        report.fallback,
        Some(nextcloud_news::Error::Status { code: 500, .. })
    ));
    // The read and unstarred items aren't listed any more.
    assert_eq!(report.reconciled, 2);
    assert_eq!(setup.local_unread(), setup.server_ids(|i| i.unread));
    assert_eq!(setup.local_starred(), setup.server_ids(|i| i.starred));
    assert_eq!(setup.status(21), Status::New);
    assert_eq!(setup.store.sync_cursor().unwrap(), Some(NOW + 600 - 60));
}

#[test]
fn a_resync_after_a_long_time_offline() {
    let mut setup = Setup::synced();
    setup.advance(31 * DAY);
    setup.server.change(2, |i| i.unread = false);

    let report = setup.sync().unwrap();
    assert_eq!(report.kind, Kind::Resync);
    assert!(report.fallback.is_none());
    let requests = setup.server.requests();
    assert!(!requests.iter().any(|r| matches!(r, Request::Updated(_))));
    assert_eq!(pages(&requests).len(), 6);
    assert_eq!(setup.local_unread(), setup.server_ids(|i| i.unread));
}

#[test]
fn an_interrupted_initial_sync_resumes() {
    let mut setup = Setup::new();
    // The third page of unread items breaks off after two items.
    setup.server.fail_when(|request| match request {
        Request::Items(query) if query.offset == 10 => Some(Fail::Network(2)),
        _ => None,
    });
    let err = setup.sync().unwrap_err();
    assert!(
        matches!(err, Error::Api(nextcloud_news::Error::Network(_))),
        "{err}"
    );
    // The first two pages are stored, the broken one isn't.
    assert_eq!(setup.local_unread(), [10, 11, 13, 14, 16, 17, 19, 20]);
    assert_eq!(setup.store.sync_cursor().unwrap(), None);

    setup.server.stop_failing();
    setup.server.requests();
    setup.advance(600);
    setup.server.change(20, |i| i.unread = false);
    let report = setup.sync().unwrap();
    assert_eq!(report.kind, Kind::Initial);
    assert!(report.resumed);
    assert_eq!(
        pages(&setup.server.requests()),
        [
            (Selection::All, 10),
            (Selection::All, 4),
            (Selection::Starred, 0),
            (Selection::Starred, 5),
        ]
    );
    // From the start of the first try, so that changes since are fetched.
    assert_eq!(setup.store.sync_cursor().unwrap(), Some(NOW - 60));
    setup.sync().unwrap();
    assert!(
        setup
            .server
            .requests()
            .contains(&Request::Updated(NOW - 60))
    );
    assert_eq!(setup.local_unread(), setup.server_ids(|i| i.unread));
    assert_eq!(setup.local_starred(), setup.server_ids(|i| i.starred));
}

#[test]
fn cancelling_stops_after_the_chunk() {
    let mut setup = Setup::new();
    let err = sync(
        &setup.server,
        &mut setup.store,
        &setup.clock,
        &setup.options,
        |progress| match progress {
            Progress::Items { .. } => ControlFlow::Break(()),
            _ => ControlFlow::Continue(()),
        },
    )
    .unwrap_err();
    assert!(matches!(err, Error::Cancelled));
    assert_eq!(setup.local_unread(), [17, 19, 20]);
    assert_eq!(pages(&setup.server.requests()).len(), 1);

    let report = setup.sync().unwrap();
    assert!(report.resumed);
    assert_eq!(setup.local_unread(), setup.server_ids(|i| i.unread));
}

#[test]
fn errors_that_stop_the_sync() {
    // Wrong credentials: no resync.
    let mut setup = Setup::synced();
    setup
        .server
        .fail_when(|request| matches!(request, Request::Updated(_)).then_some(Fail::Unauthorized));
    let err = setup.sync().unwrap_err();
    assert!(
        matches!(err, Error::Api(nextcloud_news::Error::Unauthorized)),
        "{err}"
    );
    assert!(pages(&setup.server.requests()).is_empty());

    // No connection while pushing: nothing else is tried.
    let mut setup = Setup::synced();
    setup.store.set_unread(&[1], false).unwrap();
    setup
        .server
        .fail_when(|request| matches!(request, Request::Write(..)).then_some(Fail::Network(0)));
    assert!(setup.sync().is_err());
    assert_eq!(
        setup.server.requests(),
        [Request::Write(ItemAction::Read, vec![1])]
    );
    assert_eq!(setup.store.pending_count().unwrap(), 1);
}

#[test]
fn purges_items_read_long_ago() {
    let mut setup = Setup::synced();
    setup.advance(10);
    setup.server.change(1, |i| i.unread = false);
    setup.sync().unwrap();
    assert_eq!(setup.status(1), Status::Read);
    setup.advance(2 * DAY);
    setup.store.set_unread(&[2], false).unwrap();
    setup.sync().unwrap();

    // Just over 30 days after item 1 was read, 28 after item 2.
    setup.advance(28 * DAY + 60);
    let report = setup.sync().unwrap();
    assert_eq!(report.kind, Kind::Incremental);
    assert_eq!(report.purged, 1);
    assert_eq!(setup.store.item(1).unwrap(), None);
    assert!(setup.store.item(2).unwrap().is_some());
    // Read, but starred.
    assert!(setup.store.item(15).unwrap().is_some());

    setup.options.purge_after = None;
    setup.advance(31 * DAY);
    assert_eq!(setup.sync().unwrap().purged, 0);
}

#[test]
fn the_local_clock_without_the_servers() {
    let mut setup = Setup::new();
    setup.server.with(|s| s.sends_date = false);
    setup.clock.0.set(NOW - 5);
    setup.sync().unwrap();
    assert_eq!(setup.store.sync_cursor().unwrap(), Some(NOW - 5 - 60));
}
