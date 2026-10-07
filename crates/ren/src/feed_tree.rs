// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! View model of the feed tree: folders with feeds, flattened into the rows
//! a list view shows, depending on which folders are expanded.

use std::collections::{HashMap, HashSet};

use ren_store::{Feed, Folder, Selection};

/// What a row in the feed tree stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    /// The "All items" entry, the root of the tree.
    All,
    /// The starred items, the root's first child.
    Starred,
    Folder(u64),
    Feed(u64),
}

impl Node {
    /// The items this node lists.
    pub fn selection(self) -> Selection {
        match self {
            Node::All => Selection::All,
            Node::Starred => Selection::Starred,
            Node::Folder(id) => Selection::Folder(id),
            Node::Feed(id) => Selection::Feed(id),
        }
    }
}

/// One visible row of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub node: Node,
    pub title: String,
    /// 0 for "All items", 1 for "Starred", folders and top-level feeds, 2
    /// for feeds in folders.
    pub depth: u8,
    /// Unread items; for "Starred", the starred items.
    pub count: u64,
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

struct TreeFeed {
    id: u64,
    title: String,
    unread: u64,
}

impl TreeFeed {
    /// A feed without a title is shown by its URL.
    fn new(feed: &Feed) -> Self {
        let title = if feed.title.is_empty() {
            feed.url.clone()
        } else {
            feed.title.clone()
        };
        Self {
            id: feed.id,
            title,
            unread: 0,
        }
    }
}

struct FolderEntry {
    id: u64,
    name: String,
    feeds: Vec<TreeFeed>,
    expanded: bool,
}

pub struct FeedTree {
    folders: Vec<FolderEntry>,
    top_level: Vec<TreeFeed>,
    starred: u64,
}

impl FeedTree {
    /// Builds the tree from folders and feeds, in the order given, without
    /// counts. Folders are expanded unless they are in `collapsed`. Feeds
    /// of unknown folders are shown at the top level.
    pub fn new(folders: &[Folder], feeds: &[Feed], collapsed: &HashSet<u64>) -> Self {
        let mut folders: Vec<FolderEntry> = folders
            .iter()
            .map(|folder| FolderEntry {
                id: folder.id,
                name: folder.name.clone(),
                feeds: Vec::new(),
                expanded: !collapsed.contains(&folder.id),
            })
            .collect();
        let mut top_level = Vec::new();
        for feed in feeds {
            match feed
                .folder_id
                .and_then(|id| folders.iter_mut().find(|f| f.id == id))
            {
                Some(entry) => entry.feeds.push(TreeFeed::new(feed)),
                None => top_level.push(TreeFeed::new(feed)),
            }
        }
        Self {
            folders,
            top_level,
            starred: 0,
        }
    }

    /// The collapsed folders, to build the tree again after the feeds
    /// changed.
    pub fn collapsed(&self) -> HashSet<u64> {
        self.folders
            .iter()
            .filter(|f| !f.expanded)
            .map(|f| f.id)
            .collect()
    }

    fn feeds_mut(&mut self) -> impl Iterator<Item = &mut TreeFeed> {
        self.folders
            .iter_mut()
            .flat_map(|f| &mut f.feeds)
            .chain(&mut self.top_level)
    }

    fn feeds(&self) -> impl Iterator<Item = &TreeFeed> {
        self.folders
            .iter()
            .flat_map(|f| &f.feeds)
            .chain(&self.top_level)
    }

    /// Sets the counts: unread items per feed (feeds not listed have
    /// none) and the number of starred items.
    pub fn set_counts(&mut self, unread: &[(u64, u64)], starred: u64) {
        let unread: HashMap<u64, u64> = unread.iter().copied().collect();
        for feed in self.feeds_mut() {
            feed.unread = unread.get(&feed.id).copied().unwrap_or(0);
        }
        self.starred = starred;
    }

    /// The number of unread items in all feeds.
    pub fn unread(&self) -> u64 {
        self.feeds().map(|feed| feed.unread).sum()
    }

    /// Whether the tree has this node.
    pub fn contains(&self, node: Node) -> bool {
        match node {
            Node::All | Node::Starred => true,
            Node::Folder(id) => self.folders.iter().any(|f| f.id == id),
            Node::Feed(id) => self.feeds().any(|f| f.id == id),
        }
    }

    /// Expands a collapsed folder or collapses an expanded one.
    pub fn toggle(&mut self, folder_id: u64) {
        if let Some(entry) = self.folders.iter_mut().find(|f| f.id == folder_id) {
            entry.expanded = !entry.expanded;
        }
    }

    /// The rows to show: "All items" as the root, below it "Starred", each
    /// folder followed by its feeds if expanded, then the feeds without a
    /// folder.
    pub fn rows(&self) -> Vec<Row> {
        let leaf = |node, title: &str, depth, count, last, guides| Row {
            node,
            title: title.to_owned(),
            depth,
            count,
            expanded: None,
            last,
            guides,
        };
        let children = 1 + self.folders.len() + self.top_level.len();
        let mut rows = vec![
            leaf(Node::All, "All items", 0, self.unread(), true, Vec::new()),
            leaf(
                Node::Starred,
                "Starred",
                1,
                self.starred,
                children == 1,
                Vec::new(),
            ),
        ];

        for (i, entry) in self.folders.iter().enumerate() {
            let last = i + 2 == children;
            rows.push(Row {
                node: Node::Folder(entry.id),
                title: entry.name.clone(),
                depth: 1,
                count: entry.feeds.iter().map(|feed| feed.unread).sum(),
                expanded: Some(entry.expanded),
                last,
                guides: Vec::new(),
            });
            if entry.expanded {
                let count = entry.feeds.len();
                rows.extend(entry.feeds.iter().enumerate().map(|(j, feed)| {
                    leaf(
                        Node::Feed(feed.id),
                        &feed.title,
                        2,
                        feed.unread,
                        j + 1 == count,
                        vec![!last],
                    )
                }));
            }
        }
        let offset = 1 + self.folders.len();
        rows.extend(self.top_level.iter().enumerate().map(|(i, feed)| {
            leaf(
                Node::Feed(feed.id),
                &feed.title,
                1,
                feed.unread,
                offset + i + 1 == children,
                Vec::new(),
            )
        }));
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

/// The rows that differ between `old` and `new`, if both show the same
/// nodes in the same order, so that only these need updating; `None` if
/// the rows changed otherwise (a folder expanded, feeds added).
pub fn changed_rows(old: &[Row], new: &[Row]) -> Option<Vec<usize>> {
    if old.len() != new.len() || old.iter().zip(new).any(|(a, b)| a.node != b.node) {
        return None;
    }
    Some(
        old.iter()
            .zip(new)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(id: u64, folder_id: Option<u64>) -> Feed {
        Feed {
            id,
            folder_id,
            title: format!("Feed {id}"),
            url: format!("https://example.org/{id}/feed"),
            link: None,
            favicon_link: None,
            added: None,
            ordering: 0,
            pinned: false,
            update_error_count: 0,
            last_update_error: None,
        }
    }

    fn tree() -> FeedTree {
        let folders = [Folder {
            id: 7,
            name: "Tech".to_owned(),
        }];
        let mut tree = FeedTree::new(
            &folders,
            &[feed(1, Some(7)), feed(2, None), feed(3, Some(7))],
            &HashSet::new(),
        );
        tree.set_counts(&[(1, 3), (2, 5), (3, 1)], 4);
        tree
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
                Node::Starred,
                Node::Folder(7),
                Node::Feed(1),
                Node::Feed(3),
                Node::Feed(2)
            ]
        );
        let counts: Vec<_> = rows.iter().map(|r| r.count).collect();
        assert_eq!(counts, [9, 4, 4, 3, 1, 5]);
        assert_eq!(rows[2].expanded, Some(true));
        assert_eq!(rows[3].depth, 2);
        assert_eq!(rows[5].depth, 1);
        assert_eq!(rows[1].depth, 1);
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
        let rows = FeedTree::new(&folders, &[feed(1, Some(7))], &HashSet::new()).rows();
        assert!(!rows[1].last, "starred has a sibling");
        assert!(rows[2].last);
        assert_eq!(rows[3].guides, [false]);
    }

    #[test]
    fn empty_tree() {
        let rows = FeedTree::new(&[], &[], &HashSet::new()).rows();
        assert_eq!(nodes(&rows), [Node::All, Node::Starred]);
        assert!(rows[1].last);
    }

    #[test]
    fn feed_without_title_shows_its_url() {
        let mut untitled = feed(1, None);
        untitled.title = String::new();
        let rows = FeedTree::new(&[], &[untitled], &HashSet::new()).rows();
        assert_eq!(rows[2].title, "https://example.org/1/feed");
    }

    #[test]
    fn navigate_up_down() {
        let mut tree = tree();
        assert_eq!(tree.navigate(Node::All, Key::Down), Some(Node::Starred));
        assert_eq!(
            tree.navigate(Node::Starred, Key::Down),
            Some(Node::Folder(7))
        );
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
        assert_eq!(tree.rows()[2].expanded, Some(false));
        assert_eq!(tree.navigate(Node::Folder(7), Key::Left), Some(Node::All));
        assert_eq!(tree.navigate(Node::Starred, Key::Left), Some(Node::All));
        // Right expands, then goes to the first child; on a leaf it stays.
        assert_eq!(tree.navigate(Node::Folder(7), Key::Right), None);
        assert_eq!(tree.rows()[2].expanded, Some(true));
        assert_eq!(
            tree.navigate(Node::Folder(7), Key::Right),
            Some(Node::Feed(1))
        );
        assert_eq!(tree.navigate(Node::Feed(1), Key::Right), None);
        assert_eq!(tree.navigate(Node::All, Key::Right), Some(Node::Starred));
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
        assert_eq!(
            nodes(&rows),
            [Node::All, Node::Starred, Node::Folder(7), Node::Feed(2)]
        );
        assert_eq!(rows[2].expanded, Some(false));
        assert_eq!(rows[2].count, 4);
        assert_eq!(tree.collapsed(), HashSet::from([7]));
        tree.toggle(7);
        assert_eq!(tree.rows().len(), 6);
        assert!(tree.collapsed().is_empty());
    }

    #[test]
    fn collapsed_folders_stay_collapsed() {
        let folders = [Folder {
            id: 7,
            name: "Tech".to_owned(),
        }];
        let tree = FeedTree::new(&folders, &[feed(1, Some(7))], &HashSet::from([7]));
        assert_eq!(tree.rows().len(), 3);
    }

    #[test]
    fn counts() {
        let mut tree = tree();
        tree.set_counts(&[(2, 1)], 0);
        let rows = tree.rows();
        assert_eq!(rows[0].count, 1);
        assert_eq!(rows[1].count, 0);
        assert_eq!(rows[2].count, 0);
        assert_eq!(rows[5].count, 1);
        assert_eq!(tree.unread(), 1);
    }

    #[test]
    fn contains() {
        let tree = tree();
        for node in [Node::All, Node::Starred, Node::Folder(7), Node::Feed(2)] {
            assert!(tree.contains(node), "{node:?}");
        }
        assert!(!tree.contains(Node::Folder(1)));
        assert!(!tree.contains(Node::Feed(7)));
    }

    #[test]
    fn feed_with_unknown_folder_is_top_level() {
        let rows = FeedTree::new(&[], &[feed(1, Some(99))], &HashSet::new()).rows();
        assert_eq!(nodes(&rows), [Node::All, Node::Starred, Node::Feed(1)]);
    }

    #[test]
    fn rows_changed_in_place() {
        let mut tree = tree();
        let old = tree.rows();
        tree.set_counts(&[(1, 2), (2, 5), (3, 1)], 4);
        // The feed, its folder and the root.
        assert_eq!(changed_rows(&old, &tree.rows()), Some(vec![0, 2, 3]));
        assert_eq!(changed_rows(&old, &old), Some(vec![]));
        tree.toggle(7);
        assert_eq!(changed_rows(&old, &tree.rows()), None);
    }
}
