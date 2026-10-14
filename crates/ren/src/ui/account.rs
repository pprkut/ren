// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The account page of the settings dialog: logging in (in the browser or
//! with an app password) and out, through `login`'s thread.

use ren_settings::schema::{self, Category};
use ren_settings::{Settings, Source, Value};
use slint::ComponentHandle;

use super::{App, ConfirmWindow, SettingsWindow};
use crate::login::{self, Event, Request};

/// What the account page shows besides the settings.
#[derive(Default)]
pub(super) struct AccountUi {
    /// The last outcome, and whether it is an error.
    message: String,
    error: bool,
    /// The login page while waiting for the browser.
    login_url: Option<String>,
    /// A login or logout runs.
    busy: bool,
}

impl App {
    /// Opens the settings dialog on the account page.
    pub(super) fn open_account_page(&self) {
        self.open_settings();
        self.with_dialog(|dialog| dialog.select(Category::Account));
    }

    pub(super) fn connect_account(&self, window: &SettingsWindow) {
        let this = self.this.clone();
        window.on_log_in_browser(move |server| {
            if let Some(app) = this.upgrade() {
                app.start_login(Request::Browser {
                    server: server.into(),
                });
            }
        });
        let this = self.this.clone();
        window.on_log_in_password(move |server, user, password| {
            if let Some(app) = this.upgrade() {
                app.start_login(Request::Password {
                    server: server.into(),
                    user: user.into(),
                    password: password.into(),
                });
            }
        });
        let this = self.this.clone();
        window.on_open_login_url(move || {
            if let Some(app) = this.upgrade() {
                let url = app.account_ui.borrow().login_url.clone();
                if let Some(url) = url {
                    app.open_in_browser(&url);
                }
            }
        });
        let this = self.this.clone();
        window.on_cancel_login(move || {
            if let Some(app) = this.upgrade()
                && let Some(running) = &*app.login.borrow()
            {
                running.cancel();
            }
        });
        let this = self.this.clone();
        window.on_log_out(move || {
            if let Some(app) = this.upgrade() {
                app.confirm_log_out();
            }
        });
    }

    /// Starts a login, unless one runs or the settings can't be saved.
    fn start_login(&self, request: Request) {
        if self.settings_problem.borrow().is_some() {
            self.account_message("Fix the errors in the settings file first.", true);
            return;
        }
        self.run_login(request, "Logging in…");
    }

    /// Asks in a window of its own before logging out: revoking the app
    /// password can't be undone.
    fn confirm_log_out(&self) {
        if let Some(confirm) = &*self.confirm.borrow() {
            let _ = confirm.show();
            return;
        }
        let Some(account) = self.current_settings().account().ok() else {
            return;
        };
        let confirm = match ConfirmWindow::new() {
            Ok(confirm) => confirm,
            Err(err) => {
                self.account_message(&format!("Logging out failed: {err}"), true);
                return;
            }
        };
        if let Some(dark) = self.forced_dark {
            confirm.set_forced_color_scheme(if dark {
                slint::language::ColorScheme::Dark
            } else {
                slint::language::ColorScheme::Light
            });
        }
        let (text, label) = if login::revokes(self.password_source(&account)) {
            (
                "Ren's app password will be revoked on the server and removed from this \
                 computer. This can't be undone; Ren needs a new login afterwards. The \
                 articles stored here stay.",
                "Revoke and Log Out",
            )
        } else {
            (
                "Ren forgets the account here. The app password comes from outside Ren, so \
                 it isn't revoked on the server. The articles stored here stay.",
                "Log Out",
            )
        };
        confirm.set_window_title("Log Out – Ren".into());
        confirm.set_heading(format!("Log out {} on {}?", account.user, account.server).into());
        confirm.set_text(text.into());
        confirm.set_confirm_label(label.into());
        let this = self.this.clone();
        confirm.on_confirmed(move || {
            if let Some(app) = this.upgrade() {
                app.close_confirm();
                app.log_out();
            }
        });
        let this = self.this.clone();
        confirm.on_cancelled(move || {
            if let Some(app) = this.upgrade() {
                app.close_confirm();
            }
        });
        let this = self.this.clone();
        confirm.window().on_close_requested(move || {
            if let Some(app) = this.upgrade() {
                app.close_confirm();
            }
            slint::CloseRequestResponse::HideWindow
        });
        if let Err(err) = confirm.show() {
            eprintln!("ren: showing the question before logging out: {err}");
        }
        *self.confirm.borrow_mut() = Some(confirm);
    }

    /// Hides the question, and drops it: not from its own callback.
    pub(super) fn close_confirm(&self) {
        if let Some(confirm) = &*self.confirm.borrow() {
            let _ = confirm.hide();
        }
        let this = self.this.clone();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(app) = this.upgrade() {
                app.confirm.borrow_mut().take();
            }
        });
    }

    fn log_out(&self) {
        let Some(account) = self.current_settings().account().ok() else {
            return;
        };
        let password = self
            .sync
            .borrow()
            .password
            .as_ref()
            .filter(|(of, _, _)| *of == account)
            .map(|(_, password, source)| (password.clone(), *source));
        self.run_login(Request::Logout { account, password }, "Logging out…");
    }

    fn run_login(&self, request: Request, doing: &str) {
        if self.login.borrow().is_some() {
            return;
        }
        let weak = self.window.clone();
        let wake = move || {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    window.invoke_action("login-event".into());
                }
            });
        };
        match login::start(request, wake) {
            Ok(running) => {
                *self.login.borrow_mut() = Some(running);
                let mut ui = self.account_ui.borrow_mut();
                ui.busy = true;
                ui.message = doing.to_owned();
                ui.error = false;
            }
            Err(err) => self.account_message(&format!("Starting failed: {err}"), true),
        }
        self.refresh_settings_dialog();
    }

    fn account_message(&self, message: &str, error: bool) {
        {
            let mut ui = self.account_ui.borrow_mut();
            message.clone_into(&mut ui.message);
            ui.error = error;
        }
        self.refresh_settings_dialog();
    }

    /// Handles what the login thread reported.
    pub(super) fn login_events(&self) {
        let events: Vec<Event> = match &*self.login.borrow() {
            Some(running) => running.events().collect(),
            None => return,
        };
        for event in events {
            if !matches!(event, Event::Waiting(_)) {
                self.login.borrow_mut().take();
                let mut ui = self.account_ui.borrow_mut();
                ui.busy = false;
                ui.login_url = None;
            }
            match event {
                Event::Waiting(url) => {
                    {
                        let mut ui = self.account_ui.borrow_mut();
                        ui.busy = false;
                        ui.login_url = Some(url.clone());
                        ui.message.clear();
                    }
                    self.open_in_browser(&url);
                }
                Event::LoggedIn {
                    account,
                    password,
                    stored,
                } => self.logged_in(&account, password, stored),
                Event::LoggedOut { revoked, removed } => {
                    self.change_setting(schema::USER, None);
                    self.sync.borrow_mut().password = None;
                    self.password_problem.borrow_mut().take();
                    let (message, error) = login::logout_message(&revoked, &removed);
                    self.account_message(&message, error);
                    self.status("Logged out");
                }
                Event::Failed(message) => self.account_message(&message, true),
                Event::Cancelled => self.account_message("The login was cancelled.", false),
            }
        }
        self.refresh_settings_dialog();
    }

    /// Keeps the account in the settings and the app password for the
    /// syncs, and syncs.
    fn logged_in(
        &self,
        account: &ren_settings::Account,
        password: String,
        stored: Result<(), String>,
    ) {
        self.change_setting(schema::SERVER, Some(&Value::Text(account.server.clone())));
        self.change_setting(schema::USER, Some(&Value::Text(account.user.clone())));
        let settings = self.current_settings();
        let Ok(kept) = settings.account() else {
            self.account_message(
                "Logged in, but the account couldn't be saved in the settings file.",
                true,
            );
            return;
        };
        // ren's own, kept in the Secret Service (or in memory without one).
        self.sync.borrow_mut().password = Some((kept.clone(), password, Source::Store));
        self.password_problem.borrow_mut().take();
        let (message, error) = login::login_message(&kept, &stored);
        self.account_message(&message, error);
        self.status(format!("Logged in as {} on {}", kept.user, kept.server));
        self.start_sync();
    }

    /// Where the app password for `account` comes from: the one the window
    /// has from a sync or a login, else where a sync would look it up.
    fn password_source(&self, account: &ren_settings::Account) -> Source {
        let held = self
            .sync
            .borrow()
            .password
            .as_ref()
            .filter(|(of, _, _)| of == account)
            .map(|(_, _, source)| *source);
        held.unwrap_or_else(|| login::source(account))
    }

    /// The settings in effect; the defaults without a settings file.
    pub(super) fn current_settings(&self) -> Settings {
        self.settings
            .borrow()
            .as_ref()
            .map_or_else(Settings::default, |file| file.settings().clone())
    }

    /// Shows the account page's state in the dialog.
    pub(super) fn show_account(&self, window: &SettingsWindow, settings: &Settings) {
        let ui = self.account_ui.borrow();
        let problem = self.password_problem.borrow();
        let account = settings.account().ok();
        let state = if ui.login_url.is_some() {
            1
        } else if ui.busy {
            3
        } else if account.is_some() && problem.is_none() {
            2
        } else {
            0
        };
        window.set_account_state(state);
        window.set_account_server(settings.text(schema::SERVER).unwrap_or_default().into());
        window.set_account_user(settings.text(schema::USER).unwrap_or_default().into());
        let source = account
            .as_ref()
            .map(|account| self.password_source(account));
        window.set_account_summary(
            account
                .as_ref()
                .zip(source)
                .map(|(account, source)| login::summary(account, source))
                .unwrap_or_default()
                .into(),
        );
        window.set_account_revokes(source.is_none_or(login::revokes));
        let (message, error) = match (&*problem, ui.message.is_empty()) {
            (Some(problem), true) => (problem.as_str(), true),
            _ => (ui.message.as_str(), ui.error),
        };
        window.set_account_message(message.into());
        window.set_account_message_error(error);
        window.set_login_url(ui.login_url.clone().unwrap_or_default().into());
    }
}
