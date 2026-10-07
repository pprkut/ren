// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Text handling: whitespace in single-line fields, and sorting and
//! searching without regard to case.
//!
//! SQLite's `NOCASE` and `LIKE` only fold ASCII letters, so "Über" and
//! "über" would sort apart and a search for "ü" wouldn't find "Ü". Items
//! carry the lower case of their title and author instead ([`key`]),
//! which SQLite sorts and searches as plain bytes; a collation comparing
//! the lower case costs a call into Rust per comparison, which made sorting
//! all items by title more than three times slower. Folders and feeds, few
//! as they are, are sorted with such a collation (`COLLATE caseless`).
//!
//! Either way, the order is by code point of the lower case, not by the
//! rules of a language: "Äpfel" sorts after "Zebra".

use std::borrow::Cow;
use std::cmp::Ordering;

use rusqlite::Connection;

/// Collation for sorting text without regard to case: `COLLATE caseless`.
const CASELESS: &str = "caseless";

/// Collapses runs of whitespace (line breaks too) into one space and trims
/// both ends, for fields shown on one line: real feeds put line breaks into
/// authors and spaces around titles.
pub fn collapse(s: &str) -> Cow<'_, str> {
    let clean = !s.starts_with(' ')
        && !s.ends_with(' ')
        && !s.contains("  ")
        && !s.contains(|c: char| c.is_whitespace() && c != ' ');
    if clean {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(s.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

/// The form of a text that items are sorted and searched by.
pub fn key(text: &str) -> String {
    text.to_lowercase()
}

/// The form of a search text that matches [`key`]s.
pub fn search_key(text: &str) -> String {
    key(collapse(text).as_ref())
}

/// Compares the lower case forms of two strings, without building them.
fn caseless(a: &str, b: &str) -> Ordering {
    // Most titles are ASCII: compare bytes until the first non-ASCII one,
    // which is at a character boundary in both strings.
    for (i, (x, y)) in a.bytes().zip(b.bytes()).enumerate() {
        if !x.is_ascii() || !y.is_ascii() {
            return a[i..]
                .chars()
                .flat_map(char::to_lowercase)
                .cmp(b[i..].chars().flat_map(char::to_lowercase));
        }
        match x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase()) {
            Ordering::Equal => {}
            order => return order,
        }
    }
    // One is a prefix of the other; the lower case of a character is never
    // empty, so the longer one is greater.
    a.len().cmp(&b.len())
}

pub fn register(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_collation(CASELESS, caseless)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_whitespace() {
        assert_eq!(collapse("A title"), "A title");
        assert!(matches!(collapse("A title"), Cow::Borrowed(_)));
        assert_eq!(collapse("  A  title "), "A title");
        assert_eq!(collapse("Ada\n  Lovelace"), "Ada Lovelace");
        assert_eq!(collapse("tab\there"), "tab here");
        assert_eq!(collapse("no\u{a0}break"), "no break");
        assert_eq!(collapse(" \n "), "");
        assert_eq!(collapse(""), "");
    }

    #[test]
    fn compares_without_case() {
        assert_eq!(caseless("über", "Über"), Ordering::Equal);
        assert_eq!(caseless("apple", "Banana"), Ordering::Less);
        assert_eq!(caseless("Zebra", "apple"), Ordering::Greater);
        assert_eq!(caseless("ab", "abc"), Ordering::Less);
        assert_eq!(caseless("ABC", "abc"), Ordering::Equal);
        assert_eq!(caseless("abÉ", "ABé"), Ordering::Equal);
        assert_eq!(caseless("abé", "ABf"), Ordering::Greater);
        assert_eq!(caseless("aÉ", "Ab"), Ordering::Greater);
        assert_eq!(caseless("a", "aÉ"), Ordering::Less);
        assert_eq!(caseless("", ""), Ordering::Equal);
        // Lower case with more characters than upper case: İ is i̇.
        assert_eq!(caseless("İ", "i\u{307}"), Ordering::Equal);
    }

    #[test]
    fn keys() {
        assert_eq!(key("Grüße aus KÖLN"), "grüße aus köln");
        assert_eq!(search_key("  Aus\n KÖLN "), "aus köln");
        // Keys sort like the collation.
        let mut words = ["über", "Zebra", "apfel", "Äpfel", "Apfelbaum"];
        let mut keys = words.map(key);
        words.sort_by(|a, b| caseless(a, b));
        keys.sort();
        assert_eq!(keys, words.map(key));
    }

    #[test]
    fn collation() {
        let conn = Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        let sorted: String = conn
            .query_row(
                "SELECT group_concat(t, ',') FROM (SELECT column1 AS t FROM \
                 (VALUES ('über'), ('Zebra'), ('apfel'), ('Äpfel')) ORDER BY t COLLATE caseless)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sorted, "apfel,Zebra,Äpfel,über");
    }
}
