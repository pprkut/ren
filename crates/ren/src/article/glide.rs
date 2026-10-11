// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smooth scrolling by keyboard in the article view: a key moves the
//! target, and the view eases towards it over a few frames instead of
//! jumping there.

use std::time::{Duration, Instant};

/// How long the view takes to reach a new target.
pub const DURATION: Duration = Duration::from_millis(150);

/// A scroll from one offset to another, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glide {
    from: f32,
    to: f32,
    start: Instant,
}

impl Glide {
    pub fn new(from: f32, to: f32, start: Instant) -> Self {
        Self { from, to, start }
    }

    /// Where the glide ends; the next key moves on from there.
    pub fn target(&self) -> f32 {
        self.to
    }

    /// The offset at `now`, and whether the glide is over. Fast at first,
    /// slowing down towards the target (cubic ease-out).
    pub fn at(&self, now: Instant) -> (f32, bool) {
        let elapsed = now.saturating_duration_since(self.start);
        let t = (elapsed.as_secs_f32() / DURATION.as_secs_f32()).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        (self.from + (self.to - self.from) * eased, t >= 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_to_the_target() {
        let start = Instant::now();
        let glide = Glide::new(100.0, 500.0, start);
        assert_eq!(glide.target(), 500.0);
        assert_eq!(glide.at(start), (100.0, false));
        let (half, done) = glide.at(start + DURATION / 2);
        assert!(!done);
        // More than half the way after half the time.
        assert!(half > 300.0 && half < 500.0, "{half}");
        assert_eq!(glide.at(start + DURATION), (500.0, true));
        assert_eq!(glide.at(start + DURATION * 3), (500.0, true));

        let up = Glide::new(500.0, 0.0, start);
        assert_eq!(up.at(start + DURATION), (0.0, true));
    }
}
