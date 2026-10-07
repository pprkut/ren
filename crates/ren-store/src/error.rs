// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The database was written by a newer version of ren, with a schema
    /// this version doesn't know.
    TooNew { version: u32, supported: u32 },
    /// An error from SQLite: the file can't be opened or isn't a database,
    /// the disk is full, another connection kept it locked too long, …
    Sqlite(rusqlite::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooNew { version, supported } => write!(
                f,
                "the database has schema version {version}, this version of ren only knows \
                 up to {supported}"
            ),
            Error::Sqlite(err) => write!(f, "database: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Sqlite(err) => Some(err),
            Error::TooNew { .. } => None,
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(err: rusqlite::Error) -> Self {
        Error::Sqlite(err)
    }
}
