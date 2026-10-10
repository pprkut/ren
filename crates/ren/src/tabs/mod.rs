// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Full web pages in tabs, rendered by Servo in a helper process
//! ([`helper`]) that is started with the first tab and ends with the
//! last, see `docs/decisions/0005-web-tabs.md` and
//! `docs/decisions/0011-tabs.md`.

use std::sync::Arc;
use std::time::Duration;

pub mod helper;
pub mod list;

/// Called from any thread when the helper has sent something, which the
/// UI thread then takes with [`helper::Helper::pump`].
pub type Waker = Arc<dyn Fn() + Send + Sync>;

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
