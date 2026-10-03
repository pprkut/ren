// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

mod cli;
mod dummy;
mod feed_tree;
mod item_list;
mod ui;

use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let started = Instant::now();

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

    match ui::run(&options, started) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ren: {err}");
            ExitCode::FAILURE
        }
    }
}
