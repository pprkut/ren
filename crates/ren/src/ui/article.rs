// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The article pane, showing the article as plain text.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, SharedString};

use super::MainWindow;
use crate::article::Article;

const PLACEHOLDER: &str = "No article selected";

/// Shows articles in the window's article pane.
pub struct ArticlePane {
    window: slint::Weak<MainWindow>,
    article: Option<Article>,
}

impl ArticlePane {
    pub fn new(window: &MainWindow) -> Rc<RefCell<Self>> {
        let pane = Rc::new(RefCell::new(Self {
            window: window.as_weak(),
            article: None,
        }));
        pane.borrow_mut().clear();
        pane
    }

    fn window(&self) -> MainWindow {
        self.window
            .upgrade()
            .expect("the pane only lives as long as the window")
    }

    pub fn show(&mut self, article: Article) {
        let window = self.window();
        window.set_article_title(article.title.clone().into());
        window.set_article_meta(article.meta.clone().into());
        window.set_article_content_y(0.0);
        window.set_article_body(article.plain_text().into());
        self.article = Some(article);
    }

    pub fn clear(&mut self) {
        self.article = None;
        self.show_placeholder();
    }

    fn show_placeholder(&self) {
        let window = self.window();
        window.set_article_title(PLACEHOLDER.into());
        window.set_article_meta(SharedString::new());
        window.set_article_body(SharedString::new());
    }
}
