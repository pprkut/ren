// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Changes to the feeds and folders asked for in the window. They go to
//! the server, not into the store: the sync that follows brings them in,
//! with the articles of a feed added.

use nextcloud_news::{Edit, Edited};

use crate::{Error, NewsApi};

/// A change to the feeds and folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedChange {
    /// The server finds the feed at `url`, also from a web page's address,
    /// and fetches it.
    AddFeed {
        url: String,
        folder_id: Option<u64>,
    },
    /// The News API can't change a feed's address: the feed at `url` is
    /// added in the old one's folder (`folder_id`) and named like it
    /// (`title`, unless empty), then the old one is deleted, with its
    /// articles.
    ChangeUrl {
        feed_id: u64,
        url: String,
        title: String,
        folder_id: Option<u64>,
    },
    RenameFeed {
        feed_id: u64,
        title: String,
    },
    MoveFeed {
        feed_id: u64,
        folder_id: Option<u64>,
    },
    /// With its articles.
    DeleteFeed {
        feed_id: u64,
    },
    AddFolder {
        name: String,
    },
    RenameFolder {
        folder_id: u64,
        name: String,
    },
    /// With its feeds (`feeds`) and their articles. The feeds are deleted
    /// first: News 28.7 leaves them behind otherwise, in a folder that's
    /// gone (with SQLite at least).
    DeleteFolder {
        folder_id: u64,
        feeds: Vec<u64>,
    },
}

/// What a change did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changed {
    /// The feed added.
    pub feed: Option<u64>,
    /// The folder added.
    pub folder: Option<u64>,
    /// What didn't work of a change that was made: a feed whose address
    /// changed may keep the new feed's name, or the old feed.
    pub warning: Option<String>,
}

/// Sends a change to the server.
pub fn change(api: &impl NewsApi, change: &FeedChange) -> Result<Changed, Error> {
    let edit = match change {
        FeedChange::AddFeed { url, folder_id } => Edit::AddFeed {
            url,
            folder_id: *folder_id,
        },
        FeedChange::ChangeUrl {
            feed_id,
            url,
            title,
            folder_id,
        } => return change_url(api, *feed_id, url, title, *folder_id),
        FeedChange::RenameFeed { feed_id, title } => Edit::RenameFeed {
            id: *feed_id,
            title,
        },
        FeedChange::MoveFeed { feed_id, folder_id } => Edit::MoveFeed {
            id: *feed_id,
            folder_id: *folder_id,
        },
        FeedChange::DeleteFeed { feed_id } => Edit::DeleteFeed { id: *feed_id },
        FeedChange::AddFolder { name } => Edit::AddFolder { name },
        FeedChange::RenameFolder { folder_id, name } => Edit::RenameFolder {
            id: *folder_id,
            name,
        },
        FeedChange::DeleteFolder { folder_id, feeds } => {
            for &id in feeds {
                api.edit(&Edit::DeleteFeed { id })?;
            }
            Edit::DeleteFolder { id: *folder_id }
        }
    };
    Ok(match api.edit(&edit)? {
        Edited::Feed(feed) => Changed {
            feed: Some(feed.id),
            ..Changed::default()
        },
        Edited::Folder(folder) => Changed {
            folder: Some(folder.id),
            ..Changed::default()
        },
        Edited::Done => Changed::default(),
    })
}

/// A new feed for an old one: added, named, then the old one deleted.
/// Only adding it fails the change; the rest is a warning.
fn change_url(
    api: &impl NewsApi,
    old: u64,
    url: &str,
    title: &str,
    folder_id: Option<u64>,
) -> Result<Changed, Error> {
    let Edited::Feed(added) = api.edit(&Edit::AddFeed { url, folder_id })? else {
        unreachable!("adding a feed answers with it");
    };
    let mut changed = Changed {
        feed: Some(added.id),
        ..Changed::default()
    };
    let mut warnings = Vec::new();
    if !title.is_empty()
        && added.title.as_deref() != Some(title)
        && let Err(err) = api.edit(&Edit::RenameFeed {
            id: added.id,
            title,
        })
    {
        warnings.push(format!("the new feed keeps its own name ({err})"));
    }
    if let Err(err) = api.edit(&Edit::DeleteFeed { id: old }) {
        warnings.push(format!(
            "the old feed couldn't be deleted, delete it by hand ({err})"
        ));
    }
    if !warnings.is_empty() {
        changed.warning = Some(format!(
            "The feed at the new address was added, but {}.",
            warnings.join(", and ")
        ));
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Fail, FakeServer, Request, item};

    fn server() -> FakeServer {
        let server = FakeServer::new(1_790_000_000);
        server.add_folder(10);
        server.add_feed(1, Some(10));
        server.add_feed(2, None);
        server.put(item(1, 1));
        server.put(item(2, 2));
        server
    }

    fn feeds(server: &FakeServer) -> Vec<(u64, Option<u64>, String)> {
        server.with(|s| {
            s.feeds
                .iter()
                .map(|feed| {
                    (
                        feed.id,
                        feed.folder_id,
                        feed.title.clone().unwrap_or_default(),
                    )
                })
                .collect()
        })
    }

    #[test]
    fn feeds_and_folders() {
        let server = server();
        let added = change(
            &server,
            &FeedChange::AddFeed {
                url: "https://example.org/new".into(),
                folder_id: Some(10),
            },
        )
        .unwrap();
        assert_eq!(added.feed, Some(3));
        let folder = change(
            &server,
            &FeedChange::AddFolder {
                name: "Science".into(),
            },
        )
        .unwrap();
        assert_eq!(folder.folder, Some(11));
        for edit in [
            FeedChange::RenameFeed {
                feed_id: 3,
                title: "New".into(),
            },
            FeedChange::MoveFeed {
                feed_id: 3,
                folder_id: Some(11),
            },
            FeedChange::MoveFeed {
                feed_id: 1,
                folder_id: None,
            },
            FeedChange::DeleteFeed { feed_id: 2 },
            FeedChange::RenameFolder {
                folder_id: 10,
                name: "Empty".into(),
            },
        ] {
            assert_eq!(
                change(&server, &edit).unwrap(),
                Changed::default(),
                "{edit:?}"
            );
        }
        assert_eq!(
            feeds(&server),
            [(1, None, "Feed 1".into()), (3, Some(11), "New".into())]
        );
        // The deleted feed's items went with it.
        assert!(server.with(|s| s.items.values().all(|item| item.feed_id != 2)));
        change(
            &server,
            &FeedChange::DeleteFolder {
                folder_id: 11,
                feeds: vec![3],
            },
        )
        .unwrap();
        assert_eq!(feeds(&server), [(1, None, "Feed 1".into())]);
        let names: Vec<String> =
            server.with(|s| s.folders.iter().map(|f| f.name.clone()).collect());
        assert_eq!(names, ["Empty"]);
        // The server's refusal comes back.
        let err = change(
            &server,
            &FeedChange::AddFolder {
                name: "Empty".into(),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("409"), "{err}");
    }

    #[test]
    fn a_new_address_is_a_new_feed() {
        let server = server();
        server.requests();
        let changed = change(
            &server,
            &FeedChange::ChangeUrl {
                feed_id: 1,
                url: "https://example.org/moved".into(),
                title: "Feed 1".into(),
                folder_id: Some(10),
            },
        )
        .unwrap();
        assert_eq!(
            changed,
            Changed {
                feed: Some(3),
                ..Changed::default()
            }
        );
        assert_eq!(
            feeds(&server),
            [(2, None, "Feed 2".into()), (3, Some(10), "Feed 1".into())]
        );
        assert_eq!(
            server.requests(),
            [
                Request::Edit("Post feeds".into()),
                Request::Edit("Post feeds/3/rename".into()),
                Request::Edit("Delete feeds/1".into()),
            ]
        );
    }

    #[test]
    fn a_new_address_that_fails() {
        // The feed exists already: nothing changes.
        let server = server();
        let err = change(
            &server,
            &FeedChange::ChangeUrl {
                feed_id: 1,
                url: "https://example.org/2/feed".into(),
                title: "Feed 1".into(),
                folder_id: Some(10),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("409"), "{err}");
        assert_eq!(feeds(&server).len(), 2);

        // Added, but the old one stays.
        server.fail_when(|request| {
            matches!(request, Request::Edit(edit) if edit.starts_with("Delete"))
                .then_some(Fail::Status(500, 0))
        });
        let changed = change(
            &server,
            &FeedChange::ChangeUrl {
                feed_id: 1,
                url: "https://example.org/moved".into(),
                title: String::new(),
                folder_id: None,
            },
        )
        .unwrap();
        assert_eq!(changed.feed, Some(3));
        let warning = changed.warning.unwrap();
        assert!(
            warning.starts_with("The feed at the new address was added, but the old feed couldn't"),
            "{warning}"
        );
        assert_eq!(feeds(&server).len(), 3);
    }
}
