// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The article shown in the article pane, as an HTML document for the
//! Blitz view or as plain text without it.

#[cfg_attr(not(feature = "html-view"), allow(dead_code))]
pub mod cache;
#[cfg_attr(not(feature = "html-view"), allow(dead_code))]
mod document;
#[cfg_attr(not(feature = "html-view"), allow(dead_code))]
pub mod fragment;
#[cfg(feature = "html-view")]
pub mod html;
#[cfg(feature = "html-view")]
mod images;
pub mod text;

#[cfg_attr(not(feature = "html-view"), allow(unused_imports))]
pub use document::{Rgb, Style};

/// What the article pane shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Article {
    /// Not sanitised: escape before putting it into HTML.
    pub title: String,
    /// Feed, author and date, as one line.
    pub meta: String,
    /// The original page; links and images are resolved against it. Not
    /// sanitised.
    pub url: Option<String>,
    /// Sanitised HTML, as the News app sends it.
    pub body: String,
}

impl Article {
    /// The body as plain text, without markup.
    pub fn plain_text(&self) -> String {
        text::html_to_text(&self.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text() {
        let article = Article {
            title: "Title".to_owned(),
            meta: "Feed".to_owned(),
            url: None,
            body: "<p>One</p><p>Two</p>".to_owned(),
        };
        assert_eq!(article.plain_text(), "One\n\nTwo");
    }
}
