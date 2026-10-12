// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Logging in with Nextcloud's Login Flow v2, and revoking the app
//! password it created.
//!
//! The flow: [`LoginFlow::start`] asks the server for a login page and a
//! token; the user logs in on that page in the browser (2FA and SSO
//! included, as the browser handles them) and grants access; meanwhile
//! [`LoginFlow::poll`] asks the server with the token whether that
//! happened, and gets the login name and a new app password when it did.
//! The token is valid for 20 minutes. The app password is listed as a
//! device, named after the user agent, in the user's security settings.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use ureq::Agent;

use crate::client::{Config, Credentials, agent, basic_auth, check};
use crate::decode::decode;
use crate::error::Error;

/// How long the server keeps a login flow's token.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// A login flow waiting for the user to log in in the browser.
pub struct LoginFlow {
    agent: Agent,
    /// The page to open in the browser.
    pub login_url: String,
    endpoint: String,
    token: String,
}

impl fmt::Debug for LoginFlow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginFlow")
            .field("login_url", &self.login_url)
            .field("endpoint", &self.endpoint)
            .field("token", &"…")
            .finish()
    }
}

/// What the server answers when the user granted access.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct Login {
    /// The server's URL as it knows it, e.g. `https://cloud.example.org`.
    pub server: String,
    #[serde(rename = "loginName")]
    pub login_name: String,
    #[serde(rename = "appPassword")]
    pub app_password: String,
}

impl fmt::Debug for Login {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Login")
            .field("server", &self.server)
            .field("login_name", &self.login_name)
            .field("app_password", &"…")
            .finish()
    }
}

#[derive(Deserialize)]
struct Started {
    poll: Poll,
    login: String,
}

#[derive(Deserialize)]
struct Poll {
    token: String,
    endpoint: String,
}

impl LoginFlow {
    /// Starts a login flow on `server`, the Nextcloud URL. The server
    /// names the app password after `config`'s user agent.
    pub fn start(server: &str, config: &Config) -> Result<LoginFlow, Error> {
        let agent = agent(config);
        let url = format!("{}/index.php/login/v2", server.trim_end_matches('/'));
        let response = agent
            .post(url)
            .header("Accept", "application/json")
            .send_empty()?;
        let response = check(response).map_err(|err| match err {
            Error::Status {
                code: 404,
                message: None,
            } => Error::Status {
                code: 404,
                message: Some("no Nextcloud login here; is the server URL right?".to_owned()),
            },
            err => err,
        })?;
        let started: Started = decode(response.into_body().into_reader())?;
        // The token and later the app password go to the endpoint, and
        // the user's password to the login page: not in plain text to
        // another host.
        for url in [&started.login, &started.poll.endpoint] {
            if !safe_url(url, server) {
                return Err(Error::Insecure(url.clone()));
            }
        }
        Ok(LoginFlow {
            agent,
            login_url: started.login,
            endpoint: started.poll.endpoint,
            token: started.poll.token,
        })
    }

    /// Asks whether the user logged in; `None` while the server waits.
    /// After the token expired, the server keeps answering `None`.
    pub fn poll(&self) -> Result<Option<Login>, Error> {
        let response = self
            .agent
            .post(&self.endpoint)
            .header("Accept", "application/json")
            .send_form([("token", self.token.as_str())])?;
        match check(response) {
            Ok(response) => Ok(Some(decode(response.into_body().into_reader())?)),
            Err(Error::Status { code: 404, .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

/// Revokes the app password of `credentials` on `server`: it no longer
/// works and disappears from the user's devices. A password that was
/// revoked already is an error ([`Error::Unauthorized`]).
pub fn revoke_app_password(
    server: &str,
    credentials: &Credentials,
    config: &Config,
) -> Result<(), Error> {
    let url = format!(
        "{}/ocs/v2.php/core/apppassword",
        server.trim_end_matches('/')
    );
    let response = agent(config)
        .delete(url)
        .header("Authorization", &basic_auth(credentials))
        .header("OCS-APIRequest", "true")
        .header("Accept", "application/json")
        .call()?;
    check(response).map(drop)
}

/// Whether `url` is `https://`, or on the same origin as `server` (a
/// server on the local machine, which may use `http://`).
fn safe_url(url: &str, server: &str) -> bool {
    url.starts_with("https://")
        || origin(url).is_some_and(|origin| Some(origin) == origin_of(server))
}

fn origin_of(server: &str) -> Option<&str> {
    origin(server.trim_end_matches('/'))
}

/// `scheme://host[:port]` of a URL.
fn origin(url: &str) -> Option<&str> {
    let start = url.find("://")? + 3;
    let end = url[start..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |i| start + i);
    (end > start).then(|| &url[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_urls() {
        let server = "http://127.0.0.1:8080/nc";
        assert!(safe_url("https://elsewhere.example/login", server));
        assert!(safe_url("http://127.0.0.1:8080/nc/login/v2/poll", server));
        assert!(safe_url("http://127.0.0.1:8080", server));
        assert!(!safe_url("http://127.0.0.1:8081/login", server));
        assert!(!safe_url("http://evil.example/login", server));
        assert!(!safe_url("ftp://127.0.0.1:8080/x", server));
        assert!(!safe_url("javascript:alert(1)", server));
        assert!(!safe_url("http://", "http://"));
    }

    #[test]
    fn secrets_not_in_debug_output() {
        let login = Login {
            server: "https://x".to_owned(),
            login_name: "ada".to_owned(),
            app_password: "secret".to_owned(),
        };
        assert!(!format!("{login:?}").contains("secret"));
    }
}
