// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt;
use std::io::{BufReader, Read};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use ureq::http::Response;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::{Agent, Body};

use crate::error::Error;
use crate::request::{Endpoint, ItemQuery, Selection};
use crate::types::{Feeds, Folder, Folders, Item, Items, Status, Version};

/// Upper bound for a response body. `GET /items/updated` is not paged, so
/// this has to be generous.
const BODY_LIMIT: u64 = 512 * 1024 * 1024;

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

/// The API base URL for a server URL, which may or may not end in a slash.
pub fn api_base(server: &str) -> String {
    format!(
        "{}/index.php/apps/news/api/v1-3/",
        server.trim_end_matches('/')
    )
}

/// Decodes a JSON response body.
pub fn decode<T: DeserializeOwned>(reader: impl Read) -> Result<T, Error> {
    serde_json::from_reader(BufReader::new(reader)).map_err(Error::from_decode)
}

/// The error body of the News app.
#[derive(Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

pub struct Client {
    agent: Agent,
    base: String,
    authorization: String,
}

impl Client {
    /// `server` is the Nextcloud URL, e.g. `https://cloud.example.org`.
    /// Server certificates are checked against the system's trust store.
    pub fn new(server: &str, credentials: &Credentials, user_agent: &str) -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let agent = Agent::config_builder()
            .tls_config(tls)
            .user_agent(user_agent)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_recv_body(Some(Duration::from_secs(300)))
            // Unsuccessful responses are handled here, to keep the
            // server's message.
            .http_status_as_error(false)
            .build()
            .new_agent();
        let token = STANDARD.encode(format!("{}:{}", credentials.user, credentials.password));
        Self {
            agent,
            base: api_base(server),
            authorization: format!("Basic {token}"),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// Sends a request and returns the raw response body.
    pub fn get_reader(&self, endpoint: &Endpoint) -> Result<impl Read + use<>, Error> {
        let mut request = self
            .agent
            .get(format!("{}{}", self.base, endpoint.path()))
            .header("Authorization", &self.authorization)
            .header("Accept", "application/json");
        for (name, value) in endpoint.query() {
            request = request.query(name, value);
        }
        let response = check(request.call()?)?;
        Ok(response
            .into_body()
            .into_with_config()
            .limit(BODY_LIMIT)
            .reader())
    }

    /// Sends a request and decodes the response while it is being received.
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

    /// One page of items; see [`Pager`](crate::Pager) for paging.
    pub fn items(&self, query: ItemQuery) -> Result<Vec<Item>, Error> {
        Ok(self.get::<Items>(&Endpoint::Items(query))?.items)
    }

    /// Items changed since `last_modified` (unix seconds, inclusive).
    pub fn updated_items(
        &self,
        last_modified: i64,
        selection: Selection,
    ) -> Result<Vec<Item>, Error> {
        let endpoint = Endpoint::UpdatedItems {
            last_modified,
            selection,
        };
        Ok(self.get::<Items>(&endpoint)?.items)
    }
}

/// Turns an unsuccessful response into an error.
fn check(response: Response<Body>) -> Result<Response<Body>, Error> {
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
