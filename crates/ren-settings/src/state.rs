// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The UI state: pane sizes, column widths, sort order, collapsed folders,
//! the last selection. Only ren writes this file, as a whole; it isn't
//! mixed into the settings file. Every value is optional, so the UI keeps
//! its own defaults for what isn't there.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::file::write_atomically;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct State {
    pub sort: Option<Sort>,
    /// Folders are expanded unless they're listed here, so new ones are
    /// expanded.
    pub collapsed_folders: Vec<u64>,
    pub selection: Option<Selection>,
    /// The layout with the item list beside the article.
    pub beside: Layout,
    /// The layout with the item list above the article.
    pub above: Layout,
}

/// Sizes in logical pixels for one arrangement of the panes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Layout {
    pub tree_width: Option<f32>,
    /// The item list's width beside the article, its height above it.
    pub list_size: Option<f32>,
    /// Widths of the item list's columns; the title takes the rest.
    pub columns: Columns,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Columns {
    pub feed: Option<f32>,
    pub author: Option<f32>,
    pub date: Option<f32>,
}

/// The item list's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Sort {
    pub column: SortColumn,
    pub ascending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortColumn {
    Title,
    Feed,
    Author,
    Date,
}

/// What was selected: a list in the feed tree and an item in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Selection {
    pub list: List,
    pub item: Option<u64>,
}

/// An entry of the feed tree, with the server's ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum List {
    All,
    Starred,
    Folder(u64),
    Feed(u64),
}

const HEADER: &str = "\
# The window's state, written by ren when it changes. Settings are in
# settings.toml.

";

impl State {
    /// Reads the text of a state file.
    pub fn parse(text: &str) -> Result<State, String> {
        toml::from_str(text).map_err(|err| err.message().to_owned())
    }

    /// The text of the state file.
    pub fn to_text(&self) -> String {
        let body = toml::to_string(self).expect("the state is always valid TOML");
        format!("{HEADER}{body}")
    }

    /// Reads the state file. A missing file is the empty state, and so is
    /// one that can't be read, which comes with the error: losing pane
    /// sizes is no reason not to start.
    pub fn load(path: &Path) -> (State, Option<String>) {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return (State::default(), None),
            Err(err) => return (State::default(), Some(format!("{}: {err}", path.display()))),
        };
        match State::parse(&text) {
            Ok(state) => (state, None),
            Err(err) => (State::default(), Some(format!("{}: {err}", path.display()))),
        }
    }

    /// Writes the state file, creating its directory if needed.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_atomically(path, &self.to_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            sort: Some(Sort {
                column: SortColumn::Author,
                ascending: true,
            }),
            collapsed_folders: vec![3, 7],
            selection: Some(Selection {
                list: List::Feed(12),
                item: Some(34_567),
            }),
            beside: Layout {
                tree_width: Some(250.0),
                list_size: Some(420.5),
                columns: Columns {
                    feed: Some(160.0),
                    author: None,
                    date: Some(140.0),
                },
            },
            above: Layout {
                tree_width: Some(200.0),
                ..Layout::default()
            },
        }
    }

    #[test]
    fn round_trip() {
        let state = state();
        let text = state.to_text();
        assert_eq!(State::parse(&text), Ok(state));
        assert_eq!(
            text,
            "\
# The window's state, written by ren when it changes. Settings are in
# settings.toml.

collapsed-folders = [3, 7]

[sort]
column = \"author\"
ascending = true

[selection]
item = 34567

[selection.list]
feed = 12

[beside]
tree-width = 250.0
list-size = 420.5

[beside.columns]
feed = 160.0
date = 140.0

[above]
tree-width = 200.0

[above.columns]
"
        );
    }

    #[test]
    fn empty() {
        assert_eq!(State::parse(""), Ok(State::default()));
        let text = State::default().to_text();
        assert_eq!(State::parse(&text), Ok(State::default()));
    }

    #[test]
    fn unit_lists() {
        let text = "[selection]\nlist = \"starred\"\n";
        let state = State::parse(text).unwrap();
        assert_eq!(
            state.selection,
            Some(Selection {
                list: List::Starred,
                item: None
            })
        );
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let state = State::parse("window-width = 3\n[beside]\nold = 1\ntree-width = 1\n").unwrap();
        assert_eq!(state.beside.tree_width, Some(1.0));
    }

    #[test]
    fn load_and_save() {
        let dir = crate::file::tests::test_dir("state");
        let path = dir.join("ren").join("state.toml");
        assert_eq!(State::load(&path), (State::default(), None));
        state().save(&path).unwrap();
        assert_eq!(State::load(&path), (state(), None));

        fs::write(&path, "sort = 3").unwrap();
        let (loaded, err) = State::load(&path);
        assert_eq!(loaded, State::default());
        let err = err.unwrap();
        assert!(err.starts_with(&path.display().to_string()), "{err}");
    }
}
