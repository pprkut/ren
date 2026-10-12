// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Help → About ren, and the third-party notices, shown in the article
//! view like an article.

use std::time::Duration;

use slint::ComponentHandle;

use super::{AboutWindow, App};
use crate::article::Article;

/// Where ren's source is.
const SOURCE: &str = "https://github.com/pprkut/ren";

/// The notices of what ren is built from; `just notices` generates them
/// for releases (M11).
const NOTICES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../THIRD-PARTY-NOTICES.html"
));

impl App {
    /// Opens the About window, or brings it to the front.
    pub(super) fn open_about(&self) {
        if let Some(about) = &*self.about.borrow() {
            let _ = about.show();
            return;
        }
        let about = match AboutWindow::new() {
            Ok(about) => about,
            Err(err) => {
                self.status(format!("Opening About ren failed: {err}"));
                return;
            }
        };
        if let Some(dark) = self.forced_dark {
            about.set_forced_color_scheme(if dark {
                slint::language::ColorScheme::Dark
            } else {
                slint::language::ColorScheme::Light
            });
        }
        about.set_version(env!("CARGO_PKG_VERSION").into());
        about.set_source_url(SOURCE.into());
        let this = self.this.clone();
        about.on_open_source(move || {
            if let Some(app) = this.upgrade() {
                app.open_in_browser(SOURCE);
            }
        });
        let this = self.this.clone();
        about.on_show_licenses(move || {
            if let Some(app) = this.upgrade() {
                app.show_notices();
                app.close_about();
            }
        });
        let this = self.this.clone();
        about.on_close_requested(move || {
            if let Some(app) = this.upgrade() {
                app.close_about();
            }
        });
        let this = self.this.clone();
        about.window().on_close_requested(move || {
            if let Some(app) = this.upgrade() {
                app.close_about();
            }
            slint::CloseRequestResponse::HideWindow
        });
        if let Err(err) = about.show() {
            eprintln!("ren: showing About ren: {err}");
        }
        *self.about.borrow_mut() = Some(about);
    }

    /// Hides the About window, and drops it: not from its own callback.
    fn close_about(&self) {
        if let Some(about) = &*self.about.borrow() {
            let _ = about.hide();
        }
        let this = self.this.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(app) = this.upgrade() {
                app.about.borrow_mut().take();
            }
        });
    }

    /// Shows the third-party notices in the article view, in front of the
    /// tabs; no article is selected meanwhile.
    pub(super) fn show_notices(&self) {
        self.reader.borrow_mut().clear_current();
        let window = self.window();
        window.set_current_item(-1);
        window.set_current_starred(false);
        #[cfg(feature = "servo")]
        self.pages.borrow_mut().show_article();
        self.article.borrow_mut().show(Article {
            title: "Third-party licenses".to_owned(),
            meta: format!("ren {}", env!("CARGO_PKG_VERSION")),
            url: None,
            body: NOTICES.to_owned(),
            enclosure: None,
            thumbnail: None,
        });
    }
}
