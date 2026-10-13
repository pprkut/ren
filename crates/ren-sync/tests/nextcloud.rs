// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! End to end: logging in, syncing and logging out against a real
//! Nextcloud with the News app, whose feeds this test serves itself. Needs
//! a disposable server, so it's ignored by default; run it with `just
//! test-nextcloud`, which starts one in a container.
//!
//! The account must have no feeds but this test's (it deletes them).
//! Environment:
//!
//! - `REN_E2E_SERVER`: the server's URL, e.g. `http://localhost:8089`;
//! - `REN_E2E_USER`, `REN_E2E_PASSWORD`: an admin account and an app
//!   password of it, entered on the login page as a browser would. The
//!   login hands it back, and logging out revokes it: a new one per run;
//! - `REN_E2E_FEED_HOST`: the name under which the server reaches this
//!   machine (default `host.docker.internal`).

#[path = "../../nextcloud-news/tests/mock/mod.rs"]
mod mock;

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use nextcloud_news::types::Item;
use nextcloud_news::{Client, Config, Credentials, Error, ItemQuery, LoginFlow, Pager};
use ren_store::{ItemQuery as ListQuery, MarkReadScope, Selection, Sort, Status, Store};
use ren_sync::{Kind, Options, Report, SystemClock, sync};
use serde_json::{Value, json};

use mock::{MockServer, Response};

/// The path of the test's feeds on the feed server; the account's feeds
/// must all be below it.
const FEED_PATH: &str = "/ren-e2e/";
const FOLDER: &str = "ren e2e";

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set; see the module's comment"))
}

#[test]
#[ignore = "needs a Nextcloud with News: just test-nextcloud"]
fn login_sync_and_log_out() {
    let server = env("REN_E2E_SERVER");
    let server = server.trim_end_matches('/');
    let user = env("REN_E2E_USER");
    let feed_host =
        std::env::var("REN_E2E_FEED_HOST").unwrap_or_else(|_| "host.docker.internal".to_owned());
    let config = Config::new("ren-e2e");

    // Logging in as ren does (Login Flow v2), with the browser's part done
    // by a client of the login pages.
    let flow = LoginFlow::start(server, &config).unwrap();
    assert!(flow.login_url.starts_with(server), "{}", flow.login_url);
    assert!(flow.poll().unwrap().is_none(), "logged in before the grant");
    let password = env("REN_E2E_PASSWORD");
    Browser::new(server).grant(&flow.login_url, &user, &password);
    let login = flow.poll().unwrap().expect("no login after the grant");
    assert_eq!(login.server.trim_end_matches('/'), server);
    assert_eq!(login.login_name, user);
    // Granted with an app password, the flow returns that one (granted
    // after a login with the account's password, a new one).
    assert_eq!(login.app_password, password);
    let credentials = Credentials {
        user: login.login_name.clone(),
        password: login.app_password.clone(),
    };
    let client = Client::new(server, &credentials, &config);
    let api = Api::new(server, &credentials);
    println!("News {}", client.version().unwrap());
    client.status().unwrap();

    // The feeds, served by this test.
    let feeds = FeedServer::start();
    let feed_url = |name: &str| format!("http://{feed_host}:{}{FEED_PATH}{name}", feeds.port());
    feeds.set(
        "a.xml",
        rss("Feed A", &[entry("a-1", "One"), entry("a-2", "Two")]),
    );
    feeds.set(
        "b.xml",
        rss(
            "Feed B",
            &[
                entry("b-1", "  Spaced\n   title "),
                Entry {
                    author: Some("An\n  Author"),
                    body: "<p>Text</p><script>alert(1)</script>",
                    ..entry("b-2", "Scripted")
                },
                entry("b-3", "Three"),
            ],
        ),
    );
    api.clear_account();
    let folder = api.post("folders", &json!({"name": FOLDER}))["folders"][0]["id"]
        .as_u64()
        .unwrap();
    let feed_a = api.add_feed(&feed_url("a.xml"), folder);
    let feed_b = api.add_feed(&feed_url("b.xml"), 0);

    // The first sync, in pages of two items: the server's paging (offset
    // exclusive) and the pager agree.
    let mut store = Store::open_in_memory().unwrap();
    let paged = Options {
        batch_size: 2,
        chunk_size: 2,
        ..Options::default()
    };
    let report = run(&client, &mut store, &paged);
    assert_eq!(report.kind, Kind::Initial);
    assert_eq!(report.items, 5);
    assert_eq!(report.feeds.added.len(), 2);
    let folders = store.folders().unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!((folders[0].id, folders[0].name.as_str()), (folder, FOLDER));
    let stored_feeds = store.feeds().unwrap();
    let a = stored_feeds.iter().find(|f| f.id == feed_a).unwrap();
    let b = stored_feeds.iter().find(|f| f.id == feed_b).unwrap();
    assert_eq!((a.title.as_str(), a.folder_id), ("Feed A", Some(folder)));
    assert_eq!((b.title.as_str(), b.folder_id), ("Feed B", None));
    assert_eq!(titles(&store, Selection::Feed(feed_a)), ["One", "Two"]);
    assert_eq!(
        titles(&store, Selection::Feed(feed_b)),
        ["Scripted", "Spaced title", "Three"]
    );
    assert_eq!(unread_counts(&store), [(feed_a, 2), (feed_b, 3)]);
    // Bodies are sanitised by the server, titles and authors aren't, so
    // the store collapses their whitespace.
    let scripted = item_titled(&store, feed_b, "Scripted");
    let details = store.item(scripted).unwrap().unwrap();
    assert_eq!(details.summary.author, "An Author");
    let body = details.body.unwrap();
    assert!(
        body.contains("<p>Text</p>") && !body.contains("script"),
        "{body}"
    );

    // Local changes, pushed with the next sync: one item read, one
    // starred, the feeds outside of folders marked read (feed by feed).
    let one = item_titled(&store, feed_a, "One");
    let two = item_titled(&store, feed_a, "Two");
    store.set_unread(&[one], false).unwrap();
    store.set_starred(&[two], true).unwrap();
    let newest = ids(&store, Selection::Feed(feed_b))
        .into_iter()
        .max()
        .unwrap();
    store
        .mark_all_read(MarkReadScope::OutsideFolders, newest)
        .unwrap();
    let report = run(&client, &mut store, &Options::default());
    assert_eq!(report.kind, Kind::Incremental);
    assert_eq!((report.pushed, report.marked_read), (2, 1));
    assert!(report.push_errors.is_empty(), "{:?}", report.push_errors);
    assert_eq!(store.pending_count().unwrap(), 0);
    let remote = remote_items(&client);
    assert!(!remote[&one].unread && !remote[&one].starred);
    assert!(remote[&two].unread && remote[&two].starred);
    for id in ids(&store, Selection::Feed(feed_b)) {
        assert!(!remote[&id].unread, "item {id} of feed B is unread");
    }

    // Changes on the server, as another client or the web interface makes
    // them: "One" unread and starred again, a new item in feed A.
    api.post("items/unread/multiple", &json!({"itemIds": [one]}));
    api.post("items/star/multiple", &json!({"itemIds": [one]}));
    feeds.set(
        "a.xml",
        rss(
            "Feed A",
            &[
                entry("a-1", "One"),
                entry("a-2", "Two"),
                entry("a-3", "New"),
            ],
        ),
    );
    api.update_feed(&user, feed_a);
    let report = run(&client, &mut store, &Options::default());
    assert_eq!(report.kind, Kind::Incremental);
    assert_eq!(report.added, 1);
    let new = item_titled(&store, feed_a, "New");
    assert_eq!(summary(&store, new).status, Status::New);
    let one_now = summary(&store, one);
    assert_eq!((one_now.status, one_now.starred), (Status::Unread, true));
    assert_eq!(ids(&store, Selection::Starred), sorted(vec![one, two]));

    // A resync corrects what the changes missed: here "Two", read on the
    // server since.
    api.post("items/read/multiple", &json!({"itemIds": [two]}));
    let resync = Options {
        resync_after: Duration::ZERO,
        ..Options::default()
    };
    let report = run(&client, &mut store, &resync);
    assert_eq!(report.kind, Kind::Resync);
    assert_eq!(summary(&store, two).status, Status::Read);

    // A folder marked read on the server, then a feed removed there.
    api.post(
        &format!("folders/{folder}/read"),
        &json!({"newestItemId": new}),
    );
    api.delete(&format!("feeds/{feed_b}"));
    let report = run(&client, &mut store, &Options::default());
    assert_eq!(report.feeds.removed, [feed_b]);
    assert!(store.feeds().unwrap().iter().all(|f| f.id != feed_b));
    assert!(ids(&store, Selection::Feed(feed_b)).is_empty());
    assert_eq!(unread_counts(&store), []);

    // Logging out revokes the app password.
    api.clear_account();
    nextcloud_news::revoke_app_password(server, &credentials, &config).unwrap();
    assert!(matches!(client.version(), Err(Error::Unauthorized)));
    assert!(matches!(
        nextcloud_news::revoke_app_password(server, &credentials, &config),
        Err(Error::Unauthorized)
    ));
}

fn run(client: &Client, store: &mut Store, options: &Options) -> Report {
    sync(client, store, &SystemClock, options, |_| {
        ControlFlow::Continue(())
    })
    .unwrap()
}

fn sorted(mut ids: Vec<u64>) -> Vec<u64> {
    ids.sort_unstable();
    ids
}

fn ids(store: &Store, selection: Selection) -> Vec<u64> {
    let query = ListQuery {
        selection,
        ..ListQuery::default()
    };
    sorted(store.item_ids(&query, Sort::default()).unwrap())
}

fn summary(store: &Store, id: u64) -> ren_store::ItemSummary {
    store.item_summaries(&[id]).unwrap().remove(0)
}

fn titles(store: &Store, selection: Selection) -> Vec<String> {
    let mut titles: Vec<String> = store
        .item_summaries(&ids(store, selection))
        .unwrap()
        .into_iter()
        .map(|item| item.title)
        .collect();
    titles.sort();
    titles
}

fn item_titled(store: &Store, feed_id: u64, title: &str) -> u64 {
    let ids = ids(store, Selection::Feed(feed_id));
    store
        .item_summaries(&ids)
        .unwrap()
        .into_iter()
        .find(|item| item.title == title)
        .unwrap_or_else(|| panic!("no item {title:?} in feed {feed_id}"))
        .id
}

fn unread_counts(store: &Store) -> Vec<(u64, u64)> {
    let mut counts = store.unread_counts().unwrap();
    counts.retain(|&(_, count)| count > 0);
    counts.sort_unstable();
    counts
}

/// All items on the server, read or not, by id.
fn remote_items(client: &Client) -> HashMap<u64, Item> {
    let mut items = HashMap::new();
    let mut pager = Pager::new(ItemQuery {
        get_read: true,
        ..ItemQuery::unread(Some(100))
    });
    while let Some(query) = pager.start_page() {
        client
            .items(query, |item| {
                if pager.accept(&item) {
                    items.insert(item.id, item);
                }
                Ok::<_, Error>(())
            })
            .unwrap();
        pager.finish_page();
    }
    items
}

/// Requests to the News API that ren doesn't make: setting up feeds, and
/// changes from another client.
struct Api {
    base: String,
    authorization: String,
    agent: ureq::Agent,
}

impl Api {
    fn new(server: &str, credentials: &Credentials) -> Self {
        let pair = format!("{}:{}", credentials.user, credentials.password);
        Self {
            base: nextcloud_news::api_base(server),
            authorization: format!("Basic {}", STANDARD.encode(pair)),
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build()
                .into(),
        }
    }

    fn answer(&self, what: &str, response: ureq::http::Response<ureq::Body>) -> Value {
        let status = response.status();
        let body = response.into_body().read_to_string().unwrap();
        assert!(status.is_success(), "{what}: {status} {body}");
        serde_json::from_str(&body).unwrap_or(Value::Null)
    }

    fn get(&self, path: &str) -> Value {
        let response = self
            .agent
            .get(format!("{}{path}", self.base))
            .header("Authorization", &self.authorization)
            .call()
            .unwrap();
        self.answer(path, response)
    }

    fn post(&self, path: &str, body: &Value) -> Value {
        let response = self
            .agent
            .post(format!("{}{path}", self.base))
            .header("Authorization", &self.authorization)
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .unwrap();
        self.answer(path, response)
    }

    fn delete(&self, path: &str) {
        let response = self
            .agent
            .delete(format!("{}{path}", self.base))
            .header("Authorization", &self.authorization)
            .call()
            .unwrap();
        self.answer(path, response);
    }

    fn add_feed(&self, url: &str, folder_id: u64) -> u64 {
        self.post("feeds", &json!({"url": url, "folderId": folder_id}))["feeds"][0]["id"]
            .as_u64()
            .unwrap()
    }

    /// Fetches the feed now (an admin's request, for updaters).
    fn update_feed(&self, user: &str, feed_id: u64) {
        self.get(&format!("feeds/update?userId={user}&feedId={feed_id}"));
    }

    /// Deletes the test's feeds and folders, and refuses an account with
    /// others.
    fn clear_account(&self) {
        let feeds = self.get("feeds");
        let feeds = feeds["feeds"].as_array().unwrap();
        let folders = self.get("folders");
        let folders = folders["folders"].as_array().unwrap();
        for feed in feeds {
            let url = feed["url"].as_str().unwrap_or_default();
            assert!(
                url.contains(FEED_PATH),
                "the account has feeds of its own ({url}); the test needs one without"
            );
        }
        for folder in folders {
            assert_eq!(folder["name"], FOLDER, "the account has folders of its own");
        }
        for feed in feeds {
            self.delete(&format!("feeds/{}", feed["id"]));
        }
        for folder in folders {
            self.delete(&format!("folders/{}", folder["id"]));
        }
    }
}

/// An entry of a test feed.
#[derive(Clone, Copy)]
struct Entry<'a> {
    guid: &'a str,
    title: &'a str,
    author: Option<&'a str>,
    body: &'a str,
}

fn entry<'a>(guid: &'a str, title: &'a str) -> Entry<'a> {
    Entry {
        guid,
        title,
        author: None,
        body: "<p>Body</p>",
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn rss(title: &str, entries: &[Entry]) -> String {
    let mut items = String::new();
    for entry in entries {
        let author = entry
            .author
            .map(|author| format!("<dc:creator>{}</dc:creator>", escape(author)))
            .unwrap_or_default();
        items.push_str(&format!(
            "<item><title>{}</title><link>https://example.org/{}</link>\
             <guid isPermaLink=\"false\">{}</guid>{author}\
             <description>{}</description></item>\n",
            escape(entry.title),
            entry.guid,
            entry.guid,
            escape(entry.body),
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <rss version=\"2.0\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><channel>\
         <title>{}</title><link>https://example.org/</link><description>Test</description>\n\
         {items}</channel></rss>\n",
        escape(title)
    )
}

/// Serves the test feeds to the server, on all addresses (the server may
/// be in a container).
struct FeedServer {
    server: MockServer,
    feeds: Arc<Mutex<HashMap<String, String>>>,
}

impl FeedServer {
    fn start() -> Self {
        let feeds: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        let served = Arc::clone(&feeds);
        let server = MockServer::start_on("0.0.0.0", move |request| {
            let name = request.path.strip_prefix(FEED_PATH).unwrap_or_default();
            match served.lock().unwrap().get(name) {
                Some(feed) => Response {
                    content_type: "application/rss+xml; charset=utf-8",
                    ..Response::status(200, feed.as_bytes())
                },
                None => Response::html(404, "no such feed"),
            }
        });
        Self { server, feeds }
    }

    fn port(&self) -> u16 {
        self.server.port()
    }

    fn set(&self, name: &str, feed: String) {
        self.feeds.lock().unwrap().insert(name.to_owned(), feed);
    }
}

/// The browser's part of the login flow: the login page's "app password"
/// form, which grants the flow without the account's password.
struct Browser {
    origin: String,
    agent: ureq::Agent,
    cookies: Vec<(String, String)>,
}

impl Browser {
    fn new(server: &str) -> Self {
        Self {
            origin: origin(server).to_owned(),
            agent: ureq::Agent::config_builder()
                .max_redirects(0)
                .http_status_as_error(false)
                .user_agent("ren-e2e browser")
                .build()
                .into(),
            cookies: Vec::new(),
        }
    }

    fn cookie_header(&self) -> String {
        let pairs: Vec<String> = self
            .cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        pairs.join("; ")
    }

    /// Keeps the response's cookies; returns its status, the redirect and
    /// the body.
    fn take(
        &mut self,
        response: ureq::http::Response<ureq::Body>,
    ) -> (u16, Option<String>, String) {
        for header in response.headers().get_all("set-cookie") {
            let pair = header.to_str().unwrap().split(';').next().unwrap();
            if let Some((name, value)) = pair.split_once('=') {
                self.cookies.retain(|(n, _)| n != name.trim());
                self.cookies
                    .push((name.trim().to_owned(), value.trim().to_owned()));
            }
        }
        let location = response
            .headers()
            .get("location")
            .map(|location| location.to_str().unwrap().to_owned());
        let status = response.status().as_u16();
        (
            status,
            location,
            response.into_body().read_to_string().unwrap(),
        )
    }

    fn absolute(&self, url: &str) -> String {
        if url.starts_with('/') {
            format!("{}{url}", self.origin)
        } else {
            url.to_owned()
        }
    }

    /// Opens `url` and follows its redirects; returns the page.
    fn open(&mut self, url: &str) -> String {
        let mut url = self.absolute(url);
        for _ in 0..10 {
            let response = self
                .agent
                .get(&url)
                .header("Cookie", &self.cookie_header())
                .call()
                .unwrap();
            match self.take(response) {
                (300..=399, Some(location), _) => url = self.absolute(&location),
                (200, _, page) => return page,
                (status, _, page) => panic!("{url}: {status} {page}"),
            }
        }
        panic!("{url}: too many redirects");
    }

    fn grant(&mut self, login_url: &str, user: &str, app_password: &str) {
        let page = self.open(login_url);
        let attribute = |marker: &str| {
            let value = &page[page.find(marker)? + marker.len()..];
            Some(value[..value.find('"')?].to_owned())
        };
        let request_token =
            attribute("data-requesttoken=\"").expect("no request token on the login page");
        // Nextcloud 32 has the state token in the "app password" form;
        // 33 draws the page with scripts, from its initial state.
        let state_token = attribute("name=\"stateToken\" value=\"")
            .or_else(|| {
                let state = attribute("id=\"initial-state-core-loginFlowAuth\" value=\"")?;
                let state: Value = serde_json::from_slice(&STANDARD.decode(state).ok()?).ok()?;
                Some(state["stateToken"].as_str()?.to_owned())
            })
            .expect("no state token on the login page");
        let body = [
            ("user", user),
            ("password", app_password),
            ("stateToken", &state_token),
            ("requesttoken", &request_token),
        ]
        .iter()
        .map(|(name, value)| format!("{name}={}", form_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
        let response = self
            .agent
            .post(format!("{}/login/v2/apptoken", self.origin))
            .header("Cookie", &self.cookie_header())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(body)
            .unwrap();
        let (status, _, page) = self.take(response);
        assert_eq!(status, 200, "granting the login: {page}");
    }
}

/// `scheme://host[:port]` of a URL.
fn origin(url: &str) -> &str {
    let rest = url.find("://").map_or(0, |i| i + 3);
    let end = url[rest..].find('/').map_or(url.len(), |i| rest + i);
    &url[..end]
}

fn form_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}
