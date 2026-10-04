// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Full web pages in tabs, rendered by Servo (spike S4).
//!
//! An [`Engine`] owns the Servo instance and its web views; the UI only
//! sees tabs, input and frames. Servo runs either in this process
//! ([`inprocess`]) or in a helper process ([`helper`]) that is started
//! with the first tab and ends with the last, see
//! `docs/decisions/0005-web-tabs.md`.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

mod browser;
mod headless;
pub mod helper;
pub mod inprocess;
#[cfg(feature = "servo-wgpu")]
pub mod wgpu;

/// Identifies a tab for the lifetime of its engine.
pub type TabId = u32;

/// Called from any thread when the engine has work for the UI thread, which
/// then calls [`Engine::pump`].
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// The size of the page area, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// Keyboard modifiers as a bit set: control 1, shift 2, alt 4, meta 8.
pub type Mods = u8;

/// Input for the active tab. Positions are in physical pixels relative to
/// the page area.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Input {
    Down {
        button: Button,
        x: f32,
        y: f32,
    },
    Up {
        button: Button,
        x: f32,
        y: f32,
    },
    Move {
        x: f32,
        y: f32,
    },
    /// Positive deltas scroll towards the top and left, as for the wheel.
    Wheel {
        dx: f32,
        dy: f32,
        x: f32,
        y: f32,
    },
    /// `text` is the key as Slint reports it: the character, or one of
    /// Slint's private-use code points for named keys.
    Key {
        text: String,
        mods: Mods,
        down: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cursor {
    Default,
    Pointer,
    Text,
}

/// What happened in the engine since the last [`Engine::pump`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Title {
        tab: TabId,
        title: String,
    },
    Loading {
        tab: TabId,
        loading: bool,
    },
    Cursor(Cursor),
    /// The engine stopped working, e.g. the helper process exited.
    Failed(String),
}

/// Servo with its web views, wherever it runs.
pub trait Engine {
    fn open(&mut self, url: &str) -> TabId;
    fn close(&mut self, tab: TabId);
    /// Shows `tab`, or no page for `None` (the article is shown); the
    /// others are hidden and throttled.
    fn activate(&mut self, tab: Option<TabId>);
    fn resize(&mut self, size: Size);
    fn input(&mut self, input: Input);
    fn set_dark(&mut self, dark: bool);
    /// Does the work the engine woke the UI thread for, and returns what
    /// happened.
    fn pump(&mut self) -> Vec<Event>;
    /// The newest frame of the active tab, if it changed since the last
    /// call.
    fn take_frame(&mut self) -> Option<slint::Image>;
    /// The helper process, if there is one, for memory measurements.
    fn helper_pid(&self) -> Option<u32> {
        None
    }
    /// Frames so far, and the mean and longest time from a painted frame
    /// to a Slint image: the readback from the GPU, plus the transfer from
    /// the helper process if there is one.
    fn frame_stats(&self) -> Option<(usize, Duration, Duration)>;
}

/// Per-second statistics of the tab pipeline, printed to stderr when
/// `REN_TAB_STATS` is set: counters and the total and longest time spent
/// in named stages.
pub struct StageStats {
    label: &'static str,
    enabled: bool,
    since: std::time::Instant,
    counts: Vec<(&'static str, u32)>,
    stages: Vec<(&'static str, Duration, Duration, u32)>,
}

impl StageStats {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            enabled: std::env::var_os("REN_TAB_STATS").is_some(),
            since: std::time::Instant::now(),
            counts: Vec::new(),
            stages: Vec::new(),
        }
    }

    pub fn count(&mut self, name: &'static str) {
        if !self.enabled {
            return;
        }
        match self.counts.iter_mut().find(|(n, _)| *n == name) {
            Some((_, c)) => *c += 1,
            None => self.counts.push((name, 1)),
        }
    }

    pub fn time(&mut self, name: &'static str, took: Duration) {
        if !self.enabled {
            return;
        }
        match self.stages.iter_mut().find(|(n, ..)| *n == name) {
            Some((_, total, max, n)) => {
                *total += took;
                *max = (*max).max(took);
                *n += 1;
            }
            None => self.stages.push((name, took, took, 1)),
        }
    }

    /// Prints and resets the statistics once a second.
    pub fn report(&mut self) {
        if !self.enabled || self.since.elapsed() < Duration::from_secs(1) {
            return;
        }
        let secs = self.since.elapsed().as_secs_f64();
        let mut line = format!("ren: {} stats over {secs:.2} s:", self.label);
        for (name, n) in &self.counts {
            line.push_str(&format!(" {name}={n}"));
        }
        for (name, total, max, n) in &self.stages {
            line.push_str(&format!(
                " {name}={:.1}ms/{n}x(max {:.1}, {:.0}%)",
                total.as_secs_f64() * 1000.0 / f64::from(*n),
                max.as_secs_f64() * 1000.0,
                total.as_secs_f64() / secs * 100.0
            ));
        }
        eprintln!("{line}");
        self.since = std::time::Instant::now();
        self.counts.clear();
        self.stages.clear();
    }
}

/// Counts frames and the time it took to get them to the UI.
#[derive(Debug, Default)]
pub struct FrameStats {
    count: usize,
    total: Duration,
    max: Duration,
}

impl FrameStats {
    pub fn add(&mut self, took: Duration) {
        self.count += 1;
        self.total += took;
        self.max = self.max.max(took);
    }

    pub fn summary(&self) -> Option<(usize, Duration, Duration)> {
        let count = u32::try_from(self.count).ok().filter(|&n| n > 0)?;
        Some((self.count, self.total / count, self.max))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_stats() {
        let mut stats = FrameStats::default();
        assert_eq!(stats.summary(), None);
        stats.add(Duration::from_millis(2));
        stats.add(Duration::from_millis(4));
        assert_eq!(
            stats.summary(),
            Some((2, Duration::from_millis(3), Duration::from_millis(4)))
        );
    }
}
