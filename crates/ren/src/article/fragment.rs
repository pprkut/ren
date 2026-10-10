// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Targets for jump links within an article (`#section`).
//!
//! The News app's sanitiser removes `id` and `name` attributes from the
//! bodies, so links to a heading find no target. Their fragments are
//! usually made from the heading's text, though: lower case, punctuation
//! removed, words joined by hyphens. So headings get such an id back, and
//! a fragment is compared in the same form.

use super::text::{attribute, html_to_text};

/// `text` as a fragment: lower-case letters and digits, words joined by
/// single hyphens, other characters removed. `_` separates words too.
pub fn slug(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    let mut separate = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if separate && !slug.is_empty() {
                slug.push('-');
            }
            separate = false;
            slug.extend(c.to_lowercase());
        } else if c.is_whitespace() || c == '-' || c == '_' {
            separate = true;
        }
    }
    slug
}

/// Gives each heading (`h1` to `h6`) without an id the slug of its text
/// as its id; a slug that is taken gets `-1`, `-2`, … appended.
pub fn add_heading_ids(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut taken: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some((start, level)) = find_heading(rest) {
        // `<hN`
        let name_end = start + 3;
        out.push_str(&rest[..name_end]);
        rest = &rest[name_end..];
        let Some(tag_end) = rest.find('>') else {
            break;
        };
        if attribute(&rest[..tag_end], "id").is_some() {
            continue;
        }
        let inner = &rest[tag_end + 1..];
        let close = find_ignore_case(inner, &format!("</h{level}")).unwrap_or(inner.len());
        let text = html_to_text(&inner[..close]);
        let base = slug(&text);
        if base.is_empty() {
            continue;
        }
        let mut id = base.clone();
        let mut n = 0;
        while taken.contains(&id) {
            n += 1;
            id = format!("{base}-{n}");
        }
        out.push_str(&format!(" id=\"{id}\""));
        taken.push(id);
    }
    out.push_str(rest);
    out
}

/// The position of the next heading's start tag and its level.
fn find_heading(html: &str) -> Option<(usize, u8)> {
    let bytes = html.as_bytes();
    let mut from = 0;
    while let Some(i) = html[from..].find('<').map(|i| from + i) {
        let level = bytes.get(i + 2).copied().unwrap_or_default();
        let after = bytes.get(i + 3).copied().unwrap_or(b'>');
        if bytes
            .get(i + 1)
            .is_some_and(|b| b.eq_ignore_ascii_case(&b'h'))
            && (b'1'..=b'6').contains(&level)
            && (after == b'>' || after == b'/' || after.is_ascii_whitespace())
        {
            return Some((i, level - b'0'));
        }
        from = i + 1;
    }
    None
}

fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(
            slug("GSoC 2026 Final Update - Jenkins Email Notifications"),
            "gsoc-2026-final-update-jenkins-email-notifications"
        );
        assert_eq!(slug("  Don't   stop  "), "dont-stop");
        assert_eq!(slug("snake_case and--dashes-"), "snake-case-and-dashes");
        assert_eq!(slug("C++ / Rust"), "c-rust");
        assert_eq!(slug("Über Größe"), "über-größe");
        assert_eq!(slug("?!"), "");
    }

    #[test]
    fn headings_get_ids() {
        assert_eq!(
            add_heading_ids(
                r#"<p>x</p><h2>Getting <em>started</em></h2><H3 class="a">Tom &amp; Jerry</H3>"#
            ),
            r#"<p>x</p><h2 id="getting-started">Getting <em>started</em></h2><H3 id="tom-jerry" class="a">Tom &amp; Jerry</H3>"#
        );
    }

    #[test]
    fn taken_slugs_are_numbered() {
        assert_eq!(
            add_heading_ids("<h2>Notes</h2><h3>Notes</h3><h2>Notes</h2>"),
            r#"<h2 id="notes">Notes</h2><h3 id="notes-1">Notes</h3><h2 id="notes-2">Notes</h2>"#
        );
    }

    #[test]
    fn other_tags_and_ids_are_kept() {
        let html = r#"<hr><header>x</header><h2 id="own">Own</h2><h7>No</h7><h2></h2><h2>!</h2>"#;
        assert_eq!(add_heading_ids(html), html);
        assert_eq!(
            add_heading_ids("<h2>Unclosed"),
            r#"<h2 id="unclosed">Unclosed"#
        );
        assert_eq!(add_heading_ids("<h2"), "<h2");
        assert_eq!(add_heading_ids(""), "");
    }
}
