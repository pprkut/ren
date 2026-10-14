// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Drop-downs (`<select>`), context menus, colour and file pickers and the
//! `alert`, `confirm` and `prompt` dialogs of pages, which the window
//! shows: Servo's requests as `ren_tabs` messages, and the window's
//! answers as Servo's actions.

use ren_tabs::{
    Area, DialogKind, MAX_DIALOG_MESSAGE, MAX_DIALOG_TEXT, MAX_FILE_FILTERS, MenuAction, MenuEntry,
    Rgb, SelectEntry,
};
use servo::{
    ContextMenuAction, ContextMenuItem, DeviceIntRect, FilterPattern, RgbColor,
    SelectElementOption, SelectElementOptionOrOptgroup, SimpleDialog,
};

/// The most entries of a drop-down or menu passed on; the window limits
/// them too.
const MAX_ENTRIES: usize = 2_000;

/// The longest label passed on, in characters.
const MAX_LABEL: usize = 200;

pub fn area(rect: DeviceIntRect) -> Area {
    Area {
        x: rect.min.x,
        y: rect.min.y,
        width: rect.width(),
        height: rect.height(),
    }
}

fn label(text: &str) -> String {
    text.chars().take(MAX_LABEL).collect()
}

fn option(option: &SelectElementOption) -> SelectEntry {
    SelectEntry::Option {
        id: option.id,
        label: label(&option.label),
        enabled: !option.is_disabled,
    }
}

/// A page's dialog: its kind, message and a prompt's default, cut to the
/// lengths the window takes.
pub fn dialog(dialog: &SimpleDialog) -> (DialogKind, String, String) {
    let (kind, text) = match dialog {
        SimpleDialog::Alert(_) => (DialogKind::Alert, ""),
        SimpleDialog::Confirm(_) => (DialogKind::Confirm, ""),
        SimpleDialog::Prompt(prompt) => (DialogKind::Prompt, prompt.current_value()),
    };
    (
        kind,
        dialog.message().chars().take(MAX_DIALOG_MESSAGE).collect(),
        text.chars().take(MAX_DIALOG_TEXT).collect(),
    )
}

pub fn rgb(colour: RgbColor) -> Rgb {
    Rgb {
        red: colour.red,
        green: colour.green,
        blue: colour.blue,
    }
}

pub fn servo_rgb(colour: Rgb) -> RgbColor {
    RgbColor {
        red: colour.red,
        green: colour.green,
        blue: colour.blue,
    }
}

/// The file name extensions a file input accepts (Servo's patterns are
/// extensions without the dot), in lower case: only letters, digits, `-`
/// and `_`, at most [`MAX_FILE_FILTERS`].
pub fn extensions(patterns: &[FilterPattern]) -> Vec<String> {
    let mut extensions: Vec<String> = Vec::new();
    for FilterPattern(pattern) in patterns {
        let extension = pattern.trim().trim_start_matches('.').to_lowercase();
        let valid = !extension.is_empty()
            && extension.len() <= 20
            && extension
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if valid && !extensions.contains(&extension) {
            extensions.push(extension);
        }
        if extensions.len() == MAX_FILE_FILTERS {
            break;
        }
    }
    extensions
}

/// A drop-down's options, the options of a group after its heading.
pub fn select_entries(options: &[SelectElementOptionOrOptgroup]) -> Vec<SelectEntry> {
    let mut entries = Vec::new();
    for entry in options {
        match entry {
            SelectElementOptionOrOptgroup::Option(o) => entries.push(option(o)),
            SelectElementOptionOrOptgroup::Optgroup {
                label: heading,
                options,
            } => {
                entries.push(SelectEntry::Group(label(heading)));
                entries.extend(options.iter().map(option));
            }
        }
    }
    entries.truncate(MAX_ENTRIES);
    entries
}

/// A context menu's entries; Servo's labels are left to the window.
pub fn menu_entries(items: &[ContextMenuItem]) -> Vec<MenuEntry> {
    items
        .iter()
        .take(MAX_ENTRIES)
        .map(|item| match item {
            ContextMenuItem::Item {
                action, enabled, ..
            } => MenuEntry::Action {
                action: menu_action(*action),
                enabled: *enabled,
            },
            ContextMenuItem::Separator => MenuEntry::Separator,
        })
        .collect()
}

fn menu_action(action: ContextMenuAction) -> MenuAction {
    match action {
        ContextMenuAction::GoBack => MenuAction::Back,
        ContextMenuAction::GoForward => MenuAction::Forward,
        ContextMenuAction::Reload => MenuAction::Reload,
        ContextMenuAction::CopyLink => MenuAction::CopyLink,
        ContextMenuAction::OpenLinkInNewWebView => MenuAction::OpenLink,
        ContextMenuAction::CopyImageLink => MenuAction::CopyImageLink,
        ContextMenuAction::OpenImageInNewView => MenuAction::OpenImage,
        ContextMenuAction::Cut => MenuAction::Cut,
        ContextMenuAction::Copy => MenuAction::Copy,
        ContextMenuAction::Paste => MenuAction::Paste,
        ContextMenuAction::SelectAll => MenuAction::SelectAll,
    }
}

/// What Servo does for an entry the window chose. Links and images are
/// opened and copied by the window (a tab of its own; a clipboard that
/// outlives the helper), so Servo only dismisses its menu for those.
pub fn servo_action(action: MenuAction) -> Option<ContextMenuAction> {
    Some(match action {
        MenuAction::Back => ContextMenuAction::GoBack,
        MenuAction::Forward => ContextMenuAction::GoForward,
        MenuAction::Reload => ContextMenuAction::Reload,
        MenuAction::Cut => ContextMenuAction::Cut,
        MenuAction::Copy => ContextMenuAction::Copy,
        MenuAction::Paste => ContextMenuAction::Paste,
        MenuAction::SelectAll => ContextMenuAction::SelectAll,
        MenuAction::OpenLink
        | MenuAction::CopyLink
        | MenuAction::OpenImage
        | MenuAction::CopyImageLink => return None,
    })
}

#[cfg(test)]
mod tests {
    use servo::DeviceIntPoint;

    use super::*;

    fn opt(id: usize, label: &str, is_disabled: bool) -> SelectElementOption {
        SelectElementOption {
            id,
            label: label.to_owned(),
            is_disabled,
        }
    }

    #[test]
    fn drop_downs() {
        let options = [
            SelectElementOptionOrOptgroup::Option(opt(0, "None", false)),
            SelectElementOptionOrOptgroup::Optgroup {
                label: "Europe".to_owned(),
                options: vec![opt(1, "Austria", false), opt(2, "Belgium", true)],
            },
            SelectElementOptionOrOptgroup::Option(opt(3, &"x".repeat(500), false)),
        ];
        let entries = select_entries(&options);
        assert_eq!(entries.len(), 5);
        assert_eq!(
            entries[..4],
            [
                SelectEntry::Option {
                    id: 0,
                    label: "None".to_owned(),
                    enabled: true
                },
                SelectEntry::Group("Europe".to_owned()),
                SelectEntry::Option {
                    id: 1,
                    label: "Austria".to_owned(),
                    enabled: true
                },
                SelectEntry::Option {
                    id: 2,
                    label: "Belgium".to_owned(),
                    enabled: false
                },
            ]
        );
        let SelectEntry::Option { label, .. } = &entries[4] else {
            panic!("an option");
        };
        assert_eq!(label.len(), MAX_LABEL);

        let many: Vec<_> = (0..MAX_ENTRIES + 10)
            .map(|id| SelectElementOptionOrOptgroup::Option(opt(id, "o", false)))
            .collect();
        assert_eq!(select_entries(&many).len(), MAX_ENTRIES);
    }

    #[test]
    fn menus() {
        let items = [
            ContextMenuItem::Item {
                label: "Open Link in New View".to_owned(),
                action: ContextMenuAction::OpenLinkInNewWebView,
                enabled: true,
            },
            ContextMenuItem::Separator,
            ContextMenuItem::Item {
                label: "Copy".to_owned(),
                action: ContextMenuAction::Copy,
                enabled: false,
            },
        ];
        assert_eq!(
            menu_entries(&items),
            [
                MenuEntry::Action {
                    action: MenuAction::OpenLink,
                    enabled: true
                },
                MenuEntry::Separator,
                MenuEntry::Action {
                    action: MenuAction::Copy,
                    enabled: false
                },
            ]
        );
        assert_eq!(
            servo_action(MenuAction::Copy),
            Some(ContextMenuAction::Copy)
        );
        assert_eq!(servo_action(MenuAction::CopyLink), None);
        assert_eq!(servo_action(MenuAction::OpenImage), None);
    }

    #[test]
    fn file_filters() {
        let patterns: Vec<FilterPattern> = ["pdf", ".PNG", "pdf", "", "a/b", "tar.gz", "x y"]
            .into_iter()
            .map(|p| FilterPattern(p.to_owned()))
            .collect();
        assert_eq!(extensions(&patterns), ["pdf", "png"]);
        let many: Vec<FilterPattern> = (0..100).map(|i| FilterPattern(format!("e{i}"))).collect();
        assert_eq!(extensions(&many).len(), MAX_FILE_FILTERS);
    }

    #[test]
    fn colours() {
        let colour = Rgb {
            red: 1,
            green: 2,
            blue: 3,
        };
        assert_eq!(rgb(servo_rgb(colour)), colour);
    }

    #[test]
    fn areas() {
        let rect = DeviceIntRect::new(DeviceIntPoint::new(10, 20), DeviceIntPoint::new(110, 50));
        assert_eq!(
            area(rect),
            Area {
                x: 10,
                y: 20,
                width: 100,
                height: 30
            }
        );
    }
}
