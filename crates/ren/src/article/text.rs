// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! HTML to plain text, for the article pane without the `html-view`
//! feature.
//!
//! Not an HTML parser: it removes tags, turns block elements into line
//! breaks, decodes character references and collapses white space. That is
//! enough for the sanitised article bodies the News app sends.

/// Elements that start a new paragraph.
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "dd",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "tr",
    "ul",
];

/// Elements whose content is not text.
const SKIPPED: &[&str] = &["head", "noscript", "script", "style", "svg", "template"];

/// Converts an HTML fragment to plain text: paragraphs separated by an empty
/// line, list items prefixed with a bullet.
pub fn html_to_text(html: &str) -> String {
    let mut out = Text::default();
    let mut rest = html;
    let mut skipping: Option<String> = None;
    let mut pre = 0usize;

    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            if skipping.is_none() {
                out.push_text(&decode_entities(rest), pre > 0);
            }
            break;
        };
        if skipping.is_none() {
            out.push_text(&decode_entities(&rest[..lt]), pre > 0);
        }
        rest = &rest[lt..];

        // Comments and doctype.
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        let Some(gt) = rest.find('>') else {
            // A lone '<' in text.
            if skipping.is_none() {
                out.push_text("<", pre > 0);
            }
            rest = &rest[1..];
            continue;
        };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];

        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }

        if let Some(skipped) = &skipping {
            if closing && *skipped == name {
                skipping = None;
            }
            continue;
        }
        if !closing && SKIPPED.contains(&name.as_str()) && !tag.ends_with('/') {
            skipping = Some(name);
            continue;
        }

        match name.as_str() {
            "br" => out.line_break(),
            "li" if !closing => {
                out.paragraph_break_single();
                out.push_raw("• ");
            }
            "li" => out.paragraph_break_single(),
            "pre" if closing => {
                pre = pre.saturating_sub(1);
                out.paragraph_break();
            }
            "pre" => {
                out.paragraph_break();
                pre += 1;
            }
            "img" => {
                if let Some(alt) = attribute(tag, "alt").filter(|alt| !alt.trim().is_empty()) {
                    out.push_text(&format!("[{}]", decode_entities(&alt)), false);
                }
            }
            "td" | "th" if !closing => out.push_text(" ", false),
            name if BLOCKS.contains(&name) => out.paragraph_break(),
            _ => {}
        }
    }
    out.finish()
}

/// The value of attribute `name` in the inside of a start tag, not
/// decoded.
pub fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(name) {
        let start = from + pos;
        from = start + name.len();
        let before_ok = lower[..start].ends_with(|c: char| c.is_ascii_whitespace());
        let after = lower[from..].trim_start();
        if !before_ok || !after.starts_with('=') {
            continue;
        }
        let value = tag[tag.len() - after.len() + 1..].trim_start();
        return Some(match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or("").to_owned(),
            _ => value
                .split(|c: char| c.is_ascii_whitespace() || c == '/')
                .next()
                .unwrap_or("")
                .to_owned(),
        });
    }
    None
}

/// Decodes character references: numeric ones and the common named ones.
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let end = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '#'))
            .map(|i| i + 1);
        let decoded = end.and_then(|end| {
            let name = &rest[1..end];
            let consumed = if rest[end..].starts_with(';') {
                end + 1
            } else {
                end
            };
            decode_entity(name).map(|c| (c, consumed))
        });
        match decoded {
            Some((c, consumed)) => {
                out.push(c);
                rest = &rest[consumed..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code).filter(|&c| c != '\0');
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "shy" => '\u{ad}',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "lsquo" => '‘',
        "rsquo" => '’',
        "sbquo" => '‚',
        "ldquo" => '“',
        "rdquo" => '”',
        "bdquo" => '„',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "euro" => '€',
        "times" => '×',
        "auml" => 'ä',
        "ouml" => 'ö',
        "uuml" => 'ü',
        "Auml" => 'Ä',
        "Ouml" => 'Ö',
        "Uuml" => 'Ü',
        "szlig" => 'ß',
        "eacute" => 'é',
        "egrave" => 'è',
        "agrave" => 'à',
        "ccedil" => 'ç',
        _ => return None,
    })
}

/// Accumulates text with collapsed white space and paragraph breaks.
#[derive(Default)]
struct Text {
    out: String,
    /// Line breaks wanted before the next text: 1 for a line, 2 for a
    /// paragraph.
    pending_breaks: usize,
    /// A space is wanted before the next word.
    pending_space: bool,
}

impl Text {
    fn push_text(&mut self, text: &str, preformatted: bool) {
        if preformatted {
            if !text.is_empty() {
                self.flush_breaks();
                self.out.push_str(text);
            }
            return;
        }
        for (i, word) in text
            .split(|c: char| c.is_whitespace() && c != '\u{a0}')
            .enumerate()
        {
            if i > 0 {
                self.pending_space = true;
            }
            if word.is_empty() {
                continue;
            }
            self.flush_breaks();
            if self.pending_space && !self.out.is_empty() && !self.out.ends_with('\n') {
                self.out.push(' ');
            }
            self.pending_space = false;
            self.out.push_str(word);
        }
    }

    /// Text that is added as is, e.g. a list bullet.
    fn push_raw(&mut self, text: &str) {
        self.flush_breaks();
        self.out.push_str(text);
        self.pending_space = false;
    }

    fn flush_breaks(&mut self) {
        if !self.out.is_empty() {
            for _ in 0..self.pending_breaks {
                self.out.push('\n');
            }
        }
        if self.pending_breaks > 0 {
            self.pending_space = false;
        }
        self.pending_breaks = 0;
    }

    /// `<br>`; two in a row leave an empty line.
    fn line_break(&mut self) {
        self.pending_breaks = (self.pending_breaks + 1).min(2);
    }

    fn paragraph_break(&mut self) {
        self.pending_breaks = 2;
    }

    fn paragraph_break_single(&mut self) {
        self.pending_breaks = self.pending_breaks.max(1);
    }

    fn finish(self) -> String {
        self.out.trim_end().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_and_inline_markup() {
        assert_eq!(
            html_to_text("<p>Hello <b>bold</b>\n  world.</p><p>Second</p>"),
            "Hello bold world.\n\nSecond"
        );
    }

    #[test]
    fn line_breaks_and_lists() {
        assert_eq!(
            html_to_text("One<br>Two<ul><li>a</li><li>b</li></ul>After"),
            "One\nTwo\n\n• a\n• b\n\nAfter"
        );
        assert_eq!(html_to_text("a<br><br>b<br/>c"), "a\n\nb\nc");
    }

    #[test]
    fn entities() {
        assert_eq!(
            decode_entities("a &amp; b &lt;c&gt; &#228;&#xE4; &hellip; &bogus; & x"),
            "a & b <c> ää … &bogus; & x"
        );
        assert_eq!(
            html_to_text("Fish&nbsp;&amp;&nbsp;chips"),
            "Fish\u{a0}&\u{a0}chips"
        );
    }

    #[test]
    fn skipped_elements_and_comments() {
        assert_eq!(
            html_to_text("<style>p { color: red }</style><!-- note -->Text<script>x()</script>"),
            "Text"
        );
    }

    #[test]
    fn preformatted_text_keeps_white_space() {
        assert_eq!(
            html_to_text("<p>Code:</p><pre>fn main() {\n    x\n}</pre>"),
            "Code:\n\nfn main() {\n    x\n}"
        );
    }

    #[test]
    fn image_alt_text() {
        assert_eq!(
            html_to_text(r#"<p>A <img src="x.png" alt="cat &amp; dog"> B <img src=y.png></p>"#),
            "A [cat & dog] B"
        );
        assert_eq!(
            attribute(r#"img data-alt="no" alt='yes'"#, "alt"),
            Some("yes".to_owned())
        );
        assert_eq!(attribute("img alt=bare/", "alt"), Some("bare".to_owned()));
    }

    #[test]
    fn broken_markup() {
        assert_eq!(html_to_text("a < b and c"), "a < b and c");
        assert_eq!(html_to_text("<p>unclosed"), "unclosed");
        assert_eq!(html_to_text(""), "");
    }
}
