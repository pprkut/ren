// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Command line options.

pub const USAGE: &str = "\
Usage: ren [OPTIONS]

Options:
  --backend <NAME>   Slint backend: winit or qt (default: qt when built
                     with the Qt style, else winit)
  --renderer <NAME>  Slint renderer for winit: software, femtovg or skia
                     (default: $SLINT_BACKEND, else software)
  --items <N>        Number of dummy items (default: 10000)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub backend: Option<String>,
    pub renderer: Option<String>,
    pub items: usize,
    pub arrangement: Arrangement,
    pub bundled_icons: bool,
    pub color_scheme: Option<ColorScheme>,
    pub measure: bool,
    pub autoscroll: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            backend: None,
            renderer: None,
            items: 10_000,
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
        match arg.as_str() {
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
            backend: Some("winit".to_owned()),
            renderer: Some("skia".to_owned()),
            items: 500,
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
    }
}
