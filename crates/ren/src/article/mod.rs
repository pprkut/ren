// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The article shown in the article pane.

pub mod text;

/// An item body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Plain text; paragraphs separated by an empty line.
    Text(String),
    /// Sanitised HTML, as the News app sends it.
    Html(String),
}

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
    pub body: Body,
}

impl Article {
    /// The body as plain text, without markup.
    pub fn plain_text(&self) -> String {
        match &self.body {
            Body::Text(text) => text.clone(),
            Body::Html(html) => text::html_to_text(html),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn article(body: Body) -> Article {
        Article {
            title: "Title".to_owned(),
            meta: "Feed".to_owned(),
            url: None,
            body,
        }
    }

    #[test]
    fn plain_text() {
        let html = article(Body::Html("<p>One</p><p>Two</p>".to_owned()));
        assert_eq!(html.plain_text(), "One\n\nTwo");
        let text = article(Body::Text("Already text".to_owned()));
        assert_eq!(text.plain_text(), "Already text");
    }
}
