// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic placeholder data for the S1 window spike.
//!
//! Stands in for the store until phase 2: folders, feeds and items are
//! derived from their index, so nothing but the read flags is kept in memory
//! and item rows and bodies are generated only when the UI asks for them.

/// A folder in the feed tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: u32,
    pub name: String,
}

/// A feed, optionally inside a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feed {
    pub id: u32,
    pub folder_id: Option<u32>,
    pub title: String,
}

/// Read state of an item, as shown in the item list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Unread, and arrived in the latest sync.
    New,
    Unread,
    Read,
}

/// The fields shown in the item list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemSummary {
    pub id: u32,
    pub feed_id: u32,
    pub title: String,
    pub author: String,
    /// Publication date, unix seconds.
    pub pub_date: i64,
    pub status: Status,
    pub starred: bool,
}

const FOLDERS: &[&str] = &["Technology", "Science", "News", "Comics"];

/// Feed titles; the first `FOLDER_SIZES.iter().sum()` go into folders in
/// order, the rest are top-level.
const FEEDS: &[&str] = &[
    "LWN.net",
    "Rust Blog",
    "This Week in Rust",
    "Hacker News",
    "Phoronix",
    "KDE Planet",
    "Quanta Magazine",
    "Nature News",
    "Ars Technica Science",
    "The Guardian",
    "BBC World",
    "Tagesschau",
    "NOS Nieuws",
    "Deutsche Welle",
    "xkcd",
    "Saturday Morning Breakfast Cereal",
    "Nextcloud Blog",
    "Planet GNOME",
    "Servo Blog",
];

const FOLDER_SIZES: &[usize] = &[6, 3, 5, 2];

const AUTHORS: &[&str] = &[
    "Ada Lovelace",
    "Alan Turing",
    "Barbara Liskov",
    "Dennis Ritchie",
    "Edsger Dijkstra",
    "Frances Allen",
    "Grace Hopper",
    "Hedy Lamarr",
    "Ken Thompson",
    "Margaret Hamilton",
    "Radia Perlman",
    "Sophie Wilson",
];

const WORDS: &[&str] = &[
    "release",
    "kernel",
    "memory",
    "renderer",
    "update",
    "security",
    "browser",
    "compiler",
    "desktop",
    "network",
    "storage",
    "research",
    "climate",
    "election",
    "report",
    "study",
    "announces",
    "improves",
    "breaks",
    "fixes",
    "introduces",
    "removes",
    "faster",
    "smaller",
    "new",
    "old",
    "open",
    "native",
    "portable",
    "stable",
    "experimental",
    "quantum",
    "planet",
    "graphics",
    "driver",
    "audio",
    "font",
    "layout",
    "engine",
    "toolkit",
    "community",
    "project",
];

const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod \
tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud \
exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in \
reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint \
occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.";

/// Newest item date: 2026-09-01 00:00:00 UTC.
const NEWEST: i64 = 1_788_220_800;

/// Cheap deterministic hash (splitmix64) so every field of an item can be
/// derived from its id without storing it.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn pick<T: Copy>(list: &[T], seed: u64) -> T {
    list[(seed % list.len() as u64) as usize]
}

/// Share of the items that arrived in the "latest sync": the newest 2 %.
const NEW_SHARE: usize = 50;

/// Placeholder data source with `item_count` items spread over all feeds.
pub struct DummyData {
    folders: Vec<Folder>,
    feeds: Vec<Feed>,
    /// Per-item read flag, indexed by item id. The only mutable state.
    read: Vec<bool>,
    /// Items with a lower id arrived in the latest sync.
    new_below: u32,
}

impl DummyData {
    pub fn new(item_count: usize) -> Self {
        let folders = FOLDERS
            .iter()
            .enumerate()
            .map(|(i, name)| Folder {
                id: i as u32,
                name: (*name).to_owned(),
            })
            .collect();

        let mut folder_of = Vec::with_capacity(FEEDS.len());
        for (folder, &size) in FOLDER_SIZES.iter().enumerate() {
            folder_of.extend(std::iter::repeat_n(Some(folder as u32), size));
        }
        folder_of.resize(FEEDS.len(), None);

        let feeds = FEEDS
            .iter()
            .zip(folder_of)
            .enumerate()
            .map(|(i, (title, folder_id))| Feed {
                id: i as u32,
                folder_id,
                title: (*title).to_owned(),
            })
            .collect();

        // Roughly a third of the older items is unread, the new ones all
        // are.
        let new_below = item_count.div_ceil(NEW_SHARE) as u32;
        let read = (0..item_count as u64)
            .map(|i| i >= u64::from(new_below) && !mix(i).is_multiple_of(3))
            .collect();

        Self {
            folders,
            feeds,
            read,
            new_below,
        }
    }

    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    pub fn feeds(&self) -> &[Feed] {
        &self.feeds
    }

    pub fn item_count(&self) -> usize {
        self.read.len()
    }

    pub fn feed_of(&self, item_id: u32) -> u32 {
        (mix(u64::from(item_id) ^ 0xfeed) % self.feeds.len() as u64) as u32
    }

    /// Item ids, newest first; all items, or only those of the given feeds.
    pub fn item_ids(&self, feeds: Option<&[u32]>) -> Vec<u32> {
        (0..self.item_count() as u32)
            .filter(|&id| feeds.is_none_or(|f| f.contains(&self.feed_of(id))))
            .collect()
    }

    pub fn unread_count(&self, feed_id: u32) -> usize {
        (0..self.item_count() as u32)
            .filter(|&id| !self.read[id as usize] && self.feed_of(id) == feed_id)
            .count()
    }

    pub fn item(&self, id: u32) -> ItemSummary {
        let seed = mix(u64::from(id));
        let words = 4 + (seed % 7) as usize;
        let mut title = String::new();
        for w in 0..words {
            if w > 0 {
                title.push(' ');
            }
            title.push_str(pick(WORDS, mix(seed ^ w as u64)));
        }
        if let Some(first) = title.get_mut(0..1) {
            first.make_ascii_uppercase();
        }

        let status = if self.read[id as usize] {
            Status::Read
        } else if id < self.new_below {
            Status::New
        } else {
            Status::Unread
        };

        ItemSummary {
            id,
            feed_id: self.feed_of(id),
            title,
            author: pick(AUTHORS, mix(seed ^ 0xa07)).to_owned(),
            // Items are between 5 and 60 minutes apart, id 0 is the newest.
            pub_date: NEWEST - i64::from(id) * 1_950 - (seed % 600) as i64,
            status,
            starred: seed.is_multiple_of(50),
        }
    }

    /// Plain-text article body, 3 to 12 paragraphs.
    pub fn body(&self, id: u32) -> String {
        let seed = mix(u64::from(id) ^ 0xb0d7);
        let paragraphs = 3 + (seed % 10) as usize;
        let mut body = String::new();
        for p in 0..paragraphs {
            if p > 0 {
                body.push_str("\n\n");
            }
            let repeat = 1 + (mix(seed ^ p as u64) % 3) as usize;
            for r in 0..repeat {
                if r > 0 {
                    body.push(' ');
                }
                body.push_str(LOREM);
            }
        }
        body
    }

    /// Marks an item read. Returns whether it was unread before.
    pub fn mark_read(&mut self, id: u32) -> bool {
        !std::mem::replace(&mut self.read[id as usize], true)
    }

    /// Marks an item unread. Returns whether it was read before. An item
    /// from the latest sync is new again.
    pub fn mark_unread(&mut self, id: u32) -> bool {
        std::mem::replace(&mut self.read[id as usize], false)
    }
}

/// Formats unix seconds as `YYYY-MM-DD HH:MM` (UTC).
pub fn format_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rem / 3_600,
        rem % 3_600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_are_deterministic() {
        let a = DummyData::new(100);
        let b = DummyData::new(100);
        assert_eq!(a.item(42), b.item(42));
        assert_eq!(a.body(42), b.body(42));
    }

    #[test]
    fn items_are_newest_first() {
        let data = DummyData::new(100);
        assert!((1..100).all(|id| data.item(id).pub_date < data.item(id - 1).pub_date));
    }

    #[test]
    fn feed_filter() {
        let data = DummyData::new(1_000);
        let all = data.item_ids(None);
        assert_eq!(all.len(), 1_000);
        let total: usize = data
            .feeds()
            .iter()
            .map(|f| data.item_ids(Some(&[f.id])).len())
            .sum();
        assert_eq!(total, 1_000);
        assert!(
            data.item_ids(Some(&[3, 5]))
                .iter()
                .all(|&id| [3, 5].contains(&data.feed_of(id)))
        );
    }

    #[test]
    fn mark_read() {
        let mut data = DummyData::new(100);
        let id = (0..100)
            .find(|&id| data.item(id).status != Status::Read)
            .unwrap();
        let feed = data.feed_of(id);
        let before = data.unread_count(feed);
        assert!(data.mark_read(id));
        assert!(!data.mark_read(id));
        assert_eq!(data.item(id).status, Status::Read);
        assert_eq!(data.unread_count(feed), before - 1);
        assert!(data.mark_unread(id));
        assert!(!data.mark_unread(id));
        assert_ne!(data.item(id).status, Status::Read);
        assert_eq!(data.unread_count(feed), before);
    }

    #[test]
    fn newest_items_are_new() {
        let mut data = DummyData::new(1_000);
        let statuses: Vec<_> = (0..30).map(|id| data.item(id).status).collect();
        assert!(statuses[..20].iter().all(|&s| s == Status::New));
        assert!(statuses[20..].iter().all(|&s| s != Status::New));
        assert!(data.mark_read(0));
        assert_eq!(data.item(0).status, Status::Read);
    }

    #[test]
    fn folders_and_feeds() {
        let data = DummyData::new(0);
        assert_eq!(data.folders().len(), FOLDERS.len());
        assert_eq!(data.feeds().len(), FEEDS.len());
        let in_folders = data
            .feeds()
            .iter()
            .filter(|f| f.folder_id.is_some())
            .count();
        assert_eq!(in_folders, FOLDER_SIZES.iter().sum::<usize>());
    }

    #[test]
    fn date_formatting() {
        assert_eq!(format_date(0), "1970-01-01 00:00");
        assert_eq!(format_date(NEWEST), "2026-09-01 00:00");
        assert_eq!(format_date(951_827_696), "2000-02-29 12:34");
    }
}
