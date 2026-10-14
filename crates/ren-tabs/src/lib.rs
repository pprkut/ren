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
pub mod scroll;
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

/// Blocking ads and trackers by filter lists
/// (`docs/decisions/0012-content-blocking.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocking {
    /// The filter lists, in Adblock Plus or uBlock Origin syntax, in
    /// order. Lists that don't exist (yet) are left out.
    pub lists: Vec<std::path::PathBuf>,
    /// Where the helper saves the engine it made from the lists, to load
    /// it faster next time.
    pub cache: std::path::PathBuf,
    /// Sites nothing is blocked on: host names, each with its
    /// subdomains.
    pub unblocked: Vec<String>,
}

impl Blocking {
    /// Whether nothing is blocked on pages from `host`.
    pub fn unblocks(&self, host: &str) -> bool {
        self.unblocked.iter().any(|site| site_covers(site, host))
    }
}

/// Whether `site`, a host name, covers `host`: it's the same, or `host`
/// is a subdomain of it. Case and a final dot don't matter.
pub fn site_covers(site: &str, host: &str) -> bool {
    let site = site.trim_end_matches('.');
    let host = host.trim_end_matches('.');
    if site.is_empty() || host.len() < site.len() {
        return false;
    }
    let (rest, end) = host.split_at(host.len() - site.len());
    end.eq_ignore_ascii_case(site) && (rest.is_empty() || rest.ends_with('.'))
}

/// The host of a web URL, without user, password and port.
pub fn host_of(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if host.starts_with('[') {
        &host[..=host.find(']')?]
    } else {
        host.split(':').next()?
    };
    Some(host).filter(|host| !host.is_empty())
}

/// An entry of a page's drop-down (`<select>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectEntry {
    /// An option; `id` selects it.
    Option {
        id: usize,
        label: String,
        enabled: bool,
    },
    /// The heading of a group (`<optgroup>`); its options follow.
    Group(String),
}

/// What a page's context menu can do. Servo decides which apply; the
/// window opens and copies links itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuAction {
    Back,
    Forward,
    Reload,
    OpenLink,
    CopyLink,
    OpenImage,
    CopyImageLink,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// An entry of a page's context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuEntry {
    Action { action: MenuAction, enabled: bool },
    Separator,
}

/// A dialog a page's script opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogKind {
    /// `alert()`: a message and OK.
    Alert,
    /// `confirm()`: OK or Cancel.
    Confirm,
    /// `prompt()`: a line of text, or Cancel.
    Prompt,
}

/// The answer to a page's dialog ([`Event::Dialog`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogAnswer {
    /// OK; for a prompt, with the text entered.
    Ok(Option<String>),
    Cancel,
}

/// An RGB colour of a page's colour picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

/// The longest message of a page's dialog passed on, in characters.
pub const MAX_DIALOG_MESSAGE: usize = 2_000;

/// The longest default text of a prompt passed on, in characters.
pub const MAX_DIALOG_TEXT: usize = 1_000;

/// The most file name extensions of a file picker's filter passed on.
pub const MAX_FILE_FILTERS: usize = 50;

/// Where a page's control was activated, in physical pixels relative to
/// the page area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
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
        /// Content blocking; none blocks nothing.
        blocking: Option<Blocking>,
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
    /// New settings or lists for content blocking. The helper loads the
    /// lists again if they changed; until then it blocks with the old ones.
    Blocking(Option<Blocking>),
    /// The window copied the last frame out of the buffer.
    FrameTaken,
    /// The options chosen in a drop-down ([`Event::Select`]); none
    /// dismisses it.
    Select {
        control: u32,
        selected: Option<Vec<usize>>,
    },
    /// The entry chosen in a context menu ([`Event::ContextMenu`]); none
    /// dismisses it.
    ContextMenu {
        control: u32,
        action: Option<MenuAction>,
    },
    /// The answer to a page's `alert`, `confirm` or `prompt`
    /// ([`Event::Dialog`]).
    Dialog {
        control: u32,
        answer: DialogAnswer,
    },
    /// The colour chosen in a colour picker ([`Event::ColourPicker`]);
    /// none keeps the one the page had.
    Colour {
        control: u32,
        colour: Option<Rgb>,
    },
    /// The files chosen in a file picker ([`Event::FilePicker`]); none
    /// dismisses it.
    Files {
        control: u32,
        paths: Option<Vec<std::path::PathBuf>>,
    },
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
    /// A frame of `tab` is in buffer `buffer`: RGB (pages are opaque), top
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
    /// A drop-down (`<select>`) of the page shown was opened: the window
    /// shows its options below `area` and answers with
    /// [`Command::Select`]. `selected` are the ids of the options
    /// selected. `area` is as Servo 0.7 reports it: CSS pixels relative
    /// to the document, not the view.
    Select {
        tab: TabId,
        control: u32,
        area: Area,
        entries: Vec<SelectEntry>,
        selected: Vec<usize>,
        multiple: bool,
    },
    /// A context menu was asked for in the page shown, at `area`: the
    /// window shows it and answers with [`Command::ContextMenu`]. `area`
    /// is as Servo 0.7 reports it: the click's point in CSS pixels. `link`
    /// and `image` are the URLs of the link and image under the pointer.
    ContextMenu {
        tab: TabId,
        control: u32,
        area: Area,
        entries: Vec<MenuEntry>,
        link: Option<String>,
        image: Option<String>,
    },
    /// A page's script opened a dialog (`alert`, `confirm`, `prompt`) in
    /// the page shown; the window shows it over the page and answers with
    /// [`Command::Dialog`]. The message is the page's, at most
    /// [`MAX_DIALOG_MESSAGE`] characters; `text` is a prompt's default.
    Dialog {
        tab: TabId,
        control: u32,
        kind: DialogKind,
        message: String,
        text: String,
    },
    /// A colour input (`<input type=color>`) of the page shown was
    /// clicked; the window shows a colour picker at `area` (as for
    /// [`Event::Select`]) and answers with [`Command::Colour`].
    ColourPicker {
        tab: TabId,
        control: u32,
        area: Area,
        current: Option<Rgb>,
    },
    /// A file input (`<input type=file>`) of the page shown was clicked;
    /// the window asks the desktop for files and answers with
    /// [`Command::Files`]. `extensions` are the file name extensions the
    /// page accepts (none: any file).
    FilePicker {
        tab: TabId,
        control: u32,
        multiple: bool,
        extensions: Vec<String>,
    },
    /// The page took back a control it asked for (it navigated away, the
    /// element went away); the window closes it.
    ControlHidden {
        control: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_cover_their_subdomains() {
        assert!(site_covers("heise.de", "heise.de"));
        assert!(site_covers("heise.de", "www.heise.de"));
        assert!(site_covers("heise.de.", "WWW.Heise.DE."));
        assert!(!site_covers("heise.de", "notheise.de"));
        assert!(!site_covers("heise.de", "heise.de.example"));
        assert!(!site_covers("", "heise.de"));
        assert!(!site_covers("www.heise.de", "heise.de"));
        let blocking = Blocking {
            lists: Vec::new(),
            cache: "/c".into(),
            unblocked: vec!["example.org".to_owned(), "heise.de".to_owned()],
        };
        assert!(blocking.unblocks("www.heise.de"));
        assert!(!blocking.unblocks("example.com"));
    }

    #[test]
    fn hosts() {
        assert_eq!(
            host_of("https://www.heise.de/news?x#y"),
            Some("www.heise.de")
        );
        assert_eq!(
            host_of("https://user:pw@example.org:8443/"),
            Some("example.org")
        );
        assert_eq!(host_of("http://[::1]:8000/"), Some("[::1]"));
        assert_eq!(host_of("https://example.org"), Some("example.org"));
        assert_eq!(host_of("about:blank"), None);
        assert_eq!(host_of("file:///etc/passwd"), None);
    }

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
                blocking: Some(Blocking {
                    lists: vec!["/home/u/.cache/ren/filters/easylist.txt".into()],
                    cache: "/home/u/.cache/ren/filters/engine.dat".into(),
                    unblocked: vec!["example.org".to_owned()],
                }),
            },
            Command::Blocking(None),
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
            Command::Select {
                control: 2,
                selected: Some(vec![4]),
            },
            Command::ContextMenu {
                control: 3,
                action: Some(MenuAction::Copy),
            },
            Command::Dialog {
                control: 4,
                answer: DialogAnswer::Ok(Some("a\nb".to_owned())),
            },
            Command::Dialog {
                control: 5,
                answer: DialogAnswer::Cancel,
            },
            Command::Colour {
                control: 6,
                colour: Some(Rgb {
                    red: 1,
                    green: 2,
                    blue: 3,
                }),
            },
            Command::Files {
                control: 7,
                paths: Some(vec!["/home/u/a b.pdf".into()]),
            },
        ];
        for command in commands {
            let line = serde_json::to_string(&command).unwrap();
            assert!(!line.contains('\n'));
            assert_eq!(serde_json::from_str::<Command>(&line).unwrap(), command);
        }
    }
}
