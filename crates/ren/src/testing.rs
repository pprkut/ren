// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the tests of several modules share: temporary directories, the
//! mock server of nextcloud-news's tests with the responses recorded from
//! a real server, and a settings file for it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "../../nextcloud-news/tests/mock/mod.rs"]
pub mod mock;

use mock::{MockServer, Request, Response};

/// The user and app password of the account in [`settings_file`].
pub const USER: &str = "reader";
pub const PASSWORD: &str = "app-password";

/// A new, empty directory, removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ren-test-{name}-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    pub fn write(&self, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A response recorded from the real server (nextcloud-news's fixtures).
pub fn recorded(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../nextcloud-news/tests/fixtures/recorded/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(path).unwrap()
}

/// Answers like the News app with the recorded responses: one page of
/// unread and one of starred items, empty bodies for writes.
pub fn respond_recorded(request: &Request) -> Response {
    let body = match (request.method.as_str(), request.api_path()) {
        ("GET", Some(name @ ("version" | "status" | "folders" | "feeds"))) => recorded(name),
        ("GET", Some("items")) => match (request.param("type"), request.param("offset")) {
            (Some("3"), Some("0")) => recorded("items"),
            (Some("2"), Some("0")) => recorded("starred"),
            _ => br#"{"items": []}"#.to_vec(),
        },
        ("GET", Some("items/updated")) => recorded("updated"),
        // Adding a feed answers with the feeds' first, a folder with a new
        // one.
        ("POST", Some("feeds")) => recorded("feeds"),
        ("POST", Some("folders")) => br#"{"folders": [{"id": 99, "name": "New"}]}"#.to_vec(),
        ("POST" | "PUT" | "DELETE", Some(_)) => Vec::new(),
        _ => return Response::status(404, r#"{"message": "no route"}"#),
    };
    Response {
        date: Some("Wed, 07 Oct 2026 16:44:09 GMT".to_owned()),
        ..Response::json(body)
    }
}

/// A server with the recorded responses.
pub fn recorded_server() -> MockServer {
    MockServer::start(respond_recorded)
}

/// A settings file in `dir` with the account on `server`, whose app
/// password comes from `password-command`.
pub fn settings_file(dir: &TempDir, server: &str) -> PathBuf {
    dir.write(
        "settings.toml",
        format!(
            "[account]\nserver = \"{server}\"\nuser = \"{USER}\"\n\
             password-command = \"echo {PASSWORD}\"\n"
        ),
    )
}
