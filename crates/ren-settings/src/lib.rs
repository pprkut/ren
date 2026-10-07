// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings of ren, kept apart by who writes them:
//!
//! - **Settings** (`$XDG_CONFIG_HOME/ren/settings.toml`), curated by the
//!   user, by hand or through the settings dialog. The file only holds
//!   overrides; the defaults are in the descriptor table
//!   ([`schema::SETTINGS`]), which also drives validation and the dialog.
//!   Changing a setting edits that one value in the text, so comments and
//!   formatting of a hand-edited file stay ([`set`], [`reset`]). [`SettingsFile`]
//!   reloads the file when it changed and keeps the last good settings if
//!   it has errors, which come with line and column.

mod edit;
mod error;
mod file;
pub mod paths;
pub mod schema;
mod settings;

pub use edit::{reset, set};
pub use error::Error;
pub use file::{Reload, SettingsFile, write_atomically};
pub use schema::{Category, Colour, Kind, Setting, Value};
pub use settings::{Account, Loaded, Position, Problem, Problems, Settings, Severity};
