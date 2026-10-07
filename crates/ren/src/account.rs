// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings file and the app password for the command line modes:
//! loading the settings, `--set-password` and `--check-secret-service`.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use nextcloud_news::{Client, Config, Credentials};
use ren_settings::{
    Account, PASSWORD_VAR, Problem, Reload, SecretService, SecretStore, Settings, SettingsFile,
    paths,
};

use crate::cli::Options;
use crate::procstat::proc_status_kib;
use crate::remote::USER_AGENT;

/// The settings from the file given with `--settings`, else from the
/// default one. A missing default file has the defaults; warnings go to
/// stderr.
pub fn settings(options: &Options) -> Result<Settings, String> {
    let path = match &options.settings {
        Some(path) if !path.exists() => {
            return Err(format!("{}: no such file", path.display()));
        }
        Some(path) => path.clone(),
        None => default_path()?,
    };
    let (file, reload) = SettingsFile::open(&path);
    match reload {
        Reload::Loaded(warnings) => {
            for warning in warnings {
                eprintln!("ren: {}", located(&path, &warning));
            }
            Ok(file.settings().clone())
        }
        Reload::Failed(problems) => Err(problems
            .0
            .iter()
            .map(|problem| located(&path, problem))
            .collect::<Vec<_>>()
            .join("\nren: ")),
        Reload::Unchanged => unreachable!("a file is loaded when opened"),
    }
}

pub fn default_path() -> Result<PathBuf, String> {
    paths::settings(|name| std::env::var(name).ok()).ok_or_else(|| {
        "cannot find the settings file: neither XDG_CONFIG_HOME nor HOME is set".into()
    })
}

/// `file:line:column: message`, or `file: message` without a position.
pub fn located(path: &Path, problem: &Problem) -> String {
    let separator = if problem.position.is_some() {
        ":"
    } else {
        ": "
    };
    format!("{}{separator}{problem}", path.display())
}

/// The app password for `account`: from [`PASSWORD_VAR`], `password-command`
/// or the Secret Service.
pub fn password(account: &Account) -> Result<String, String> {
    ren_settings::app_password(
        account,
        std::env::var(PASSWORD_VAR).ok(),
        SecretService::connect,
    )
    .map(|(password, _)| password)
    .map_err(|err| err.to_string())
}

/// `--set-password`: reads an app password from stdin, checks it against
/// the server and stores it in the Secret Service.
pub fn set_password(options: &Options) -> Result<(), String> {
    let account = settings(options)?.account()?;
    let password = read_password(&format!(
        "App password for {} on {}: ",
        account.user, account.server
    ))
    .map_err(|err| format!("cannot read the password: {err}"))?;
    if password.is_empty() {
        return Err("no password given".to_owned());
    }

    let credentials = Credentials {
        user: account.user.clone(),
        password: password.clone(),
    };
    let client = Client::new(&account.server, &credentials, &Config::new(USER_AGENT));
    client
        .folders()
        .map_err(|err| format!("{}: {err}", client.base_url()))?;

    let service = SecretService::connect().map_err(|err| err.to_string())?;
    service
        .set_password(&account, &password)
        .map_err(|err| err.to_string())?;
    let provider = SecretService::provider().unwrap_or_else(|err| err.to_string());
    println!("The app password works and is stored in the Secret Service: {provider}");
    if account.password_command.is_some() {
        println!("Note: password-command is set in [account] and is used instead.");
    }
    Ok(())
}

/// The first line of stdin, without echoing it if stdin is a terminal.
fn read_password(prompt: &str) -> std::io::Result<String> {
    let stdin = std::io::stdin();
    let terminal = stdin.is_terminal();
    if terminal {
        eprint!("{prompt}");
        std::io::stderr().flush()?;
        stty("-echo");
    }
    let mut line = String::new();
    let result = stdin.lock().read_line(&mut line);
    if terminal {
        stty("echo");
        eprintln!();
    }
    result?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

/// Changes the terminal on stdin; without stty, the password is echoed.
fn stty(setting: &str) {
    let _ = Command::new("stty")
        .arg(setting)
        .stdin(Stdio::inherit())
        .status();
}

/// `--check-secret-service`: which program answers the Secret Service,
/// whether a password can be stored, read and removed, whether the
/// account's app password is there, and what connecting costs.
pub fn check_secret_service(options: &Options) -> Result<(), String> {
    let provider = SecretService::provider().map_err(|err| err.to_string())?;
    println!("Secret Service: {provider}");

    let rss_before = proc_status_kib("VmRSS:");
    let started = Instant::now();
    let service = SecretService::connect().map_err(|err| err.to_string())?;
    let connected = started.elapsed();
    let rss_connected = proc_status_kib("VmRSS:");
    let test = Account {
        server: "https://ren-check.invalid".to_owned(),
        user: format!("check-{}", std::process::id()),
        password_command: None,
    };
    let round_trip = (|| {
        service.set_password(&test, "test password")?;
        let read = service.password(&test)?;
        service.delete_password(&test)?;
        let gone = service.password(&test)?;
        Ok::<_, ren_settings::SecretError>(
            read.as_deref() == Some("test password") && gone.is_none(),
        )
    })();
    match round_trip {
        Ok(true) => println!("Storing, reading and removing a test password: ok"),
        Ok(false) => return Err("the test password didn't read back as stored".to_owned()),
        Err(err) => return Err(format!("storing a test password: {err}")),
    }

    match settings(options).and_then(|settings| settings.account()) {
        Ok(account) => {
            let stored = match service.password(&account) {
                Ok(Some(_)) => "stored",
                Ok(None) => "not stored (ren --set-password stores it)",
                Err(err) => return Err(err.to_string()),
            };
            println!(
                "App password for {} on {}: {stored}",
                account.user, account.server
            );
            if account.password_command.is_some() {
                println!("password-command is set in [account] and is used instead.");
            }
        }
        Err(err) => println!("No account: {err}"),
    }
    let kib = |kib: Option<u64>| kib.map_or("?".to_owned(), |kib| format!("{kib} KiB"));
    println!(
        "Connecting: {:.1} ms; RSS {} before, {} connected, {} at the end, peak {}",
        connected.as_secs_f64() * 1000.0,
        kib(rss_before),
        kib(rss_connected),
        kib(proc_status_kib("VmRSS:")),
        kib(proc_status_kib("VmHWM:")),
    );
    Ok(())
}
