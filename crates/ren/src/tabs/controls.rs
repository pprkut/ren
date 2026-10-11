// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The drop-downs (`<select>`) and context menus of pages, which Servo
//! asks the window to show: what they show and what the window does with
//! the reader's choice. Without Slint types, so it is tested without a
//! window. The helper runs web content, so what it sends is limited
//! again here.

use ren_tabs::{MenuAction, MenuEntry, SelectEntry, TabId};

/// The most entries shown of a drop-down.
const MAX_ENTRIES: usize = 2_000;

/// A row of a drop-down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionRow {
    pub label: String,
    pub enabled: bool,
    /// The heading of a group of options.
    pub heading: bool,
    pub selected: bool,
}

/// A drop-down the window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Select {
    pub tab: TabId,
    pub control: u32,
    entries: Vec<SelectEntry>,
    selected: Vec<usize>,
    multiple: bool,
}

impl Select {
    pub fn new(
        tab: TabId,
        control: u32,
        mut entries: Vec<SelectEntry>,
        selected: Vec<usize>,
        multiple: bool,
    ) -> Self {
        entries.truncate(MAX_ENTRIES);
        Self {
            tab,
            control,
            entries,
            selected,
            multiple,
        }
    }

    pub fn rows(&self) -> Vec<OptionRow> {
        self.entries
            .iter()
            .map(|entry| match entry {
                SelectEntry::Option { id, label, enabled } => OptionRow {
                    label: label.clone(),
                    enabled: *enabled,
                    heading: false,
                    selected: self.selected.contains(id),
                },
                SelectEntry::Group(label) => OptionRow {
                    label: label.clone(),
                    enabled: false,
                    heading: true,
                    selected: false,
                },
            })
            .collect()
    }

    /// The options selected once the option in `row` was chosen: only it,
    /// or for a drop-down of several, the others with it added or taken
    /// away. `None` for a heading or an option that is disabled.
    pub fn choose(&self, row: usize) -> Option<Vec<usize>> {
        let Some(SelectEntry::Option {
            id, enabled: true, ..
        }) = self.entries.get(row)
        else {
            return None;
        };
        if !self.multiple {
            return Some(vec![*id]);
        }
        let mut selected = self.selected.clone();
        match selected.iter().position(|s| s == id) {
            Some(index) => {
                selected.remove(index);
            }
            None => selected.push(*id),
        }
        Some(selected)
    }
}

/// A context menu the window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    pub tab: TabId,
    pub control: u32,
    entries: Vec<MenuEntry>,
    pub link: Option<String>,
    pub image: Option<String>,
}

/// What the window does for an entry of a context menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Servo does it in the page.
    Page(MenuAction),
    /// Opens a link in a new tab.
    Open(String),
    /// Opens a link in the system browser.
    Browser(String),
    /// Puts a link on the clipboard.
    Copy(String),
}

impl Menu {
    pub fn new(
        tab: TabId,
        control: u32,
        entries: Vec<MenuEntry>,
        link: Option<String>,
        image: Option<String>,
    ) -> Self {
        Self {
            tab,
            control,
            entries,
            link: link.filter(|link| !link.is_empty()),
            image: image.filter(|image| !image.is_empty()),
        }
    }

    /// Whether Servo offers `action` and it applies.
    pub fn enabled(&self, action: MenuAction) -> bool {
        self.entries.iter().any(|entry| {
            *entry
                == MenuEntry::Action {
                    action,
                    enabled: true,
                }
        })
    }

    /// What to do for an entry of the window's menu, by its name, and the
    /// answer for Servo, which dismisses its menu for links and images.
    pub fn choose(&self, name: &str) -> Option<(Choice, MenuAction)> {
        let link = || self.link.clone();
        let image = || self.image.clone();
        let page = |action: MenuAction| {
            self.enabled(action)
                .then_some((Choice::Page(action), action))
        };
        match name {
            "open-link" => link().map(|url| (Choice::Open(url), MenuAction::OpenLink)),
            "browser-link" => link().map(|url| (Choice::Browser(url), MenuAction::OpenLink)),
            "copy-link" => link().map(|url| (Choice::Copy(url), MenuAction::CopyLink)),
            "open-image" => image().map(|url| (Choice::Open(url), MenuAction::OpenImage)),
            "copy-image" => image().map(|url| (Choice::Copy(url), MenuAction::CopyImageLink)),
            "cut" => page(MenuAction::Cut),
            "copy" => page(MenuAction::Copy),
            "paste" => page(MenuAction::Paste),
            "select-all" => page(MenuAction::SelectAll),
            "back" => page(MenuAction::Back),
            "forward" => page(MenuAction::Forward),
            "reload" => page(MenuAction::Reload),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: usize, label: &str, enabled: bool) -> SelectEntry {
        SelectEntry::Option {
            id,
            label: label.to_owned(),
            enabled,
        }
    }

    fn entries() -> Vec<SelectEntry> {
        vec![
            option(0, "None", true),
            SelectEntry::Group("Europe".to_owned()),
            option(1, "Austria", true),
            option(2, "Belgium", false),
            option(3, "Croatia", true),
        ]
    }

    #[test]
    fn one_option() {
        let select = Select::new(1, 7, entries(), vec![1], false);
        let rows = select.rows();
        assert_eq!(rows.len(), 5);
        assert_eq!(
            rows[1],
            OptionRow {
                label: "Europe".to_owned(),
                enabled: false,
                heading: true,
                selected: false
            }
        );
        assert!(rows[2].selected && rows[2].enabled);
        assert!(!rows[3].enabled);
        assert_eq!(select.choose(4), Some(vec![3]));
        assert_eq!(select.choose(1), None);
        assert_eq!(select.choose(3), None);
        assert_eq!(select.choose(9), None);
    }

    #[test]
    fn several_options() {
        let select = Select::new(1, 7, entries(), vec![1, 3], true);
        assert_eq!(select.choose(0), Some(vec![1, 3, 0]));
        assert_eq!(select.choose(2), Some(vec![3]));
    }

    #[test]
    fn many_options() {
        let entries = (0..MAX_ENTRIES + 5)
            .map(|id| option(id, "o", true))
            .collect();
        assert_eq!(
            Select::new(1, 1, entries, Vec::new(), false).rows().len(),
            MAX_ENTRIES
        );
    }

    #[test]
    fn menus() {
        let menu = Menu::new(
            2,
            8,
            vec![
                MenuEntry::Action {
                    action: MenuAction::OpenLink,
                    enabled: true,
                },
                MenuEntry::Separator,
                MenuEntry::Action {
                    action: MenuAction::Copy,
                    enabled: false,
                },
                MenuEntry::Action {
                    action: MenuAction::Reload,
                    enabled: true,
                },
            ],
            Some("https://example.org/".to_owned()),
            Some(String::new()),
        );
        assert_eq!(menu.image, None);
        assert!(menu.enabled(MenuAction::Reload));
        assert!(!menu.enabled(MenuAction::Copy));
        assert!(!menu.enabled(MenuAction::Back));
        let url = "https://example.org/".to_owned();
        assert_eq!(
            menu.choose("open-link"),
            Some((Choice::Open(url.clone()), MenuAction::OpenLink))
        );
        assert_eq!(
            menu.choose("browser-link"),
            Some((Choice::Browser(url.clone()), MenuAction::OpenLink))
        );
        assert_eq!(
            menu.choose("copy-link"),
            Some((Choice::Copy(url), MenuAction::CopyLink))
        );
        assert_eq!(
            menu.choose("reload"),
            Some((Choice::Page(MenuAction::Reload), MenuAction::Reload))
        );
        assert_eq!(menu.choose("copy"), None);
        assert_eq!(menu.choose("open-image"), None);
        assert_eq!(menu.choose("format-disk"), None);
    }
}
