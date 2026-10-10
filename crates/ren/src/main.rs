// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

mod account;
mod article;
mod cli;
mod demo;
mod feed_tree;
mod hash;
mod item_list;
mod procstat;
mod reader;
mod remote;
mod sync_thread;
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

    // With glibc's dynamic threshold for comparison (`just
    // measure-articles`).
    if std::env::var_os("REN_DYNAMIC_MMAP_THRESHOLD").is_none() {
        procstat::fix_mmap_threshold(procstat::MMAP_THRESHOLD);
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
        cli::Mode::Window => ui::run(&options, started),
        cli::Mode::SetPassword => account::set_password(&options),
        cli::Mode::CheckSecretService => account::check_secret_service(&options),
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
