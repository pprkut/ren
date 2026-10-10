// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The open tabs and which one is shown, without the UI.

use ren_tabs::{Event, TabId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub id: TabId,
    /// The page's title, else its URL.
    pub title: String,
    pub url: String,
    pub loading: bool,
    pub back: bool,
    pub forward: bool,
}

impl Tab {
    fn new(id: TabId, url: &str) -> Self {
        Self {
            id,
            title: String::new(),
            url: url.to_owned(),
            loading: true,
            back: false,
            forward: false,
        }
    }

    /// What the tab bar shows.
    pub fn label(&self) -> &str {
        if self.title.is_empty() {
            &self.url
        } else {
            &self.title
        }
    }
}

/// What the view has to do after the list changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Changes {
    /// Rows changed in place.
    pub rows: Vec<usize>,
    /// Rows were added or removed.
    pub list: bool,
    /// Another tab (or the article) is shown.
    pub current: bool,
    /// A link for the desktop (mail).
    pub external: Option<String>,
    /// A page crashed, with the reason.
    pub crashed: Option<String>,
}

#[derive(Debug, Default)]
pub struct TabList {
    tabs: Vec<Tab>,
    /// The tab shown; `None` shows the article.
    current: Option<usize>,
}

impl TabList {
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn current(&self) -> Option<usize> {
        self.current
    }

    pub fn current_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.current?)
    }

    fn index(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Adds a tab at the end and shows it.
    pub fn open(&mut self, id: TabId, url: &str) -> usize {
        self.tabs.push(Tab::new(id, url));
        self.current = Some(self.tabs.len() - 1);
        self.tabs.len() - 1
    }

    /// Shows the tab at `index`, or the article for `None`; returns
    /// whether that changed anything.
    pub fn select(&mut self, index: Option<usize>) -> bool {
        let index = index.filter(|&i| i < self.tabs.len());
        let changed = index != self.current;
        self.current = index;
        changed
    }

    /// Removes the tab at `index`. When it was shown, its right neighbour
    /// is shown, else its left one, else the article.
    pub fn close(&mut self, index: usize) -> Option<TabId> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        self.current = match self.current {
            Some(current) if current == index => {
                (!self.tabs.is_empty()).then(|| index.min(self.tabs.len() - 1))
            }
            Some(current) if current > index => Some(current - 1),
            current => current,
        };
        Some(tab.id)
    }

    /// Removes all tabs, e.g. when the helper ended.
    pub fn clear(&mut self) {
        self.tabs.clear();
        self.current = None;
    }

    /// Applies an event from the helper.
    pub fn apply(&mut self, event: Event, changes: &mut Changes) {
        let mut update = |id: TabId, f: &mut dyn FnMut(&mut Tab)| {
            if let Some(index) = self.index(id) {
                f(&mut self.tabs[index]);
                changes.rows.push(index);
            }
        };
        match event {
            Event::Title { tab, title } => update(tab, &mut |t| t.title = title.clone()),
            Event::Url { tab, url } => update(tab, &mut |t| t.url = url.clone()),
            Event::Loading { tab, loading } => update(tab, &mut |t| t.loading = loading),
            Event::History { tab, back, forward } => update(tab, &mut |t| {
                t.back = back;
                t.forward = forward;
            }),
            Event::Opened { tab, opener } => {
                // Next to its opener, and shown, as pages open tabs on
                // clicks.
                let at = self.index(opener).map_or(self.tabs.len(), |i| i + 1);
                self.tabs.insert(at, Tab::new(tab, ""));
                self.current = Some(at);
                changes.list = true;
                changes.current = true;
            }
            Event::Closed { tab } => {
                if let Some(index) = self.index(tab) {
                    let shown = self.current == Some(index);
                    self.close(index);
                    changes.list = true;
                    changes.current |= shown;
                }
            }
            Event::External { url } => changes.external = Some(url),
            Event::Crashed { tab, reason } => {
                update(tab, &mut |t| t.loading = false);
                changes.crashed = Some(reason);
            }
            // The cursor and frames are the view's.
            Event::Cursor(_) | Event::Frame { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: u32) -> TabList {
        let mut list = TabList::default();
        for id in 0..n {
            list.open(id, &format!("https://example.org/{id}"));
        }
        list
    }

    fn ids(list: &TabList) -> Vec<TabId> {
        list.tabs().iter().map(|t| t.id).collect()
    }

    #[test]
    fn opening_shows_the_tab() {
        let list = list(3);
        assert_eq!(list.current(), Some(2));
        assert_eq!(list.current_tab().unwrap().label(), "https://example.org/2");
        assert!(list.current_tab().unwrap().loading);
    }

    #[test]
    fn closing_the_shown_tab_shows_a_neighbour() {
        let mut list = list(3);
        list.select(Some(1));
        assert_eq!(list.close(1), Some(1));
        assert_eq!(ids(&list), [0, 2]);
        assert_eq!(list.current(), Some(1));
        list.close(1);
        assert_eq!(list.current(), Some(0));
        list.close(0);
        assert_eq!(list.current(), None);
        assert!(list.is_empty());
        assert_eq!(list.close(0), None);
    }

    #[test]
    fn closing_other_tabs_keeps_the_shown_one() {
        let mut list = list(3);
        list.select(Some(2));
        list.close(0);
        assert_eq!(list.current_tab().unwrap().id, 2);
        list.select(None);
        list.close(0);
        assert_eq!(list.current(), None);
    }

    #[test]
    fn select() {
        let mut list = list(2);
        assert!(list.select(None));
        assert!(!list.select(None));
        assert!(list.select(Some(0)));
        assert!(list.select(Some(5)));
        assert_eq!(list.current(), None);
    }

    #[test]
    fn events_update_tabs() {
        let mut list = list(2);
        let mut changes = Changes::default();
        for event in [
            Event::Title {
                tab: 0,
                title: "Zero".to_owned(),
            },
            Event::Url {
                tab: 0,
                url: "https://example.org/moved".to_owned(),
            },
            Event::Loading {
                tab: 0,
                loading: false,
            },
            Event::History {
                tab: 0,
                back: true,
                forward: false,
            },
            Event::Title {
                tab: 9,
                title: "Gone".to_owned(),
            },
        ] {
            list.apply(event, &mut changes);
        }
        let tab = &list.tabs()[0];
        assert_eq!(tab.label(), "Zero");
        assert_eq!(tab.url, "https://example.org/moved");
        assert!(!tab.loading && tab.back && !tab.forward);
        assert_eq!(changes.rows, [0, 0, 0, 0]);
        assert!(!changes.list && !changes.current);
    }

    #[test]
    fn pages_open_and_close_tabs() {
        let mut list = list(3);
        let mut changes = Changes::default();
        list.apply(
            Event::Opened {
                tab: 100,
                opener: 0,
            },
            &mut changes,
        );
        assert_eq!(ids(&list), [0, 100, 1, 2]);
        assert_eq!(list.current(), Some(1));
        assert!(changes.list && changes.current);

        let mut changes = Changes::default();
        list.apply(Event::Closed { tab: 100 }, &mut changes);
        assert_eq!(ids(&list), [0, 1, 2]);
        assert_eq!(list.current(), Some(1));
        assert!(changes.list && changes.current);

        let mut changes = Changes::default();
        list.apply(Event::Closed { tab: 2 }, &mut changes);
        assert!(changes.list && !changes.current);
    }

    #[test]
    fn external_links_and_crashes() {
        let mut list = list(1);
        let mut changes = Changes::default();
        list.apply(
            Event::External {
                url: "mailto:a@example.org".to_owned(),
            },
            &mut changes,
        );
        list.apply(
            Event::Crashed {
                tab: 0,
                reason: "boom".to_owned(),
            },
            &mut changes,
        );
        assert_eq!(changes.external.as_deref(), Some("mailto:a@example.org"));
        assert_eq!(changes.crashed.as_deref(), Some("boom"));
        assert!(!list.tabs()[0].loading);
    }
}
