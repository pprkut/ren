// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the feed dialog asks the server for, and what the window says
//! about it afterwards. Without Slint, so it's tested.

use ren_store::{Feed, Folder};
use ren_sync::{Changed, FeedChange};

/// What the dialog says about a new address: the News API can't change
/// one.
pub const URL_NOTE: &str = "A new address adds the feed anew and deletes this one, with its \
                            articles, starred ones included; its name and folder are kept.";

/// The folder chosen in the dialog's list: "No folder", then `folders`.
pub fn folder_at(folders: &[Folder], index: i32) -> Option<u64> {
    let index = usize::try_from(index).ok()?.checked_sub(1)?;
    folders.get(index).map(|folder| folder.id)
}

/// The position of a folder in the dialog's list.
pub fn folder_index(folders: &[Folder], folder_id: Option<u64>) -> i32 {
    folder_id
        .and_then(|id| folders.iter().position(|folder| folder.id == id))
        .map_or(0, |i| i as i32 + 1)
}

/// The changes the edit dialog asks for: a new address replaces the feed
/// (with the name and folder from the dialog); otherwise a new name and
/// another folder. An empty name keeps the feed's.
pub fn feed_changes(feed: &Feed, name: &str, url: &str, folder_id: Option<u64>) -> Vec<FeedChange> {
    let name = name.trim();
    let url = url.trim();
    let title = if name.is_empty() { &feed.title } else { name };
    if !url.is_empty() && url != feed.url {
        return vec![FeedChange::ChangeUrl {
            feed_id: feed.id,
            url: url.to_owned(),
            title: title.to_owned(),
            folder_id,
        }];
    }
    let mut changes = Vec::new();
    if title != feed.title {
        changes.push(FeedChange::RenameFeed {
            feed_id: feed.id,
            title: title.to_owned(),
        });
    }
    if folder_id != feed.folder_id {
        changes.push(FeedChange::MoveFeed {
            feed_id: feed.id,
            folder_id,
        });
    }
    changes
}

/// What the status bar says after a change, and whether it failed.
pub fn outcome(change: &FeedChange, result: &Result<Changed, ren_sync::Error>) -> (String, bool) {
    let (done, failed) = match change {
        FeedChange::AddFeed { .. } => ("Feed added", "Adding the feed failed"),
        FeedChange::ChangeUrl { .. } => (
            "Feed replaced by the one at the new address",
            "Changing the feed's address failed",
        ),
        FeedChange::RenameFeed { .. } => ("Feed renamed", "Renaming the feed failed"),
        FeedChange::MoveFeed { .. } => ("Feed moved", "Moving the feed failed"),
        FeedChange::DeleteFeed { .. } => ("Feed deleted", "Deleting the feed failed"),
        FeedChange::AddFolder { .. } => ("Folder added", "Adding the folder failed"),
        FeedChange::RenameFolder { .. } => ("Folder renamed", "Renaming the folder failed"),
        FeedChange::DeleteFolder { .. } => ("Folder deleted", "Deleting the folder failed"),
    };
    match result {
        Ok(Changed {
            warning: Some(warning),
            ..
        }) => (warning.clone(), true),
        Ok(_) => (done.to_owned(), false),
        Err(err) => (format!("{failed}: {}", reason(err)), true),
    }
}

/// The server's reason, without the error's prefixes where the server
/// said why.
fn reason(err: &ren_sync::Error) -> String {
    match err {
        ren_sync::Error::Api(nextcloud_news::Error::Status {
            message: Some(message),
            ..
        }) => message.clone(),
        err => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders() -> Vec<Folder> {
        vec![
            Folder {
                id: 4,
                name: "News".to_owned(),
            },
            Folder {
                id: 2,
                name: "Tech".to_owned(),
            },
        ]
    }

    fn feed() -> Feed {
        Feed {
            id: 9,
            folder_id: Some(2),
            title: "Example".to_owned(),
            url: "https://example.org/feed".to_owned(),
            link: None,
            favicon_link: None,
            added: None,
            ordering: 0,
            pinned: false,
            update_error_count: 0,
            last_update_error: None,
        }
    }

    #[test]
    fn folder_choices() {
        let folders = folders();
        assert_eq!(folder_at(&folders, 0), None);
        assert_eq!(folder_at(&folders, 1), Some(4));
        assert_eq!(folder_at(&folders, 2), Some(2));
        assert_eq!(folder_at(&folders, 3), None);
        assert_eq!(folder_at(&folders, -1), None);
        assert_eq!(folder_index(&folders, Some(2)), 2);
        assert_eq!(folder_index(&folders, None), 0);
        assert_eq!(folder_index(&folders, Some(7)), 0);
    }

    #[test]
    fn edits() {
        let feed = feed();
        assert_eq!(
            feed_changes(&feed, " Example ", "https://example.org/feed", Some(2)),
            []
        );
        // An empty name keeps the feed's.
        assert_eq!(feed_changes(&feed, "", "", Some(2)), []);
        assert_eq!(
            feed_changes(&feed, "Mine", "https://example.org/feed", None),
            [
                FeedChange::RenameFeed {
                    feed_id: 9,
                    title: "Mine".to_owned()
                },
                FeedChange::MoveFeed {
                    feed_id: 9,
                    folder_id: None
                },
            ]
        );
        assert_eq!(
            feed_changes(&feed, "", " https://example.org/new ", Some(4)),
            [FeedChange::ChangeUrl {
                feed_id: 9,
                url: "https://example.org/new".to_owned(),
                title: "Example".to_owned(),
                folder_id: Some(4),
            }]
        );
    }

    #[test]
    fn outcomes() {
        let add = FeedChange::AddFeed {
            url: "x".to_owned(),
            folder_id: None,
        };
        assert_eq!(
            outcome(&add, &Ok(Changed::default())),
            ("Feed added".to_owned(), false)
        );
        let refused = Err(ren_sync::Error::Api(nextcloud_news::Error::Status {
            code: 409,
            message: Some("Feed exists already".to_owned()),
        }));
        assert_eq!(
            outcome(&add, &refused),
            (
                "Adding the feed failed: Feed exists already".to_owned(),
                true
            )
        );
        let warned = Ok(Changed {
            warning: Some("Added, but".to_owned()),
            ..Changed::default()
        });
        assert_eq!(
            outcome(
                &FeedChange::DeleteFolder {
                    folder_id: 1,
                    feeds: vec![]
                },
                &warned
            ),
            ("Added, but".to_owned(), true)
        );
    }
}
