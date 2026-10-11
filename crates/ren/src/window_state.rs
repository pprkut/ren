// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window's state between runs (`$XDG_STATE_HOME/ren/state.toml`,
//! [`State`]): kept in memory, converted from and to the view model's
//! types, and written when it changed. Without Slint types, so that it is
//! tested without a window.

use std::path::PathBuf;

use ren_settings::{List, Selection, State};
use ren_store::{Sort, SortColumn};

use crate::feed_tree::Node;
use crate::reader::View;

/// The state of this run and the file it goes to.
pub struct Saved {
    /// `None` for demo data, whose state isn't kept.
    path: Option<PathBuf>,
    state: State,
    /// Changed since it was written.
    dirty: bool,
}

impl Saved {
    /// Reads the state file. One that can't be read is the empty state,
    /// with a message: losing pane sizes is no reason not to start.
    pub fn load(path: PathBuf) -> (Self, Option<String>) {
        let (state, problem) = State::load(&path);
        let saved = Self {
            path: Some(path),
            state,
            dirty: false,
        };
        (saved, problem)
    }

    /// The default state, which is never written.
    pub fn unsaved() -> Self {
        Self {
            path: None,
            state: State::default(),
            dirty: false,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Changes the state; returns whether it has to be written.
    pub fn change(&mut self, f: impl FnOnce(&mut State)) -> bool {
        let before = self.state.clone();
        f(&mut self.state);
        if self.state != before {
            self.dirty = true;
        }
        self.dirty && self.path.is_some()
    }

    /// Writes the file if the state changed since it was last written.
    pub fn save(&mut self) -> Result<(), String> {
        let Some(path) = self.path.as_ref().filter(|_| self.dirty) else {
            return Ok(());
        };
        self.state
            .save(path)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        self.dirty = false;
        Ok(())
    }
}

/// The view model's part of the state.
pub fn view(state: &State) -> View {
    View {
        collapsed: state.collapsed_folders.iter().copied().collect(),
        sort: state.sort.map_or_else(Sort::default, |sort| Sort {
            column: match sort.column {
                ren_settings::SortColumn::Title => SortColumn::Title,
                ren_settings::SortColumn::Feed => SortColumn::Feed,
                ren_settings::SortColumn::Author => SortColumn::Author,
                ren_settings::SortColumn::Date => SortColumn::Date,
            },
            ascending: sort.ascending,
        }),
        selected: state
            .selection
            .map_or(Node::All, |selection| match selection.list {
                List::All => Node::All,
                List::Starred => Node::Starred,
                List::Folder(id) => Node::Folder(id),
                List::Feed(id) => Node::Feed(id),
            }),
    }
}

/// Keeps the view model's part and the item shown in the state.
pub fn set_view(state: &mut State, view: &View, item: Option<u64>) {
    let mut collapsed: Vec<u64> = view.collapsed.iter().copied().collect();
    collapsed.sort_unstable();
    state.collapsed_folders = collapsed;
    state.sort = Some(ren_settings::Sort {
        column: match view.sort.column {
            SortColumn::Title => ren_settings::SortColumn::Title,
            SortColumn::Feed => ren_settings::SortColumn::Feed,
            SortColumn::Author => ren_settings::SortColumn::Author,
            SortColumn::Date => ren_settings::SortColumn::Date,
        },
        ascending: view.sort.ascending,
    });
    state.selection = Some(Selection {
        list: match view.selected {
            Node::All => List::All,
            Node::Starred => List::Starred,
            Node::Folder(id) => List::Folder(id),
            Node::Feed(id) => List::Feed(id),
        },
        item,
    });
}

/// A rectangle on the screen, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// The size of the part of `self` inside `other`.
    fn overlap(&self, other: &Rect) -> (u32, u32) {
        let span = |a: i32, a_len: u32, b: i32, b_len: u32| {
            let start = i64::from(a.max(b));
            let end = (i64::from(a) + i64::from(a_len)).min(i64::from(b) + i64::from(b_len));
            u32::try_from((end - start).max(0)).unwrap_or(0)
        };
        (
            span(self.x, self.width, other.x, other.width),
            span(self.y, self.height, other.y, other.height),
        )
    }
}

/// How much of a window must be on a screen to be left where it is:
/// enough to grab it.
const MIN_VISIBLE: (u32, u32) = (100, 50);

/// Where a window restored at `window` goes on the `screens` (the first
/// one is the primary screen): left where it is if enough of it is on a
/// screen, else centred on the primary one, as when the screen it was on
/// is no longer connected. Its size is shrunk to fit the screen.
pub fn on_screen(window: Rect, screens: &[Rect]) -> Rect {
    let area = |(w, h): (u32, u32)| u64::from(w) * u64::from(h);
    let Some(best) = screens
        .iter()
        .max_by_key(|screen| area(window.overlap(screen)))
    else {
        return window;
    };
    let (visible_width, visible_height) = window.overlap(best);
    let visible = visible_width >= MIN_VISIBLE.0 && visible_height >= MIN_VISIBLE.1;
    let screen = if visible { best } else { &screens[0] };
    let width = window.width.min(screen.width);
    let height = window.height.min(screen.height);
    let centred = |start: i32, screen_len: u32, len: u32| {
        start.saturating_add(i32::try_from((screen_len - len) / 2).unwrap_or(0))
    };
    if visible {
        // Left where it is, unless it shrank and no longer fits.
        let place = |pos: i32, len: u32, old_len: u32, start: i32, screen_len: u32| {
            let fits = i64::from(pos) >= i64::from(start)
                && i64::from(pos) + i64::from(len) <= i64::from(start) + i64::from(screen_len);
            if fits || len == old_len {
                pos
            } else {
                centred(start, screen_len, len)
            }
        };
        return Rect {
            x: place(window.x, width, window.width, screen.x, screen.width),
            y: place(window.y, height, window.height, screen.y, screen.height),
            width,
            height,
        };
    }
    Rect {
        x: centred(screen.x, screen.width, width),
        y: centred(screen.y, screen.height, height),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn view_round_trip() {
        let view = View {
            collapsed: HashSet::from([9, 3]),
            sort: Sort {
                column: SortColumn::Author,
                ascending: true,
            },
            selected: Node::Folder(9),
        };
        let mut state = State::default();
        set_view(&mut state, &view, Some(42));
        assert_eq!(state.collapsed_folders, [3, 9]);
        assert_eq!(
            state.selection,
            Some(Selection {
                list: List::Folder(9),
                item: Some(42),
            })
        );
        assert_eq!(super::view(&state), view);

        for node in [Node::All, Node::Starred, Node::Feed(4)] {
            let view = View {
                selected: node,
                ..view.clone()
            };
            set_view(&mut state, &view, None);
            assert_eq!(super::view(&state), view);
        }
    }

    #[test]
    fn empty_state() {
        let view = view(&State::default());
        assert_eq!(view.selected, Node::All);
        assert_eq!(view.sort, Sort::default());
        assert!(view.collapsed.is_empty());
    }

    #[test]
    fn saved_only_with_a_file_and_changes() {
        let dir = std::env::temp_dir().join(format!("ren-window-state-{}", std::process::id()));
        let path = dir.join("state.toml");
        let (mut saved, problem) = Saved::load(path.clone());
        assert_eq!(problem, None);
        assert!(!saved.change(|_| {}));
        saved.save().unwrap();
        assert!(!path.exists());

        assert!(saved.change(|state| state.window.width = Some(800.0)));
        saved.save().unwrap();
        assert_eq!(State::load(&path).0.window.width, Some(800.0));
        // Written; the same value again changes nothing.
        assert!(!saved.change(|state| state.window.width = Some(800.0)));
        std::fs::remove_dir_all(&dir).unwrap();

        let mut unsaved = Saved::unsaved();
        assert!(!unsaved.change(|state| state.window.width = Some(800.0)));
        unsaved.save().unwrap();
        assert_eq!(unsaved.state().window.width, Some(800.0));
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn windows_kept_on_screen() {
        let laptop = rect(0, 0, 1920, 1080);
        let monitor = rect(1920, 0, 2560, 1440);
        let screens = [laptop, monitor];

        // On a screen, or partly: left alone.
        for window in [
            rect(100, 100, 1200, 800),
            rect(2000, 50, 1200, 800),
            rect(1500, 200, 1200, 800),
            rect(-1000, -500, 1200, 800),
        ] {
            assert_eq!(on_screen(window, &screens), window);
        }
        // On the monitor, which is gone: centred on the laptop.
        assert_eq!(
            on_screen(rect(2200, 300, 1200, 800), &[laptop]),
            rect(360, 140, 1200, 800)
        );
        // Barely visible: the same.
        assert_eq!(
            on_screen(rect(1880, 1050, 1200, 800), &[laptop]),
            rect(360, 140, 1200, 800)
        );
        // Too large for the laptop: shrunk, and moved in.
        assert_eq!(
            on_screen(rect(10, 10, 2500, 1400), &[laptop]),
            rect(0, 0, 1920, 1080)
        );
        assert_eq!(
            on_screen(rect(400, 300, 1200, 1400), &[laptop]),
            rect(400, 0, 1200, 1080)
        );
        // Off the screens, larger than the primary one.
        assert_eq!(
            on_screen(rect(9000, 9000, 2500, 1400), &screens),
            rect(0, 0, 1920, 1080)
        );
        // No screens known.
        assert_eq!(
            on_screen(rect(9000, 9000, 10, 10), &[]),
            rect(9000, 9000, 10, 10)
        );
    }
}
