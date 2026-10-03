// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! View model of the feed tree: folders with feeds, flattened into the rows
//! a list view shows, depending on which folders are expanded.

use crate::dummy::{Feed, Folder};

/// What a row in the feed tree stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    /// The "All items" entry, the root of the tree.
    All,
    Folder(u32),
    Feed(u32),
}

/// One visible row of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub node: Node,
    pub title: String,
    /// 0 for "All items", 1 for folders and top-level feeds, 2 for feeds in
    /// folders.
    pub depth: u8,
    pub unread: usize,
    /// `Some(expanded)` for folders, `None` for leaves.
    pub expanded: Option<bool>,
    /// Whether this is the last child of its parent. The branch line of the
    /// last child ends at the row instead of running on to the next one.
    pub last: bool,
    /// For each ancestor level below the root (depth 1 up to `depth - 1`),
    /// whether a branch line passes through this row because that ancestor
    /// has further siblings below.
    pub guides: Vec<bool>,
}

/// Keys that move the selection in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    /// Collapses an expanded folder, otherwise moves to the parent.
    Left,
    /// Expands a collapsed folder, otherwise moves to its first child.
    Right,
    Home,
    End,
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

    fn folder_mut(&mut self, folder_id: u32) -> Option<&mut FolderEntry> {
        self.folders.iter_mut().find(|f| f.folder.id == folder_id)
    }

    /// Expands a collapsed folder or collapses an expanded one.
    pub fn toggle(&mut self, folder_id: u32) {
        if let Some(entry) = self.folder_mut(folder_id) {
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

    /// The feeds whose items a node shows; `None` for all feeds.
    pub fn feeds_of(&self, node: Node) -> Option<Vec<u32>> {
        match node {
            Node::All => None,
            Node::Feed(id) => Some(vec![id]),
            Node::Folder(id) => Some(
                self.folders
                    .iter()
                    .filter(|f| f.folder.id == id)
                    .flat_map(|f| &f.feeds)
                    .map(|(feed, _)| feed.id)
                    .collect(),
            ),
        }
    }

    /// The rows to show: "All items" as the root, below it each folder
    /// followed by its feeds if expanded, then the feeds without a folder.
    pub fn rows(&self) -> Vec<Row> {
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
            last: true,
            guides: Vec::new(),
        }];

        let children = self.folders.len() + self.top_level.len();
        for (i, entry) in self.folders.iter().enumerate() {
            let last = i + 1 == children;
            rows.push(Row {
                node: Node::Folder(entry.folder.id),
                title: entry.folder.name.clone(),
                depth: 1,
                unread: entry.feeds.iter().map(|(_, unread)| unread).sum(),
                expanded: Some(entry.expanded),
                last,
                guides: Vec::new(),
            });
            if entry.expanded {
                let count = entry.feeds.len();
                rows.extend(
                    entry
                        .feeds
                        .iter()
                        .enumerate()
                        .map(|(j, (feed, unread))| Row {
                            node: Node::Feed(feed.id),
                            title: feed.title.clone(),
                            depth: 2,
                            unread: *unread,
                            expanded: None,
                            last: j + 1 == count,
                            guides: vec![!last],
                        }),
                );
            }
        }
        let offset = self.folders.len();
        rows.extend(
            self.top_level
                .iter()
                .enumerate()
                .map(|(i, (feed, unread))| Row {
                    node: Node::Feed(feed.id),
                    title: feed.title.clone(),
                    depth: 1,
                    unread: *unread,
                    expanded: None,
                    last: offset + i + 1 == children,
                    guides: Vec::new(),
                }),
        );
        rows
    }

    /// Handles a navigation key with `current` selected. Returns the node
    /// to select next, if the selection moves; expanding and collapsing
    /// folders happens here too.
    pub fn navigate(&mut self, current: Node, key: Key) -> Option<Node> {
        let rows = self.rows();
        let index = rows.iter().position(|r| r.node == current);
        let target = match (key, index) {
            (Key::Home, _) | (_, None) => 0,
            (Key::End, _) => rows.len() - 1,
            (Key::Up, Some(i)) => i.saturating_sub(1),
            (Key::Down, Some(i)) => (i + 1).min(rows.len() - 1),
            (Key::Left, Some(i)) => {
                if rows[i].expanded == Some(true) {
                    if let Node::Folder(id) = current {
                        self.toggle(id);
                    }
                    return None;
                }
                // The parent is the closest row above with a lower depth.
                rows[..i]
                    .iter()
                    .rposition(|r| r.depth < rows[i].depth)
                    .unwrap_or(i)
            }
            (Key::Right, Some(i)) => match rows[i].expanded {
                Some(false) => {
                    if let Node::Folder(id) = current {
                        self.toggle(id);
                    }
                    return None;
                }
                Some(true) | None if rows.get(i + 1).is_some_and(|r| r.depth > rows[i].depth) => {
                    i + 1
                }
                _ => i,
            },
        };
        let node = rows[target].node;
        (node != current).then_some(node)
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
        assert_eq!(rows[2].depth, 2);
        assert_eq!(rows[4].depth, 1);
    }

    #[test]
    fn branch_lines() {
        let rows = tree().rows();
        let lines: Vec<_> = rows.iter().map(|r| (r.last, r.guides.clone())).collect();
        assert_eq!(
            lines,
            [
                (true, vec![]),
                (false, vec![]),
                (false, vec![true]),
                (true, vec![true]),
                (true, vec![]),
            ]
        );
    }

    #[test]
    fn last_folder_has_no_guide() {
        let folders = [Folder {
            id: 7,
            name: "Tech".to_owned(),
        }];
        let feed = Feed {
            id: 1,
            folder_id: Some(7),
            title: "Feed".to_owned(),
        };
        let rows = FeedTree::new(&folders, [(feed, 0)]).rows();
        assert!(rows[1].last);
        assert_eq!(rows[2].guides, [false]);
    }

    #[test]
    fn feeds_of_nodes() {
        let tree = tree();
        assert_eq!(tree.feeds_of(Node::All), None);
        assert_eq!(tree.feeds_of(Node::Folder(7)), Some(vec![1, 3]));
        assert_eq!(tree.feeds_of(Node::Feed(2)), Some(vec![2]));
    }

    #[test]
    fn navigate_up_down() {
        let mut tree = tree();
        assert_eq!(tree.navigate(Node::All, Key::Down), Some(Node::Folder(7)));
        assert_eq!(
            tree.navigate(Node::Folder(7), Key::Down),
            Some(Node::Feed(1))
        );
        assert_eq!(tree.navigate(Node::Feed(1), Key::Up), Some(Node::Folder(7)));
        assert_eq!(tree.navigate(Node::All, Key::Up), None);
        assert_eq!(tree.navigate(Node::Feed(2), Key::Down), None);
        assert_eq!(tree.navigate(Node::Feed(1), Key::End), Some(Node::Feed(2)));
        assert_eq!(tree.navigate(Node::Feed(1), Key::Home), Some(Node::All));
    }

    #[test]
    fn navigate_left_right() {
        let mut tree = tree();
        // Left on a feed goes to its folder, then collapses it, then goes up
        // to the root.
        assert_eq!(
            tree.navigate(Node::Feed(3), Key::Left),
            Some(Node::Folder(7))
        );
        assert_eq!(tree.navigate(Node::Folder(7), Key::Left), None);
        assert_eq!(tree.rows()[1].expanded, Some(false));
        assert_eq!(tree.navigate(Node::Folder(7), Key::Left), Some(Node::All));
        // Right expands, then goes to the first child; on a leaf it stays.
        assert_eq!(tree.navigate(Node::Folder(7), Key::Right), None);
        assert_eq!(tree.rows()[1].expanded, Some(true));
        assert_eq!(
            tree.navigate(Node::Folder(7), Key::Right),
            Some(Node::Feed(1))
        );
        assert_eq!(tree.navigate(Node::Feed(1), Key::Right), None);
        assert_eq!(tree.navigate(Node::All, Key::Right), Some(Node::Folder(7)));
    }

    #[test]
    fn navigate_from_hidden_node_goes_to_root() {
        let mut tree = tree();
        tree.toggle(7);
        assert_eq!(tree.navigate(Node::Feed(1), Key::Down), Some(Node::All));
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
