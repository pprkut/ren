// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

mod article;
mod cli;
mod dummy;
mod dump;
mod feed_tree;
mod item_list;
mod procstat;
mod remote;
mod settings;
#[cfg(feature = "servo")]
mod tabs;
mod ui;

use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let started = Instant::now();

    #[cfg(feature = "servo")]
    {
        let mut args = std::env::args().skip(1);
        if args.next().as_deref() == Some(tabs::helper::HELPER_ARG) {
            let socket = args.next().unwrap_or_default();
            return match tabs::helper::run(socket.as_ref()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("ren: Servo helper: {err}");
                    ExitCode::FAILURE
                }
            };
        }
    }

    let options = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::Command::Run(options)) => options,
        Ok(cli::Command::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("ren: {err}\n\n{}", cli::USAGE);
            return ExitCode::FAILURE;
        }
    };

    let result = match options.mode {
        cli::Mode::Window => ui::run(&options, started).map_err(|err| err.to_string()),
        _ => remote::run(&options),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ren: {err}");
            ExitCode::FAILURE
        }
    }
}
