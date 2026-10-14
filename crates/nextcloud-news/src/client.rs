// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use ureq::http::Response;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::{Agent, Body};

use crate::decode::{decode, decode_items};
use crate::error::Error;
use crate::request::{Edit, Endpoint, ItemQuery, Method, PageInfo, Pager, Selection, Update};
use crate::types::{Feed, Feeds, Folder, Folders, Item, Status, Version};

/// How much of an error response is read for the News app's message.
const ERROR_BODY_LIMIT: u64 = 64 * 1024;

/// User name and (app) password for HTTP basic auth.
#[derive(Clone)]
pub struct Credentials {
    pub user: String,
    pub password: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("user", &self.user)
            .field("password", &"…")
            .finish()
    }
}

/// HTTP settings of a [`Client`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub user_agent: String,
    /// For connecting, including the TLS handshake.
    pub connect_timeout: Duration,
    /// From sending a request until the response headers arrive. The News
    /// app builds an item list before it sends anything; S2 saw 1.5–2.2 s
    /// per page of 200–2000 items.
    pub response_timeout: Duration,
    /// For receiving a whole response body.
    pub body_timeout: Duration,
    /// Upper bound for a response body. `GET /items/updated` is not paged,
    /// so this has to be generous; item lists are decoded while they are
    /// received, so a large response doesn't cost memory.
    pub body_limit: u64,
}

impl Config {
    pub fn new(user_agent: impl Into<String>) -> Self {
        Self {
            user_agent: user_agent.into(),
            connect_timeout: Duration::from_secs(15),
            response_timeout: Duration::from_secs(60),
            body_timeout: Duration::from_secs(300),
            body_limit: 512 * 1024 * 1024,
        }
    }
}

/// The API base URL for a server URL, which may or may not end in a slash.
pub fn api_base(server: &str) -> String {
    format!(
        "{}/index.php/apps/news/api/v1-3/",
        server.trim_end_matches('/')
    )
}

/// An HTTP agent with the TLS setup and the timeouts of `config`. Server
/// certificates are checked against the system's trust store.
pub(crate) fn agent(config: &Config) -> Agent {
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    Agent::config_builder()
        .tls_config(tls)
        .user_agent(&config.user_agent)
        .timeout_connect(Some(config.connect_timeout))
        .timeout_recv_response(Some(config.response_timeout))
        .timeout_recv_body(Some(config.body_timeout))
        // Unsuccessful responses are handled here, to keep the
        // server's message.
        .http_status_as_error(false)
        .build()
        .new_agent()
}

/// The `Authorization` header for HTTP basic auth.
pub(crate) fn basic_auth(credentials: &Credentials) -> String {
    let token = STANDARD.encode(format!("{}:{}", credentials.user, credentials.password));
    format!("Basic {token}")
}

/// The error body of the News app.
#[derive(Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

/// What the server answered to an [`Edit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edited {
    /// The feed added.
    Feed(Feed),
    /// The folder added.
    Folder(Folder),
    /// Renamed, moved or deleted.
    Done,
}

/// The answer to `POST /folders`.
#[derive(Deserialize)]
struct NewFolders {
    folders: Vec<Folder>,
}

/// [`Client::server_time`] before a response with a date.
const NO_TIME: i64 = i64::MIN;

pub struct Client {
    agent: Agent,
    base: String,
    authorization: String,
    body_limit: u64,
    /// The `Date` of the latest response, unix seconds, or [`NO_TIME`].
    server_time: AtomicI64,
}

impl Client {
    /// `server` is the Nextcloud URL, e.g. `https://cloud.example.org`.
    /// Server certificates are checked against the system's trust store.
    pub fn new(server: &str, credentials: &Credentials, config: &Config) -> Self {
        let agent = agent(config);
        Self {
            agent,
            base: api_base(server),
            authorization: basic_auth(credentials),
            body_limit: config.body_limit,
            server_time: AtomicI64::new(NO_TIME),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// The server's clock at its latest response, in unix seconds, from the
    /// `Date` header. `None` before the first response with a valid date.
    ///
    /// The sync needs the server's "now" for its cursor: item timestamps
    /// are the server's, and the local clock may be off.
    pub fn server_time(&self) -> Option<i64> {
        Some(self.server_time.load(Ordering::Relaxed)).filter(|&t| t != NO_TIME)
    }

    /// Notes the response's date and turns an unsuccessful response into
    /// an error.
    fn check(&self, response: Response<Body>) -> Result<Response<Body>, Error> {
        if let Some(time) = response_date(&response) {
            self.server_time.store(time, Ordering::Relaxed);
        }
        check(response)
    }

    /// Sends a `GET` request and returns the raw response body.
    pub fn get_reader(&self, endpoint: &Endpoint) -> Result<impl Read + use<>, Error> {
        let mut request = self
            .agent
            .get(format!("{}{}", self.base, endpoint.path()))
            .header("Authorization", &self.authorization)
            .header("Accept", "application/json");
        for (name, value) in endpoint.query() {
            request = request.query(name, value);
        }
        let response = self.check(request.call()?)?;
        Ok(response
            .into_body()
            .into_with_config()
            .limit(self.body_limit)
            .reader())
    }

    /// Sends a `GET` request and decodes the response while it is being
    /// received.
    pub fn get<T: DeserializeOwned>(&self, endpoint: &Endpoint) -> Result<T, Error> {
        decode(self.get_reader(endpoint)?)
    }

    pub fn version(&self) -> Result<String, Error> {
        Ok(self.get::<Version>(&Endpoint::Version)?.version)
    }

    pub fn status(&self) -> Result<Status, Error> {
        self.get(&Endpoint::Status)
    }

    pub fn folders(&self) -> Result<Vec<Folder>, Error> {
        Ok(self.get::<Folders>(&Endpoint::Folders)?.folders)
    }

    pub fn feeds(&self) -> Result<Feeds, Error> {
        self.get(&Endpoint::Feeds)
    }

    /// Decodes an item list while it is received and calls `f` with each
    /// item. Returns the number of items. An error from `f` stops the
    /// request and is returned as it is.
    pub fn each_item<E: From<Error>>(
        &self,
        endpoint: &Endpoint,
        f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<usize, E> {
        decode_items(self.get_reader(endpoint)?, f)
    }

    /// One page of items, handed to `f` one by one; see [`Pager`] for
    /// paging, or [`next_page`](Self::next_page).
    pub fn items<E: From<Error>>(
        &self,
        query: ItemQuery,
        f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<usize, E> {
        self.each_item(&Endpoint::Items(query), f)
    }

    /// Fetches the next page of `pager` and calls `f` with each item that
    /// wasn't on an earlier page. `None` when all pages were fetched.
    pub fn next_page<E: From<Error>>(
        &self,
        pager: &mut Pager,
        mut f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<Option<PageInfo>, E> {
        let Some(query) = pager.start_page() else {
            return Ok(None);
        };
        self.items(
            query,
            |item| {
                if pager.accept(&item) { f(item) } else { Ok(()) }
            },
        )?;
        Ok(Some(pager.finish_page()))
    }

    /// Items changed since `last_modified` (unix seconds, inclusive),
    /// handed to `f` one by one. Not paged: the response can be large.
    pub fn updated_items<E: From<Error>>(
        &self,
        last_modified: i64,
        selection: Selection,
        f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<usize, E> {
        let endpoint = Endpoint::UpdatedItems {
            last_modified,
            selection,
        };
        self.each_item(&endpoint, f)
    }

    /// Sends a write request. An empty list of item ids sends nothing.
    pub fn update(&self, update: &Update) -> Result<(), Error> {
        if let Update::Items { ids: [], .. } = update {
            return Ok(());
        }
        let response = self
            .agent
            .post(format!("{}{}", self.base, update.path()))
            .header("Authorization", &self.authorization)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send(&update.body()[..])?;
        let response = self.check(response)?;
        // The body is empty or `[]`. Reading it lets the connection be
        // reused; the change is made, whatever happens to the rest.
        let _ = std::io::copy(
            &mut response
                .into_body()
                .into_with_config()
                .limit(ERROR_BODY_LIMIT)
                .reader(),
            &mut std::io::sink(),
        );
        Ok(())
    }
}

impl Client {
    /// Changes the feeds or folders. Fails with [`Error::Status`] and the
    /// server's message if it refuses: 409 for a feed or folder that
    /// exists already, 422 for an invalid folder name, 404 for a feed
    /// that can't be found or read, or one or a folder that is gone.
    pub fn edit(&self, edit: &Edit<'_>) -> Result<Edited, Error> {
        let url = format!("{}{}", self.base, edit.path());
        let response = match (edit.method(), edit.body()) {
            (Method::Delete, _) => self
                .agent
                .delete(url)
                .header("Authorization", &self.authorization)
                .header("Accept", "application/json")
                .call()?,
            (method, body) => {
                let request = match method {
                    Method::Put => self.agent.put(url),
                    _ => self.agent.post(url),
                };
                request
                    .header("Authorization", &self.authorization)
                    .header("Accept", "application/json")
                    .header("Content-Type", "application/json")
                    .send(&body.unwrap_or_default()[..])?
            }
        };
        let response = self.check(response)?;
        let reader = response
            .into_body()
            .into_with_config()
            .limit(self.body_limit)
            .reader();
        match edit {
            Edit::AddFeed { .. } => {
                let feeds: Feeds = decode(reader)?;
                let feed = feeds
                    .feeds
                    .into_iter()
                    .next()
                    .ok_or_else(|| missing("the feed added"))?;
                Ok(Edited::Feed(feed))
            }
            Edit::AddFolder { .. } => {
                let folders: NewFolders = decode(reader)?;
                let folder = folders
                    .folders
                    .into_iter()
                    .next()
                    .ok_or_else(|| missing("the folder added"))?;
                Ok(Edited::Folder(folder))
            }
            _ => {
                // The body is empty or `[]`.
                let _ = std::io::copy(&mut reader.take(ERROR_BODY_LIMIT), &mut std::io::sink());
                Ok(Edited::Done)
            }
        }
    }
}

/// An answer that lacks `what`.
fn missing(what: &str) -> Error {
    Error::Decode(serde::de::Error::custom(format!(
        "the server answered without {what}"
    )))
}

/// The `Date` header of a response, in unix seconds.
fn response_date(response: &Response<Body>) -> Option<i64> {
    let date = response.headers().get("date")?.to_str().ok()?;
    let time = httpdate::parse_http_date(date).ok()?;
    let seconds = time.duration_since(UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(seconds).ok()
}

/// Turns an unsuccessful response into an error.
pub(crate) fn check(response: Response<Body>) -> Result<Response<Body>, Error> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if status.as_u16() == 401 {
        return Err(Error::Unauthorized);
    }
    let message = response
        .into_body()
        .into_with_config()
        .limit(ERROR_BODY_LIMIT)
        .read_to_vec()
        .ok()
        .and_then(|body| serde_json::from_slice::<ErrorBody>(&body).ok())
        .and_then(|body| body.message)
        .map(|message| message.trim().to_owned())
        .filter(|message| !message.is_empty());
    Err(Error::Status {
        code: status.as_u16(),
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url() {
        assert_eq!(
            api_base("https://cloud.example.org"),
            "https://cloud.example.org/index.php/apps/news/api/v1-3/"
        );
        assert_eq!(
            api_base("https://example.org/nextcloud/"),
            "https://example.org/nextcloud/index.php/apps/news/api/v1-3/"
        );
    }

    #[test]
    fn password_not_in_debug_output() {
        let credentials = Credentials {
            user: "user".to_owned(),
            password: "secret".to_owned(),
        };
        assert!(!format!("{credentials:?}").contains("secret"));
    }
}
