// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

mod account;
mod article;
mod cli;
mod demo;
mod favicons;
mod feed_edit;
mod feed_tree;
mod hash;
mod http;
mod instance;
mod item_list;
mod login;
mod notices;
mod procstat;
mod reader;
mod remote;
mod settings_dialog;
mod sync_thread;
#[cfg(feature = "servo")]
mod tabs;
#[cfg(test)]
mod testing;
mod ui;
mod welcome;
mod window_state;

use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let started = Instant::now();

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
        cli::Mode::UpdateFilters => update_filters(&options),
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

/// `--update-filters`, with the lists of the settings file if there is one.
#[cfg(feature = "servo")]
fn update_filters(options: &cli::Options) -> Result<(), String> {
    let path = match &options.settings {
        Some(path) => path.clone(),
        None => account::default_path()?,
    };
    let (file, reload) = ren_settings::SettingsFile::open(path);
    if let ren_settings::Reload::Failed(problems) = reload {
        return Err(format!("{}: {problems}", file.path().display()));
    }
    tabs::filters::update_now(file.settings())
}

#[cfg(not(feature = "servo"))]
fn update_filters(_options: &cli::Options) -> Result<(), String> {
    Err("built without web page tabs (the servo feature)".to_owned())
}
