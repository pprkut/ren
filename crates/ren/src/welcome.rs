// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the welcome page says: shown in the article pane while no article
//! is, with the counts and what the last sync did.

use crate::item_list::format_date;
use crate::reader::Overview;

/// The unread articles and the feeds.
pub fn articles_text(overview: &Overview) -> String {
    let feeds = match overview.feeds {
        0 => return "No feeds yet.".to_owned(),
        1 => "1 feed".to_owned(),
        n => format!("{n} feeds"),
    };
    match overview.unread {
        0 => format!("No unread articles in {feeds}."),
        1 => format!("1 unread article in {feeds}."),
        n => format!("{n} unread articles in {feeds}."),
    }
}

/// When the last sync ended and how many articles it brought; `failure`
/// is why the last attempt since, if any, failed.
pub fn sync_text(overview: &Overview, failure: Option<&str>) -> String {
    let mut text = match overview.last_sync {
        None => "Not synced yet.".to_owned(),
        Some(at) => {
            let new = match overview.new {
                0 => "no new articles".to_owned(),
                1 => "1 new article".to_owned(),
                n => format!("{n} new articles"),
            };
            format!("Last synced {}: {new}.", format_date(at))
        }
    };
    if let Some(failure) = failure {
        text.push_str(&format!(" The last sync failed: {failure}"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overview(unread: u64, feeds: usize, new: u64, last_sync: Option<i64>) -> Overview {
        Overview {
            unread,
            feeds,
            new,
            last_sync,
        }
    }

    #[test]
    fn articles() {
        assert_eq!(articles_text(&overview(0, 0, 0, None)), "No feeds yet.");
        assert_eq!(
            articles_text(&overview(0, 1, 0, None)),
            "No unread articles in 1 feed."
        );
        assert_eq!(
            articles_text(&overview(1, 3, 0, None)),
            "1 unread article in 3 feeds."
        );
        assert_eq!(
            articles_text(&overview(42, 3, 0, None)),
            "42 unread articles in 3 feeds."
        );
    }

    #[test]
    fn syncs() {
        assert_eq!(sync_text(&overview(0, 0, 0, None), None), "Not synced yet.");
        let at = 1_790_000_000;
        let date = format_date(at);
        assert_eq!(
            sync_text(&overview(0, 1, 0, Some(at)), None),
            format!("Last synced {date}: no new articles.")
        );
        assert_eq!(
            sync_text(&overview(0, 1, 1, Some(at)), None),
            format!("Last synced {date}: 1 new article.")
        );
        assert_eq!(
            sync_text(&overview(0, 1, 7, Some(at)), Some("timed out")),
            format!("Last synced {date}: 7 new articles. The last sync failed: timed out")
        );
    }
}
