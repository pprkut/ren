// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings as read from the file: the overrides, with the defaults
//! from the descriptors for everything else.

use std::fmt;
use std::time::Duration;

use toml_edit::Document;

use crate::schema::{self, SETTINGS, Setting, Value};

/// The settings in effect.
#[derive(Debug, Clone)]
pub struct Settings {
    table: &'static [Setting],
    /// What the file sets, per descriptor of `table`.
    values: Vec<Option<Value>>,
}

/// Settings read from a file, with what was odd about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub settings: Settings,
    /// Unknown tables and keys: typos, or settings of a newer ren.
    pub warnings: Vec<Problem>,
}

/// Something wrong in the settings file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub severity: Severity,
    /// Where in the file; `None` if the file can't be read at all.
    pub position: Option<Position>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The settings aren't used.
    Error,
    /// Ignored, the rest is used.
    Warning,
}

/// A position in the file, both counted from 1. The column counts
/// characters, not bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    /// The position of byte `offset` in `text`.
    pub fn of(text: &str, offset: usize) -> Position {
        let before = &text[..text.floor_char_boundary(offset)];
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        Position {
            line: before.matches('\n').count() + 1,
            column: before[line_start..].chars().count() + 1,
        }
    }
}

/// `line:column: message`, the format compilers use, so that editors can
/// jump there when it follows the file name.
impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(Position { line, column }) = self.position {
            write!(f, "{line}:{column}: ")?;
        }
        if self.severity == Severity::Warning {
            f.write_str("warning: ")?;
        }
        f.write_str(&self.message)
    }
}

/// The problems that made a file unusable, at least one an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problems(pub Vec<Problem>);

/// One problem per line.
impl fmt::Display for Problems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, problem) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            problem.fmt(f)?;
        }
        Ok(())
    }
}

impl std::error::Error for Problems {}

/// The account as far as the settings file knows it; the app password is
/// elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// The Nextcloud URL, e.g. `https://cloud.example.org`.
    pub server: String,
    pub user: String,
    /// Shell command printing the app password on its first line.
    pub password_command: Option<String>,
}

/// The same values of the same descriptor table.
impl PartialEq for Settings {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.table, other.table) && self.values == other.values
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings::defaults(SETTINGS)
    }
}

impl Settings {
    /// The defaults of `table`.
    pub fn defaults(table: &'static [Setting]) -> Settings {
        Settings {
            table,
            values: vec![None; table.len()],
        }
    }

    /// Reads the text of a settings file with the settings of
    /// [`SETTINGS`].
    pub fn parse(text: &str) -> Result<Loaded, Problems> {
        Settings::parse_with(text, SETTINGS)
    }

    /// Reads the text of a settings file with the settings of `table`.
    ///
    /// Unknown tables and keys are warnings; a file that isn't TOML or a
    /// value a setting doesn't take are errors, and then none of the file
    /// is used: half of a file can mean something else than the whole
    /// (a server without its user).
    pub fn parse_with(text: &str, table: &'static [Setting]) -> Result<Loaded, Problems> {
        let problem = |severity, span: Option<std::ops::Range<usize>>, message| Problem {
            severity,
            position: span.map(|span| Position::of(text, span.start)),
            message,
        };
        let document = Document::parse(text).map_err(|err| {
            Problems(vec![problem(
                Severity::Error,
                err.span(),
                err.message().to_owned(),
            )])
        })?;

        let mut settings = Settings::defaults(table);
        let mut problems = Vec::new();
        for (name, item) in document.iter() {
            let key_span = document.key(name).and_then(|key| key.span());
            let known = table.iter().any(|setting| setting.table() == name);
            let Some(section) = item.as_table_like() else {
                problems.push(if known {
                    problem(
                        Severity::Error,
                        item.span().or(key_span),
                        format!("{name} must be a table: [{name}]"),
                    )
                } else {
                    problem(
                        Severity::Warning,
                        key_span,
                        format!("unknown setting {name}"),
                    )
                });
                continue;
            };
            if !known {
                problems.push(problem(
                    Severity::Warning,
                    key_span,
                    format!("unknown table [{name}]"),
                ));
                continue;
            }
            for (key, item) in section.iter() {
                let full = format!("{name}.{key}");
                let key_span = section.key(key).and_then(|key| key.span());
                let Some(index) = table.iter().position(|setting| setting.key == full) else {
                    problems.push(problem(
                        Severity::Warning,
                        key_span,
                        format!("unknown setting {full}"),
                    ));
                    continue;
                };
                let result = match item.as_value() {
                    Some(value) => table[index]
                        .read(value)
                        .map_err(|message| (value.span(), message)),
                    None => Err((item.span(), "must be a value, not a table".to_owned())),
                };
                match result {
                    Ok(value) => settings.values[index] = Some(value),
                    Err((span, message)) => problems.push(problem(
                        Severity::Error,
                        span.or(key_span),
                        format!("{full} {message}"),
                    )),
                }
            }
        }

        problems.sort_by_key(|problem| problem.position);
        if problems.iter().any(|p| p.severity == Severity::Error) {
            Err(Problems(problems))
        } else {
            Ok(Loaded {
                settings,
                warnings: problems,
            })
        }
    }

    /// The descriptors these settings follow.
    pub fn table(&self) -> &'static [Setting] {
        self.table
    }

    /// The value of the setting `key`, from the file or the default;
    /// `None` for text that isn't set.
    ///
    /// Panics if there's no such setting: keys are constants of the code.
    pub fn get(&self, key: &str) -> Option<Value> {
        let index = self.index(key);
        self.values[index]
            .clone()
            .or_else(|| self.table[index].default_value())
    }

    /// Whether the file sets `key`, rather than it having its default.
    pub fn is_set(&self, key: &str) -> bool {
        self.values[self.index(key)].is_some()
    }

    fn index(&self, key: &str) -> usize {
        self.table
            .iter()
            .position(|setting| setting.key == key)
            .unwrap_or_else(|| panic!("no setting {key}"))
    }

    /// A switch. Panics if `key` isn't one.
    pub fn bool(&self, key: &str) -> bool {
        match self.get(key) {
            Some(Value::Bool(b)) => b,
            other => panic!("{key} is not a switch: {other:?}"),
        }
    }

    /// A number. Panics if `key` isn't one.
    pub fn number(&self, key: &str) -> i64 {
        match self.get(key) {
            Some(Value::Number(n)) => n,
            other => panic!("{key} is not a number: {other:?}"),
        }
    }

    /// A text or a choice; `None` if it's empty or not set. Panics if
    /// `key` is neither.
    pub fn text(&self, key: &str) -> Option<String> {
        match self.get(key) {
            Some(Value::Text(text)) => Some(text).filter(|text| !text.is_empty()),
            None => None,
            other => panic!("{key} is not text: {other:?}"),
        }
    }

    /// The account, if the server and user are set.
    pub fn account(&self) -> Result<Account, String> {
        match (self.text(schema::SERVER), self.text(schema::USER)) {
            (Some(server), Some(user)) => Ok(Account {
                server,
                user,
                password_command: self.text(schema::PASSWORD_COMMAND),
            }),
            (server, user) => {
                let missing: Vec<_> = [("server", server.is_none()), ("user", user.is_none())]
                    .into_iter()
                    .filter_map(|(key, missing)| missing.then_some(key))
                    .collect();
                Err(format!(
                    "{} not set in [account], e.g.\n\n{ACCOUNT_EXAMPLE}",
                    missing.join(" and ")
                ))
            }
        }
    }

    /// The time between automatic syncs; `None` syncs only when asked to.
    pub fn sync_interval(&self) -> Option<Duration> {
        minutes(self.number(schema::SYNC_INTERVAL))
    }

    /// How long read items are kept; `None` keeps them.
    pub fn keep_read(&self) -> Option<Duration> {
        minutes(self.number(schema::KEEP_READ) * 24 * 60)
    }
}

/// A duration of `n` minutes, `None` for 0.
fn minutes(n: i64) -> Option<Duration> {
    u64::try_from(n)
        .ok()
        .filter(|&n| n > 0)
        .map(|n| Duration::from_secs(n * 60))
}

const ACCOUNT_EXAMPLE: &str = "\
[account]
server = \"https://cloud.example.org\"
user = \"name\"
password-command = \"pass show nextcloud/ren\"  # or the Secret Service";

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::schema::{Choice, Colour, Kind};

    /// One setting of each kind.
    pub(crate) static TABLE: &[Setting] = &[
        Setting {
            key: "a.switch",
            title: "Switch",
            description: "",
            category: schema::Category::Account,
            kind: Kind::Switch { default: true },
        },
        Setting {
            key: "a.choice",
            title: "Choice",
            description: "",
            category: schema::Category::Account,
            kind: Kind::Choice {
                options: &[
                    Choice {
                        value: "one",
                        title: "One",
                    },
                    Choice {
                        value: "two",
                        title: "Two",
                    },
                ],
                default: "one",
            },
        },
        Setting {
            key: "b.number",
            title: "Number",
            description: "",
            category: schema::Category::Sync,
            kind: Kind::Number {
                min: 1,
                max: 10,
                default: 5,
                unit: "",
            },
        },
        Setting {
            key: "b.text",
            title: "Text",
            description: "",
            category: schema::Category::Sync,
            kind: Kind::Text {
                default: None,
                check: Some(|text| {
                    if text.starts_with('x') {
                        Ok(())
                    } else {
                        Err("must start with x".to_owned())
                    }
                }),
            },
        },
        Setting {
            key: "b.colour",
            title: "Colour",
            description: "",
            category: schema::Category::Account,
            kind: Kind::Colour {
                default: Colour {
                    red: 0,
                    green: 0,
                    blue: 0,
                },
            },
        },
    ];

    fn parse(text: &str) -> Result<Loaded, Problems> {
        Settings::parse_with(text, TABLE)
    }

    /// The problems as `line:column: message`.
    fn problems(text: &str) -> Vec<String> {
        match parse(text) {
            Ok(loaded) => loaded.warnings,
            Err(Problems(problems)) => problems,
        }
        .iter()
        .map(Problem::to_string)
        .collect()
    }

    #[test]
    fn defaults() {
        let settings = parse("").unwrap().settings;
        assert_eq!(settings, Settings::defaults(TABLE));
        assert!(settings.bool("a.switch"));
        assert_eq!(settings.text("a.choice").as_deref(), Some("one"));
        assert_eq!(settings.number("b.number"), 5);
        assert_eq!(settings.text("b.text"), None);
        assert_eq!(
            settings.get("b.colour"),
            Some(Value::Colour(Colour {
                red: 0,
                green: 0,
                blue: 0
            }))
        );
        assert!(!settings.is_set("b.number"));
    }

    #[test]
    fn values() {
        let text = r##"
            # A comment
            [a]
            switch = false
            choice = "two"

            [b]
            number = 10  # the most
            text = "xyz"
            colour = "#FF8000"
        "##;
        let loaded = parse(text).unwrap();
        assert_eq!(loaded.warnings, []);
        let settings = loaded.settings;
        assert!(!settings.bool("a.switch"));
        assert_eq!(settings.text("a.choice").as_deref(), Some("two"));
        assert_eq!(settings.number("b.number"), 10);
        assert!(settings.is_set("b.number"));
        assert_eq!(settings.text("b.text").as_deref(), Some("xyz"));
        assert_eq!(
            settings.get("b.colour"),
            Some(Value::Colour(Colour {
                red: 0xff,
                green: 0x80,
                blue: 0
            }))
        );
    }

    #[test]
    fn dotted_keys_and_inline_tables() {
        let settings = parse("a.switch = false\nb = { number = 2 }\n")
            .unwrap()
            .settings;
        assert!(!settings.bool("a.switch"));
        assert_eq!(settings.number("b.number"), 2);
    }

    #[test]
    fn empty_text_is_unset() {
        let settings = parse("[b]\ntext = \"\"").unwrap().settings;
        assert_eq!(settings.text("b.text"), None);
    }

    #[test]
    fn unknown_keys_are_warnings() {
        let text = "top = 1\n[a]\nswitch = true\nswtich = false\n[c]\nx = 1\n";
        let loaded = parse(text).unwrap();
        assert!(loaded.settings.is_set("a.switch"));
        assert_eq!(
            problems(text),
            [
                "1:1: warning: unknown setting top",
                "4:1: warning: unknown setting a.swtich",
                "5:2: warning: unknown table [c]",
            ]
        );
    }

    #[test]
    fn invalid_values_are_errors() {
        let text = "[a]\nswitch = 1\nchoice = \"three\"\n\
                    [b]\nnumber = 11\ntext = \"abc\"\ncolour = \"red\"\nnope = 1\n";
        assert!(parse(text).is_err());
        assert_eq!(
            problems(text),
            [
                "2:10: a.switch must be true or false",
                "3:10: a.choice must be one of \"one\", \"two\"",
                "5:10: b.number must be between 1 and 10",
                "6:8: b.text must start with x",
                "7:10: b.colour must be a colour as a string \"#rrggbb\"",
                "8:1: warning: unknown setting b.nope",
            ]
        );
        assert_eq!(
            problems("[b]\nnumber = 1.5\ntext = 3"),
            [
                "2:10: b.number must be a whole number",
                "3:8: b.text must be a string"
            ]
        );
        assert_eq!(problems("a = 1"), ["1:5: a must be a table: [a]"]);
        assert_eq!(
            problems("[a.switch]\nx = 1"),
            ["1:1: a.switch must be a value, not a table"]
        );
    }

    #[test]
    fn syntax_errors() {
        assert_eq!(
            problems("[a]\nswitch = tru\n"),
            ["2:10: invalid boolean, expected `true`"]
        );
        let err = parse("[a\n").unwrap_err();
        assert_eq!(err.0.len(), 1);
        assert_eq!(err.0[0].position.map(|p| p.line), Some(1));
        // A key twice.
        assert_eq!(
            parse("[a]\nswitch = true\nswitch = false").unwrap_err().0[0]
                .position
                .map(|p| p.line),
            Some(3)
        );
    }

    #[test]
    fn positions_count_characters() {
        let text = "a = \"ü\"\nbä = 1";
        assert_eq!(Position::of(text, 0), Position { line: 1, column: 1 });
        assert_eq!(Position::of(text, 9), Position { line: 2, column: 1 });
        assert_eq!(Position::of(text, 12), Position { line: 2, column: 3 });
        // Inside a character, and past the end.
        assert_eq!(Position::of(text, 6), Position { line: 1, column: 6 });
        assert_eq!(Position::of(text, 100), Position { line: 2, column: 7 });
    }

    #[test]
    fn app_settings() {
        let text = r#"
            [account]
            server = "https://cloud.example.org"
            user = "user"
            password-command = "pass show nextcloud/ren"

            [sync]
            interval-minutes = 0
            keep-read-days = 2
        "#;
        let settings = Settings::parse(text).unwrap().settings;
        assert_eq!(
            settings.account(),
            Ok(Account {
                server: "https://cloud.example.org".to_owned(),
                user: "user".to_owned(),
                password_command: Some("pass show nextcloud/ren".to_owned()),
            })
        );
        assert_eq!(settings.sync_interval(), None);
        assert_eq!(
            settings.keep_read(),
            Some(Duration::from_secs(2 * 24 * 60 * 60))
        );

        let defaults = Settings::default();
        assert_eq!(defaults.sync_interval(), Some(Duration::from_secs(15 * 60)));
        assert_eq!(
            defaults.keep_read(),
            Some(Duration::from_secs(30 * 24 * 60 * 60))
        );
    }

    #[test]
    fn account_errors() {
        let err = Settings::default().account().unwrap_err();
        assert!(
            err.starts_with("server and user not set in [account]"),
            "{err}"
        );
        let settings = Settings::parse("[account]\nserver = \"https://x\"")
            .unwrap()
            .settings;
        assert!(settings.account().unwrap_err().starts_with("user not set"));
        let http = "[account]\nserver = \"http://cloud.example.org\"\nuser = \"u\"";
        let err = Settings::parse(http).unwrap_err().to_string();
        assert_eq!(
            err,
            "2:10: account.server must be an https:// URL: http://cloud.example.org"
        );
    }
}
