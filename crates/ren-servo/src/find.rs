// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Finding text in a page for the find bar: Servo has no find-in-page of
//! its own, so a script (`find.js`) in the page selects the match through
//! the page's selection, which Servo paints, and scrolls it into view.

use ren_tabs::FindStep;
use servo::JSValue;

const SCRIPT: &str = include_str!("find.js");

/// The match to ask the script for, from the one selected last: it goes
/// round the ends itself.
pub fn index(last: Option<i64>, step: FindStep) -> i64 {
    match (step, last) {
        (FindStep::First, _) | (FindStep::Next, None) => 0,
        (FindStep::Previous, None) => -1,
        (FindStep::Next, Some(last)) => last + 1,
        (FindStep::Previous, Some(last)) => last - 1,
    }
}

/// The script that finds `needle` and selects match `index`.
pub fn script(needle: &str, match_case: bool, index: i64) -> String {
    format!("{SCRIPT}({}, {match_case}, {index})", js_string(needle))
}

/// The script's answer: the index of the match selected and the number of
/// matches; none if there are none, or the script failed.
pub fn answer(value: &JSValue) -> Option<(i64, u32)> {
    let JSValue::Array(values) = value else {
        return None;
    };
    let [JSValue::Number(index), JSValue::Number(count)] = values.as_slice() else {
        return None;
    };
    let (index, count) = (*index, *count);
    (count >= 1.0 && index >= 0.0 && index < count).then_some((index as i64, count as u32))
}

/// `text` as a JavaScript string literal.
fn js_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            // Line ends and other control characters, and the separators
            // that end a line in older JavaScript.
            c if c.is_control() || c == '\u{2028}' || c == '\u{2029}' => {
                quoted.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
            }
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings() {
        assert_eq!(js_string("a cat"), "\"a cat\"");
        assert_eq!(js_string("\"x\" \\ y"), r#""\"x\" \\ y""#);
        assert_eq!(js_string("a\nb\u{2028}"), r#""a\u{a}b\u{2028}""#);
        assert_eq!(js_string("</script>"), "\"</script>\"");
    }

    #[test]
    fn scripts() {
        let script = script("x\")", true, -1);
        assert!(script.starts_with("// SPDX"));
        assert!(script.ends_with("})\n(\"x\\\")\", true, -1)"));
    }

    #[test]
    fn indices() {
        assert_eq!(index(Some(4), FindStep::First), 0);
        assert_eq!(index(None, FindStep::Next), 0);
        assert_eq!(index(None, FindStep::Previous), -1);
        assert_eq!(index(Some(4), FindStep::Next), 5);
        assert_eq!(index(Some(0), FindStep::Previous), -1);
    }

    #[test]
    fn answers() {
        let n = JSValue::Number;
        assert_eq!(answer(&JSValue::Array(vec![n(2.0), n(7.0)])), Some((2, 7)));
        assert_eq!(answer(&JSValue::Array(vec![n(0.0), n(0.0)])), None);
        assert_eq!(answer(&JSValue::Array(vec![n(7.0), n(7.0)])), None);
        assert_eq!(answer(&JSValue::Undefined), None);
        assert_eq!(answer(&JSValue::Array(vec![n(1.0)])), None);
    }
}
