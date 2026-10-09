// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sync on a background thread.
//!
//! One thread per sync, started on request and never two at once. The
//! thread fetches the app password if the UI has none yet (the Secret
//! Service blocks, also while the keyring asks to be unlocked, so this
//! never happens on the UI thread; `docs/decisions/0008-settings.md`),
//! opens its own connection to the database, and runs [`ren_sync::sync`]
//! at a lower priority. It reports through a channel; the UI is woken up
//! to read it. Everything the thread opened (the HTTP agent, the database
//! connection, the D-Bus connection) goes with it, and the memory it
//! freed is given back to the system.

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use nextcloud_news::{Client, Config, Credentials};
use ren_settings::{Account, PASSWORD_VAR, SecretService};
use ren_store::Store;
use ren_sync::{Kind, Progress, Report, SystemClock};

use crate::procstat::trim_heap;
use crate::remote::USER_AGENT;

/// The `nice` increment of the sync thread: below the UI, but not idle
/// priority, so a sync still finishes on a busy machine.
const NICENESS: i32 = 10;

/// `sync_state` key of the account a database belongs to.
const ACCOUNT_KEY: &str = "account";

/// What a sync needs.
pub struct Job {
    pub database: PathBuf,
    pub account: Account,
    /// The app password, if the UI has it from an earlier sync.
    pub password: Option<String>,
    pub options: ren_sync::Options,
}

/// What the sync thread reports.
#[derive(Debug)]
pub enum Event {
    /// The app password, fetched for this sync, for the next ones.
    Password(String),
    Progress(Progress),
    Finished(Result<Report, Failure>),
}

#[derive(Debug)]
pub enum Failure {
    /// No app password.
    Password(String),
    /// The database was synced with another account.
    OtherAccount,
    Sync(ren_sync::Error),
}

impl Failure {
    /// The server refused the app password: it was revoked or changed.
    pub fn unauthorized(&self) -> bool {
        matches!(
            self,
            Failure::Sync(ren_sync::Error::Api(nextcloud_news::Error::Unauthorized))
        )
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Password(err) => write!(f, "{err}"),
            Failure::OtherAccount => write!(
                f,
                "the database belongs to another account; move it away to start anew"
            ),
            Failure::Sync(err) => write!(f, "{err}"),
        }
    }
}

/// A 64-bit FNV-1a hash: stable across Rust versions, unlike the
/// standard library's hasher.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Checks that the database belongs to `account`, and records that it
/// does if it belongs to none yet. Changes queued for one account must
/// never be sent to another: the item ids would mean other items.
pub fn claim_database(store: &mut Store, account: &Account) -> ren_store::Result<bool> {
    let id = fnv1a(&format!("{}@{}", account.user, account.server)).cast_signed();
    match store.sync_value(ACCOUNT_KEY)? {
        Some(stored) => Ok(stored == id),
        None => {
            store.set_sync_values(&[(ACCOUNT_KEY, Some(id))])?;
            Ok(true)
        }
    }
}

/// Lowers the priority of the calling thread. On Linux, `nice` changes
/// only the calling thread, not the whole process.
fn lower_priority() {
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn nice(inc: std::ffi::c_int) -> std::ffi::c_int;
        }
        // SAFETY: nice only changes the scheduling priority; it takes no
        // pointers. Failing (-1 with errno) just leaves the priority.
        unsafe { nice(NICENESS) };
    }
}

/// A sync running on its thread.
pub struct Running {
    events: Receiver<Event>,
    cancel: Arc<AtomicBool>,
}

impl Running {
    /// Asks the sync to stop after the current chunk of items.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The events received so far.
    pub fn events(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter()
    }
}

/// Dropping a running sync (the window closed) cancels it.
impl Drop for Running {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Starts a sync on a new thread. `wake` is called after each event, from
/// the sync thread, to have the UI read the events.
pub fn start(job: Job, wake: impl Fn() + Send + 'static) -> std::io::Result<Running> {
    let (sender, events) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    std::thread::Builder::new()
        .name("sync".to_owned())
        .spawn(move || {
            lower_priority();
            let send = |event| {
                // The UI is gone if this fails; the sync then ends at the
                // next progress.
                let _ = Sender::send(&sender, event);
                wake();
            };
            let result = run(job, &stop, &send);
            send(Event::Finished(result));
        })?;
    Ok(Running { events, cancel })
}

fn run(job: Job, cancel: &AtomicBool, send: &impl Fn(Event)) -> Result<Report, Failure> {
    let password = match job.password {
        Some(password) => password,
        None => {
            let (password, _) = ren_settings::app_password(
                &job.account,
                std::env::var(PASSWORD_VAR).ok(),
                SecretService::connect,
            )
            .map_err(|err| Failure::Password(err.to_string()))?;
            send(Event::Password(password.clone()));
            password
        }
    };
    let credentials = Credentials {
        user: job.account.user.clone(),
        password,
    };
    let client = Client::new(&job.account.server, &credentials, &Config::new(USER_AGENT));
    let mut store = Store::open(&job.database).map_err(|err| Failure::Sync(err.into()))?;
    if !claim_database(&mut store, &job.account).map_err(|err| Failure::Sync(err.into()))? {
        return Err(Failure::OtherAccount);
    }
    let result = ren_sync::sync(
        &client,
        &mut store,
        &SystemClock,
        &job.options,
        |progress| {
            send(Event::Progress(progress));
            if cancel.load(Ordering::Relaxed) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        },
    );
    drop((store, client));
    // What the thread freed stays with the allocator otherwise: 2–3 MiB
    // after every incremental sync too, unlike the command line's sync on
    // the main thread (0007, 0009).
    trim_heap();
    result.map_err(Failure::Sync)
}

/// The status bar text while a sync runs.
pub fn progress_text(progress: Option<Progress>) -> String {
    match progress {
        None => "Syncing: sending changes…".to_owned(),
        Some(Progress::Pushed) => "Syncing: fetching feeds…".to_owned(),
        Some(Progress::Feeds) => "Syncing: fetching articles…".to_owned(),
        Some(Progress::Items {
            stored,
            expected: Some(expected),
        }) => format!(
            "Syncing: {stored} of about {} articles",
            expected.max(stored as u64)
        ),
        Some(Progress::Items {
            stored,
            expected: None,
        }) => format!("Syncing: {stored} articles"),
    }
}

/// The status bar text after a sync.
pub fn result_text(result: &Result<Report, Failure>) -> String {
    let report = match result {
        Ok(report) => report,
        Err(Failure::Sync(ren_sync::Error::Cancelled)) => return "Sync cancelled".to_owned(),
        Err(err) => return format!("Sync failed: {err}"),
    };
    let mut text = match (report.kind, report.added) {
        (Kind::Initial, _) => format!("Synced: {} articles", report.items),
        (_, 0) => "Synced: no new articles".to_owned(),
        (_, 1) => "Synced: 1 new article".to_owned(),
        (_, added) => format!("Synced: {added} new articles"),
    };
    if let Some(err) = &report.fallback {
        text.push_str(&format!(
            " (fetching the changes failed, synced all instead: {err})"
        ));
    }
    match report.push_errors.as_slice() {
        [] => {}
        [err] => text.push_str(&format!(
            "; the server refused a change, kept for the next sync: {err}"
        )),
        [err, ..] => text.push_str(&format!(
            "; the server refused {} changes, kept for the next sync: {err}",
            report.push_errors.len()
        )),
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(user: &str) -> Account {
        Account {
            server: "https://cloud.example.org".to_owned(),
            user: user.to_owned(),
            password_command: None,
        }
    }

    #[test]
    fn a_database_belongs_to_one_account() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(claim_database(&mut store, &account("ada")).unwrap());
        assert!(claim_database(&mut store, &account("ada")).unwrap());
        assert!(!claim_database(&mut store, &account("bob")).unwrap());
        let mut other_server = account("ada");
        other_server.server = "https://other.example.org".to_owned();
        assert!(!claim_database(&mut store, &other_server).unwrap());
    }

    #[test]
    fn stable_hash() {
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn progress() {
        assert_eq!(progress_text(None), "Syncing: sending changes…");
        assert_eq!(
            progress_text(Some(Progress::Items {
                stored: 1200,
                expected: Some(63_000)
            })),
            "Syncing: 1200 of about 63000 articles"
        );
        // The expected number is an estimate.
        assert_eq!(
            progress_text(Some(Progress::Items {
                stored: 1200,
                expected: Some(1000)
            })),
            "Syncing: 1200 of about 1200 articles"
        );
        assert_eq!(
            progress_text(Some(Progress::Items {
                stored: 200,
                expected: None
            })),
            "Syncing: 200 articles"
        );
    }

    #[test]
    fn results() {
        let report = |kind, added| Report {
            kind,
            added,
            items: 500,
            ..Report::default()
        };
        assert_eq!(
            result_text(&Ok(report(Kind::Initial, 500))),
            "Synced: 500 articles"
        );
        assert_eq!(
            result_text(&Ok(report(Kind::Incremental, 0))),
            "Synced: no new articles"
        );
        assert_eq!(
            result_text(&Ok(report(Kind::Resync, 1))),
            "Synced: 1 new article"
        );
        let mut refused = report(Kind::Incremental, 3);
        refused.push_errors = vec![
            nextcloud_news::Error::Status {
                code: 500,
                message: None,
            },
            nextcloud_news::Error::Unauthorized,
        ];
        assert_eq!(
            result_text(&Ok(refused)),
            "Synced: 3 new articles; the server refused 2 changes, kept for the next sync: \
             HTTP status 500"
        );
        assert_eq!(
            result_text(&Err(Failure::Sync(ren_sync::Error::Cancelled))),
            "Sync cancelled"
        );
        let unauthorized = Failure::Sync(ren_sync::Error::Api(nextcloud_news::Error::Unauthorized));
        assert!(unauthorized.unauthorized());
        assert_eq!(
            result_text(&Err(unauthorized)),
            "Sync failed: server: authentication failed (HTTP 401)"
        );
        assert!(!Failure::OtherAccount.unauthorized());
    }
}
