// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The article as an HTML document with ren's stylesheet, for the Blitz
//! view.

use super::Article;

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
        let body = replace_iframes(&self.body);
        format!(
            "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{title}</title>\
             <style>{}</style></head><body><article>\
             <header><h1>{heading}</h1><p class=\"meta\">{}</p></header>\
             <div class=\"content\">{body}</div></article></body></html>",
            stylesheet(style),
            escape(&self.meta),
        )
    }
}

/// Replaces `<iframe>`s with links to their source. Embedded players need
/// JavaScript, which the article view doesn't run, and loading them would
/// only cost memory and tell the embedding site what is being read. Links
/// can later open in a full-page tab.
fn replace_iframes(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = find_ignore_case(rest, "<iframe") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(tag_end) = after.find('>') else {
            rest = "";
            break;
        };
        let tag = &after[1..tag_end];
        let close = find_ignore_case(&after[tag_end..], "</iframe>")
            .map_or(tag_end + 1, |i| tag_end + i + "</iframe>".len());
        if let Some(src) = super::text::attribute(tag, "src").filter(|src| !src.is_empty()) {
            let src = escape(&super::text::decode_entities(&src));
            out.push_str(&format!(
                r#"<p class="embed"><a href="{src}">Embedded content: {src}</a></p>"#
            ));
        }
        rest = &after[close..];
    }
    out.push_str(rest);
    out
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
pre {{ white-space: pre-wrap; background: {subtle}; padding: 0.6em 0.8em; border-radius: 4px; }}
code {{ font-family: monospace; font-size: 0.92em; }}
:not(pre) > code {{ background: {subtle}; padding: 0.1em 0.3em; border-radius: 3px; }}
blockquote {{ margin: 1em 0; padding-left: 1em; border-left: 3px solid {border}; }}
table {{ border-collapse: collapse; }}
td, th {{ border: 1px solid {border}; padding: 0.25em 0.5em; }}
hr {{ border: none; border-top: 1px solid {border}; }}
",
        bg = bg.css(),
        fg = fg.css(),
        scheme = if style.dark { "dark" } else { "light" },
        meta = bg.mix(fg, 170).css(),
        link = style.link.css(),
        subtle = subtle.css(),
        border = border.css(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
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
