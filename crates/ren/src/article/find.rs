// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Finding text in the article: the matches in a block of text, and which
//! match the find bar goes to.

use std::ops::Range;

/// Where the find bar goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The text or the options changed: the first match.
    First,
    Next,
    Previous,
}

/// The byte ranges of `needle` in `text`, without overlaps. Without
/// `match_case`, letters match in either case, also where a letter's
/// lower case has another length in UTF-8.
pub fn matches(text: &str, needle: &str, match_case: bool) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    if needle.is_empty() {
        return found;
    }
    if match_case {
        let mut from = 0;
        while let Some(at) = text[from..].find(needle) {
            let start = from + at;
            found.push(start..start + needle.len());
            from = start + needle.len();
        }
        return found;
    }
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let mut starts = text.char_indices().map(|(at, _)| at).peekable();
    while let Some(start) = starts.next() {
        if let Some(end) = match_at(text, start, &needle) {
            found.push(start..end);
            // No overlaps: go on after the match.
            while starts.peek().is_some_and(|&next| next < end) {
                starts.next();
            }
        }
    }
    found
}

/// Where a match of `needle` (in lower case) starting at `start` ends.
fn match_at(text: &str, start: usize, needle: &[char]) -> Option<usize> {
    let mut wanted = needle.iter();
    for (at, c) in text[start..].char_indices() {
        for lower in c.to_lowercase() {
            if wanted.next() != Some(&lower) {
                return None;
            }
        }
        if wanted.len() == 0 {
            return Some(start + at + c.len_utf8());
        }
    }
    None
}

/// The match to go to among `count`, from the `current` one: the first
/// for a new text, the next or previous one round the end.
pub fn step(current: Option<usize>, count: usize, step: Step) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match (step, current) {
        (Step::First, _) | (Step::Next, None) => 0,
        (Step::Previous, None) => count - 1,
        (Step::Next, Some(current)) => (current + 1) % count,
        (Step::Previous, Some(current)) => (current + count - 1) % count,
    })
}

/// What the find bar says after going to match `current` (from 1; 0 for
/// none) of `total` for `needle`.
pub fn status(needle: &str, current: usize, total: usize) -> String {
    if needle.is_empty() {
        String::new()
    } else if total == 0 {
        "Not found".to_owned()
    } else {
        format!("{current} of {total}")
    }
}

/// [`Step`] as the window passes it: 0 for a new text, 1 for the next
/// match, 2 for the previous one.
pub fn step_from(step: i32) -> Step {
    match step {
        1 => Step::Next,
        2 => Step::Previous,
        _ => Step::First,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(found: Vec<Range<usize>>) -> Vec<(usize, usize)> {
        found.into_iter().map(|r| (r.start, r.end)).collect()
    }

    #[test]
    fn matching() {
        let m = |text, needle, case| spans(matches(text, needle, case));
        assert_eq!(m("A cat, a CAT.", "cat", true), [(2, 5)]);
        assert_eq!(m("A cat, a CAT.", "cat", false), [(2, 5), (9, 12)]);
        assert_eq!(m("aaaa", "aa", false), [(0, 2), (2, 4)]);
        assert_eq!(m("text", "", false), []);
        assert_eq!(m("", "x", false), []);
        // Other lengths in lower case, and text after them.
        assert_eq!(m("ÄRGER ärger", "ärger", false), [(0, 6), (7, 13)]);
        assert_eq!(m("İstanbul, istanbul", "istanbul", false), [(11, 19)]);
        assert_eq!(m("İstanbul", "i̇stanbul", false), [(0, 9)]);
        // A needle longer than the rest of the text.
        assert_eq!(m("ab", "abc", false), []);
    }

    #[test]
    fn statuses() {
        assert_eq!(status("", 0, 0), "");
        assert_eq!(status("cat", 0, 0), "Not found");
        assert_eq!(status("cat", 2, 7), "2 of 7");
        assert_eq!(step_from(0), Step::First);
        assert_eq!(step_from(1), Step::Next);
        assert_eq!(step_from(2), Step::Previous);
    }

    #[test]
    fn stepping() {
        assert_eq!(step(None, 0, Step::First), None);
        assert_eq!(step(Some(2), 3, Step::First), Some(0));
        assert_eq!(step(None, 3, Step::Next), Some(0));
        assert_eq!(step(None, 3, Step::Previous), Some(2));
        assert_eq!(step(Some(2), 3, Step::Next), Some(0));
        assert_eq!(step(Some(0), 3, Step::Previous), Some(2));
        assert_eq!(step(Some(1), 3, Step::Next), Some(2));
    }
}
