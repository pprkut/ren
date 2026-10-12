// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The drop-downs (`<select>`) and context menus of pages, which Servo
//! asks the window to show: what they show and what the window does with
//! the reader's choice. Without Slint types, so it is tested without a
//! window. The helper runs web content, so what it sends is limited
//! again here.

use ren_tabs::{Area, MenuAction, MenuEntry, SelectEntry, TabId};

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

/// Where a drop-down's element is, in physical pixels relative to the
/// page area. Servo 0.7 reports it in CSS pixels relative to the document
/// (a FIXME in its script crate), so it is off by the page's scroll and the
/// scale factor. The element is under the click that opened it: where the
/// reported box doesn't contain the click, the box is moved there, centred
/// on it. Without a click (opened by keyboard), the box as reported.
pub fn select_area(css: Area, click: Option<(f32, f32)>, scale: f32) -> Area {
    let px = |value: i32| (value as f32 * scale).round() as i32;
    let (x, y, width, height) = (px(css.x), px(css.y), px(css.width), px(css.height));
    let place = |start: i32, len: i32, click: f32| {
        let click = click.round() as i32;
        if (start..=start + len).contains(&click) {
            start
        } else {
            click - len / 2
        }
    };
    let (x, y) = match click {
        Some((cx, cy)) => (place(x, width, cx), place(y, height, cy)),
        None => (x, y),
    };
    Area {
        x,
        y,
        width,
        height,
    }
}

/// Where a context menu opens, in physical pixels relative to the page
/// area: at the click that asked for it. Servo 0.7 reports the click's
/// point in CSS pixels as if they were device pixels (and relative to its
/// frame), so it is used, scaled, only without a click (the menu key).
pub fn menu_point(css: Area, click: Option<(f32, f32)>, scale: f32) -> (f32, f32) {
    click.unwrap_or((css.x as f32 * scale, css.y as f32 * scale))
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
    fn drop_downs_under_the_click() {
        let css = Area {
            x: 100,
            y: 900,
            width: 150,
            height: 20,
        };
        // Scrolled by 800 CSS pixels: the box is moved to the click.
        assert_eq!(
            select_area(css, Some((170.0, 108.0)), 1.0),
            Area {
                x: 100,
                y: 98,
                width: 150,
                height: 20
            }
        );
        // At a scale of 2, not scrolled: the box, in physical pixels.
        assert_eq!(
            select_area(css, Some((300.0, 1810.0)), 2.0),
            Area {
                x: 200,
                y: 1800,
                width: 300,
                height: 40
            }
        );
        // Opened by keyboard: as reported.
        assert_eq!(
            select_area(css, None, 1.5),
            Area {
                x: 150,
                y: 1350,
                width: 225,
                height: 30
            }
        );
    }

    #[test]
    fn menus_at_the_click() {
        let css = Area {
            x: 100,
            y: 50,
            width: 0,
            height: 0,
        };
        assert_eq!(menu_point(css, Some((210.0, 95.0)), 2.0), (210.0, 95.0));
        assert_eq!(menu_point(css, None, 2.0), (200.0, 100.0));
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
