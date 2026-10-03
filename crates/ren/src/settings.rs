// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `[account]` table of the settings file and the app password.
//!
//! A minimal reader for spike S2; the settings model of M4 replaces it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

/// Environment variable with the app password; takes precedence over
/// `password-command`.
pub const PASSWORD_VAR: &str = "REN_APP_PASSWORD";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Account {
    /// The Nextcloud URL, e.g. `https://cloud.example.org`.
    pub server: String,
    pub user: String,
    /// Shell command printing the app password on its first line.
    pub password_command: Option<String>,
}

#[derive(Deserialize)]
struct SettingsFile {
    account: Option<Account>,
}

/// `$XDG_CONFIG_HOME/ren/settings.toml`, falling back to `~/.config`.
/// `var` looks up environment variables.
pub fn default_path(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let config = var("XDG_CONFIG_HOME")
        .filter(|dir| Path::new(dir).is_absolute())
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|home| Path::new(&home).join(".config")))?;
    Some(config.join("ren").join("settings.toml"))
}

/// Reads the `[account]` table from the text of a settings file.
pub fn parse_account(text: &str) -> Result<Account, String> {
    let file: SettingsFile = toml::from_str(text).map_err(|err| err.to_string())?;
    let account = file.account.ok_or("no [account] table")?;
    check_server(&account.server)?;
    Ok(account)
}

pub fn load_account(path: &Path) -> Result<Account, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    parse_account(&text).map_err(|err| format!("{}: {err}", path.display()))
}

/// Basic auth sends the password with every request, so only HTTPS is
/// accepted, except for a server on the local machine.
fn check_server(server: &str) -> Result<(), String> {
    let local = ["http://localhost", "http://127.0.0.1", "http://[::1]"];
    let is_local = |prefix: &&str| {
        server
            .strip_prefix(*prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
    };
    if server.starts_with("https://") || local.iter().any(is_local) {
        Ok(())
    } else {
        Err(format!("server must be an https:// URL: {server}"))
    }
}

/// The app password: `env_password` (from [`PASSWORD_VAR`]) if set, else
/// the first line printed by `password-command`.
pub fn password(account: &Account, env_password: Option<String>) -> Result<String, String> {
    if let Some(password) = env_password.filter(|p| !p.is_empty()) {
        return Ok(password);
    }
    let Some(command) = &account.password_command else {
        return Err(format!(
            "no app password: set {PASSWORD_VAR} or password-command in [account]"
        ));
    };
    // stdin and stderr stay connected, so the command can ask for a
    // passphrase (pass, gpg) and report errors.
    let output = Command::new("sh")
        .args(["-c", command])
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|err| format!("cannot run password-command: {err}"))?;
    if !output.status.success() {
        return Err(format!("password-command failed ({})", output.status));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "password-command printed invalid UTF-8".to_owned())?;
    match stdout.lines().next() {
        Some(line) if !line.is_empty() => Ok(line.to_owned()),
        _ => Err("password-command printed no password".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(command: Option<&str>) -> Account {
        Account {
            server: "https://cloud.example.org".to_owned(),
            user: "user".to_owned(),
            password_command: command.map(str::to_owned),
        }
    }

    #[test]
    fn parse() {
        let text = r#"
            # comment
            [account]
            server = "https://cloud.example.org"
            user = "user"
            password-command = "pass show nextcloud/ren"

            [sync]
            interval-minutes = 15
        "#;
        assert_eq!(
            parse_account(text),
            Ok(account(Some("pass show nextcloud/ren")))
        );
    }

    #[test]
    fn parse_errors() {
        assert!(parse_account("").unwrap_err().contains("[account]"));
        assert!(parse_account("[account]\nserver = \"https://x\"").is_err());
        assert!(parse_account("[account").is_err());
        let http = "[account]\nserver = \"http://cloud.example.org\"\nuser = \"u\"";
        assert!(parse_account(http).unwrap_err().contains("https"));
    }

    #[test]
    fn local_http_allowed() {
        assert!(check_server("http://localhost:8080").is_ok());
        assert!(check_server("http://127.0.0.1").is_ok());
        assert!(check_server("http://[::1]:80/nc").is_ok());
        assert!(check_server("http://localhost.example.org").is_err());
    }

    #[test]
    fn path() {
        let env = |pairs: &'static [(&str, &str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            default_path(env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/u")])),
            Some(PathBuf::from("/xdg/ren/settings.toml"))
        );
        assert_eq!(
            default_path(env(&[("XDG_CONFIG_HOME", "relative"), ("HOME", "/home/u")])),
            Some(PathBuf::from("/home/u/.config/ren/settings.toml"))
        );
        assert_eq!(default_path(env(&[])), None);
    }

    #[test]
    fn password_sources() {
        let with_command = account(Some("printf 'from-command\\nsecond line\\n'"));
        assert_eq!(
            password(&with_command, Some("from-env".to_owned())),
            Ok("from-env".to_owned())
        );
        assert_eq!(
            password(&with_command, Some(String::new())),
            Ok("from-command".to_owned())
        );
        assert_eq!(password(&with_command, None), Ok("from-command".to_owned()));
        assert!(password(&account(None), None).is_err());
        assert!(password(&account(Some("exit 3")), None).is_err());
        assert!(password(&account(Some("true")), None).is_err());
    }
}
