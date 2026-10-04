// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The server rejected the credentials (HTTP 401): a wrong user name or
    /// app password, or the app password was revoked.
    Unauthorized,
    /// Any other unsuccessful HTTP status, with the message the News app
    /// sent along (`{"message": "..."}`), if any.
    Status { code: u16, message: Option<String> },
    /// The request didn't get through or the response didn't arrive
    /// completely: name resolution, connection, TLS, timeouts, a connection
    /// lost while the body was received, a body above the size limit.
    Network(ureq::Error),
    /// The response wasn't the JSON we expected.
    Decode(serde_json::Error),
}

impl Error {
    /// An error from decoding a response body: transport errors that
    /// happened while the body was read are network errors, not decode
    /// errors.
    pub(crate) fn from_decode(err: serde_json::Error) -> Self {
        if err.is_io() {
            Error::Network(ureq::Error::from(std::io::Error::from(err)))
        } else {
            Error::Decode(err)
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unauthorized => write!(f, "authentication failed (HTTP 401)"),
            Error::Status { code, message } => {
                write!(f, "HTTP status {code}")?;
                match message {
                    Some(message) => write!(f, ": {message}"),
                    None if *code == 404 => write!(
                        f,
                        ": not found; is the News app installed and the server URL right?"
                    ),
                    None => Ok(()),
                }
            }
            Error::Network(err) => write!(f, "{err}"),
            Error::Decode(err) => write!(f, "unexpected response: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Network(err) => Some(err),
            Error::Decode(err) => Some(err),
            Error::Unauthorized | Error::Status { .. } => None,
        }
    }
}

impl From<ureq::Error> for Error {
    fn from(err: ureq::Error) -> Self {
        match err {
            ureq::Error::StatusCode(401) => Error::Unauthorized,
            ureq::Error::StatusCode(code) => Error::Status {
                code,
                message: None,
            },
            err => Error::Network(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display() {
        let status = |code, message: Option<&str>| Error::Status {
            code,
            message: message.map(str::to_owned),
        };
        assert_eq!(
            status(404, Some("Feed not found")).to_string(),
            "HTTP status 404: Feed not found"
        );
        assert!(status(404, None).to_string().contains("News app installed"));
        assert_eq!(status(500, None).to_string(), "HTTP status 500");
        assert_eq!(
            Error::Unauthorized.to_string(),
            "authentication failed (HTTP 401)"
        );
    }

    #[test]
    fn transport_errors_while_decoding_are_network_errors() {
        let broken = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        let err =
            serde_json::from_reader::<_, serde_json::Value>(Broken(Some(broken))).unwrap_err();
        assert!(matches!(Error::from_decode(err), Error::Network(_)));

        // ureq's own errors, wrapped in an I/O error by its body reader.
        let wrapped = ureq::Error::BodyExceedsLimit(10).into_io();
        let err =
            serde_json::from_reader::<_, serde_json::Value>(Broken(Some(wrapped))).unwrap_err();
        assert!(matches!(
            Error::from_decode(err),
            Error::Network(ureq::Error::BodyExceedsLimit(10))
        ));

        let err = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        assert!(matches!(Error::from_decode(err), Error::Decode(_)));
    }

    #[test]
    fn status_codes() {
        assert!(matches!(
            Error::from(ureq::Error::StatusCode(401)),
            Error::Unauthorized
        ));
        assert!(matches!(
            Error::from(ureq::Error::StatusCode(503)),
            Error::Status {
                code: 503,
                message: None
            }
        ));
    }

    /// A reader that fails with the given error.
    struct Broken(Option<std::io::Error>);

    impl std::io::Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(self
                .0
                .take()
                .unwrap_or_else(|| std::io::Error::other("again")))
        }
    }
}
