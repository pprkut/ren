// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The HTTP client for what ren fetches besides the server: the feeds'
//! icons, the images of articles and the filter lists of the tabs. The
//! same TLS setup as nextcloud-news.

use std::sync::Arc;
use std::time::Duration;

use crate::remote::USER_AGENT;

pub fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .new_agent()
}

/// The body of `url`, at most `limit` bytes.
pub fn fetch(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    agent
        .get(url)
        .call()
        .map_err(|err| err.to_string())?
        .into_body()
        .into_with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mock::{MockServer, Response};

    #[test]
    fn fetch_with_a_limit() {
        let server = MockServer::start(|request| match request.path.as_str() {
            "/body" => Response::status(200, "0123456789"),
            _ => Response::status(404, ""),
        });
        let agent = agent();
        let url = format!("{}/body", server.url());
        assert_eq!(fetch(&agent, &url, 11), Ok(b"0123456789".to_vec()));
        // ureq refuses a body of the limit's size already.
        assert!(fetch(&agent, &url, 10).is_err());
        let err = fetch(&agent, &format!("{}/missing", server.url()), 10).unwrap_err();
        assert!(err.contains("404"), "{err}");
        // Sent as ren.
        assert_eq!(server.requests()[0].header("user-agent"), Some(USER_AGENT));
    }
}
