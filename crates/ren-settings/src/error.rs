// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::settings::Problems;

#[derive(Debug)]
pub enum Error {
    /// The file can't be read or written.
    Io { path: PathBuf, err: io::Error },
    /// The settings file has errors; it's left alone until they're fixed.
    Problems(Problems),
    /// A value the setting doesn't take, or a file that has something else
    /// where the setting goes.
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, err } => write!(f, "{}: {err}", path.display()),
            Error::Problems(problems) => problems.fmt(f),
            Error::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { err, .. } => Some(err),
            Error::Problems(problems) => Some(problems),
            Error::Invalid(_) => None,
        }
    }
}
