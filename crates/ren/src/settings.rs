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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// The Nextcloud URL, e.g. `https://cloud.example.org`.
    pub server: String,
    pub user: String,
    /// Shell command printing the app password on its first line.
    pub password_command: Option<String>,
}

/// `[account]` as written in the file; every key may be missing.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct AccountTable {
    server: Option<String>,
    user: Option<String>,
    password_command: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SettingsFile {
    #[serde(default)]
    account: AccountTable,
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
    account(file.account)
}

/// Checks that the account has everything needed to connect.
fn account(table: AccountTable) -> Result<Account, String> {
    let (server, user) = match (table.server, table.user) {
        (Some(server), Some(user)) => (server, user),
        (server, user) => {
            let missing: Vec<_> = [("server", server.is_none()), ("user", user.is_none())]
                .into_iter()
                .filter_map(|(key, missing)| missing.then_some(key))
                .collect();
            return Err(format!(
                "{} not set in [account], e.g.\n\n{ACCOUNT_EXAMPLE}",
                missing.join(" and ")
            ));
        }
    };
    check_server(&server)?;
    Ok(Account {
        server,
        user,
        password_command: table.password_command,
    })
}

const ACCOUNT_EXAMPLE: &str = "\
[account]
server = \"https://cloud.example.org\"
user = \"name\"
password-command = \"pass show nextcloud/ren\"  # or set REN_APP_PASSWORD";

/// Reads the account from a settings file. A missing file counts as empty
/// unless it was named explicitly (`--settings`), so the error says which
/// values are missing rather than that the file is.
pub fn load_account(path: &Path, explicit: bool) -> Result<Account, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound && !explicit => String::new(),
        Err(err) => return Err(format!("cannot read {}: {err}", path.display())),
    };
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
        let err = parse_account("").unwrap_err();
        assert!(
            err.starts_with("server and user not set in [account]"),
            "{err}"
        );
        let err = parse_account("[account]\nserver = \"https://x\"").unwrap_err();
        assert!(err.starts_with("user not set"), "{err}");
        assert!(parse_account("[account").is_err());
        let http = "[account]\nserver = \"http://cloud.example.org\"\nuser = \"u\"";
        assert!(parse_account(http).unwrap_err().contains("https"));
    }

    #[test]
    fn missing_file() {
        let path = Path::new("/nonexistent/ren/settings.toml");
        let err = load_account(path, false).unwrap_err();
        assert!(err.contains("server and user not set"), "{err}");
        let err = load_account(path, true).unwrap_err();
        assert!(err.starts_with("cannot read"), "{err}");
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
