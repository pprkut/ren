// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The app password: never in a file. It comes from the Secret Service
//! (GNOME Keyring, KWallet, KeePassXC), from the `password-command`
//! setting on systems without one, or from an environment variable for
//! scripts and measurements.
//!
//! Calls into the Secret Service block, also while the user is asked to
//! unlock the keyring, so they don't belong on the UI thread.

use std::collections::HashMap;
use std::fmt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use keyring_core::Entry;
use keyring_core::api::CredentialStoreApi;

use crate::settings::Account;

/// Environment variable with the app password; takes precedence over
/// everything else.
pub const PASSWORD_VAR: &str = "REN_APP_PASSWORD";

/// Where app passwords are kept, per account.
pub trait SecretStore {
    /// The app password of `account`; `None` if there's none.
    fn password(&self, account: &Account) -> Result<Option<String>, SecretError>;
    fn set_password(&self, account: &Account, password: &str) -> Result<(), SecretError>;
    /// Removes the app password of `account`; none is no error.
    fn delete_password(&self, account: &Account) -> Result<(), SecretError>;
}

/// Why the secret store failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// There's no Secret Service, or no session bus.
    Unavailable(String),
    /// The keyring is locked and the user didn't unlock it, or access was
    /// refused.
    Denied(String),
    /// Anything else.
    Failed(String),
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretError::Unavailable(err) => write!(f, "no Secret Service: {err}"),
            SecretError::Denied(err) => write!(f, "the Secret Service refused access: {err}"),
            SecretError::Failed(err) => write!(f, "Secret Service: {err}"),
        }
    }
}

impl std::error::Error for SecretError {}

/// The Secret Service, through the D-Bus API that GNOME Keyring, KWallet
/// (since Frameworks 5.97) and KeePassXC implement.
///
/// An app password is stored with the attributes `service` = `ren` and
/// `username` = `<user>@<server>`, so it can also be stored by hand:
///
/// ```sh
/// secret-tool store --label='ren' service ren username 'user@https://cloud.example.org'
/// ```
pub struct SecretService {
    store: Arc<zbus_secret_service_keyring_store::Store>,
}

/// The Secret Service's `service` attribute of ren's app passwords.
const SERVICE: &str = "ren";

impl SecretService {
    /// Connects to the Secret Service on the session bus.
    pub fn connect() -> Result<SecretService, SecretError> {
        let store = zbus_secret_service_keyring_store::Store::new().map_err(|err| match err {
            keyring_core::Error::PlatformFailure(err) => SecretError::Unavailable(err.to_string()),
            err => error(err),
        })?;
        Ok(SecretService { store })
    }

    fn entry(&self, account: &Account) -> Result<Entry, SecretError> {
        let label = format!(
            "ren: Nextcloud News app password for {} on {}",
            account.user, account.server
        );
        let modifiers = HashMap::from([("label", label.as_str())]);
        self.store
            .build(SERVICE, &username(account), Some(&modifiers))
            .map_err(error)
    }

    /// The program that answers the Secret Service on the session bus,
    /// e.g. `kwalletd6` or `gnome-keyring-d`, as the kernel names it.
    pub fn provider() -> Result<String, SecretError> {
        use zbus::blocking::{Connection, fdo::DBusProxy};
        use zbus::names::BusName;

        let unavailable = |err: zbus::Error| SecretError::Unavailable(err.to_string());
        let connection = Connection::session().map_err(unavailable)?;
        let bus = DBusProxy::new(&connection).map_err(unavailable)?;
        let name = BusName::try_from("org.freedesktop.secrets").expect("a valid bus name");
        let owner = bus
            .get_name_owner(name)
            .map_err(|err| SecretError::Unavailable(err.to_string()))?;
        let pid = bus
            .get_connection_unix_process_id(owner.into())
            .map_err(|err| SecretError::Failed(err.to_string()))?;
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map_err(|err| SecretError::Failed(err.to_string()))?;
        Ok(format!("{} (pid {pid})", comm.trim_end()))
    }
}

/// The `username` attribute of the account's app password.
fn username(account: &Account) -> String {
    format!("{}@{}", account.user, account.server)
}

fn error(err: keyring_core::Error) -> SecretError {
    match err {
        keyring_core::Error::NoStorageAccess(err) => SecretError::Denied(err.to_string()),
        err => SecretError::Failed(err.to_string()),
    }
}

impl SecretStore for SecretService {
    fn password(&self, account: &Account) -> Result<Option<String>, SecretError> {
        match self.entry(account)?.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(err) => Err(error(err)),
        }
    }

    fn set_password(&self, account: &Account, password: &str) -> Result<(), SecretError> {
        self.entry(account)?.set_password(password).map_err(error)
    }

    fn delete_password(&self, account: &Account) -> Result<(), SecretError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(err) => Err(error(err)),
        }
    }
}

/// A secret store in memory, for tests.
#[derive(Debug, Default)]
pub struct MemoryStore {
    passwords: Mutex<HashMap<(String, String), String>>,
    /// Returned by every call instead of doing it.
    pub failure: Option<SecretError>,
}

impl MemoryStore {
    fn check(&self) -> Result<(), SecretError> {
        self.failure.clone().map_or(Ok(()), Err)
    }

    fn key(account: &Account) -> (String, String) {
        (account.server.clone(), account.user.clone())
    }
}

impl SecretStore for MemoryStore {
    fn password(&self, account: &Account) -> Result<Option<String>, SecretError> {
        self.check()?;
        let passwords = self.passwords.lock().unwrap();
        Ok(passwords.get(&MemoryStore::key(account)).cloned())
    }

    fn set_password(&self, account: &Account, password: &str) -> Result<(), SecretError> {
        self.check()?;
        let mut passwords = self.passwords.lock().unwrap();
        passwords.insert(MemoryStore::key(account), password.to_owned());
        Ok(())
    }

    fn delete_password(&self, account: &Account) -> Result<(), SecretError> {
        self.check()?;
        self.passwords
            .lock()
            .unwrap()
            .remove(&MemoryStore::key(account));
        Ok(())
    }
}

/// Where an app password came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// [`PASSWORD_VAR`].
    Environment,
    /// The `password-command` setting.
    Command,
    /// The secret store.
    Store,
}

/// Why there's no app password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordError {
    /// The secret store has none for the account: the user needs to log in.
    NotStored,
    /// `password-command` failed.
    Command(String),
    Store(SecretError),
}

impl fmt::Display for PasswordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PasswordError::NotStored => write!(
                f,
                "no app password in the Secret Service: store one with ren --set-password, \
                 or set password-command in [account]"
            ),
            PasswordError::Command(err) => write!(f, "password-command: {err}"),
            PasswordError::Store(err) => write!(
                f,
                "{err}; without a Secret Service, set password-command in [account]"
            ),
        }
    }
}

impl std::error::Error for PasswordError {}

/// The app password of `account`, from the first of:
///
/// 1. `env_password` (from [`PASSWORD_VAR`]), unless it's empty,
/// 2. the account's `password-command`, if set,
/// 3. the secret store, which `store` connects to only then.
pub fn app_password<S: SecretStore>(
    account: &Account,
    env_password: Option<String>,
    store: impl FnOnce() -> Result<S, SecretError>,
) -> Result<(String, Source), PasswordError> {
    if let Some(password) = env_password.filter(|p| !p.is_empty()) {
        return Ok((password, Source::Environment));
    }
    if let Some(command) = &account.password_command {
        return run_password_command(command)
            .map(|password| (password, Source::Command))
            .map_err(PasswordError::Command);
    }
    let store = store().map_err(PasswordError::Store)?;
    match store.password(account) {
        Ok(Some(password)) if !password.is_empty() => Ok((password, Source::Store)),
        Ok(_) => Err(PasswordError::NotStored),
        Err(err) => Err(PasswordError::Store(err)),
    }
}

/// The first line the shell command `command` prints.
pub fn run_password_command(command: &str) -> Result<String, String> {
    // stdin and stderr stay connected, so the command can ask for a
    // passphrase (pass, gpg) and report errors.
    let output = Command::new("sh")
        .args(["-c", command])
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|err| format!("cannot run it: {err}"))?;
    if !output.status.success() {
        return Err(format!("failed ({})", output.status));
    }
    let stdout =
        String::from_utf8(output.stdout).map_err(|_| "printed invalid UTF-8".to_owned())?;
    match stdout.lines().next() {
        Some(line) if !line.is_empty() => Ok(line.to_owned()),
        _ => Err("printed no password".to_owned()),
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

    fn store_with(password: &str) -> MemoryStore {
        let store = MemoryStore::default();
        store.set_password(&account(None), password).unwrap();
        store
    }

    #[test]
    fn memory_store() {
        let store = MemoryStore::default();
        let account = account(None);
        let other = Account {
            user: "other".to_owned(),
            ..account.clone()
        };
        assert_eq!(store.password(&account), Ok(None));
        store.set_password(&account, "secret").unwrap();
        store.set_password(&other, "other secret").unwrap();
        assert_eq!(store.password(&account), Ok(Some("secret".to_owned())));
        store.set_password(&account, "new").unwrap();
        assert_eq!(store.password(&account), Ok(Some("new".to_owned())));
        store.delete_password(&account).unwrap();
        store.delete_password(&account).unwrap();
        assert_eq!(store.password(&account), Ok(None));
        assert_eq!(store.password(&other), Ok(Some("other secret".to_owned())));

        let locked = MemoryStore {
            failure: Some(SecretError::Denied("locked".to_owned())),
            ..MemoryStore::default()
        };
        assert!(locked.password(&account).is_err());
        assert!(locked.set_password(&account, "x").is_err());
    }

    #[test]
    fn environment_first() {
        let account = account(Some("echo from-command"));
        let result = app_password(&account, Some("from-env".to_owned()), || {
            Ok::<_, SecretError>(store_with("stored"))
        });
        assert_eq!(result, Ok(("from-env".to_owned(), Source::Environment)));
    }

    #[test]
    fn command_before_store() {
        let account = account(Some("printf 'from-command\\nsecond line\\n'"));
        let result = app_password(
            &account,
            Some(String::new()),
            || -> Result<MemoryStore, _> { panic!("the store isn't needed") },
        );
        assert_eq!(result, Ok(("from-command".to_owned(), Source::Command)));
    }

    #[test]
    fn command_errors() {
        let failing = |command| {
            app_password(&account(Some(command)), None, || {
                Ok::<_, SecretError>(store_with("stored"))
            })
            .unwrap_err()
        };
        assert_eq!(
            failing("exit 3"),
            PasswordError::Command("failed (exit status: 3)".to_owned())
        );
        assert_eq!(
            failing("true"),
            PasswordError::Command("printed no password".to_owned())
        );
    }

    #[test]
    fn store_last() {
        let result = app_password(&account(None), None, || {
            Ok::<_, SecretError>(store_with("stored"))
        });
        assert_eq!(result, Ok(("stored".to_owned(), Source::Store)));

        let result = app_password(&account(None), None, || {
            Ok::<_, SecretError>(MemoryStore::default())
        });
        assert_eq!(result, Err(PasswordError::NotStored));

        let unavailable = SecretError::Unavailable("no bus".to_owned());
        let result = app_password(&account(None), None, || {
            Err::<MemoryStore, _>(unavailable.clone())
        });
        assert_eq!(result, Err(PasswordError::Store(unavailable)));

        let denied = SecretError::Denied("dismissed".to_owned());
        let store = MemoryStore {
            failure: Some(denied.clone()),
            ..MemoryStore::default()
        };
        let result = app_password(&account(None), None, || Ok::<_, SecretError>(store));
        assert_eq!(result, Err(PasswordError::Store(denied)));
    }

    #[test]
    fn usernames() {
        assert_eq!(username(&account(None)), "user@https://cloud.example.org");
    }
}
