// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The client against a local mock server.

mod mock;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nextcloud_news::{
    Client, Config, Credentials, Error, ItemAction, ItemQuery, Pager, ReadScope, Selection, Update,
};

use mock::{MockServer, Request, Response, closed_port_url};

const API: &str = "/index.php/apps/news/api/v1-3/";

fn credentials() -> Credentials {
    Credentials {
        user: "reader".to_owned(),
        password: "app-password".to_owned(),
    }
}

fn client_for(url: &str, config: &Config) -> Client {
    Client::new(url, &credentials(), config)
}

fn client(server: &MockServer) -> Client {
    client_for(&server.url(), &Config::new("ren-test/1.0"))
}

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/handwritten")
        .join(name);
    std::fs::read(path).unwrap()
}

/// Serves the handwritten fixtures by path.
fn fixture_server() -> MockServer {
    MockServer::start(|request| {
        let name = match request.api_path() {
            Some("version") => "version.json",
            Some("status") => "status.json",
            Some("folders") => "folders.json",
            Some("feeds") => "feeds.json",
            Some("items") => "items.json",
            Some("items/updated") => "updated.json",
            _ => return Response::status(404, r#"{"message": "no route"}"#),
        };
        Response::json(fixture(name))
    })
}

#[test]
fn requests_carry_credentials_and_headers() {
    let server = fixture_server();
    assert_eq!(client(&server).version().unwrap(), "28.7.0");

    let [request] = &server.requests()[..] else {
        panic!("expected one request");
    };
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, format!("{API}version"));
    // "reader:app-password"
    assert_eq!(
        request.header("authorization"),
        Some("Basic cmVhZGVyOmFwcC1wYXNzd29yZA==")
    );
    assert_eq!(request.header("user-agent"), Some("ren-test/1.0"));
    assert_eq!(request.header("accept"), Some("application/json"));
}

#[test]
fn server_in_a_subdirectory() {
    let server = fixture_server();
    let url = format!("{}/nextcloud/", server.url());
    let client = client_for(&url, &Config::new("ren-test"));
    assert!(client.version().is_err());
    assert_eq!(server.requests()[0].path, format!("/nextcloud{API}version"));
}

#[test]
fn read_endpoints() {
    let server = fixture_server();
    let client = client(&server);
    let status = client.status().unwrap();
    assert_eq!(status.version, "28.7.0");
    assert!(!status.warnings.improperly_configured_cron);
    assert_eq!(client.folders().unwrap().len(), 2);
    let feeds = client.feeds().unwrap();
    assert_eq!(feeds.feeds.len(), 3);
    assert_eq!(feeds.newest_item_id, Some(227_491));
}

#[test]
fn updated_items() {
    let server = fixture_server();
    let mut ids = Vec::new();
    let count = client(&server)
        .updated_items(1_767_399_999, Selection::All, |item| {
            ids.push(item.id);
            Ok::<_, Error>(())
        })
        .unwrap();
    assert_eq!(count, 3);
    assert_eq!(ids, [227_491, 226_990, 227_492]);

    let request = &server.requests()[0];
    assert_eq!(request.path, format!("{API}items/updated"));
    assert_eq!(request.param("lastModified"), Some("1767399999"));
    assert_eq!(request.param("type"), Some("3"));
    assert_eq!(request.param("id"), Some("0"));
}

/// A server with `total` unread items (ids `total` down to 1) that pages
/// like the News app: newest first, below the offset (exclusive), or at
/// and below it (`inclusive`, as the documentation reads).
fn paging_server(total: u64, inclusive: bool) -> MockServer {
    MockServer::start(move |request| {
        assert_eq!(request.api_path(), Some("items"));
        assert_eq!(request.param("getRead"), Some("false"));
        let batch: u64 = request.param("batchSize").unwrap().parse().unwrap();
        let offset: u64 = request.param("offset").unwrap().parse().unwrap();
        let top = match (offset, inclusive) {
            (0, _) => total,
            (offset, true) => offset,
            (offset, false) => offset - 1,
        };
        let items: Vec<_> = (1..=top)
            .rev()
            .take(batch as usize)
            .map(|id| format!(r#"{{"id": {id}, "feedId": 1, "unread": true, "lastModified": 5}}"#))
            .collect();
        Response::json(format!(r#"{{"items": [{}]}}"#, items.join(",")))
    })
}

fn fetch_all(server: &MockServer, batch_size: u32) -> (Vec<u64>, usize) {
    let client = client(server);
    let mut pager = Pager::new(ItemQuery::unread(Some(batch_size)));
    let mut ids = Vec::new();
    let mut repeated = 0;
    while let Some(info) = client
        .next_page(&mut pager, |item| {
            ids.push(item.id);
            Ok::<_, Error>(())
        })
        .unwrap()
    {
        repeated += info.repeated;
    }
    (ids, repeated)
}

#[test]
fn paging() {
    let server = paging_server(25, false);
    let (ids, repeated) = fetch_all(&server, 10);
    assert_eq!(ids, (1..=25).rev().collect::<Vec<_>>());
    assert_eq!(repeated, 0);

    let offsets: Vec<_> = server
        .requests()
        .iter()
        .map(|r| r.param("offset").unwrap().to_owned())
        .collect();
    assert_eq!(offsets, ["0", "16", "6"]);
    let request = &server.requests()[0];
    assert_eq!(request.param("type"), Some("3"));
    assert_eq!(request.param("batchSize"), Some("10"));
}

#[test]
fn paging_ends_on_a_full_last_page() {
    let server = paging_server(20, false);
    let (ids, _) = fetch_all(&server, 10);
    assert_eq!(ids.len(), 20);
    // The third page is empty.
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn paging_with_an_inclusive_offset() {
    let server = paging_server(25, true);
    let (ids, repeated) = fetch_all(&server, 10);
    assert_eq!(ids, (1..=25).rev().collect::<Vec<_>>());
    assert_eq!(repeated, 2);
}

#[test]
fn write_endpoints() {
    let server = MockServer::start(|_| Response::json("[]"));
    let client = client(&server);
    let ids = [5, 3, 8];
    let actions = [
        ItemAction::Read,
        ItemAction::Unread,
        ItemAction::Star,
        ItemAction::Unstar,
    ];
    for action in actions {
        client.update(&Update::Items { action, ids: &ids }).unwrap();
    }
    for scope in [ReadScope::All, ReadScope::Feed(39), ReadScope::Folder(0)] {
        client
            .update(&Update::MarkRead {
                scope,
                newest_item_id: 227_491,
            })
            .unwrap();
    }

    let requests = server.requests();
    let summary: Vec<_> = requests
        .iter()
        .map(|r| {
            (
                r.method.as_str(),
                r.api_path().unwrap(),
                String::from_utf8(r.body.clone()).unwrap(),
            )
        })
        .collect();
    let ids = r#"{"itemIds":[5,3,8]}"#.to_owned();
    let newest = r#"{"newestItemId":227491}"#.to_owned();
    assert_eq!(
        summary,
        [
            ("POST", "items/read/multiple", ids.clone()),
            ("POST", "items/unread/multiple", ids.clone()),
            ("POST", "items/star/multiple", ids.clone()),
            ("POST", "items/unstar/multiple", ids),
            ("POST", "items/read", newest.clone()),
            ("POST", "feeds/39/read", newest.clone()),
            ("POST", "folders/0/read", newest),
        ]
    );
    for request in &requests {
        assert_eq!(request.header("content-type"), Some("application/json"));
        assert!(request.header("authorization").is_some());
        assert!(request.query.is_empty());
    }
}

#[test]
fn empty_id_list_sends_nothing() {
    let server = MockServer::start(|_| Response::json("[]"));
    client(&server)
        .update(&Update::Items {
            action: ItemAction::Read,
            ids: &[],
        })
        .unwrap();
    assert!(server.requests().is_empty());
}

#[test]
fn write_responses_without_a_body() {
    // A controller returning nothing: HTTP 200 with an empty body.
    let server = MockServer::start(|_| Response::json(""));
    let update = Update::MarkRead {
        scope: ReadScope::All,
        newest_item_id: 1,
    };
    client(&server).update(&update).unwrap();
}

fn get_error(response: Response) -> Error {
    let server = MockServer::start(move |_| response.clone());
    client(&server).feeds().unwrap_err()
}

#[test]
fn unauthorized() {
    let err = get_error(Response::status(
        401,
        r#"{"message": "Current user is not logged in"}"#,
    ));
    assert!(matches!(err, Error::Unauthorized), "{err:?}");
}

#[test]
fn status_with_message() {
    let err = get_error(Response::status(404, r#"{"message":"Feed not found!"}"#));
    let Error::Status { code, message } = err else {
        panic!("{err:?}");
    };
    assert_eq!(code, 404);
    assert_eq!(message.as_deref(), Some("Feed not found!"));
}

#[test]
fn status_without_message() {
    let err = get_error(Response::html(
        500,
        "<html><body>Internal error</body></html>",
    ));
    assert!(
        matches!(
            err,
            Error::Status {
                code: 500,
                message: None
            }
        ),
        "{err:?}"
    );
    let err = get_error(Response::status(503, ""));
    assert!(matches!(err, Error::Status { code: 503, .. }), "{err:?}");
}

#[test]
fn write_errors() {
    let server = MockServer::start(|_| Response::status(404, r#"{"message":"Folder not found"}"#));
    let err = client(&server)
        .update(&Update::MarkRead {
            scope: ReadScope::Folder(99),
            newest_item_id: 1,
        })
        .unwrap_err();
    assert!(
        matches!(&err, Error::Status { code: 404, message: Some(m) } if m == "Folder not found"),
        "{err:?}"
    );
}

#[test]
fn decode_errors() {
    let err = get_error(Response::json(r#"{"feeds": [{"id": "x"}]}"#));
    assert!(matches!(err, Error::Decode(_)), "{err:?}");
    // A login page instead of the API.
    let err = get_error(Response::html(200, "<html></html>"));
    assert!(matches!(err, Error::Decode(_)), "{err:?}");

    let server = MockServer::start(|_| Response::json(r#"{"message": "not allowed"}"#));
    let err = client(&server)
        .items(ItemQuery::starred(Some(10)), |_| Ok::<_, Error>(()))
        .unwrap_err();
    assert!(matches!(err, Error::Decode(_)), "{err:?}");
    assert!(err.to_string().contains("not allowed"), "{err}");
}

#[test]
fn connection_refused() {
    let client = client_for(&closed_port_url(), &Config::new("ren-test"));
    let err = client.version().unwrap_err();
    assert!(matches!(err, Error::Network(_)), "{err:?}");
}

#[test]
fn response_timeout() {
    let server = MockServer::start(|_| Response {
        delay: Duration::from_secs(3),
        ..Response::json(r#"{"version": "1"}"#)
    });
    let config = Config {
        response_timeout: Duration::from_millis(200),
        ..Config::new("ren-test")
    };
    let err = client_for(&server.url(), &config).version().unwrap_err();
    assert!(
        matches!(err, Error::Network(ureq::Error::Timeout(_))),
        "{err:?}"
    );
}

#[test]
fn connection_lost_during_the_body() {
    let mut body = fixture("items.json");
    body.truncate(body.len() / 2);
    let server = MockServer::start(move |_| Response {
        missing: 1000,
        ..Response::json(body.clone())
    });
    let mut seen = 0;
    let err = client(&server)
        .items(ItemQuery::unread(Some(10)), |_| {
            seen += 1;
            Ok::<_, Error>(())
        })
        .unwrap_err();
    assert!(matches!(err, Error::Network(_)), "{err:?}");
    // The items before the break were handed over.
    assert!(seen >= 1);
}

#[test]
fn body_limit() {
    let server = MockServer::start(|_| Response::json(fixture("items.json")));
    let config = Config {
        body_limit: 1000,
        ..Config::new("ren-test")
    };
    let err = client_for(&server.url(), &config)
        .items(ItemQuery::unread(Some(10)), |_| Ok::<_, Error>(()))
        .unwrap_err();
    assert!(
        matches!(err, Error::Network(ureq::Error::BodyExceedsLimit(_))),
        "{err:?}"
    );
}

#[derive(Debug)]
enum SyncError {
    StoreFull,
    Api(Error),
}

impl From<Error> for SyncError {
    fn from(err: Error) -> Self {
        SyncError::Api(err)
    }
}

#[test]
fn caller_errors_stop_the_request() {
    let server = fixture_server();
    let client = client(&server);
    let calls = Arc::new(AtomicUsize::new(0));
    let result = client.items(ItemQuery::unread(Some(10)), |_| {
        if calls.fetch_add(1, Ordering::Relaxed) == 1 {
            Err(SyncError::StoreFull)
        } else {
            Ok(())
        }
    });
    assert!(matches!(result, Err(SyncError::StoreFull)), "{result:?}");
    assert_eq!(calls.load(Ordering::Relaxed), 2);

    let server = MockServer::start(|_| Response::status(401, ""));
    let result = client_for(&server.url(), &Config::new("ren-test"))
        .items(ItemQuery::unread(Some(10)), |_| Ok::<_, SyncError>(()));
    assert!(matches!(result, Err(SyncError::Api(Error::Unauthorized))));
}

#[test]
fn recorded_requests() {
    // The mock server itself: query strings and bodies are split up.
    let server = fixture_server();
    client(&server)
        .items(
            ItemQuery {
                offset: 7,
                ..ItemQuery::unread(Some(2))
            },
            |_| Ok::<_, Error>(()),
        )
        .unwrap();
    let request: &Request = &server.requests()[0];
    assert_eq!(
        request.query,
        [
            ("type".to_owned(), "3".to_owned()),
            ("id".to_owned(), "0".to_owned()),
            ("getRead".to_owned(), "false".to_owned()),
            ("batchSize".to_owned(), "2".to_owned()),
            ("offset".to_owned(), "7".to_owned()),
        ]
    );
    assert!(request.body.is_empty());
}
