// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Changing single settings in the text of a settings file. The rest of the
//! text stays as it is: comments, order, formatting. The file only holds
//! overrides, so setting the default removes the key.

use toml_edit::{Decor, DocumentMut, Item, TableLike};

use crate::error::Error;
use crate::schema::{self, Setting, Value};
use crate::settings::{Position, Problem, Problems, Severity};

/// `text` with the setting `key` set to `value`.
///
/// Panics if `table` has no setting `key`: keys are constants of the code.
pub fn set(text: &str, table: &[Setting], key: &str, value: &Value) -> Result<String, Error> {
    let setting = find(table, key);
    setting
        .check(value)
        .map_err(|message| Error::Invalid(format!("{key} {message}")))?;
    if setting.default_value().as_ref() == Some(value) {
        return reset(text, table, key);
    }
    let mut document = parse(text)?;
    let empty = document.is_empty();
    let section = document
        .as_table_mut()
        .entry(setting.table())
        .or_insert_with(|| {
            let mut section = toml_edit::Table::new();
            if empty {
                // No blank line at the start of the file.
                section.decor_mut().set_prefix("");
            }
            Item::Table(section)
        });
    let Some(section) = section.as_table_like_mut() else {
        return Err(Error::Invalid(format!(
            "{} must be a table: [{}]",
            setting.table(),
            setting.table()
        )));
    };
    let mut new = value.to_toml();
    match section.get_mut(setting.name()) {
        // The comment after the old value stays.
        Some(Item::Value(old)) => {
            *new.decor_mut() = old.decor().clone();
            *old = new;
        }
        _ => {
            section.insert(setting.name(), Item::Value(new));
        }
    }
    Ok(document.to_string())
}

/// `text` without the setting `key`, so that it has its default. A table
/// left empty goes too, unless it has a comment.
///
/// Panics if `table` has no setting `key`.
pub fn reset(text: &str, table: &[Setting], key: &str) -> Result<String, Error> {
    let setting = find(table, key);
    let mut document = parse(text)?;
    let Some(item) = document.get_mut(setting.table()) else {
        return Ok(text.to_owned());
    };
    let Some(section) = item.as_table_like_mut() else {
        return Ok(text.to_owned());
    };
    if section.remove(setting.name()).is_none() {
        return Ok(text.to_owned());
    }
    let commented = item
        .as_table()
        .is_some_and(|section| has_comment(section.decor()));
    if item.as_table_like().is_some_and(TableLike::is_empty) && !commented {
        document.remove(setting.table());
    }
    Ok(document.to_string())
}

fn find<'a>(table: &'a [Setting], key: &str) -> &'a Setting {
    schema::find(table, key).unwrap_or_else(|| panic!("no setting {key}"))
}

fn parse(text: &str) -> Result<DocumentMut, Error> {
    text.parse::<DocumentMut>().map_err(|err| {
        Error::Problems(Problems(vec![Problem {
            severity: Severity::Error,
            position: err.span().map(|span| Position::of(text, span.start)),
            message: err.message().to_owned(),
        }]))
    })
}

/// Whether there's a comment before a table's header or after it.
fn has_comment(decor: &Decor) -> bool {
    [decor.prefix(), decor.suffix()]
        .into_iter()
        .flatten()
        .any(|raw| raw.as_str().is_some_and(|s| s.contains('#')))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Colour, SETTINGS};
    use crate::settings::{Settings, tests::TABLE};

    fn set(text: &str, key: &str, value: Value) -> String {
        super::set(text, TABLE, key, &value).unwrap()
    }

    fn reset(text: &str, key: &str) -> String {
        super::reset(text, TABLE, key).unwrap()
    }

    #[test]
    fn set_keeps_comments_and_order() {
        let text = "\
# My settings

[b]
# Not too many
number = 3  # for now
text = 'x'

[a]
choice = \"two\"
";
        assert_eq!(
            set(text, "b.number", Value::Number(7)),
            "\
# My settings

[b]
# Not too many
number = 7  # for now
text = 'x'

[a]
choice = \"two\"
"
        );
        assert_eq!(
            set(text, "a.switch", Value::Bool(false)),
            "\
# My settings

[b]
# Not too many
number = 3  # for now
text = 'x'

[a]
choice = \"two\"
switch = false
"
        );
    }

    #[test]
    fn set_adds_a_table() {
        assert_eq!(set("", "b.number", Value::Number(7)), "[b]\nnumber = 7\n");
        assert_eq!(
            set(
                "[a]\nswitch = false\n",
                "b.text",
                Value::Text("x\"y".into())
            ),
            "[a]\nswitch = false\n\n[b]\ntext = 'x\"y'\n"
        );
        let colour = Value::Colour(Colour {
            red: 1,
            green: 2,
            blue: 255,
        });
        assert_eq!(set("", "b.colour", colour), "[b]\ncolour = \"#0102ff\"\n");
    }

    #[test]
    fn set_dotted_and_inline() {
        assert_eq!(
            set("a.switch = true\n", "a.switch", Value::Bool(false)),
            "a.switch = false\n"
        );
        assert_eq!(
            set("b = { number = 2 }\n", "b.number", Value::Number(3)),
            "b = { number = 3 }\n"
        );
    }

    #[test]
    fn set_default_removes_the_key() {
        let text = "[b]\nnumber = 3\ntext = \"x\"\n";
        assert_eq!(
            set(text, "b.number", Value::Number(5)),
            "[b]\ntext = \"x\"\n"
        );
        assert_eq!(set("", "b.number", Value::Number(5)), "");
    }

    #[test]
    fn set_checks_the_value() {
        let err = |key, value| super::set("", TABLE, key, &value).unwrap_err().to_string();
        assert_eq!(
            err("b.number", Value::Number(0)),
            "b.number must be between 1 and 10"
        );
        assert_eq!(
            err("b.number", Value::Bool(true)),
            "b.number must be a whole number"
        );
        assert_eq!(
            err("a.choice", Value::Text("three".into())),
            "a.choice must be one of \"one\", \"two\""
        );
        assert_eq!(
            err("b.text", Value::Text("y".into())),
            "b.text must start with x"
        );
        assert!(super::set("[b", TABLE, "b.number", &Value::Number(2)).is_err());
        assert_eq!(
            super::set("b = 1", TABLE, "b.number", &Value::Number(2))
                .unwrap_err()
                .to_string(),
            "b must be a table: [b]"
        );
    }

    #[test]
    fn reset_removes_the_key_and_an_empty_table() {
        let text = "[a]\nswitch = false\n\n[b]\nnumber = 3\n";
        assert_eq!(reset(text, "b.number"), "[a]\nswitch = false\n");
        assert_eq!(reset(text, "a.switch"), "\n[b]\nnumber = 3\n");
        assert_eq!(reset(text, "b.text"), text);
        assert_eq!(reset("", "b.text"), "");
        // The comment about the table stays, and so the table.
        let text = "# Numbers\n[b]\nnumber = 3\n";
        assert_eq!(reset(text, "b.number"), "# Numbers\n[b]\n");
        assert_eq!(
            reset("[b]\nnumber = 3\ntext = 'x'\n", "b.number"),
            "[b]\ntext = 'x'\n"
        );
    }

    #[test]
    fn round_trip() {
        let text = "\
# ren settings

[account]
server = \"https://cloud.example.org\"  # at home
user = \"user\"

# Sync often.
[sync]
interval-minutes = 5
";
        let changed =
            super::set(text, SETTINGS, schema::SYNC_INTERVAL, &Value::Number(10)).unwrap();
        let settings = Settings::parse(&changed).unwrap().settings;
        assert_eq!(settings.number(schema::SYNC_INTERVAL), 10);
        let back =
            super::set(&changed, SETTINGS, schema::SYNC_INTERVAL, &Value::Number(5)).unwrap();
        assert_eq!(back, text);
        let reset = super::reset(text, SETTINGS, schema::SYNC_INTERVAL).unwrap();
        assert_eq!(
            reset,
            "\
# ren settings

[account]
server = \"https://cloud.example.org\"  # at home
user = \"user\"

# Sync often.
[sync]
"
        );
    }
}
