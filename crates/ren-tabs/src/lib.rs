// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Web page tabs: what ren's window and its Servo helper (`ren-servo`)
//! say to each other.
//!
//! The window starts the helper with one end of a Unix socket pair as its
//! standard input. Both sides send messages on it: a little-endian u32
//! length followed by a JSON [`Command`] (window to helper) or [`Event`]
//! (helper to window). Frames don't go through the socket: the window
//! makes a shared memory buffer ([`frames`]), sends its file descriptor
//! with [`Command::Buffer`], and the helper writes each frame into it and
//! announces it with [`Event::Frame`]. The helper writes the next frame
//! only after the window answered [`Command::FrameTaken`], so frames are
//! never written while the window reads them, and frames the window
//! couldn't show aren't read back from the GPU at all.
//!
//! The helper runs web content, so the window doesn't trust what it
//! sends: messages are at most [`MAX_MESSAGE_BYTES`] long, and a frame
//! must fit the buffer it names. When the socket closes, the helper
//! shuts Servo down and exits.

use serde::{Deserialize, Serialize};

pub mod frames;
pub mod stats;
pub mod wire;

/// Identifies a tab while the helper runs. The window numbers the tabs it
/// opens from 0, the helper those that pages open from
/// [`FIRST_PAGE_TAB`].
pub type TabId = u32;

/// The first id of tabs that pages open (`window.open`, links to new
/// windows).
pub const FIRST_PAGE_TAB: TabId = 1 << 31;

/// The longest message either side accepts.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// The size of the page area, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// Keyboard modifiers as a bit set of [`CONTROL`], [`SHIFT`], [`ALT`] and
/// [`META`].
pub type Mods = u8;
pub const CONTROL: Mods = 1;
pub const SHIFT: Mods = 2;
pub const ALT: Mods = 4;
pub const META: Mods = 8;

/// Keys without a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamedKey {
    Backspace,
    Tab,
    Enter,
    Escape,
    Delete,
    Shift,
    Control,
    Alt,
    Meta,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Key {
    Character(String),
    Named(NamedKey),
}

/// Input for the active tab. Positions are in physical pixels relative to
/// the page area.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Input {
    Down {
        button: Button,
        x: f32,
        y: f32,
    },
    Up {
        button: Button,
        x: f32,
        y: f32,
    },
    Move {
        x: f32,
        y: f32,
    },
    /// Positive deltas scroll towards the top and left, as for the wheel.
    Wheel {
        dx: f32,
        dy: f32,
        x: f32,
        y: f32,
    },
    Key {
        key: Key,
        mods: Mods,
        down: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cursor {
    Default,
    Pointer,
    Text,
}

/// From the window to the helper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// The first command, once.
    Start {
        size: Size,
        dark: bool,
        /// Where Servo keeps cookies and site data; none keeps nothing
        /// after the helper exits.
        profile: Option<std::path::PathBuf>,
    },
    /// The shared memory for frames, sent with its file descriptor; `len`
    /// bytes, sealed against shrinking. Replaces the previous buffer.
    Buffer {
        id: u32,
        len: u64,
    },
    Open {
        tab: TabId,
        url: String,
    },
    /// Loads another page in a tab.
    Load {
        tab: TabId,
        url: String,
    },
    Close {
        tab: TabId,
    },
    /// Shows `tab`, or no page for `None` (the article is shown); the
    /// others are hidden.
    Activate {
        tab: Option<TabId>,
    },
    Back {
        tab: TabId,
    },
    Forward {
        tab: TabId,
    },
    Reload {
        tab: TabId,
    },
    Resize(Size),
    Input(Input),
    Dark(bool),
    /// The window copied the last frame out of the buffer.
    FrameTaken,
}

/// From the helper to the window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Title {
        tab: TabId,
        title: String,
    },
    Loading {
        tab: TabId,
        loading: bool,
    },
    Url {
        tab: TabId,
        url: String,
    },
    /// Whether the tab can go back and forward in its history.
    History {
        tab: TabId,
        back: bool,
        forward: bool,
    },
    Cursor(Cursor),
    /// A page opened a tab, next to its own (`opener`).
    Opened {
        tab: TabId,
        opener: TabId,
    },
    /// A page closed its tab (`window.close()`).
    Closed {
        tab: TabId,
    },
    /// A link that isn't loaded in a tab (`mailto:`), for the desktop.
    External {
        url: String,
    },
    /// A frame of `tab` is in buffer `buffer`: RGBA, premultiplied, top
    /// row first. `readback_us` is how long the helper took to get it
    /// from the GPU into the buffer.
    Frame {
        tab: TabId,
        buffer: u32,
        width: u32,
        height: u32,
        readback_us: u32,
    },
    /// The page's content process crashed.
    Crashed {
        tab: TabId,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_one_line() {
        let commands = [
            Command::Start {
                size: Size {
                    width: 800,
                    height: 600,
                    scale: 1.5,
                },
                dark: true,
                profile: Some("/home/u/.local/share/ren/servo".into()),
            },
            Command::Open {
                tab: 3,
                url: "https://example.org/\n".to_owned(),
            },
            Command::Input(Input::Key {
                key: Key::Named(NamedKey::Enter),
                mods: CONTROL | SHIFT,
                down: true,
            }),
            Command::FrameTaken,
        ];
        for command in commands {
            let line = serde_json::to_string(&command).unwrap();
            assert!(!line.contains('\n'));
            assert_eq!(serde_json::from_str::<Command>(&line).unwrap(), command);
        }
    }
}
