// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! View model of the feed tree: folders with feeds, flattened into the rows
//! a list view shows, depending on which folders are expanded.

use crate::dummy::{Feed, Folder};

/// What a row in the feed tree stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    /// The "All items" entry at the top.
    All,
    Folder(u32),
    Feed(u32),
}

/// One visible row of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub node: Node,
    pub title: String,
    pub depth: u8,
    pub unread: usize,
    /// `Some(expanded)` for folders, `None` for leaves.
    pub expanded: Option<bool>,
}

struct FolderEntry {
    folder: Folder,
    feeds: Vec<(Feed, usize)>,
    expanded: bool,
}

pub struct FeedTree {
    folders: Vec<FolderEntry>,
    top_level: Vec<(Feed, usize)>,
}

impl FeedTree {
    /// Builds the tree from folders and feeds with their unread counts.
    /// All folders start expanded.
    pub fn new(folders: &[Folder], feeds: impl IntoIterator<Item = (Feed, usize)>) -> Self {
        let mut folders: Vec<FolderEntry> = folders
            .iter()
            .map(|folder| FolderEntry {
                folder: folder.clone(),
                feeds: Vec::new(),
                expanded: true,
            })
            .collect();
        let mut top_level = Vec::new();
        for (feed, unread) in feeds {
            match feed
                .folder_id
                .and_then(|id| folders.iter_mut().find(|f| f.folder.id == id))
            {
                Some(entry) => entry.feeds.push((feed, unread)),
                None => top_level.push((feed, unread)),
            }
        }
        Self { folders, top_level }
    }

    /// Expands a collapsed folder or collapses an expanded one.
    pub fn toggle(&mut self, folder_id: u32) {
        if let Some(entry) = self.folders.iter_mut().find(|f| f.folder.id == folder_id) {
            entry.expanded = !entry.expanded;
        }
    }

    /// Updates the unread count of a feed.
    pub fn set_unread(&mut self, feed_id: u32, unread: usize) {
        if let Some((_, count)) = self
            .folders
            .iter_mut()
            .flat_map(|f| &mut f.feeds)
            .chain(&mut self.top_level)
            .find(|(feed, _)| feed.id == feed_id)
        {
            *count = unread;
        }
    }

    /// The rows to show: "All items", then each folder followed by its feeds
    /// if expanded, then the feeds without a folder.
    pub fn rows(&self) -> Vec<Row> {
        let leaf = |(feed, unread): &(Feed, usize), depth| Row {
            node: Node::Feed(feed.id),
            title: feed.title.clone(),
            depth,
            unread: *unread,
            expanded: None,
        };

        let total = self
            .folders
            .iter()
            .flat_map(|f| &f.feeds)
            .chain(&self.top_level)
            .map(|(_, unread)| unread)
            .sum();
        let mut rows = vec![Row {
            node: Node::All,
            title: "All items".to_owned(),
            depth: 0,
            unread: total,
            expanded: None,
        }];

        for entry in &self.folders {
            rows.push(Row {
                node: Node::Folder(entry.folder.id),
                title: entry.folder.name.clone(),
                depth: 0,
                unread: entry.feeds.iter().map(|(_, unread)| unread).sum(),
                expanded: Some(entry.expanded),
            });
            if entry.expanded {
                rows.extend(entry.feeds.iter().map(|f| leaf(f, 1)));
            }
        }
        rows.extend(self.top_level.iter().map(|f| leaf(f, 0)));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> FeedTree {
        let folders = [Folder {
            id: 7,
            name: "Tech".to_owned(),
        }];
        let feed = |id, folder_id, unread| {
            (
                Feed {
                    id,
                    folder_id,
                    title: format!("Feed {id}"),
                },
                unread,
            )
        };
        FeedTree::new(
            &folders,
            [feed(1, Some(7), 3), feed(2, None, 5), feed(3, Some(7), 1)],
        )
    }

    fn nodes(rows: &[Row]) -> Vec<Node> {
        rows.iter().map(|r| r.node).collect()
    }

    #[test]
    fn expanded_rows() {
        let rows = tree().rows();
        assert_eq!(
            nodes(&rows),
            [
                Node::All,
                Node::Folder(7),
                Node::Feed(1),
                Node::Feed(3),
                Node::Feed(2)
            ]
        );
        assert_eq!(rows[0].unread, 9);
        assert_eq!(rows[1].unread, 4);
        assert_eq!(rows[1].expanded, Some(true));
        assert_eq!(rows[2].depth, 1);
        assert_eq!(rows[4].depth, 0);
    }

    #[test]
    fn collapse_and_expand() {
        let mut tree = tree();
        tree.toggle(7);
        let rows = tree.rows();
        assert_eq!(nodes(&rows), [Node::All, Node::Folder(7), Node::Feed(2)]);
        assert_eq!(rows[1].expanded, Some(false));
        assert_eq!(rows[1].unread, 4);
        tree.toggle(7);
        assert_eq!(tree.rows().len(), 5);
    }

    #[test]
    fn unread_update() {
        let mut tree = tree();
        tree.set_unread(3, 0);
        tree.set_unread(2, 1);
        let rows = tree.rows();
        assert_eq!(rows[0].unread, 4);
        assert_eq!(rows[1].unread, 3);
        assert_eq!(rows[3].unread, 0);
    }

    #[test]
    fn feed_with_unknown_folder_is_top_level() {
        let feed = Feed {
            id: 1,
            folder_id: Some(99),
            title: "Orphan".to_owned(),
        };
        let rows = FeedTree::new(&[], [(feed, 0)]).rows();
        assert_eq!(nodes(&rows), [Node::All, Node::Feed(1)]);
    }
}
