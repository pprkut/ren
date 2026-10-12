// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Login Flow v2 and revoking app passwords against a local mock server.

mod mock;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nextcloud_news::{Config, Credentials, Error, LoginFlow, revoke_app_password};

use mock::{MockServer, Response, closed_port_url};

fn config() -> Config {
    Config::new("ren-test/1.0")
}

/// A server whose login flow completes on the `polls`th poll.
fn login_server(polls: usize, endpoint: Option<&'static str>) -> MockServer {
    let count = Arc::new(AtomicUsize::new(0));
    let base = Arc::new(std::sync::OnceLock::<String>::new());
    let base_in = base.clone();
    let server = MockServer::start(move |request| {
        let base = base_in.get().unwrap();
        match (request.method.as_str(), request.path.as_str()) {
            ("POST", "/index.php/login/v2") => Response::json(format!(
                r#"{{"poll": {{"token": "to+ken", "endpoint": "{}"}},
                    "login": "{base}/index.php/login/v2/flow/abc"}}"#,
                endpoint.map_or(format!("{base}/index.php/login/v2/poll"), str::to_owned)
            )),
            ("POST", "/index.php/login/v2/poll") => {
                if count.fetch_add(1, Ordering::SeqCst) + 1 < polls {
                    Response::status(404, "[]")
                } else {
                    Response::json(format!(
                        r#"{{"server": "{base}", "loginName": "ada", "appPassword": "app-pw"}}"#
                    ))
                }
            }
            _ => Response::status(404, ""),
        }
    });
    base.set(server.url()).unwrap();
    server
}

#[test]
fn login_flow() {
    let server = login_server(3, None);
    let flow = LoginFlow::start(&format!("{}/", server.url()), &config()).unwrap();
    assert_eq!(
        flow.login_url,
        format!("{}/index.php/login/v2/flow/abc", server.url())
    );
    assert!(!format!("{flow:?}").contains("to+ken"));
    assert_eq!(flow.poll().unwrap(), None);
    assert_eq!(flow.poll().unwrap(), None);
    let login = flow.poll().unwrap().unwrap();
    assert_eq!(login.server, server.url());
    assert_eq!(login.login_name, "ada");
    assert_eq!(login.app_password, "app-pw");

    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    let start = &requests[0];
    assert_eq!(start.method, "POST");
    assert_eq!(start.header("user-agent"), Some("ren-test/1.0"));
    assert_eq!(start.header("authorization"), None);
    let poll = &requests[1];
    assert_eq!(poll.path, "/index.php/login/v2/poll");
    assert_eq!(
        poll.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(poll.body, b"token=to%2Bken");
}

#[test]
fn login_flow_refuses_plain_text_elsewhere() {
    let server = login_server(1, Some("http://elsewhere.example/poll"));
    let err = LoginFlow::start(&server.url(), &config()).unwrap_err();
    assert!(
        matches!(&err, Error::Insecure(url) if url == "http://elsewhere.example/poll"),
        "{err:?}"
    );
}

#[test]
fn login_flow_errors() {
    // Not a Nextcloud.
    let server = MockServer::start(|_| Response::html(404, "<h1>Not found</h1>"));
    let err = LoginFlow::start(&server.url(), &config()).unwrap_err();
    assert!(err.to_string().contains("no Nextcloud login here"), "{err}");

    let err = LoginFlow::start(&closed_port_url(), &config()).unwrap_err();
    assert!(matches!(err, Error::Network(_)), "{err:?}");

    let server = MockServer::start(|_| Response::json("{}"));
    let err = LoginFlow::start(&server.url(), &config()).unwrap_err();
    assert!(matches!(err, Error::Decode(_)), "{err:?}");
}

#[test]
fn revoking_an_app_password() {
    let server = MockServer::start(|request| {
        if request.header("authorization") == Some("Basic YWRhOmFwcC1wdw==") {
            Response::json(r#"{"ocs": {"meta": {"status": "ok"}, "data": []}}"#)
        } else {
            Response::status(401, "")
        }
    });
    let credentials = |password: &str| Credentials {
        user: "ada".to_owned(),
        password: password.to_owned(),
    };
    revoke_app_password(&server.url(), &credentials("app-pw"), &config()).unwrap();
    let err = revoke_app_password(&server.url(), &credentials("old"), &config()).unwrap_err();
    assert!(matches!(err, Error::Unauthorized), "{err:?}");

    let request = &server.requests()[0];
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path, "/ocs/v2.php/core/apppassword");
    assert_eq!(request.header("ocs-apirequest"), Some("true"));
}
