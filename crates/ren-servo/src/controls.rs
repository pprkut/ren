// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Drop-downs (`<select>`) and context menus of pages, which the window
//! shows: Servo's requests as `ren_tabs` messages, and the window's
//! answers as Servo's actions.

use ren_tabs::{Area, MenuAction, MenuEntry, SelectEntry};
use servo::{
    ContextMenuAction, ContextMenuItem, DeviceIntRect, SelectElementOption,
    SelectElementOptionOrOptgroup,
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
