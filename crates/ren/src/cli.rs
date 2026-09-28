// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Command line options.

pub const USAGE: &str = "\
Usage: ren [OPTIONS]

Options:
  --renderer <NAME>  Slint renderer: software, femtovg or skia
                     (default: Slint's choice, or $SLINT_BACKEND)
  --items <N>        Number of dummy items (default: 10000)
  --measure          Print startup timings to stderr
  --autoscroll       After 3 s, scroll through the item list once, print
                     the time it took and quit
  -h, --help         Show this help";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub renderer: Option<String>,
    pub items: usize,
    pub measure: bool,
    pub autoscroll: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            renderer: None,
            items: 10_000,
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
            measure: true,
            autoscroll: true,
        };
        assert_eq!(
            parse_args(&[
                "--renderer",
                "skia",
                "--items",
                "500",
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
        assert_eq!(parse_args(&["--help", "--bogus"]), Ok(Command::Help));
    }
}
