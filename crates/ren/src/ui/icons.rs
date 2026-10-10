// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Icons from the desktop's icon theme, with a bundled fallback.

use std::path::{Path, PathBuf};

use slint::{ComponentHandle, Image};

use super::{Icons, MainWindow};

/// Size the icons are looked up in; the largest one used (tool bar).
const SIZE: u16 = 22;

/// One icon: names to try in the icon theme, in order, and the bundled
/// fallback.
struct Icon {
    names: &'static [&'static str],
    fallback: &'static [u8],
    set: fn(&Icons<'_>, Image),
}

macro_rules! bundled {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/icons/",
            $name,
            ".svg"
        ))
    };
}

const ICONS: &[Icon] = &[
    Icon {
        names: &["feed-subscribe", "list-add"],
        fallback: bundled!("plus"),
        set: |icons, image| icons.set_add_feed(image),
    },
    Icon {
        names: &["go-down"],
        fallback: bundled!("arrow-down-to-line"),
        set: |icons, image| icons.set_fetch_feed(image),
    },
    Icon {
        names: &["go-bottom"],
        fallback: bundled!("chevrons-down"),
        set: |icons, image| icons.set_fetch_all(image),
    },
    Icon {
        names: &["dialog-cancel", "process-stop"],
        fallback: bundled!("circle-x"),
        set: |icons, image| icons.set_cancel(image),
    },
    Icon {
        names: &["mail-mark-read"],
        fallback: bundled!("mail-check"),
        set: |icons, image| icons.set_mark_read(image),
    },
    Icon {
        names: &["application-exit"],
        fallback: bundled!("log-out"),
        set: |icons, image| icons.set_quit(image),
    },
    Icon {
        names: &["edit-find"],
        fallback: bundled!("search"),
        set: |icons, image| icons.set_find(image),
    },
    Icon {
        names: &["configure", "preferences-system"],
        fallback: bundled!("settings"),
        set: |icons, image| icons.set_configure(image),
    },
    Icon {
        names: &["mail-folder-inbox", "folder"],
        fallback: bundled!("library"),
        set: |icons, image| icons.set_all_items(image),
    },
    Icon {
        names: &["starred", "rating", "emblem-favorite"],
        fallback: bundled!("star"),
        set: |icons, image| icons.set_starred(image),
    },
    Icon {
        names: &["folder"],
        fallback: bundled!("folder"),
        set: |icons, image| icons.set_folder(image),
    },
    Icon {
        names: &["application-rss+xml", "feed-subscribe"],
        fallback: bundled!("rss"),
        set: |icons, image| icons.set_feed(image),
    },
    Icon {
        names: &["go-previous"],
        fallback: bundled!("arrow-left"),
        set: |icons, image| icons.set_back(image),
    },
    Icon {
        names: &["go-next"],
        fallback: bundled!("arrow-right"),
        set: |icons, image| icons.set_forward(image),
    },
    Icon {
        names: &["view-refresh"],
        fallback: bundled!("rotate-cw"),
        set: |icons, image| icons.set_reload(image),
    },
    Icon {
        names: &["internet-web-browser", "applications-internet"],
        fallback: bundled!("external-link"),
        set: |icons, image| icons.set_browser(image),
    },
];

/// The configured icon theme: KDE's from `kdeglobals`, else GTK's.
fn theme_name() -> Option<String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".config")))?;
    kde_icon_theme(&std::fs::read_to_string(config.join("kdeglobals")).unwrap_or_default())
        .or_else(freedesktop_icons::default_theme_gtk)
}

/// The `Theme` key of the `[Icons]` group of a `kdeglobals` file.
fn kde_icon_theme(kdeglobals: &str) -> Option<String> {
    let mut in_icons = false;
    for line in kdeglobals.lines().map(str::trim) {
        if line.starts_with('[') {
            in_icons = line == "[Icons]";
        } else if in_icons && let Some(theme) = line.strip_prefix("Theme=") {
            return Some(theme.trim().to_owned()).filter(|t| !t.is_empty());
        }
    }
    None
}

fn lookup(names: &[&str], theme: &str, scale: u16) -> Option<PathBuf> {
    names.iter().find_map(|name| {
        freedesktop_icons::lookup(name)
            .with_theme(theme)
            .with_size(SIZE)
            .with_scale(scale)
            .find()
    })
}

/// The variant of an icon theme for a forced colour scheme: Breeze and
/// others come as `name` and `name-dark`.
fn theme_variant(theme: String, dark: Option<bool>) -> String {
    match (dark, theme.strip_suffix("-dark")) {
        (Some(false), Some(light)) => light.to_owned(),
        (Some(true), None)
            if freedesktop_icons::list_themes().contains(&format!("{theme}-dark")) =>
        {
            format!("{theme}-dark")
        }
        _ => theme,
    }
}

/// Loads all icons into the window. Uses the icon theme if it has all of
/// them (and `use_theme` is set), else the bundled set, so the tool bar
/// doesn't mix styles. `dark` is a forced colour scheme, if any.
pub fn load(window: &MainWindow, use_theme: bool, dark: Option<bool>) {
    let icons = window.global::<Icons>();
    let scale = window.window().scale_factor().ceil().max(1.0) as u16;
    let theme = theme_name()
        .filter(|_| use_theme)
        .map(|theme| theme_variant(theme, dark));
    let themed: Option<Vec<Image>> = theme.and_then(|theme| {
        ICONS
            .iter()
            .map(|icon| {
                let path = lookup(icon.names, &theme, scale)?;
                Image::load_from_path(&path).ok()
            })
            .collect()
    });

    match themed {
        Some(images) => {
            for (icon, image) in ICONS.iter().zip(images) {
                (icon.set)(&icons, image);
            }
        }
        None => {
            icons.set_monochrome(true);
            for icon in ICONS {
                if let Ok(image) = Image::load_from_svg_data(icon.fallback) {
                    (icon.set)(&icons, image);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kde_icon_theme_from_kdeglobals() {
        let config = "[General]\nTheme=wrong\n\n[Icons]\nTheme=breeze-dark\n\n[KDE]\nTheme=x\n";
        assert_eq!(kde_icon_theme(config).as_deref(), Some("breeze-dark"));
        assert_eq!(kde_icon_theme("[Icons]\nTheme=\n"), None);
        assert_eq!(kde_icon_theme("[General]\nTheme=breeze\n"), None);
    }

    #[test]
    fn bundled_icons_are_valid_svg() {
        for icon in ICONS {
            assert!(
                Image::load_from_svg_data(icon.fallback).is_ok(),
                "{:?}",
                icon.names
            );
        }
    }
}
