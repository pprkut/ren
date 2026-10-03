// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Command line options.

pub const USAGE: &str = "\
Usage: ren [OPTIONS]

Options:
  --renderer <NAME>  Slint renderer: software, femtovg or skia
                     (default: $SLINT_BACKEND, else software)
  --items <N>        Number of dummy items (default: 10000)
  --arrangement <A>  Item list beside or above the article (default: beside)
  --measure          Print startup timings to stderr
  --autoscroll       After 3 s, scroll through the item list once, print
                     the time it took and quit
  -h, --help         Show this help";

/// Where the item list goes relative to the article.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Arrangement {
    #[default]
    Beside,
    Above,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub renderer: Option<String>,
    pub items: usize,
    pub arrangement: Arrangement,
    pub measure: bool,
    pub autoscroll: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            renderer: None,
            items: 10_000,
            arrangement: Arrangement::default(),
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
            renderer: Some("skia".to_owned()),
            items: 500,
            arrangement: Arrangement::Above,
            measure: true,
            autoscroll: true,
        };
        assert_eq!(
            parse_args(&[
                "--renderer",
                "skia",
                "--items",
                "500",
                "--arrangement",
                "above",
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
        assert_eq!(parse_args(&["--help", "--bogus"]), Ok(Command::Help));
    }
}
