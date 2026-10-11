// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The article as an HTML document with ren's stylesheet, for the Blitz
//! view.

use super::{Article, Kind};

/// The article's font size, in CSS pixels.
const FONT_SIZE: u32 = 15;
/// The widest the article's text gets, in `em`: a readable line length.
const CONTENT_EMS: u32 = 46;
/// The widest the article's content gets, in CSS pixels, so also the widest
/// an image is shown at.
pub const CONTENT_WIDTH: u32 = FONT_SIZE * CONTENT_EMS;

/// Colours for the article stylesheet, taken from the UI's palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub background: Rgb,
    pub foreground: Rgb,
    pub link: Rgb,
    pub dark: bool,
}

impl Style {
    /// The background of selected text: the accent, toned down so that the
    /// text stays readable on it, in light and dark mode.
    pub fn selection(&self) -> Rgb {
        self.background.mix(self.link, 100)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    fn css(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// `self` mixed with `other`, `amount` (0–255) of the way.
    fn mix(self, other: Rgb, amount: u8) -> Rgb {
        let m = |a: u8, b: u8| {
            let (a, b, t) = (u16::from(a), u16::from(b), u16::from(amount));
            ((a * (255 - t) + b * t) / 255) as u8
        };
        Rgb(m(self.0, other.0), m(self.1, other.1), m(self.2, other.2))
    }
}

impl Article {
    /// A complete HTML document: the stylesheet, a header with title and
    /// meta line, and the body.
    pub fn html_document(&self, style: &Style) -> String {
        let title = escape(&self.title);
        let heading = match &self.url {
            Some(url) => format!(r#"<a href="{}">{title}</a>"#, escape(url)),
            None => title.clone(),
        };
        let body = replace_media(&replace_iframes(&self.body), self.url.as_deref());
        let body = super::fragment::add_heading_ids(&body);
        format!(
            "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{title}</title>\
             <style>{}</style></head><body><article>\
             <header><h1>{heading}</h1><p class=\"meta\">{}</p></header>\
             <div class=\"content\">{}{body}</div></article></body></html>",
            stylesheet(style),
            escape(&self.meta),
            self.attachments(),
        )
    }

    /// The enclosure and the thumbnail, before the body: an image is
    /// shown, other files are linked. A thumbnail is shown only if the
    /// body has no image of its own, and a file only if the body doesn't
    /// link to it already.
    fn attachments(&self) -> String {
        let mut html = String::new();
        let mut image = false;
        if let Some(enclosure) = &self.enclosure
            && !self.body.contains(&enclosure.link)
            && !self.body.contains(&escape(&enclosure.link))
        {
            let link = escape(&enclosure.link);
            if enclosure.kind() == Kind::Image {
                image = true;
                html.push_str(&format!(
                    r#"<figure class="enclosure"><img src="{link}" alt=""></figure>"#
                ));
            } else {
                let mime = enclosure
                    .mime
                    .as_deref()
                    .filter(|mime| !mime.is_empty())
                    .map(|mime| format!(r#" <span class="type">({})</span>"#, escape(mime)))
                    .unwrap_or_default();
                html.push_str(&format!(
                    r#"<p class="enclosure"><a href="{link}">{}: {}</a>{mime}</p>"#,
                    enclosure.kind().title(),
                    escape(enclosure.name()),
                ));
            }
        }
        let has_image = image || find_element(&self.body, "img").is_some();
        if let Some(thumbnail) = self.thumbnail.as_ref().filter(|_| !has_image) {
            html.insert_str(
                0,
                &format!(
                    r#"<figure class="thumbnail"><img src="{}" alt=""></figure>"#,
                    escape(thumbnail)
                ),
            );
        }
        html
    }
}

/// Replaces `<iframe>`s with links to their source. Embedded players need
/// JavaScript, which the article view doesn't run, and loading them would
/// only cost memory and tell the embedding site what is being read. Links
/// can later open in a full-page tab.
fn replace_iframes(html: &str) -> String {
    replace_elements(html, "iframe", |tag, _| {
        let src = attribute(tag, "src").filter(|src| !src.is_empty())?;
        let src = escape(&src);
        Some(format!(
            r#"<p class="embed"><a href="{src}">Embedded content: {src}</a></p>"#
        ))
    })
}

/// Replaces `<video>` and `<audio>` with their poster image and a link to
/// the page, where they can be played: Blitz plays no media. Without a page,
/// the link goes to the media file.
fn replace_media(html: &str, page: Option<&str>) -> String {
    let media = |kind: &str, tag: &str, inner: &str| {
        let source = attribute(tag, "src")
            .or_else(|| first_source(inner))
            .filter(|src| !src.is_empty());
        let link = page.map(str::to_owned).or(source).map(|href| escape(&href));
        let poster = attribute(tag, "poster")
            .filter(|poster| !poster.is_empty())
            .map(|poster| format!(r#"<img src="{}" alt="">"#, escape(&poster)));
        Some(match (link, poster) {
            (Some(href), Some(poster)) => format!(
                r#"<figure class="media"><a href="{href}">{poster}</a><figcaption><a href="{href}">{kind}: play it on the page</a></figcaption></figure>"#
            ),
            (Some(href), None) => {
                format!(r#"<p class="media"><a href="{href}">{kind}: play it on the page</a></p>"#)
            }
            (None, Some(poster)) => format!(r#"<figure class="media">{poster}</figure>"#),
            (None, None) => format!(r#"<p class="media">{kind} (not shown)</p>"#),
        })
    };
    let html = replace_elements(html, "video", |tag, inner| media("Video", tag, inner));
    replace_elements(&html, "audio", |tag, inner| media("Audio", tag, inner))
}

/// The `src` of the first `<source>` element in `html`.
fn first_source(html: &str) -> Option<String> {
    let start = find_element(html, "source")?;
    let tag = &html[start + 1..];
    attribute(&tag[..tag.find('>')?], "src")
}

/// The value of an attribute in a start tag, with character references
/// decoded.
fn attribute(tag: &str, name: &str) -> Option<String> {
    super::text::attribute(tag, name).map(|value| super::text::decode_entities(&value))
}

/// Replaces each `name` element, with its content, by what `replace` makes
/// of its start tag (without `<` and `>`) and its content; `None` removes
/// it.
fn replace_elements(
    html: &str,
    name: &str,
    mut replace: impl FnMut(&str, &str) -> Option<String>,
) -> String {
    let end_tag = format!("</{name}>");
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = find_element(rest, name) {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(tag_end) = after.find('>') else {
            rest = "";
            break;
        };
        let tag = &after[1..tag_end];
        let content = &after[tag_end + 1..];
        let (inner, close) = match find_ignore_case(content, &end_tag) {
            Some(i) if !tag.ends_with('/') => (&content[..i], tag_end + 1 + i + end_tag.len()),
            _ => ("", tag_end + 1),
        };
        if let Some(replacement) = replace(tag, inner) {
            out.push_str(&replacement);
        }
        rest = &after[close..];
    }
    out.push_str(rest);
    out
}

/// The position of the next start tag of a `name` element.
fn find_element(html: &str, name: &str) -> Option<usize> {
    let open = format!("<{name}");
    let mut from = 0;
    while let Some(i) = find_ignore_case(&html[from..], &open).map(|i| from + i) {
        match html.as_bytes().get(i + open.len()) {
            Some(b) if b.is_ascii_whitespace() || *b == b'>' || *b == b'/' => return Some(i),
            // `<sourcefoo` or the end of the text.
            _ => from = i + open.len(),
        }
    }
    None
}

fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Escapes text for HTML content and attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Readable typography in the UI's colours.
fn stylesheet(style: &Style) -> String {
    let bg = style.background;
    let fg = style.foreground;
    let subtle = bg.mix(fg, 20);
    let border = bg.mix(fg, 60);
    format!(
        "html {{ background: {bg}; color: {fg}; color-scheme: {scheme}; }}
body {{ margin: 0; padding: 16px 20px 32px; font-family: sans-serif; font-size: {FONT_SIZE}px;
  line-height: 1.55; overflow-wrap: break-word; }}
article {{ max-width: {CONTENT_EMS}em; }}
header h1 {{ font-size: 1.5em; line-height: 1.25; margin: 0 0 0.3em; }}
header h1 a {{ color: inherit; text-decoration: none; }}
.meta {{ margin: 0 0 1.5em; font-size: 0.9em; color: {meta}; }}
a {{ color: {link}; }}
img, video, iframe {{ max-width: 100%; height: auto; }}
figure {{ margin: 1em 0; }}
figcaption {{ font-size: 0.9em; color: {meta}; }}
.enclosure {{ margin: 0 0 1em; }}
.enclosure .type {{ font-size: 0.9em; color: {meta}; }}
pre {{ white-space: pre-wrap; background: {subtle}; padding: 0.6em 0.8em; border-radius: 4px; }}
code {{ font-family: monospace; font-size: 0.92em; }}
:not(pre) > code {{ background: {subtle}; padding: 0.1em 0.3em; border-radius: 3px; }}
blockquote {{ margin: 1em 0; padding-left: 1em; border-left: 3px solid {border}; }}
table {{ border-collapse: collapse; }}
td, th {{ border: 1px solid {border}; padding: 0.25em 0.5em; }}
hr {{ border: none; border-top: 1px solid {border}; }}
{dark}",
        dark = if style.dark {
            dark_overrides(style)
        } else {
            String::new()
        },
        bg = bg.css(),
        fg = fg.css(),
        scheme = if style.dark { "dark" } else { "light" },
        meta = bg.mix(fg, 170).css(),
        link = style.link.css(),
        subtle = subtle.css(),
        border = border.css(),
    )
}

/// In dark mode, the colours authors set (`color`, `background-color`,
/// `bgcolor`, `<font color>`) are replaced by the stylesheet's: they are
/// chosen for a light background, so dark text on the dark background, or
/// light boxes around the light text, would be unreadable. `!important` in
/// the stylesheet beats the authors' inline styles. Images get a light
/// backdrop: transparent ones with dark lines (diagrams, logos) are drawn
/// for a light page; opaque ones cover it.
fn dark_overrides(style: &Style) -> String {
    let subtle = style.background.mix(style.foreground, 20).css();
    format!(
        ".content * {{ color: inherit !important; background-color: transparent !important; }}
.content a, .content a * {{ color: {link} !important; }}
.content pre, .content :not(pre) > code {{ background-color: {subtle} !important; }}
.content mark {{ background-color: {mark} !important; }}
.content img {{ background-color: {backdrop} !important; border-radius: 2px; }}
",
        link = style.link.css(),
        mark = style.background.mix(style.link, 90).css(),
        backdrop = style.foreground.mix(style.background, 25).css(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::article::Enclosure;

    const STYLE: Style = Style {
        background: Rgb(255, 255, 255),
        foreground: Rgb(0, 0, 0),
        link: Rgb(0, 0, 255),
        dark: false,
    };

    fn article(body: &str) -> Article {
        Article {
            title: "Tom & <Jerry>".to_owned(),
            meta: "Feed · Someone".to_owned(),
            url: Some(r#"https://example.org/?a=1&b="2""#.to_owned()),
            body: body.to_owned(),
            enclosure: None,
            thumbnail: None,
        }
    }

    fn enclosure(link: &str, mime: &str) -> Option<Enclosure> {
        Some(Enclosure {
            link: link.to_owned(),
            mime: Some(mime.to_owned()),
        })
    }

    #[test]
    fn enclosures_and_thumbnails() {
        let podcast = Article {
            enclosure: enclosure("https://cdn.example/ep1.mp3?a=1&b=2", "audio/mpeg"),
            thumbnail: Some("https://cdn.example/cover.jpg".to_owned()),
            ..article("<p>Notes</p>")
        };
        assert_eq!(
            podcast.attachments(),
            "<figure class=\"thumbnail\"><img src=\"https://cdn.example/cover.jpg\" alt=\"\"></figure>\
             <p class=\"enclosure\"><a href=\"https://cdn.example/ep1.mp3?a=1&amp;b=2\">Audio: ep1.mp3</a> \
             <span class=\"type\">(audio/mpeg)</span></p>"
        );
        let doc = podcast.html_document(&STYLE);
        assert!(doc.contains("<div class=\"content\"><figure class=\"thumbnail\">"));

        // The body has an image: no thumbnail; it links to the file: no link.
        let linked = Article {
            body: r#"<p><img src="x.png"> <a href="https://cdn.example/ep1.mp3?a=1&amp;b=2">Listen</a></p>"#
                .to_owned(),
            ..podcast.clone()
        };
        assert_eq!(linked.attachments(), "");

        // An image enclosure is shown, and takes the thumbnail's place.
        let picture = Article {
            enclosure: enclosure("https://i.example/<big>.png", "image/png"),
            ..podcast
        };
        assert_eq!(
            picture.attachments(),
            "<figure class=\"enclosure\"><img src=\"https://i.example/&lt;big&gt;.png\" alt=\"\"></figure>"
        );
    }

    #[test]
    fn unsanitised_fields_are_escaped() {
        let doc = article("<p>Body</p>").html_document(&STYLE);
        assert!(doc.contains("<h1><a href=\"https://example.org/?a=1&amp;b=&quot;2&quot;\">"));
        assert!(doc.contains("Tom &amp; &lt;Jerry&gt;</a></h1>"));
        assert!(doc.contains("<div class=\"content\"><p>Body</p></div>"));
        assert!(!doc.contains("<Jerry>"));
    }

    #[test]
    fn iframes_become_links() {
        assert_eq!(
            replace_iframes(
                r#"<p>a</p><IFRAME width=560 src="https://v.example/e?a=1&amp;b=2"></iframe><p>b</p>"#
            ),
            r#"<p>a</p><p class="embed"><a href="https://v.example/e?a=1&amp;b=2">Embedded content: https://v.example/e?a=1&amp;b=2</a></p><p>b</p>"#
        );
        assert_eq!(replace_iframes("<iframe></iframe>x"), "x");
        assert_eq!(replace_iframes("<iframe src=x"), "");
        assert_eq!(replace_iframes("no frames"), "no frames");
        assert_eq!(
            replace_iframes("<iframes>x</iframes>"),
            "<iframes>x</iframes>"
        );
    }

    #[test]
    fn media_become_posters_and_links() {
        let page = Some("https://example.org/post?a=1&b=2");
        assert_eq!(
            replace_media(
                r#"<p>a</p><video controls poster="https://example.org/p.jpg?x=1&amp;y=2"><source src="v.mp4">No video.</video><p>b</p>"#,
                page
            ),
            r#"<p>a</p><figure class="media"><a href="https://example.org/post?a=1&amp;b=2"><img src="https://example.org/p.jpg?x=1&amp;y=2" alt=""></a><figcaption><a href="https://example.org/post?a=1&amp;b=2">Video: play it on the page</a></figcaption></figure><p>b</p>"#
        );
        assert_eq!(
            replace_media(r#"<AUDIO src="a.mp3"></AUDIO>"#, page),
            r#"<p class="media"><a href="https://example.org/post?a=1&amp;b=2">Audio: play it on the page</a></p>"#
        );
        // Without a page, the link goes to the file.
        assert_eq!(
            replace_media(
                r#"<video><source type="video/mp4" src="v.mp4"></video>"#,
                None
            ),
            r#"<p class="media"><a href="v.mp4">Video: play it on the page</a></p>"#
        );
        assert_eq!(
            replace_media(r#"<video poster="p.jpg"/>x"#, None),
            r#"<figure class="media"><img src="p.jpg" alt=""></figure>x"#
        );
        assert_eq!(
            replace_media("<audio></audio>", None),
            r#"<p class="media">Audio (not shown)</p>"#
        );
        assert_eq!(
            replace_media("<videos>x</videos>", None),
            "<videos>x</videos>"
        );
    }

    #[test]
    fn author_colours_only_replaced_in_dark_mode() {
        let doc = article(r#"<p style="color: #333">x</p>"#);
        assert!(!doc.html_document(&STYLE).contains("!important"));
        let dark = Style {
            background: Rgb(0, 0, 0),
            foreground: Rgb(255, 255, 255),
            dark: true,
            ..STYLE
        };
        let html = doc.html_document(&dark);
        assert!(html.contains(".content * { color: inherit !important;"));
        assert!(html.contains(".content a, .content a * { color: #0000ff !important; }"));
        assert!(html.contains(".content img { background-color: #e6e6e6 !important;"));
    }

    #[test]
    fn colours() {
        assert_eq!(Rgb(0, 128, 255).css(), "#0080ff");
        assert_eq!(
            Rgb(0, 0, 0).mix(Rgb(255, 255, 255), 255),
            Rgb(255, 255, 255)
        );
        assert_eq!(Rgb(0, 0, 0).mix(Rgb(255, 255, 255), 0), Rgb(0, 0, 0));
    }
}
