// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sync with the real client, against a local mock server that serves
//! the responses recorded from a real server.

#[path = "../../nextcloud-news/tests/mock/mod.rs"]
mod mock;

use std::fs::File;
use std::ops::ControlFlow;
use std::path::Path;

use nextcloud_news::types::Item;
use nextcloud_news::{Client, Config, Credentials, decode_items};
use ren_store::{ItemQuery, MarkReadScope, Selection, Sort, Store};
use ren_sync::{Clock, Kind, Options, sync};

use mock::{MockServer, Request, Response};

/// The `Date` of the mock server's responses: 2026-10-07 16:44:09 UTC.
const DATE: &str = "Wed, 07 Oct 2026 16:44:09 GMT";
const DATE_SECONDS: i64 = 1_791_391_449;

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> i64 {
        DATE_SECONDS
    }
}

fn fixture_path(name: &str) -> String {
    format!(
        "{}/../nextcloud-news/tests/fixtures/recorded/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixture_path(name)).unwrap()
}

fn fixture_items(name: &str) -> Vec<Item> {
    let mut items = Vec::new();
    let file = File::open(Path::new(&fixture_path(name))).unwrap();
    decode_items(file, |item| {
        items.push(item);
        Ok::<_, nextcloud_news::Error>(())
    })
    .unwrap();
    items
}

/// Answers like the News app: the recorded lists as the first (and only)
/// page, empty bodies for writes.
fn respond(request: &Request) -> Response {
    let body = match (request.method.as_str(), request.api_path()) {
        ("GET", Some("folders")) => fixture("folders"),
        ("GET", Some("feeds")) => fixture("feeds"),
        ("GET", Some("items")) => match (request.param("type"), request.param("offset")) {
            (Some("3"), Some("0")) => fixture("items"),
            (Some("2"), Some("0")) => fixture("starred"),
            _ => br#"{"items": []}"#.to_vec(),
        },
        ("GET", Some("items/updated")) => fixture("updated"),
        ("POST", Some(_)) => Vec::new(),
        _ => return Response::status(404, r#"{"message": "no route"}"#),
    };
    Response {
        date: Some(DATE.to_owned()),
        ..Response::json(body)
    }
}

fn requests(server: &MockServer, from: usize) -> Vec<String> {
    server.requests()[from..]
        .iter()
        .map(|request| {
            let query: Vec<String> = request
                .query
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            let body = String::from_utf8_lossy(&request.body);
            format!(
                "{} {}?{} {body}",
                request.method,
                request.api_path().unwrap_or(&request.path),
                query.join("&")
            )
            .trim_end()
            .to_owned()
        })
        .collect()
}

fn listed(store: &Store, selection: Selection, unread_only: bool) -> Vec<u64> {
    let query = ItemQuery {
        selection,
        unread_only,
        search: String::new(),
    };
    let mut ids = store.item_ids(&query, Sort::default()).unwrap();
    ids.sort_unstable();
    ids
}

#[test]
fn sync_with_the_client() {
    let server = MockServer::start(respond);
    let credentials = Credentials {
        user: "reader".to_owned(),
        password: "app-password".to_owned(),
    };
    let client = Client::new(&server.url(), &credentials, &Config::new("ren-test"));
    let mut store = Store::open_in_memory().unwrap();
    let options = Options::default();
    let run = |store: &mut Store| {
        sync(&client, store, &FixedClock, &options, |_| {
            ControlFlow::Continue(())
        })
        .unwrap()
    };

    // The first sync.
    let report = run(&mut store);
    assert_eq!(report.kind, Kind::Initial);
    let unread = fixture_items("items");
    let starred = fixture_items("starred");
    assert_eq!(report.items, unread.len() + starred.len());
    assert_eq!(report.feeds.added.len(), 49);
    assert_eq!(store.folders().unwrap().len(), 11);
    let ids = |items: &[Item], f: fn(&Item) -> bool| {
        let mut ids: Vec<u64> = items.iter().filter(|i| f(i)).map(|i| i.id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    let all: Vec<Item> = unread.iter().chain(&starred).cloned().collect();
    assert_eq!(
        listed(&store, Selection::All, true),
        ids(&all, |i| i.unread && !i.filtered)
    );
    assert_eq!(
        listed(&store, Selection::Starred, false),
        ids(&all, |i| i.starred && !i.filtered)
    );
    assert_eq!(store.sync_cursor().unwrap(), Some(DATE_SECONDS - 60));
    assert_eq!(
        requests(&server, 0),
        [
            "GET folders?",
            "GET feeds?",
            "GET items?type=3&id=0&getRead=false&batchSize=1000&offset=0",
            "GET items?type=2&id=0&getRead=true&batchSize=1000&offset=0",
        ]
    );

    // Local changes, sent with the next sync, before it fetches the
    // changes since the first.
    let first = &unread[0];
    let other = unread.iter().find(|i| i.feed_id != first.feed_id).unwrap();
    store
        .mark_all_read(MarkReadScope::Feed(first.feed_id), first.id)
        .unwrap();
    store.set_starred(&[other.id], true).unwrap();
    let seen = server.requests().len();
    let report = run(&mut store);
    assert_eq!(report.kind, Kind::Incremental);
    assert_eq!((report.pushed, report.marked_read), (1, 1));
    assert_eq!(report.items, fixture_items("updated").len());
    assert_eq!(
        requests(&server, seen),
        [
            format!(
                "POST feeds/{}/read? {{\"newestItemId\":{}}}",
                first.feed_id, first.id
            ),
            format!("POST items/star/multiple? {{\"itemIds\":[{}]}}", other.id),
            "GET folders?".to_owned(),
            "GET feeds?".to_owned(),
            format!(
                "GET items/updated?lastModified={}&type=3&id=0",
                DATE_SECONDS - 60
            ),
        ]
    );
    assert_eq!(store.pending_count().unwrap(), 0);
}
