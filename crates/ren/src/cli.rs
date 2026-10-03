// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Command line options.

use std::path::PathBuf;

pub const USAGE: &str = "\
Usage: ren [OPTIONS]

Server commands (instead of opening the window):
  --check              Print the server's News app version, folder, feed,
                       unread and starred counts
  --fetch-unread       Fetch all unread items without storing them and
                       print time, size and peak memory
  --dump-items <DIR>   Store the raw responses (folders, feeds, unread and
                       starred items) in DIR, outside the repository
  --batch-size <N>     Items per request, or \"all\" (default: 1000)
  --settings <FILE>    Settings file (default:
                       $XDG_CONFIG_HOME/ren/settings.toml)
  The app password comes from $REN_APP_PASSWORD or from password-command
  in the [account] table of the settings file.

Window options:
  --backend <NAME>   Slint backend: winit or qt (default: qt when built
                     with the Qt style, else winit)
  --renderer <NAME>  Slint renderer for winit: software, femtovg or skia
                     (default: $SLINT_BACKEND, else software)
  --items <N>        Number of dummy items, or the most items loaded with
                     --dump (default: 10000)
  --dump <DIR>       Show the items of a --dump-items directory instead
                     of dummy data
  --arrangement <A>  Item list beside or above the article (default: beside)
  --bundled-icons    Use the bundled icons instead of the icon theme
  --color-scheme <S> light or dark instead of the desktop's
  --measure          Print startup timings to stderr
  --autoscroll       After 3 s, scroll through the item list once, print
                     the time it took and quit
  -h, --help         Show this help";

/// A colour scheme forced instead of the desktop's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

/// Where the item list goes relative to the article.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Arrangement {
    #[default]
    Beside,
    Above,
}

/// What ren does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Window,
    Check,
    FetchUnread,
    DumpItems(PathBuf),
}

/// Items per request of the server commands, see
/// `docs/decisions/0003-nextcloud-client.md`.
pub const DEFAULT_BATCH_SIZE: u32 = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub mode: Mode,
    pub settings: Option<PathBuf>,
    /// `None` fetches all items in one request.
    pub batch_size: Option<u32>,
    pub backend: Option<String>,
    pub renderer: Option<String>,
    pub items: usize,
    pub dump: Option<PathBuf>,
    pub arrangement: Arrangement,
    pub bundled_icons: bool,
    pub color_scheme: Option<ColorScheme>,
    pub measure: bool,
    pub autoscroll: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::Window,
            settings: None,
            batch_size: Some(DEFAULT_BATCH_SIZE),
            backend: None,
            renderer: None,
            items: 10_000,
            dump: None,
            arrangement: Arrangement::default(),
            bundled_icons: false,
            color_scheme: None,
            measure: false,
            autoscroll: false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Run(Options),
    Help,
}

/// Parses the arguments, without the program name.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        let mode = match arg.as_str() {
            "--check" => Some(Mode::Check),
            "--fetch-unread" => Some(Mode::FetchUnread),
            "--dump-items" => Some(Mode::DumpItems(value("--dump-items")?.into())),
            _ => None,
        };
        if let Some(mode) = mode {
            if options.mode != Mode::Window {
                return Err("only one of --check, --fetch-unread and --dump-items".to_owned());
            }
            options.mode = mode;
            continue;
        }
        match arg.as_str() {
            "--settings" => options.settings = Some(value("--settings")?.into()),
            "--batch-size" => {
                options.batch_size = match value("--batch-size")?.as_str() {
                    "all" => None,
                    n => match n.parse() {
                        Ok(n) if n > 0 => Some(n),
                        _ => return Err(format!("invalid batch size: {n}")),
                    },
                }
            }
            "--backend" => options.backend = Some(value("--backend")?),
            "--renderer" => options.renderer = Some(value("--renderer")?),
            "--items" => {
                let n = value("--items")?;
                options.items = n.parse().map_err(|_| format!("invalid item count: {n}"))?;
            }
            "--arrangement" => {
                options.arrangement = match value("--arrangement")?.as_str() {
                    "beside" => Arrangement::Beside,
                    "above" => Arrangement::Above,
                    other => return Err(format!("invalid arrangement: {other}")),
                }
            }
            "--dump" => options.dump = Some(value("--dump")?.into()),
            "--bundled-icons" => options.bundled_icons = true,
            "--color-scheme" => {
                options.color_scheme = match value("--color-scheme")?.as_str() {
                    "light" => Some(ColorScheme::Light),
                    "dark" => Some(ColorScheme::Dark),
                    other => return Err(format!("invalid colour scheme: {other}")),
                }
            }
            "--measure" => options.measure = true,
            "--autoscroll" => options.autoscroll = true,
            "-h" | "--help" => return Ok(Command::Help),
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    Ok(Command::Run(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn defaults() {
        assert_eq!(parse_args(&[]), Ok(Command::Run(Options::default())));
    }

    #[test]
    fn all_options() {
        let expected = Options {
            mode: Mode::Window,
            settings: None,
            batch_size: Some(DEFAULT_BATCH_SIZE),
            backend: Some("winit".to_owned()),
            renderer: Some("skia".to_owned()),
            items: 500,
            dump: Some("/tmp/dump".into()),
            arrangement: Arrangement::Above,
            bundled_icons: true,
            color_scheme: Some(ColorScheme::Light),
            measure: true,
            autoscroll: true,
        };
        assert_eq!(
            parse_args(&[
                "--backend",
                "winit",
                "--renderer",
                "skia",
                "--items",
                "500",
                "--dump",
                "/tmp/dump",
                "--arrangement",
                "above",
                "--bundled-icons",
                "--color-scheme",
                "light",
                "--measure",
                "--autoscroll"
            ]),
            Ok(Command::Run(expected))
        );
    }

    #[test]
    fn errors() {
        assert!(parse_args(&["--renderer"]).is_err());
        assert!(parse_args(&["--items", "many"]).is_err());
        assert!(parse_args(&["--bogus"]).is_err());
        assert!(parse_args(&["--arrangement", "below"]).is_err());
        assert!(parse_args(&["--color-scheme", "blue"]).is_err());
        assert_eq!(parse_args(&["--help", "--bogus"]), Ok(Command::Help));
        assert!(parse_args(&["--batch-size", "0"]).is_err());
        assert!(parse_args(&["--batch-size", "-1"]).is_err());
        assert!(parse_args(&["--dump-items"]).is_err());
        assert!(parse_args(&["--check", "--fetch-unread"]).is_err());
    }

    #[test]
    fn server_commands() {
        let Ok(Command::Run(options)) = parse_args(&[
            "--dump-items",
            "/tmp/dump",
            "--batch-size",
            "all",
            "--settings",
            "/etc/ren.toml",
        ]) else {
            panic!("parse failed");
        };
        assert_eq!(options.mode, Mode::DumpItems("/tmp/dump".into()));
        assert_eq!(options.batch_size, None);
        assert_eq!(options.settings, Some("/etc/ren.toml".into()));

        let Ok(Command::Run(options)) = parse_args(&["--check", "--batch-size", "50"]) else {
            panic!("parse failed");
        };
        assert_eq!(options.mode, Mode::Check);
        assert_eq!(options.batch_size, Some(50));
    }
}
