// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The third-party notices (`THIRD-PARTY-NOTICES.html`) as blocks for the
//! About window: headings, paragraphs and preformatted licence texts. Not
//! an HTML parser: enough for the notices file, whose licence texts are in
//! `<pre>` and everything else in headings, paragraphs and lists.

use crate::article::text::decode_entities;

/// A block of the notices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(String),
    Paragraph(String),
    /// A licence text, its lines as they are.
    Pre(String),
}

/// Elements that end the text before them and start a new paragraph.
const PARAGRAPHS: &[&str] = &[
    "article",
    "blockquote",
    "dd",
    "div",
    "dl",
    "dt",
    "footer",
    "header",
    "li",
    "main",
    "ol",
    "p",
    "section",
    "table",
    "tr",
    "ul",
];

/// Elements whose content isn't shown.
const SKIPPED: &[&str] = &["head", "script", "style", "title"];

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Heading,
    Paragraph,
    Pre,
}

/// The blocks of the notices' HTML.
pub fn blocks(html: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut text = String::new();
    let mut kind = Kind::Paragraph;
    let mut skipping: Option<String> = None;
    let mut rest = html;
    loop {
        let (before, tag) = match rest.find('<') {
            Some(i) => (&rest[..i], Some(&rest[i..])),
            None => (rest, None),
        };
        if skipping.is_none() {
            text.push_str(&decode_entities(before));
        }
        let Some(tag) = tag else {
            break;
        };
        if let Some(comment) = tag.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        let Some(end) = tag.find('>') else {
            break;
        };
        rest = &tag[end + 1..];
        let inner = &tag[1..end];
        let closing = inner.starts_with('/');
        let name = inner
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if let Some(skipped) = &skipping {
            if closing && name == *skipped {
                skipping = None;
            }
            continue;
        }
        match name.as_str() {
            name if SKIPPED.contains(&name) && !closing => skipping = Some(name.to_owned()),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                flush(&mut blocks, &mut text, kind);
                kind = if closing {
                    Kind::Paragraph
                } else {
                    Kind::Heading
                };
            }
            "pre" => {
                flush(&mut blocks, &mut text, kind);
                kind = if closing { Kind::Paragraph } else { Kind::Pre };
            }
            "br" => text.push('\n'),
            name if PARAGRAPHS.contains(&name) => flush(&mut blocks, &mut text, kind),
            _ => {}
        }
    }
    flush(&mut blocks, &mut text, kind);
    blocks
}

/// Ends the block of `text`, if it has any.
fn flush(blocks: &mut Vec<Block>, text: &mut String, kind: Kind) {
    let block = std::mem::take(text);
    if kind == Kind::Pre {
        let block = block.trim_matches('\n').trim_end();
        if !block.is_empty() {
            blocks.push(Block::Pre(block.to_owned()));
        }
        return;
    }
    let block = block.split_whitespace().collect::<Vec<_>>().join(" ");
    if block.is_empty() {
        return;
    }
    blocks.push(match kind {
        Kind::Heading => Block::Heading(block),
        _ => Block::Paragraph(block),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(text: &str) -> Block {
        Block::Heading(text.to_owned())
    }

    fn paragraph(text: &str) -> Block {
        Block::Paragraph(text.to_owned())
    }

    #[test]
    fn cargo_about_layout() {
        // The shape of cargo-about's default template.
        let html = r##"<!DOCTYPE html>
            <html><head><title>Licenses</title><style>h1 { color: red }</style></head>
            <body><main>
              <h1>Third Party Licenses</h1>
              <p>This page lists the licenses of the projects used in ren.</p>
              <h2>Overview of licenses:</h2>
              <ul class="licenses-overview">
                <li><a href="#Apache-2.0">Apache License 2.0</a> (310)</li>
                <li><a href="#MIT">MIT License</a> (290)</li>
              </ul>
              <h2>All license text:</h2>
              <ul class="licenses-list">
                <li class="license">
                  <h3 id="MIT">MIT License</h3>
                  <h4>Used by:</h4>
                  <ul class="license-used-by">
                    <li><a href="https://crates.io/crates/serde">serde 1.0.228</a></li>
                  </ul>
                  <pre class="license-text">Copyright (c) &lt;year&gt; &lt;owner&gt;

Permission is hereby granted, free of charge,
  to any person obtaining a copy</pre>
                </li>
              </ul>
            </main></body></html>"##;
        assert_eq!(
            blocks(html),
            [
                heading("Third Party Licenses"),
                paragraph("This page lists the licenses of the projects used in ren."),
                heading("Overview of licenses:"),
                paragraph("Apache License 2.0 (310)"),
                paragraph("MIT License (290)"),
                heading("All license text:"),
                heading("MIT License"),
                heading("Used by:"),
                paragraph("serde 1.0.228"),
                Block::Pre(
                    "Copyright (c) <year> <owner>\n\nPermission is hereby granted, free of \
                     charge,\n  to any person obtaining a copy"
                        .to_owned()
                ),
            ]
        );
    }

    #[test]
    fn comments_breaks_and_loose_text() {
        let html = "<!-- a comment -->\nloose <b>bold</b> text<br>next\
                    <p>a &amp; b</p><!-- unclosed";
        assert_eq!(
            blocks(html),
            [paragraph("loose bold text next"), paragraph("a & b")]
        );
        assert_eq!(blocks(""), []);
        assert_eq!(blocks("<pre>\n\n</pre>"), []);
    }

    #[test]
    fn the_notices_file() {
        let blocks = blocks(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../THIRD-PARTY-NOTICES.html"
        )));
        assert!(blocks.contains(&heading("Lucide icons")));
        assert_eq!(
            blocks
                .iter()
                .filter(|block| matches!(block, Block::Pre(_)))
                .count(),
            2
        );
    }
}
