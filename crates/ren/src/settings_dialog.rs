// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings dialog without Slint: which settings it shows for a search
//! or a category, what their controls show, and what an edit does. The
//! rows are generated from the setting descriptors; only the account page
//! is made by hand (`ui/settings.rs`).

use std::collections::HashMap;

use ren_settings::schema::{self, Category, Colour, Kind, Setting};
use ren_settings::{Settings, Value};

/// Choices shown as pictures (`Control::Pictures`).
const PICTURES: &[&str] = &[schema::ARRANGEMENT];

/// Settings the account page shows itself: the login sets them.
const ACCOUNT_PAGE: &[&str] = &[schema::SERVER, schema::USER];

/// The dialog's state: the search, the category shown, and what the user
/// typed that isn't a valid value.
#[derive(Debug)]
pub struct Dialog {
    table: &'static [Setting],
    query: String,
    category: Category,
    /// While searching: the category the results are narrowed to; `None`
    /// shows the results of all categories.
    scope: Option<Category>,
    /// Per key: the text typed, and what's wrong with it.
    invalid: HashMap<&'static str, (String, String)>,
}

/// A row of the dialog: one setting.
#[derive(Debug, Clone)]
pub struct Row {
    pub setting: &'static Setting,
    /// The category's title above the first row of each category, while
    /// searching.
    pub heading: Option<&'static str>,
    pub control: Control,
    /// The text of a text field: number, text, colour or list (one item
    /// per line).
    pub input: String,
    /// The file sets it: it can be reset to its default.
    pub modified: bool,
    /// The switch it requires is on (`Setting::requires`); otherwise the
    /// row is disabled and says which one.
    pub enabled: bool,
    pub requires: Option<&'static str>,
    /// What's wrong with the text typed, which is kept until it is fixed.
    pub error: Option<String>,
}

/// The control of a row.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Switch(bool),
    Choice {
        options: Vec<&'static str>,
        index: usize,
    },
    /// A choice shown as pictures of its options: the arrangement.
    Pictures {
        options: Vec<&'static str>,
        index: usize,
    },
    Number {
        unit: &'static str,
    },
    /// The placeholder says what an empty field means.
    Text {
        placeholder: &'static str,
    },
    /// The colour in effect; `None` while the app's own colours are used.
    Colour(Option<Colour>),
    List,
}

/// What an edit in the dialog does to the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Set(Value),
    /// Back to the default: the key is removed.
    Reset,
    /// The text isn't a valid value; it is kept with its error, and the
    /// file isn't changed.
    Invalid,
}

impl Dialog {
    pub fn new() -> Dialog {
        Dialog::with_table(schema::SETTINGS)
    }

    pub fn with_table(table: &'static [Setting]) -> Dialog {
        Dialog {
            table,
            query: String::new(),
            category: table.first().map_or(Category::Account, |s| s.category),
            scope: None,
            invalid: HashMap::new(),
        }
    }

    /// The categories in the order of the descriptors.
    pub fn categories(&self) -> Vec<Category> {
        let mut categories: Vec<Category> = Vec::new();
        for setting in self.table {
            if !categories.contains(&setting.category) {
                categories.push(setting.category);
            }
        }
        categories
    }

    /// Shows a category, or while searching, narrows the results to it;
    /// the category narrowed to already widens them to all again.
    pub fn select(&mut self, category: Category) {
        if self.searching() {
            self.scope = (self.scope != Some(category)).then_some(category);
        }
        self.category = category;
    }

    /// Searches all categories; a new search shows the results of all of
    /// them.
    pub fn search(&mut self, query: &str) {
        let was_searching = self.searching();
        query.clone_into(&mut self.query);
        if !was_searching || !self.searching() {
            self.scope = None;
        }
    }

    /// The category highlighted in the list: the one shown, or while
    /// searching, the one the results are narrowed to.
    pub fn current(&self) -> Option<Category> {
        if self.searching() {
            self.scope
        } else {
            Some(self.category)
        }
    }

    /// While searching, the number of results per category, in the order
    /// of [`categories`](Self::categories); `None` otherwise.
    pub fn counts(&self) -> Option<Vec<usize>> {
        if !self.searching() {
            return None;
        }
        Some(
            self.categories()
                .into_iter()
                .map(|category| {
                    self.table
                        .iter()
                        .filter(|s| s.category == category && s.matches(&self.query))
                        .count()
                })
                .collect(),
        )
    }

    /// Whether a search is shown instead of a category.
    pub fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// Whether the account page is shown: its category, not searching.
    pub fn account_page(&self) -> bool {
        !self.searching() && self.category == Category::Account
    }

    /// The rows for the search, or else the category shown.
    pub fn rows(&self, settings: &Settings) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut last = None;
        for setting in self.table {
            let shown = if self.searching() {
                setting.matches(&self.query)
                    && self.scope.is_none_or(|scope| scope == setting.category)
            } else {
                setting.category == self.category && !ACCOUNT_PAGE.contains(&setting.key)
            };
            if !shown {
                continue;
            }
            let heading =
                (self.searching() && self.scope.is_none() && last != Some(setting.category))
                    .then(|| setting.category.title());
            last = Some(setting.category);
            rows.push(self.row(setting, settings, heading));
        }
        rows
    }

    fn row(
        &self,
        setting: &'static Setting,
        settings: &Settings,
        heading: Option<&'static str>,
    ) -> Row {
        let value = settings.get(setting.key);
        let control = match (&setting.kind, &value) {
            (Kind::Switch { .. }, Some(Value::Bool(on))) => Control::Switch(*on),
            (Kind::Choice { options, .. }, Some(Value::Text(text))) => {
                let titles = options.iter().map(|option| option.title).collect();
                let index = options
                    .iter()
                    .position(|option| option.value == text)
                    .unwrap_or(0);
                if PICTURES.contains(&setting.key) {
                    Control::Pictures {
                        options: titles,
                        index,
                    }
                } else {
                    Control::Choice {
                        options: titles,
                        index,
                    }
                }
            }
            (Kind::Number { unit, .. }, _) => Control::Number { unit },
            (Kind::Text { .. }, _) => Control::Text {
                placeholder: "Not set",
            },
            (Kind::Colour { .. }, Some(Value::Colour(colour))) => Control::Colour(Some(*colour)),
            (Kind::Colour { .. }, _) => Control::Colour(None),
            (Kind::List { .. }, _) => Control::List,
            (Kind::Switch { .. } | Kind::Choice { .. }, _) => {
                unreachable!("switches and choices have defaults")
            }
        };
        let (input, error) = match self.invalid.get(setting.key) {
            Some((input, error)) => (input.clone(), Some(error.clone())),
            None => (
                value.map(|value| value.to_input()).unwrap_or_default(),
                None,
            ),
        };
        Row {
            setting,
            heading,
            control,
            input,
            modified: settings.is_set(setting.key),
            enabled: settings.effective(setting.key),
            requires: setting
                .requires
                .and_then(|key| schema::find(self.table, key))
                .map(|required| required.title),
            error,
        }
    }

    /// The text typed into a row's field was accepted.
    pub fn input(&mut self, key: &str, text: &str) -> Edit {
        let Some(setting) = schema::find(self.table, key) else {
            return Edit::Invalid;
        };
        match setting.parse_input(text) {
            Ok(value) => {
                self.invalid.remove(setting.key);
                value.map_or(Edit::Reset, Edit::Set)
            }
            Err(message) => {
                self.invalid.insert(setting.key, (text.to_owned(), message));
                Edit::Invalid
            }
        }
    }

    /// A switch was flipped.
    pub fn switch(&mut self, key: &str, on: bool) -> Edit {
        self.input(key, if on { "true" } else { "false" })
    }

    /// An option of a choice was picked.
    pub fn choose(&mut self, key: &str, index: usize) -> Edit {
        match schema::find(self.table, key).map(|setting| &setting.kind) {
            Some(Kind::Choice { options, .. }) if index < options.len() => {
                Edit::Set(Value::Text(options[index].value.to_owned()))
            }
            _ => Edit::Invalid,
        }
    }

    /// A colour was picked.
    pub fn pick_colour(&mut self, key: &str, colour: Colour) -> Edit {
        match schema::find(self.table, key).map(|setting| &setting.kind) {
            Some(Kind::Colour { .. }) => {
                self.invalid.retain(|invalid, _| *invalid != key);
                Edit::Set(Value::Colour(colour))
            }
            _ => Edit::Invalid,
        }
    }

    /// A row was reset; what was typed into it is forgotten.
    pub fn reset(&mut self, key: &str) -> Edit {
        self.invalid.retain(|invalid, _| *invalid != key);
        Edit::Reset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(text: &str) -> Settings {
        Settings::parse(text).unwrap().settings
    }

    fn keys(rows: &[Row]) -> Vec<&'static str> {
        rows.iter().map(|row| row.setting.key).collect()
    }

    #[test]
    fn categories_in_order() {
        let dialog = Dialog::new();
        assert_eq!(
            dialog.categories(),
            [
                Category::Account,
                Category::Sync,
                Category::List,
                Category::Articles,
                Category::Pages,
                Category::Window
            ]
        );
        assert_eq!(dialog.current(), Some(Category::Account));
        assert!(dialog.account_page());
    }

    #[test]
    fn rows_of_a_category() {
        let mut dialog = Dialog::new();
        // The account page shows the server and user itself.
        assert_eq!(
            keys(&dialog.rows(&Settings::default())),
            [schema::PASSWORD_COMMAND]
        );
        dialog.select(Category::Sync);
        assert!(!dialog.account_page());
        let rows = dialog.rows(&settings("[sync]\ninterval-minutes = 5\n"));
        assert_eq!(
            keys(&rows),
            [schema::SYNC_INTERVAL, schema::KEEP_READ, schema::FEED_ICONS]
        );
        assert_eq!(rows[0].control, Control::Number { unit: "minutes" });
        assert_eq!(rows[0].input, "5");
        assert!(rows[0].modified);
        assert_eq!(rows[1].input, "30");
        assert!(!rows[1].modified);
        assert_eq!(rows[2].control, Control::Switch(true));
        assert!(rows.iter().all(|row| row.heading.is_none()));
    }

    #[test]
    fn controls() {
        let mut dialog = Dialog::new();
        dialog.select(Category::List);
        let rows = dialog.rows(&settings(
            "[list]\narrangement = \"above\"\nunread-colour = \"#00ff00\"\n",
        ));
        assert_eq!(
            rows[0].control,
            Control::Pictures {
                options: vec!["Beside the article", "Above the article"],
                index: 1
            }
        );
        assert_eq!(rows[1].control, Control::Colour(None));
        assert_eq!(rows[1].input, "");
        assert_eq!(
            rows[2].control,
            Control::Colour(Some(Colour {
                red: 0,
                green: 0xff,
                blue: 0
            }))
        );
        assert_eq!(rows[2].input, "#00ff00");

        dialog.select(Category::Pages);
        let rows = dialog.rows(&Settings::default());
        let lists = rows
            .iter()
            .find(|row| row.setting.key == schema::FILTER_LISTS)
            .unwrap();
        assert_eq!(lists.control, Control::List);
        assert_eq!(lists.input.lines().count(), 5);
    }

    #[test]
    fn search_across_categories() {
        let mut dialog = Dialog::new();
        dialog.select(Category::Window);
        dialog.search("icon");
        assert!(dialog.searching());
        assert!(!dialog.account_page());
        let rows = dialog.rows(&Settings::default());
        assert_eq!(
            keys(&rows),
            [
                schema::FEED_ICONS,
                schema::TRAY_ICON,
                schema::TRAY_ICON_STYLE,
                schema::CLOSE_TO_TRAY
            ]
        );
        let headings: Vec<_> = rows.iter().map(|row| row.heading).collect();
        assert_eq!(headings, [Some("Sync"), Some("Window"), None, None]);
        // Searching finds the account's settings too.
        dialog.search("server");
        assert!(keys(&dialog.rows(&Settings::default())).contains(&schema::SERVER));
        dialog.search("icon");
        assert_eq!(dialog.current(), None);
        assert_eq!(dialog.counts(), Some(vec![0, 1, 0, 0, 0, 3]));
        // A category narrows the results, without headings, and keeps the
        // search.
        dialog.select(Category::Window);
        assert!(dialog.searching());
        assert_eq!(dialog.current(), Some(Category::Window));
        let rows = dialog.rows(&Settings::default());
        assert_eq!(
            keys(&rows),
            [
                schema::TRAY_ICON,
                schema::TRAY_ICON_STYLE,
                schema::CLOSE_TO_TRAY
            ]
        );
        assert!(rows.iter().all(|row| row.heading.is_none()));
        // Typing on stays in it.
        dialog.search("icon tray");
        assert_eq!(dialog.current(), Some(Category::Window));
        // Again: all categories.
        dialog.select(Category::Window);
        assert_eq!(dialog.current(), None);
        assert_eq!(dialog.rows(&Settings::default()).len(), 3);
        // The search ends: the category chosen last is shown.
        dialog.select(Category::Sync);
        dialog.search("  ");
        assert!(!dialog.searching());
        assert_eq!(dialog.current(), Some(Category::Sync));
        assert_eq!(dialog.counts(), None);
        // A new search starts with all categories.
        dialog.search("i");
        assert_eq!(dialog.current(), None);
    }

    #[test]
    fn settings_that_need_a_switch() {
        let mut dialog = Dialog::new();
        dialog.select(Category::Window);
        let rows = dialog.rows(&Settings::default());
        assert!(rows.iter().all(|row| row.enabled));
        assert_eq!(rows[1].requires, Some("Icon in the system tray"));
        let rows = dialog.rows(&settings("[window]\ntray-icon = false\n"));
        assert!(rows[0].enabled);
        assert!(!rows[1].enabled);
        dialog.select(Category::Pages);
        let off = settings("[pages]\nblock-content = false\n");
        let disabled: Vec<_> = dialog
            .rows(&off)
            .into_iter()
            .filter(|row| !row.enabled)
            .map(|row| row.setting.key)
            .collect();
        assert_eq!(disabled, [schema::FILTER_LISTS, schema::UNBLOCKED_SITES]);
    }

    #[test]
    fn edits() {
        let mut dialog = Dialog::new();
        dialog.select(Category::Sync);
        assert_eq!(
            dialog.input(schema::SYNC_INTERVAL, "20"),
            Edit::Set(Value::Number(20))
        );
        assert_eq!(dialog.input(schema::SYNC_INTERVAL, "-1"), Edit::Invalid);
        // The text typed stays, with its error, until it is fixed.
        let row = dialog.rows(&Settings::default())[0].clone();
        assert_eq!(row.input, "-1");
        assert_eq!(row.error.as_deref(), Some("must be between 0 and 1440"));
        assert_eq!(
            dialog.input(schema::SYNC_INTERVAL, "15"),
            Edit::Set(Value::Number(15))
        );
        let row = dialog.rows(&Settings::default())[0].clone();
        assert_eq!((row.input.as_str(), row.error), ("15", None));

        assert_eq!(
            dialog.switch(schema::FEED_ICONS, false),
            Edit::Set(Value::Bool(false))
        );
        assert_eq!(
            dialog.choose(schema::ARRANGEMENT, 1),
            Edit::Set(Value::Text("above".to_owned()))
        );
        assert_eq!(dialog.choose(schema::ARRANGEMENT, 2), Edit::Invalid);
        assert_eq!(dialog.choose(schema::FEED_ICONS, 0), Edit::Invalid);
        let orange = Colour {
            red: 0xff,
            green: 0x80,
            blue: 0,
        };
        assert_eq!(dialog.input(schema::NEW_COLOUR, "orange"), Edit::Invalid);
        assert_eq!(
            dialog.pick_colour(schema::NEW_COLOUR, orange),
            Edit::Set(Value::Colour(orange))
        );
        // What was typed before is forgotten.
        dialog.select(Category::List);
        assert_eq!(dialog.rows(&Settings::default())[1].error, None);
        dialog.select(Category::Sync);
        assert_eq!(
            dialog.pick_colour(schema::FEED_ICONS, orange),
            Edit::Invalid
        );
        // Empty text unsets.
        assert_eq!(dialog.input(schema::PASSWORD_COMMAND, " "), Edit::Reset);
        assert_eq!(dialog.input(schema::NEW_COLOUR, ""), Edit::Reset);
        assert_eq!(dialog.input("no.such-key", "1"), Edit::Invalid);

        // Resetting forgets what was typed.
        assert_eq!(dialog.input(schema::KEEP_READ, "x"), Edit::Invalid);
        assert_eq!(dialog.reset(schema::KEEP_READ), Edit::Reset);
        assert_eq!(dialog.rows(&Settings::default())[1].error, None);
    }
}
