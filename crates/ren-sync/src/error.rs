// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// A request failed.
    Api(nextcloud_news::Error),
    /// The local database failed.
    Store(ren_store::Error),
    /// The caller asked to stop.
    Cancelled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Api(err) => write!(f, "server: {err}"),
            Error::Store(err) => write!(f, "{err}"),
            Error::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Api(err) => Some(err),
            Error::Store(err) => Some(err),
            Error::Cancelled => None,
        }
    }
}

impl From<nextcloud_news::Error> for Error {
    fn from(err: nextcloud_news::Error) -> Self {
        Error::Api(err)
    }
}

impl From<ren_store::Error> for Error {
    fn from(err: ren_store::Error) -> Self {
        Error::Store(err)
    }
}
