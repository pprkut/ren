// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The server rejected the credentials (HTTP 401).
    Unauthorized,
    /// Any other unsuccessful HTTP status.
    Status(u16),
    /// Connection, TLS, timeout or other transport problems.
    Transport(ureq::Error),
    /// The response wasn't the JSON we expected.
    Decode(serde_json::Error),
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unauthorized => write!(f, "authentication failed (HTTP 401)"),
            Error::Status(404) => write!(
                f,
                "HTTP 404: not found; is the News app installed and the server URL right?"
            ),
            Error::Status(code) => write!(f, "HTTP status {code}"),
            Error::Transport(err) => write!(f, "{err}"),
            Error::Decode(err) => write!(f, "unexpected response: {err}"),
            Error::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Transport(err) => Some(err),
            Error::Decode(err) => Some(err),
            Error::Io(err) => Some(err),
            Error::Unauthorized | Error::Status(_) => None,
        }
    }
}

impl From<ureq::Error> for Error {
    fn from(err: ureq::Error) -> Self {
        match err {
            ureq::Error::StatusCode(401) => Error::Unauthorized,
            ureq::Error::StatusCode(code) => Error::Status(code),
            err => Error::Transport(err),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Decode(err)
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}
