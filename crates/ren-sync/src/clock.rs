// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

use std::time::{SystemTime, UNIX_EPOCH};

/// The local time, for purge ages and the time since the last sync.
pub trait Clock {
    /// Unix seconds.
    fn now(&self) -> i64;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock() {
        // After 2026-01-01.
        assert!(SystemClock.now() > 1_767_225_600);
    }
}
