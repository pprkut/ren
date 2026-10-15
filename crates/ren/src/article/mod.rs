// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The article shown in the article pane, as an HTML document for the
//! Blitz view or as plain text without it.

#[cfg_attr(not(feature = "html-view"), allow(dead_code))]
pub mod cache;
#[cfg_attr(not(feature = "html-view"), allow(dead_code))]
mod document;
#[cfg_attr(not(feature = "html-view"), allow(dead_code))]
pub mod find;
#[cfg_attr(not(feature = "html-view"), allow(dead_code))]
pub mod fragment;
#[cfg_attr(not(feature = "html-view"), allow(dead_code))]
pub mod glide;
#[cfg(feature = "html-view")]
pub mod html;
#[cfg(feature = "html-view")]
mod images;
#[cfg(feature = "html-view")]
mod paint;
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
    /// A file that comes with the item: a podcast's episode, a video.
    pub enclosure: Option<Enclosure>,
    /// The item's picture (`media:thumbnail`). Not sanitised.
    pub thumbnail: Option<String>,
}

/// A file that comes with an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enclosure {
    /// Not sanitised.
    pub link: String,
    pub mime: Option<String>,
}

/// What an enclosure is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Audio,
    Video,
    Image,
    File,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Kind::Audio => "Audio",
            Kind::Video => "Video",
            Kind::Image => "Image",
            Kind::File => "File",
        }
    }
}

impl Enclosure {
    /// What the file is, by its type, else by its name's extension.
    pub fn kind(&self) -> Kind {
        let mime = self
            .mime
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let (kind, _) = mime.split_once('/').unwrap_or_default();
        match kind {
            "audio" => return Kind::Audio,
            "video" => return Kind::Video,
            "image" => return Kind::Image,
            _ => {}
        }
        let extension = self
            .name()
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .unwrap_or_default();
        match extension.as_str() {
            "mp3" | "m4a" | "aac" | "ogg" | "oga" | "opus" | "flac" | "wav" => Kind::Audio,
            "mp4" | "m4v" | "webm" | "mkv" | "mov" | "ogv" => Kind::Video,
            "jpg" | "jpeg" | "png" | "gif" | "webp" => Kind::Image,
            _ => Kind::File,
        }
    }

    /// The file's name: the last part of the link's path.
    pub fn name(&self) -> &str {
        let path = self.link.split(['?', '#']).next().unwrap_or_default();
        path.rsplit('/')
            .find(|part| !part.is_empty())
            .filter(|part| !part.contains(':'))
            .unwrap_or(&self.link)
    }
}

/// `url` if it is a web address: unsanitised links of items go nowhere
/// else.
pub fn web_link(url: Option<String>) -> Option<String> {
    url.filter(|url| {
        let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
        lower.starts_with("https://") || lower.starts_with("http://")
    })
}

impl Article {
    /// The body as plain text, without markup, after the enclosure's
    /// link.
    pub fn plain_text(&self) -> String {
        let text = text::html_to_text(&self.body);
        match &self.enclosure {
            Some(enclosure) => {
                format!("{}: {}\n\n{text}", enclosure.kind().title(), enclosure.link)
            }
            None => text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text() {
        let mut article = Article {
            title: "Title".to_owned(),
            meta: "Feed".to_owned(),
            url: None,
            body: "<p>One</p><p>Two</p>".to_owned(),
            enclosure: None,
            thumbnail: None,
        };
        assert_eq!(article.plain_text(), "One\n\nTwo");
        article.enclosure = Some(Enclosure {
            link: "https://example.org/e1.mp3".to_owned(),
            mime: None,
        });
        assert_eq!(
            article.plain_text(),
            "Audio: https://example.org/e1.mp3\n\nOne\n\nTwo"
        );
    }

    fn enclosure(link: &str, mime: Option<&str>) -> Enclosure {
        Enclosure {
            link: link.to_owned(),
            mime: mime.map(str::to_owned),
        }
    }

    #[test]
    fn enclosures() {
        let podcast = enclosure("https://cdn.example/show/ep-12.mp3?x=1", Some("audio/mpeg"));
        assert_eq!(podcast.kind(), Kind::Audio);
        assert_eq!(podcast.name(), "ep-12.mp3");
        assert_eq!(
            enclosure("https://v.example/a.webm", None).kind(),
            Kind::Video
        );
        assert_eq!(
            enclosure("https://v.example/a", Some("Video/MP4")).kind(),
            Kind::Video
        );
        assert_eq!(
            enclosure("https://i.example/p.JPG", Some("")).kind(),
            Kind::Image
        );
        assert_eq!(
            enclosure("https://f.example/doc.pdf", Some("application/pdf")).kind(),
            Kind::File
        );
        assert_eq!(enclosure("https://f.example/", None).name(), "f.example");
        assert_eq!(enclosure("https://", None).name(), "https://");
    }

    #[test]
    fn web_links() {
        let link = |url: &str| web_link(Some(url.to_owned()));
        assert!(link("https://example.org/a").is_some());
        assert!(link("HTTP://example.org").is_some());
        assert_eq!(link("javascript:alert(1)"), None);
        assert_eq!(link("file:///etc/passwd"), None);
        assert_eq!(link("http"), None);
        assert_eq!(web_link(None), None);
    }
}
