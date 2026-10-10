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
}

/// The groups of the settings dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Account,
    Sync,
    Articles,
}

impl Category {
    pub fn title(self) -> &'static str {
        match self {
            Category::Account => "Account",
            Category::Sync => "Sync",
            Category::Articles => "Articles",
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
    /// A colour as `#rrggbb`.
    Colour { default: Colour },
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
            Kind::Colour { default } => Some(Value::Colour(*default)),
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
            _ => Err(format!("must be {}", self.expected())),
        }
    }

    /// What the file must contain, for error messages.
    fn expected(&self) -> &'static str {
        match self.kind {
            Kind::Switch { .. } => "true or false",
            Kind::Choice { .. } | Kind::Text { .. } => "a string",
            Kind::Number { .. } => "a whole number",
            Kind::Colour { .. } => "a colour as a string \"#rrggbb\"",
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
            _ => return Err(wrong_type()),
        };
        self.check(&value)?;
        Ok(value)
    }
}

impl Value {
    /// The value as written to the file.
    pub(crate) fn to_toml(&self) -> toml_edit::Value {
        match self {
            Value::Bool(b) => (*b).into(),
            Value::Number(n) => (*n).into(),
            Value::Text(text) => text.as_str().into(),
            Value::Colour(colour) => colour.to_string().into(),
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
pub const LOAD_IMAGES: &str = "articles.load-images";

/// All settings, in the order of the settings dialog.
pub static SETTINGS: &[Setting] = &[
    Setting {
        key: SERVER,
        title: "Server",
        description: "The address of your Nextcloud, e.g. https://cloud.example.org.",
        category: Category::Account,
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
        kind: Kind::Number {
            min: 0,
            max: 10 * 365,
            default: 30,
            unit: "days",
        },
    },
    Setting {
        key: LOAD_IMAGES,
        title: "Load images",
        description: "Load the images of articles from their servers. When off, they are loaded \
                      only on request (Article → Load Images), so the servers don't learn which \
                      articles you read; images loaded before are still shown.",
        category: Category::Articles,
        kind: Kind::Switch { default: true },
    },
];

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
