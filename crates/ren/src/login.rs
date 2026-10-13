// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Logging in and out for the account page, on a thread of its own: the
//! requests to the server and the Secret Service block (the Secret
//! Service also while the keyring asks to be unlocked).
//!
//! - **In the browser** (Login Flow v2): the server's login page opens in
//!   the browser, where the user logs in (2FA and SSO included) and grants
//!   access; the thread asks the server every two seconds whether that
//!   happened, for at most 20 minutes, and gets a new app password.
//! - **With an app password** the user created in Nextcloud by hand: it is
//!   checked with the server first.
//! - **Logging out** revokes the app password on the server and removes it
//!   from the Secret Service. A password from `REN_APP_PASSWORD` or the
//!   password command isn't revoked: ren didn't create it, and it may be
//!   in use elsewhere (or be the account's own password).
//!
//! A login stores the app password in the Secret Service. Without one,
//! the password is still handed to the window, which keeps it until ren
//! quits.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use nextcloud_news::{
    Client, Config, Credentials, Error, LOGIN_TIMEOUT, LoginFlow, revoke_app_password,
};
use ren_settings::schema::{check_server, normalise_server};
use ren_settings::{Account, SecretError, SecretService, SecretStore, Source};

use crate::remote::USER_AGENT;

/// How often the server is asked whether the user logged in.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// What the account page asks for.
#[derive(Debug, Clone)]
pub enum Request {
    /// Log in in the browser, on `server` as typed.
    Browser { server: String },
    /// Log in with an app password created in Nextcloud.
    Password {
        server: String,
        user: String,
        password: String,
    },
    /// Log out: revoke the app password, which the window may have from a
    /// sync or a login, with where it came from; else it is looked up as
    /// for a sync.
    Logout {
        account: Account,
        password: Option<(String, Source)>,
    },
}

/// What the thread reports.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// The login page to open in the browser; the thread waits for the
    /// user now.
    Waiting(String),
    /// Logged in. The account has no `password_command`; `stored` says
    /// where the password was put: the Secret Service's provider, or why
    /// it wasn't stored.
    LoggedIn {
        account: Account,
        password: String,
        stored: Result<(), String>,
    },
    /// Logged out: what happened to the app password on the server, and
    /// whether removing it from the Secret Service failed.
    LoggedOut {
        revoked: Revoked,
        removed: Result<(), String>,
    },
    Failed(String),
    Cancelled,
}

/// What logging out did to the password on the server.
#[derive(Debug, PartialEq, Eq)]
pub enum Revoked {
    /// Revoked, now or before (e.g. in the web interface).
    Yes,
    /// Not tried: the password comes from `REN_APP_PASSWORD` or the
    /// password command, not from ren's login.
    NotOurs,
    /// The server says the request didn't log in with an app password:
    /// it was the account's own password, which this can't revoke.
    AccountPassword,
    Failed(String),
}

/// Where the app password for `account` comes from when the window has
/// none, as a sync looks it up: `REN_APP_PASSWORD` (`env_password`), the
/// password command, the Secret Service.
pub fn source(account: &Account, env_password: Option<&str>) -> Source {
    if env_password.is_some_and(|password| !password.is_empty()) {
        Source::Environment
    } else if account.password_command.is_some() {
        Source::Command
    } else {
        Source::Store
    }
}

/// Whether logging out revokes a password from `source` on the server:
/// only ren's own, from a login (kept in the Secret Service, or in memory
/// without one), not one from `REN_APP_PASSWORD` or the password command,
/// which ren didn't create.
pub fn revokes(source: Source) -> bool {
    source == Source::Store
}

/// A login or logout running on its thread. Dropping it cancels a login
/// waiting for the browser.
pub struct Running {
    events: Receiver<Event>,
    cancel: Arc<AtomicBool>,
}

impl Running {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The events received so far.
    pub fn events(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Timings of a login, shortened in tests.
struct Timing {
    poll: Duration,
    timeout: Duration,
}

/// Starts `request` on a new thread; `wake` is called after each event,
/// from that thread.
pub fn start(request: Request, wake: impl Fn() + Send + 'static) -> std::io::Result<Running> {
    let (sender, events) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    std::thread::Builder::new()
        .name("login".to_owned())
        .spawn(move || {
            let send = |event| {
                let _ = sender.send(event);
                wake();
            };
            let timing = Timing {
                poll: POLL_INTERVAL,
                timeout: LOGIN_TIMEOUT,
            };
            let env = std::env::var(ren_settings::PASSWORD_VAR).ok();
            let last = run(request, &timing, &stop, &send, SecretService::connect, env);
            send(last);
        })?;
    Ok(Running { events, cancel })
}

fn config() -> Config {
    let mut config = Config::new(USER_AGENT);
    // Logging in waits for nothing big.
    config.response_timeout = Duration::from_secs(30);
    config.body_timeout = Duration::from_secs(30);
    config.body_limit = 1024 * 1024;
    config
}

/// Runs a request; returns the last event.
fn run<S: SecretStore>(
    request: Request,
    timing: &Timing,
    cancel: &AtomicBool,
    send: &dyn Fn(Event),
    store: impl Fn() -> Result<S, SecretError>,
    env_password: Option<String>,
) -> Event {
    let result = match request {
        Request::Browser { server } => browser(&server, timing, cancel, send),
        Request::Password {
            server,
            user,
            password,
        } => with_password(&server, &user, &password),
        Request::Logout { account, password } => {
            return logout(&account, password, &store, env_password);
        }
    };
    match result {
        Ok((account, password)) => {
            let stored = store()
                .and_then(|store| store.set_password(&account, &password))
                .map_err(|err| err.to_string());
            Event::LoggedIn {
                account,
                password,
                stored,
            }
        }
        Err(event) => event,
    }
}

/// The server as typed, as the settings keep it, if it is one.
fn server(input: &str) -> Result<String, Event> {
    let server = normalise_server(input);
    if server.is_empty() {
        return Err(Event::Failed(
            "Enter the address of your Nextcloud.".to_owned(),
        ));
    }
    check_server(&server)
        .map(|()| server)
        .map_err(|err| Event::Failed(format!("The server {err}.")))
}

fn browser(
    input: &str,
    timing: &Timing,
    cancel: &AtomicBool,
    send: &dyn Fn(Event),
) -> Result<(Account, String), Event> {
    let server = server(input)?;
    let flow = LoginFlow::start(&server, &config())
        .map_err(|err| Event::Failed(format!("{server}: {err}")))?;
    send(Event::Waiting(flow.login_url.clone()));
    let started = Instant::now();
    let login = loop {
        if wait(cancel, timing.poll) {
            return Err(Event::Cancelled);
        }
        match flow.poll() {
            Ok(Some(login)) => break login,
            Ok(None) => {}
            // A network that comes and goes (a laptop's) doesn't end it.
            Err(Error::Network(_)) => {}
            Err(err) => return Err(Event::Failed(format!("{server}: {err}"))),
        }
        if started.elapsed() >= timing.timeout {
            return Err(Event::Failed(
                "The login wasn't completed in the browser within 20 minutes.".to_owned(),
            ));
        }
    };
    // The server's own address, unless it isn't one ren accepts (http
    // behind a proxy that isn't set up for https).
    let own = normalise_server(&login.server);
    let server = if check_server(&own).is_ok() {
        own
    } else {
        server
    };
    let account = Account {
        server,
        user: login.login_name,
        password_command: None,
    };
    Ok((account, login.app_password))
}

/// Sleeps for `duration`, in short steps; whether the request was
/// cancelled.
fn wait(cancel: &AtomicBool, duration: Duration) -> bool {
    let step = Duration::from_millis(50);
    let until = Instant::now() + duration;
    while Instant::now() < until {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        std::thread::sleep(step.min(until.saturating_duration_since(Instant::now())));
    }
    cancel.load(Ordering::Relaxed)
}

fn with_password(input: &str, user: &str, password: &str) -> Result<(Account, String), Event> {
    let server = server(input)?;
    let user = user.trim();
    let password = password.trim();
    if user.is_empty() || password.is_empty() {
        return Err(Event::Failed(
            "Enter your login name and an app password.".to_owned(),
        ));
    }
    let credentials = Credentials {
        user: user.to_owned(),
        password: password.to_owned(),
    };
    let client = Client::new(&server, &credentials, &config());
    match client.version() {
        Ok(_) => Ok((
            Account {
                server,
                user: user.to_owned(),
                password_command: None,
            },
            password.to_owned(),
        )),
        Err(Error::Unauthorized) => Err(Event::Failed(
            "The server refused the login name or the app password.".to_owned(),
        )),
        Err(err) => Err(Event::Failed(format!("{server}: {err}"))),
    }
}

fn logout<S: SecretStore>(
    account: &Account,
    password: Option<(String, Source)>,
    store: &impl Fn() -> Result<S, SecretError>,
    env_password: Option<String>,
) -> Event {
    let password = match password {
        Some(password) => Ok(password),
        None => ren_settings::app_password(account, env_password.clone(), store)
            .map_err(|err| err.to_string()),
    };
    let revoked = match password {
        Ok((password, source)) if revokes(source) => revoke(account, password),
        Ok(_) => Revoked::NotOurs,
        // Nothing to revoke with, but one from outside isn't revoked anyway.
        Err(_) if !revokes(source(account, env_password.as_deref())) => Revoked::NotOurs,
        Err(err) => Revoked::Failed(err),
    };
    let removed = match store().and_then(|store| store.delete_password(account)) {
        // Nothing to remove without a Secret Service.
        Ok(()) | Err(SecretError::Unavailable(_)) => Ok(()),
        Err(err) => Err(err.to_string()),
    };
    Event::LoggedOut { revoked, removed }
}

fn revoke(account: &Account, password: String) -> Revoked {
    let credentials = Credentials {
        user: account.user.clone(),
        password,
    };
    match revoke_app_password(&account.server, &credentials, &config()) {
        // Revoked already, e.g. in the web interface.
        Ok(()) | Err(Error::Unauthorized) => Revoked::Yes,
        // Nextcloud's answer when the request didn't log in with an app
        // password ("no app password in use"): nothing was revoked.
        Err(Error::Status { code: 403, .. }) => Revoked::AccountPassword,
        Err(err) => Revoked::Failed(format!("{}: {err}", account.server)),
    }
}

/// Who is logged in where, and where the app password in use comes from,
/// for the account page.
pub fn summary(account: &Account, source: Source) -> String {
    let mut text = format!("Logged in as {} on {}.", account.user, account.server);
    match source {
        Source::Environment => text.push_str(" The app password comes from REN_APP_PASSWORD."),
        Source::Command => {
            text.push_str(" The app password comes from the password command (below).");
        }
        Source::Store => {}
    }
    text
}

/// What the account page says after a login, and whether it is a problem;
/// `env_password` is `REN_APP_PASSWORD`, which syncs prefer after a
/// restart.
pub fn login_message(
    account: &Account,
    stored: &Result<(), String>,
    env_password: Option<&str>,
) -> (String, bool) {
    let (mut text, mut error) = match stored {
        Ok(()) => (
            "Logged in; the app password is kept in the Secret Service.".to_owned(),
            false,
        ),
        Err(err) => (
            format!(
                "Logged in, but the app password couldn't be stored ({err}). ren keeps it \
                 until it quits; without a Secret Service, set a password command below."
            ),
            true,
        ),
    };
    let other = match source(account, env_password) {
        Source::Environment => Some("REN_APP_PASSWORD is set"),
        Source::Command => Some("The password command is set"),
        Source::Store => None,
    };
    if let Some(other) = other {
        text.push_str(&format!(
            " {other}: ren uses the new app password until it quits, and that one again \
             after a restart; remove it to keep the new one."
        ));
        error = true;
    }
    (text, error)
}

/// What the account page says after logging out, and whether something
/// failed.
pub fn logout_message(revoked: &Revoked, removed: &Result<(), String>) -> (String, bool) {
    let (mut text, mut error) = match revoked {
        Revoked::Yes => (
            "Logged out; the app password was revoked on the server.".to_owned(),
            false,
        ),
        Revoked::NotOurs => (
            "Logged out here. The app password comes from REN_APP_PASSWORD or the password \
             command, so ren didn't revoke it; remove it in your Nextcloud's security \
             settings if nothing else needs it."
                .to_owned(),
            false,
        ),
        Revoked::AccountPassword => (
            "Logged out here. The server says the password was your account's own, not an \
             app password, so nothing was revoked. Log in through the browser to give ren \
             an app password of its own."
                .to_owned(),
            true,
        ),
        Revoked::Failed(err) => (
            format!(
                "Logged out here, but the app password couldn't be revoked on the server \
                 ({err}); remove it in your Nextcloud's security settings."
            ),
            true,
        ),
    };
    if let Err(err) = removed {
        text.push_str(&format!(" Removing it from this computer failed: {err}"));
        error = true;
    }
    (text, error)
}

#[cfg(test)]
#[path = "../../nextcloud-news/tests/mock/mod.rs"]
mod mock;

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    use ren_settings::MemoryStore;

    use super::mock::{self, MockServer, Response};
    use super::*;

    const FAST: Timing = Timing {
        poll: Duration::from_millis(10),
        timeout: Duration::from_secs(5),
    };

    /// A Nextcloud whose login completes on the third poll, and which
    /// takes the app password "app-pw" of "ada".
    fn nextcloud() -> MockServer {
        let polls = Arc::new(AtomicUsize::new(0));
        let base = Arc::new(Mutex::new(String::new()));
        let base_in = base.clone();
        let server = MockServer::start(move |request| {
            let base = base_in.lock().unwrap().clone();
            let authorized = request.header("authorization") == Some("Basic YWRhOmFwcC1wdw==");
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/index.php/login/v2") => Response::json(format!(
                    r#"{{"poll": {{"token": "t", "endpoint": "{base}/poll"}},
                        "login": "{base}/flow"}}"#
                )),
                ("POST", "/poll") if polls.fetch_add(1, Ordering::SeqCst) < 2 => {
                    Response::status(404, "")
                }
                ("POST", "/poll") => Response::json(format!(
                    r#"{{"server": "{base}/", "loginName": "ada", "appPassword": "app-pw"}}"#
                )),
                ("GET", "/index.php/apps/news/api/v1-3/version") if authorized => {
                    Response::json(r#"{"version": "28.7.0"}"#)
                }
                ("DELETE", "/ocs/v2.php/core/apppassword") if authorized => {
                    Response::json(r#"{"ocs": {}}"#)
                }
                // "ada:secret", the account's own password: Nextcloud
                // revokes only the app password a request logged in with.
                ("DELETE", "/ocs/v2.php/core/apppassword")
                    if request.header("authorization") == Some("Basic YWRhOnNlY3JldA==") =>
                {
                    Response::status(
                        403,
                        r#"{"ocs": {"meta": {"status": "failure", "statuscode": 403, "message": "no app password in use"}, "data": []}}"#,
                    )
                }
                _ => Response::status(401, ""),
            }
        });
        *base.lock().unwrap() = server.url();
        server
    }

    fn run_with(request: Request, store: &Arc<MemoryStore>) -> (Vec<Event>, Event) {
        run_with_env(request, store, None)
    }

    /// With `env_password` as `REN_APP_PASSWORD`.
    fn run_with_env(
        request: Request,
        store: &Arc<MemoryStore>,
        env_password: Option<&str>,
    ) -> (Vec<Event>, Event) {
        let events = Mutex::new(Vec::new());
        let send = |event| events.lock().unwrap().push(event);
        let store = store.clone();
        let last = run(
            request,
            &FAST,
            &AtomicBool::new(false),
            &send,
            || Ok(StoreRef(store.clone())),
            env_password.map(str::to_owned),
        );
        (events.into_inner().unwrap(), last)
    }

    /// A shared [`MemoryStore`], as a store connected to anew each time.
    struct StoreRef(Arc<MemoryStore>);

    impl SecretStore for StoreRef {
        fn password(&self, account: &Account) -> Result<Option<String>, SecretError> {
            self.0.password(account)
        }
        fn set_password(&self, account: &Account, password: &str) -> Result<(), SecretError> {
            self.0.set_password(account, password)
        }
        fn delete_password(&self, account: &Account) -> Result<(), SecretError> {
            self.0.delete_password(account)
        }
    }

    fn ada(server: &MockServer) -> Account {
        Account {
            server: server.url(),
            user: "ada".to_owned(),
            password_command: None,
        }
    }

    #[test]
    fn login_in_the_browser() {
        let server = nextcloud();
        let store = Arc::new(MemoryStore::default());
        let (events, last) = run_with(
            Request::Browser {
                server: format!(" {}/ ", server.url()),
            },
            &store,
        );
        assert_eq!(events, [Event::Waiting(format!("{}/flow", server.url()))]);
        assert_eq!(
            last,
            Event::LoggedIn {
                account: ada(&server),
                password: "app-pw".to_owned(),
                stored: Ok(()),
            }
        );
        assert_eq!(
            store.password(&ada(&server)).unwrap().as_deref(),
            Some("app-pw")
        );
    }

    #[test]
    fn login_without_a_secret_service() {
        let server = nextcloud();
        let mut store = MemoryStore::default();
        store.failure = Some(SecretError::Unavailable("no bus".to_owned()));
        let store = Arc::new(store);
        let (_, last) = run_with(
            Request::Browser {
                server: server.url(),
            },
            &store,
        );
        let Event::LoggedIn {
            password, stored, ..
        } = last
        else {
            panic!("not logged in: {last:?}");
        };
        // The window keeps it.
        assert_eq!(password, "app-pw");
        assert_eq!(stored, Err("no Secret Service: no bus".to_owned()));
    }

    #[test]
    fn login_cancelled_and_failing() {
        let server = nextcloud();
        let cancel = AtomicBool::new(true);
        let last = run(
            Request::Browser {
                server: server.url(),
            },
            &FAST,
            &cancel,
            &|_| {},
            || Ok(MemoryStore::default()),
            None,
        );
        assert_eq!(last, Event::Cancelled);

        let store = Arc::new(MemoryStore::default());
        let failed = |input: &str| match run_with(
            Request::Browser {
                server: input.to_owned(),
            },
            &store,
        )
        .1
        {
            Event::Failed(message) => message,
            other => panic!("{other:?}"),
        };
        assert_eq!(failed(" "), "Enter the address of your Nextcloud.");
        assert!(failed("http://cloud.example.org").contains("must be an https:// URL"));
        assert!(failed(&mock::closed_port_url()).starts_with("http://127.0.0.1:"));
    }

    #[test]
    fn login_with_an_app_password() {
        let server = nextcloud();
        let store = Arc::new(MemoryStore::default());
        let (_, last) = run_with(
            Request::Password {
                server: server.url(),
                user: " ada ".to_owned(),
                password: "app-pw\n".to_owned(),
            },
            &store,
        );
        assert_eq!(
            last,
            Event::LoggedIn {
                account: ada(&server),
                password: "app-pw".to_owned(),
                stored: Ok(()),
            }
        );
        let (_, last) = run_with(
            Request::Password {
                server: server.url(),
                user: "ada".to_owned(),
                password: "wrong".to_owned(),
            },
            &store,
        );
        assert_eq!(
            last,
            Event::Failed("The server refused the login name or the app password.".to_owned())
        );
    }

    #[test]
    fn messages() {
        let mut account = Account {
            server: "https://x.org".to_owned(),
            user: "ada".to_owned(),
            password_command: None,
        };
        assert_eq!(
            summary(&account, Source::Store),
            "Logged in as ada on https://x.org."
        );
        assert!(summary(&account, Source::Environment).contains("REN_APP_PASSWORD"));
        assert!(summary(&account, Source::Command).contains("password command"));
        // Where a sync looks first, without a password in the window.
        assert_eq!(source(&account, None), Source::Store);
        assert_eq!(source(&account, Some("")), Source::Store);
        assert_eq!(source(&account, Some("pw")), Source::Environment);
        assert!(revokes(Source::Store));
        assert!(!revokes(Source::Environment) && !revokes(Source::Command));
        assert!(!login_message(&account, &Ok(()), None).1);
        let (text, error) = login_message(&account, &Err("no Secret Service".to_owned()), None);
        assert!(error);
        assert!(text.contains("(no Secret Service)"), "{text}");
        // A login while REN_APP_PASSWORD is set: it wins after a restart.
        let (text, error) = login_message(&account, &Ok(()), Some("pw"));
        assert!(error && text.contains("REN_APP_PASSWORD is set"), "{text}");
        account.password_command = Some("pass x".to_owned());
        assert_eq!(source(&account, None), Source::Command);
        let (text, error) = login_message(&account, &Ok(()), None);
        assert!(error && text.contains("password command is set"), "{text}");

        assert_eq!(
            logout_message(&Revoked::Yes, &Ok(())),
            (
                "Logged out; the app password was revoked on the server.".to_owned(),
                false
            )
        );
        let (text, error) = logout_message(&Revoked::NotOurs, &Ok(()));
        assert!(!error && text.contains("didn't revoke"), "{text}");
        let (text, error) = logout_message(&Revoked::AccountPassword, &Ok(()));
        assert!(error && text.contains("account's own"), "{text}");
        let (text, error) = logout_message(
            &Revoked::Failed("HTTP status 500".to_owned()),
            &Err("locked".to_owned()),
        );
        assert!(error);
        assert!(
            text.contains("(HTTP status 500)") && text.ends_with("failed: locked"),
            "{text}"
        );
    }

    #[test]
    fn logout() {
        let server = nextcloud();
        let store = Arc::new(MemoryStore::default());
        store.set_password(&ada(&server), "app-pw").unwrap();
        // The password from the store.
        let (_, last) = run_with(
            Request::Logout {
                account: ada(&server),
                password: None,
            },
            &store,
        );
        assert_eq!(
            last,
            Event::LoggedOut {
                revoked: Revoked::Yes,
                removed: Ok(())
            }
        );
        assert_eq!(store.password(&ada(&server)).unwrap(), None);
        let revoke = server
            .requests()
            .into_iter()
            .find(|request| request.method == "DELETE")
            .unwrap();
        assert_eq!(revoke.header("ocs-apirequest"), Some("true"));

        // Revoked already (401), and nothing stored any more.
        let (_, last) = run_with(
            Request::Logout {
                account: ada(&server),
                password: Some(("old".to_owned(), Source::Store)),
            },
            &store,
        );
        assert_eq!(
            last,
            Event::LoggedOut {
                revoked: Revoked::Yes,
                removed: Ok(())
            }
        );
        let (_, last) = run_with(
            Request::Logout {
                account: ada(&server),
                password: None,
            },
            &store,
        );
        let Event::LoggedOut { revoked, removed } = last else {
            panic!("{last:?}");
        };
        let Revoked::Failed(err) = revoked else {
            panic!("{revoked:?}");
        };
        assert!(err.contains("no app password"), "{err}");
        assert_eq!(removed, Ok(()));
    }

    #[test]
    fn logout_revokes_a_logins_password_with_the_variable_set() {
        // Logged in through ren while REN_APP_PASSWORD was set: the window
        // has the login's password, which is ren's own.
        let server = nextcloud();
        let store = Arc::new(MemoryStore::default());
        let (_, last) = run_with_env(
            Request::Logout {
                account: ada(&server),
                password: Some(("app-pw".to_owned(), Source::Store)),
            },
            &store,
            Some("secret"),
        );
        assert_eq!(
            last,
            Event::LoggedOut {
                revoked: Revoked::Yes,
                removed: Ok(())
            }
        );
    }

    #[test]
    fn logout_with_the_accounts_own_password() {
        let server = nextcloud();
        let store = Arc::new(MemoryStore::default());
        store.set_password(&ada(&server), "secret").unwrap();
        let (_, last) = run_with(
            Request::Logout {
                account: ada(&server),
                password: None,
            },
            &store,
        );
        assert_eq!(
            last,
            Event::LoggedOut {
                revoked: Revoked::AccountPassword,
                removed: Ok(())
            }
        );
    }

    #[test]
    fn logout_without_revoking_a_password_from_outside() {
        let server = nextcloud();
        let store = Arc::new(MemoryStore::default());
        // From REN_APP_PASSWORD, also when the window has it from a sync.
        let (_, last) = run_with_env(
            Request::Logout {
                account: ada(&server),
                password: Some(("app-pw".to_owned(), Source::Environment)),
            },
            &store,
            Some("app-pw"),
        );
        assert_eq!(
            last,
            Event::LoggedOut {
                revoked: Revoked::NotOurs,
                removed: Ok(())
            }
        );
        // From the password command.
        let account = Account {
            password_command: Some("echo app-pw".to_owned()),
            ..ada(&server)
        };
        let (_, last) = run_with(
            Request::Logout {
                account,
                password: None,
            },
            &store,
        );
        assert_eq!(
            last,
            Event::LoggedOut {
                revoked: Revoked::NotOurs,
                removed: Ok(())
            }
        );
        assert!(
            !server
                .requests()
                .iter()
                .any(|request| request.method == "DELETE"),
            "revoked"
        );
    }
}
