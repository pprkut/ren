// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The setting descriptors: what each setting is called, what it does,
//! which values it takes and its default. One table drives loading,
//! validation, writing and the settings dialog (M8b).

use std::fmt;

/// One setting.
#[derive(Debug)]
pub struct Setting {
    /// `table.key` in the settings file, e.g. `sync.interval-minutes`.
    pub key: &'static str,
    /// A short name for the settings dialog.
    pub title: &'static str,
    /// One or two sentences for the settings dialog.
    pub description: &'static str,
    pub category: Category,
    pub kind: Kind,
    /// A switch that must be on for this setting to have an effect, e.g.
    /// closing to the tray needs the tray icon. The dialog disables the
    /// setting while the switch is off.
    pub requires: Option<&'static str>,
}

/// The groups of the settings dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Account,
    Sync,
    List,
    Articles,
    Pages,
    Window,
}

impl Category {
    pub fn title(self) -> &'static str {
        match self {
            Category::Account => "Account",
            Category::Sync => "Sync",
            Category::List => "Article list",
            Category::Articles => "Articles",
            Category::Pages => "Web pages",
            Category::Window => "Window",
        }
    }
}

/// The values a setting takes, which also decides its control in the
/// settings dialog.
#[derive(Debug)]
pub enum Kind {
    /// `true` or `false`.
    Switch { default: bool },
    /// One of a few strings.
    Choice {
        options: &'static [Choice],
        default: &'static str,
    },
    /// A whole number in `min..=max`.
    Number {
        min: i64,
        max: i64,
        default: i64,
        /// The unit, for the dialog, e.g. "minutes".
        unit: &'static str,
    },
    /// Free text. Without a default, the setting is unset until the user
    /// sets it; an empty string counts as unset too.
    Text {
        default: Option<&'static str>,
        /// Checks a value that isn't empty; the error says what's wrong.
        check: Option<Check>,
    },
    /// A colour as `#rrggbb`. Without a default, the setting is unset
    /// until the user sets it, and the app uses a colour of its own (e.g.
    /// one for light and one for dark mode).
    Colour { default: Option<Colour> },
    /// A list of strings, an array in the file. Empty strings are left
    /// out.
    List {
        default: &'static [&'static str],
        /// Checks each string that isn't empty.
        check: Option<Check>,
    },
}

/// Checks a text value; the error says what's wrong with it.
pub type Check = fn(&str) -> Result<(), String>;

/// An option of a [`Kind::Choice`].
#[derive(Debug)]
pub struct Choice {
    /// What the file says.
    pub value: &'static str,
    /// What the dialog says.
    pub title: &'static str,
}

/// A setting's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Number(i64),
    /// The value of a text or a choice.
    Text(String),
    Colour(Colour),
    List(Vec<String>),
}

/// An RGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Colour {
    /// Reads `#rrggbb`, in either case.
    pub fn parse(text: &str) -> Option<Colour> {
        let hex = text.strip_prefix('#')?;
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Colour {
            red: channel(0)?,
            green: channel(2)?,
            blue: channel(4)?,
        })
    }
}

/// `#rrggbb` in lower case.
impl fmt::Display for Colour {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.red, self.green, self.blue)
    }
}

impl Setting {
    /// The table the setting is in: `sync` for `sync.interval-minutes`.
    pub fn table(&self) -> &'static str {
        self.key.split_once('.').map_or("", |(table, _)| table)
    }

    /// The setting's key inside its table: `interval-minutes`.
    pub fn name(&self) -> &'static str {
        self.key.split_once('.').map_or(self.key, |(_, name)| name)
    }

    /// The value when the file doesn't set one; `None` for text without a
    /// default.
    pub fn default_value(&self) -> Option<Value> {
        match &self.kind {
            Kind::Switch { default } => Some(Value::Bool(*default)),
            Kind::Choice { default, .. } => Some(Value::Text((*default).to_owned())),
            Kind::Number { default, .. } => Some(Value::Number(*default)),
            Kind::Text { default, .. } => default.map(|text| Value::Text(text.to_owned())),
            Kind::Colour { default } => default.map(Value::Colour),
            Kind::List { default, .. } => Some(Value::List(
                default.iter().map(|item| (*item).to_owned()).collect(),
            )),
        }
    }

    /// Checks that `value` is one this setting takes.
    pub fn check(&self, value: &Value) -> Result<(), String> {
        match (&self.kind, value) {
            (Kind::Switch { .. }, Value::Bool(_)) | (Kind::Colour { .. }, Value::Colour(_)) => {
                Ok(())
            }
            (Kind::Choice { options, .. }, Value::Text(text)) => {
                if options.iter().any(|option| option.value == text) {
                    Ok(())
                } else {
                    let values: Vec<_> = options
                        .iter()
                        .map(|option| format!("\"{}\"", option.value))
                        .collect();
                    Err(format!("must be one of {}", values.join(", ")))
                }
            }
            (Kind::Number { min, max, .. }, Value::Number(number)) => {
                if (*min..=*max).contains(number) {
                    Ok(())
                } else {
                    Err(format!("must be between {min} and {max}"))
                }
            }
            (Kind::Text { check, .. }, Value::Text(text)) => match check {
                Some(check) if !text.is_empty() => check(text),
                _ => Ok(()),
            },
            (Kind::List { check, .. }, Value::List(items)) => match check {
                Some(check) => items
                    .iter()
                    .filter(|item| !item.is_empty())
                    .try_for_each(|item| check(item)),
                None => Ok(()),
            },
            _ => Err(format!("must be {}", self.expected())),
        }
    }

    /// Whether the setting matches a search of the settings dialog: each
    /// word of `query` is in its title, description, key, category or the
    /// title of one of its options, in any case.
    pub fn matches(&self, query: &str) -> bool {
        let mut haystack = format!(
            "{} {} {} {}",
            self.title,
            self.description,
            self.key,
            self.category.title()
        );
        if let Kind::Choice { options, .. } = &self.kind {
            for option in *options {
                haystack.push(' ');
                haystack.push_str(option.title);
            }
        }
        let haystack = haystack.to_lowercase();
        query
            .to_lowercase()
            .split_whitespace()
            .all(|word| haystack.contains(word))
    }

    /// Reads what the user typed into the settings dialog: a number, a
    /// colour (`#` optional), text, a choice's value, or a list with one
    /// item per line. `None` for empty text or colour, which unsets the
    /// setting. The value is checked.
    pub fn parse_input(&self, input: &str) -> Result<Option<Value>, String> {
        let trimmed = input.trim();
        let value = match &self.kind {
            Kind::Switch { .. } => match trimmed {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => return Err("must be on or off".to_owned()),
            },
            Kind::Number { .. } => Value::Number(
                trimmed
                    .parse()
                    .map_err(|_| "must be a whole number".to_owned())?,
            ),
            Kind::Choice { .. } => Value::Text(trimmed.to_owned()),
            Kind::Text { .. } if trimmed.is_empty() => return Ok(None),
            Kind::Text { .. } => Value::Text(trimmed.to_owned()),
            Kind::Colour { .. } if trimmed.is_empty() => return Ok(None),
            Kind::Colour { .. } => {
                let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
                Value::Colour(
                    Colour::parse(&format!("#{hex}"))
                        .ok_or_else(|| "must be a colour as #rrggbb".to_owned())?,
                )
            }
            Kind::List { .. } => Value::List(
                input
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect(),
            ),
        };
        self.check(&value)?;
        Ok(Some(value))
    }

    /// What the file must contain, for error messages.
    fn expected(&self) -> &'static str {
        match self.kind {
            Kind::Switch { .. } => "true or false",
            Kind::Choice { .. } | Kind::Text { .. } => "a string",
            Kind::Number { .. } => "a whole number",
            Kind::Colour { .. } => "a colour as a string \"#rrggbb\"",
            Kind::List { .. } => "a list of strings",
        }
    }

    /// Reads and checks a value from the file.
    pub(crate) fn read(&self, value: &toml_edit::Value) -> Result<Value, String> {
        let wrong_type = || format!("must be {}", self.expected());
        let value = match (&self.kind, value) {
            (Kind::Switch { .. }, toml_edit::Value::Boolean(b)) => Value::Bool(*b.value()),
            (Kind::Number { .. }, toml_edit::Value::Integer(n)) => Value::Number(*n.value()),
            (Kind::Choice { .. } | Kind::Text { .. }, toml_edit::Value::String(s)) => {
                Value::Text(s.value().clone())
            }
            (Kind::Colour { .. }, toml_edit::Value::String(s)) => {
                Value::Colour(Colour::parse(s.value()).ok_or_else(wrong_type)?)
            }
            (Kind::List { .. }, toml_edit::Value::Array(array)) => Value::List(
                array
                    .iter()
                    .map(|item| item.as_str().map(str::to_owned))
                    .collect::<Option<_>>()
                    .ok_or_else(wrong_type)?,
            ),
            _ => return Err(wrong_type()),
        };
        self.check(&value)?;
        Ok(value)
    }
}

impl Value {
    /// The value as the settings dialog shows it for editing, the reverse
    /// of [`Setting::parse_input`]: a list has one item per line.
    pub fn to_input(&self) -> String {
        match self {
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::Text(text) => text.clone(),
            Value::Colour(colour) => colour.to_string(),
            Value::List(items) => items.join("\n"),
        }
    }

    /// The value as written to the file.
    pub(crate) fn to_toml(&self) -> toml_edit::Value {
        match self {
            Value::Bool(b) => (*b).into(),
            Value::Number(n) => (*n).into(),
            Value::Text(text) => text.as_str().into(),
            Value::Colour(colour) => colour.to_string().into(),
            Value::List(items) => toml_edit::Value::Array(items.iter().collect()),
        }
    }
}

/// The setting with this key in `table`.
pub fn find<'a>(table: &'a [Setting], key: &str) -> Option<&'a Setting> {
    table.iter().find(|setting| setting.key == key)
}

pub const SERVER: &str = "account.server";
pub const USER: &str = "account.user";
pub const PASSWORD_COMMAND: &str = "account.password-command";
pub const SYNC_INTERVAL: &str = "sync.interval-minutes";
pub const KEEP_READ: &str = "sync.keep-read-days";
pub const FEED_ICONS: &str = "sync.feed-icons";
pub const ARRANGEMENT: &str = "list.arrangement";
pub const NEW_COLOUR: &str = "list.new-colour";
pub const UNREAD_COLOUR: &str = "list.unread-colour";
pub const LOAD_IMAGES: &str = "articles.load-images";
pub const IMAGE_CACHE: &str = "articles.image-cache-mib";
pub const KEEP_SITE_DATA: &str = "pages.keep-site-data";
pub const BLOCK_CONTENT: &str = "pages.block-content";
pub const FILTER_LISTS: &str = "pages.filter-lists";
pub const UNBLOCKED_SITES: &str = "pages.unblocked-sites";
pub const TRAY_ICON: &str = "window.tray-icon";
pub const CLOSE_TO_TRAY: &str = "window.close-to-tray";

/// A filter list for content blocking that `pages.filter-lists` can name.
#[derive(Debug)]
pub struct FilterList {
    /// What the settings file says.
    pub name: &'static str,
    pub title: &'static str,
    pub url: &'static str,
}

/// The filter lists known by name; see
/// `docs/decisions/0012-content-blocking.md`.
pub static FILTER_LIST_CATALOGUE: &[FilterList] = &[
    FilterList {
        name: "easylist",
        title: "EasyList",
        url: "https://easylist.to/easylist/easylist.txt",
    },
    FilterList {
        name: "easyprivacy",
        title: "EasyPrivacy",
        url: "https://easylist.to/easylist/easyprivacy.txt",
    },
    FilterList {
        name: "ublock",
        title: "uBlock filters",
        url: "https://ublockorigin.github.io/uAssets/filters/filters.txt",
    },
    FilterList {
        name: "ublock-privacy",
        title: "uBlock filters – Privacy",
        url: "https://ublockorigin.github.io/uAssets/filters/privacy.txt",
    },
    FilterList {
        name: "ublock-unbreak",
        title: "uBlock filters – Unbreak",
        url: "https://ublockorigin.github.io/uAssets/filters/unbreak.txt",
    },
    FilterList {
        name: "easylist-germany",
        title: "EasyList Germany",
        url: "https://easylist.to/easylistgermany/easylistgermany.txt",
    },
    FilterList {
        name: "fanboy-annoyance",
        title: "Fanboy's Annoyance List",
        url: "https://easylist.to/easylist/fanboy-annoyance.txt",
    },
];

/// The URL of a filter list setting: a name from the catalogue, or a URL.
pub fn filter_list_url(list: &str) -> Option<&str> {
    match FILTER_LIST_CATALOGUE
        .iter()
        .find(|known| known.name == list)
    {
        Some(known) => Some(known.url),
        None => list.starts_with("https://").then_some(list),
    }
}

/// All settings, in the order of the settings dialog.
pub static SETTINGS: &[Setting] = &[
    Setting {
        key: SERVER,
        title: "Server",
        description: "The address of your Nextcloud, e.g. https://cloud.example.org.",
        category: Category::Account,
        requires: None,
        kind: Kind::Text {
            default: None,
            check: Some(check_server),
        },
    },
    Setting {
        key: USER,
        title: "User",
        description: "Your login name on the Nextcloud.",
        category: Category::Account,
        requires: None,
        kind: Kind::Text {
            default: None,
            check: None,
        },
    },
    Setting {
        key: PASSWORD_COMMAND,
        title: "Password command",
        description: "A command that prints the app password, e.g. \"pass show nextcloud/ren\", \
                      for systems without a Secret Service (GNOME Keyring, KWallet). Used \
                      instead of the Secret Service when set.",
        category: Category::Account,
        requires: None,
        kind: Kind::Text {
            default: None,
            check: None,
        },
    },
    Setting {
        key: SYNC_INTERVAL,
        title: "Sync interval",
        description: "Minutes between automatic syncs. 0 syncs only when asked to.",
        category: Category::Sync,
        requires: None,
        kind: Kind::Number {
            min: 0,
            max: 24 * 60,
            default: 15,
            unit: "minutes",
        },
    },
    Setting {
        key: KEEP_READ,
        title: "Keep read items",
        description: "Days to keep read items that aren't starred, counted from when they were \
                      read. 0 keeps them all.",
        category: Category::Sync,
        requires: None,
        kind: Kind::Number {
            min: 0,
            max: 10 * 365,
            default: 30,
            unit: "days",
        },
    },
    Setting {
        key: FEED_ICONS,
        title: "Feed icons",
        description: "Download the icons of the feeds from their sites after a sync, again \
                      when they are a month old. When off, no requests go to the sites; icons \
                      downloaded before are still shown.",
        category: Category::Sync,
        requires: None,
        kind: Kind::Switch { default: true },
    },
    Setting {
        key: ARRANGEMENT,
        title: "Arrangement",
        description: "Where the list of articles goes: beside the article, with two lines per \
                      article, or above it, as a table (akregator's arrangement). Also in the \
                      View menu.",
        category: Category::List,
        requires: None,
        kind: Kind::Choice {
            options: &[
                Choice {
                    value: "beside",
                    title: "Beside the article",
                },
                Choice {
                    value: "above",
                    title: "Above the article",
                },
            ],
            default: "beside",
        },
    },
    Setting {
        key: NEW_COLOUR,
        title: "Colour of new articles",
        description: "The colour of articles that arrived with the latest sync, as #rrggbb. \
                      Unset, it is a red that suits the light or dark colour scheme.",
        category: Category::List,
        requires: None,
        kind: Kind::Colour { default: None },
    },
    Setting {
        key: UNREAD_COLOUR,
        title: "Colour of unread articles",
        description: "The colour of unread articles, as #rrggbb. Unset, it is a green that \
                      suits the light or dark colour scheme.",
        category: Category::List,
        requires: None,
        kind: Kind::Colour { default: None },
    },
    Setting {
        key: LOAD_IMAGES,
        title: "Load images",
        description: "Load the images of articles from their servers. When off, they are loaded \
                      only on request (Article → Load Images), so the servers don't learn which \
                      articles you read; images loaded before are still shown.",
        category: Category::Articles,
        requires: None,
        kind: Kind::Switch { default: true },
    },
    Setting {
        key: IMAGE_CACHE,
        title: "Image cache",
        description: "Disk space for the images of articles, so that articles read again \
                      show them without fetching them, also offline. The images used least \
                      recently are removed first.",
        category: Category::Articles,
        requires: None,
        kind: Kind::Number {
            min: 10,
            max: 100_000,
            default: 100,
            unit: "MiB",
        },
    },
    Setting {
        key: KEEP_SITE_DATA,
        title: "Keep site data",
        description: "Keep the cookies and site data of web pages opened in tabs, so logins and \
                      consent choices last. When off, they are forgotten when the last tab \
                      closes. Settings → Clear Web Site Data removes what was kept.",
        category: Category::Pages,
        requires: None,
        kind: Kind::Switch { default: true },
    },
    Setting {
        key: BLOCK_CONTENT,
        title: "Block ads and trackers",
        description: "Don't load what the filter lists name as ads or trackers in web pages. \
                      The lists are downloaded when a page is opened and they are older than \
                      four days.",
        category: Category::Pages,
        requires: None,
        kind: Kind::Switch { default: true },
    },
    Setting {
        key: FILTER_LISTS,
        title: "Filter lists",
        description: "The lists of ads and trackers: easylist, easyprivacy, ublock, \
                      ublock-privacy, ublock-unbreak, easylist-germany, fanboy-annoyance, or \
                      the https:// URL of another list in Adblock Plus or uBlock Origin syntax.",
        category: Category::Pages,
        requires: Some(BLOCK_CONTENT),
        kind: Kind::List {
            default: &[
                "easylist",
                "easyprivacy",
                "ublock",
                "ublock-privacy",
                "ublock-unbreak",
            ],
            check: Some(check_filter_list),
        },
    },
    Setting {
        key: UNBLOCKED_SITES,
        title: "Sites without blocking",
        description: "Sites on which nothing is blocked, e.g. because they don't work with \
                      blocking: host names such as example.org, each with its subdomains.",
        category: Category::Pages,
        requires: Some(BLOCK_CONTENT),
        kind: Kind::List {
            default: &[],
            check: Some(check_site),
        },
    },
    Setting {
        key: TRAY_ICON,
        title: "Icon in the system tray",
        description: "Show an icon with the number of unread articles in the desktop's system \
                      tray (Plasma; GNOME needs the AppIndicator extension), with a menu to show \
                      or hide the window, fetch the feeds and quit.",
        category: Category::Window,
        requires: None,
        kind: Kind::Switch { default: true },
    },
    Setting {
        key: CLOSE_TO_TRAY,
        title: "Close to the tray",
        description: "Closing the window hides it in the system tray instead of quitting, so \
                      the feeds are still fetched; the tray's menu quits. Only with the icon \
                      in the tray, and a desktop that shows it.",
        category: Category::Window,
        requires: Some(TRAY_ICON),
        kind: Kind::Switch { default: false },
    },
];

fn check_filter_list(list: &str) -> Result<(), String> {
    match filter_list_url(list) {
        Some(_) => Ok(()),
        None => Err(format!(
            "must name a known list or be an https:// URL: {list}"
        )),
    }
}

/// A host name: letters, digits, hyphens and dots.
pub fn check_site(site: &str) -> Result<(), String> {
    let valid = site.split('.').all(|label| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    });
    if valid {
        Ok(())
    } else {
        Err(format!("must be a host name such as example.org: {site}"))
    }
}

/// Basic auth sends the password with every request, so only HTTPS is
/// accepted, except for a server on the local machine.
pub fn check_server(server: &str) -> Result<(), String> {
    let local = ["http://localhost", "http://127.0.0.1", "http://[::1]"];
    let is_local = |prefix: &&str| {
        server
            .strip_prefix(*prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
    };
    let host = server.strip_prefix("https://").unwrap_or_default();
    if !host.is_empty() || local.iter().any(is_local) {
        Ok(())
    } else {
        Err(format!("must be an https:// URL: {server}"))
    }
}

/// A server address as typed by the user, as the settings keep it:
/// without spaces and trailing slashes, `https://` if it has no scheme.
pub fn normalise_server(input: &str) -> String {
    let server = input.trim().trim_end_matches('/');
    if server.is_empty() || server.contains("://") {
        server.to_owned()
    } else {
        format!("https://{server}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_in_a_table() {
        for (i, setting) in SETTINGS.iter().enumerate() {
            assert!(!setting.table().is_empty(), "{}", setting.key);
            assert!(!setting.name().contains('.'), "{}", setting.key);
            assert!(
                SETTINGS[..i].iter().all(|other| other.key != setting.key),
                "{} twice",
                setting.key
            );
        }
    }

    #[test]
    fn requirements_are_switches() {
        for setting in SETTINGS {
            if let Some(key) = setting.requires {
                let required = find(SETTINGS, key).unwrap();
                assert!(
                    matches!(required.kind, Kind::Switch { .. }),
                    "{} requires {key}",
                    setting.key
                );
            }
        }
    }

    #[test]
    fn defaults_are_valid() {
        for setting in SETTINGS {
            if let Some(value) = setting.default_value() {
                assert_eq!(setting.check(&value), Ok(()), "{}", setting.key);
            }
        }
    }

    #[test]
    fn colours() {
        let colour = Colour {
            red: 0x12,
            green: 0xab,
            blue: 0xff,
        };
        assert_eq!(Colour::parse("#12abff"), Some(colour));
        assert_eq!(Colour::parse("#12ABFF"), Some(colour));
        assert_eq!(colour.to_string(), "#12abff");
        for invalid in [
            "12abff", "#12abf", "#12abffa", "#12abfg", "#+2abff", "#ü2abf",
        ] {
            assert_eq!(Colour::parse(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn search() {
        let setting = |key| find(SETTINGS, key).unwrap();
        assert!(setting(SYNC_INTERVAL).matches(""));
        assert!(setting(SYNC_INTERVAL).matches("  "));
        assert!(setting(SYNC_INTERVAL).matches("Interval"));
        assert!(setting(SYNC_INTERVAL).matches("sync MINUTES"));
        assert!(setting(SYNC_INTERVAL).matches("sync.interval"));
        assert!(!setting(SYNC_INTERVAL).matches("sync images"));
        // A category, an option.
        assert!(setting(NEW_COLOUR).matches("article list"));
        assert!(setting(ARRANGEMENT).matches("above"));
        assert!(setting(LOAD_IMAGES).matches("ARTICLES"));
    }

    #[test]
    fn input() {
        let setting = |key| find(SETTINGS, key).unwrap();
        let parse = |key, input| setting(key).parse_input(input);
        assert_eq!(parse(SYNC_INTERVAL, " 20 "), Ok(Some(Value::Number(20))));
        assert_eq!(
            parse(SYNC_INTERVAL, "2000"),
            Err("must be between 0 and 1440".to_owned())
        );
        assert_eq!(
            parse(SYNC_INTERVAL, "ten"),
            Err("must be a whole number".to_owned())
        );
        assert_eq!(parse(FEED_ICONS, "false"), Ok(Some(Value::Bool(false))));
        assert_eq!(
            parse(ARRANGEMENT, "above"),
            Ok(Some(Value::Text("above".to_owned())))
        );
        assert!(parse(ARRANGEMENT, "below").is_err());
        assert_eq!(parse(PASSWORD_COMMAND, "  "), Ok(None));
        assert_eq!(
            parse(PASSWORD_COMMAND, " pass show x "),
            Ok(Some(Value::Text("pass show x".to_owned())))
        );
        assert!(parse(SERVER, "http://cloud.example.org").is_err());
        let orange = Value::Colour(Colour {
            red: 0xff,
            green: 0x80,
            blue: 0,
        });
        assert_eq!(parse(NEW_COLOUR, "#FF8000"), Ok(Some(orange.clone())));
        assert_eq!(parse(NEW_COLOUR, "ff8000"), Ok(Some(orange.clone())));
        assert_eq!(parse(NEW_COLOUR, ""), Ok(None));
        assert!(parse(NEW_COLOUR, "orange").is_err());
        assert_eq!(orange.to_input(), "#ff8000");
        let list = Value::List(vec!["easylist".to_owned(), "ublock".to_owned()]);
        assert_eq!(
            parse(FILTER_LISTS, "easylist\n\n  ublock \n"),
            Ok(Some(list.clone()))
        );
        assert_eq!(list.to_input(), "easylist\nublock");
        assert_eq!(parse(FILTER_LISTS, ""), Ok(Some(Value::List(vec![]))));
        assert!(parse(UNBLOCKED_SITES, "https://x.org").is_err());
        assert_eq!(Value::Number(5).to_input(), "5");
    }

    #[test]
    fn normalised_servers() {
        assert_eq!(
            normalise_server(" cloud.example.org/ "),
            "https://cloud.example.org"
        );
        assert_eq!(
            normalise_server("https://example.org/nextcloud//"),
            "https://example.org/nextcloud"
        );
        assert_eq!(
            normalise_server("http://localhost:8080"),
            "http://localhost:8080"
        );
        assert_eq!(normalise_server("  "), "");
    }

    #[test]
    fn server() {
        assert!(check_server("https://cloud.example.org").is_ok());
        assert!(check_server("http://localhost:8080").is_ok());
        assert!(check_server("http://127.0.0.1").is_ok());
        assert!(check_server("http://[::1]:80/nc").is_ok());
        assert!(check_server("http://localhost.example.org").is_err());
        assert!(check_server("http://cloud.example.org").is_err());
        assert!(check_server("https://").is_err());
        assert!(check_server("cloud.example.org").is_err());
    }
}
