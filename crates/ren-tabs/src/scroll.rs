// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smooth wheel scrolling, for pages in the helper (physical pixels) and
//! for the window's article view (logical pixels).

use std::time::{Duration, Instant};

/// Wheel steps at least this long (pixels) are spread over a few frames;
/// shorter ones, as from touchpads, are already smooth.
const SMOOTH_MIN: f32 = 40.0;
/// Time between the parts of a wheel step.
pub const SMOOTH_TICK: Duration = Duration::from_millis(16);
/// The part of what is left that each tick scrolls.
const SMOOTH_PART: f32 = 0.35;
/// Below this, the rest is scrolled at once.
const SMOOTH_REST: f32 = 2.0;

/// Servo and Blitz scroll each wheel step at once; this spreads long steps
/// over about 100 ms, each tick scrolling a part of what is left.
#[derive(Debug, Default)]
pub struct SmoothScroll {
    /// What is left to scroll, and where.
    pending: Option<Wheel>,
    next_tick: Option<Instant>,
}

/// A wheel delta at a position, as in [`crate::Input::Wheel`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wheel {
    pub dx: f32,
    pub dy: f32,
    pub x: f32,
    pub y: f32,
}

impl SmoothScroll {
    /// Takes a wheel step; returns what to scroll now if it isn't spread.
    pub fn add(&mut self, wheel: Wheel, now: Instant) -> Option<Wheel> {
        if wheel.dx.abs().max(wheel.dy.abs()) < SMOOTH_MIN {
            // Whatever is pending goes first.
            let rest = self.pending.take().map(|p| Wheel {
                dx: p.dx + wheel.dx,
                dy: p.dy + wheel.dy,
                ..wheel
            });
            self.next_tick = None;
            return Some(rest.unwrap_or(wheel));
        }
        let pending = match self.pending {
            // The same direction adds up; another starts over.
            Some(p) if p.dx * wheel.dx >= 0.0 && p.dy * wheel.dy >= 0.0 => Wheel {
                dx: p.dx + wheel.dx,
                dy: p.dy + wheel.dy,
                ..wheel
            },
            _ => wheel,
        };
        self.pending = Some(pending);
        if self.next_tick.is_none() {
            self.next_tick = Some(now);
        }
        None
    }

    /// When the next part is due, if anything is left.
    pub fn next_tick(&self) -> Option<Instant> {
        self.next_tick
    }

    /// The part to scroll now, if one is due.
    pub fn tick(&mut self, now: Instant) -> Option<Wheel> {
        if self.next_tick.is_none_or(|due| now < due) {
            return None;
        }
        self.step(now)
    }

    /// The next part, if anything is left, whether or not it is due: for
    /// callers with a timer of their own that fires every [`SMOOTH_TICK`]
    /// (and may fire a little early).
    pub fn step(&mut self, now: Instant) -> Option<Wheel> {
        let pending = self.pending?;
        let part = |d: f32| {
            if d.abs() * (1.0 - SMOOTH_PART) < SMOOTH_REST {
                d
            } else {
                d * SMOOTH_PART
            }
        };
        let step = Wheel {
            dx: part(pending.dx),
            dy: part(pending.dy),
            ..pending
        };
        let rest = Wheel {
            dx: pending.dx - step.dx,
            dy: pending.dy - step.dy,
            ..pending
        };
        if rest.dx == 0.0 && rest.dy == 0.0 {
            self.pending = None;
            self.next_tick = None;
        } else {
            self.pending = Some(rest);
            self.next_tick = Some(now + SMOOTH_TICK);
        }
        Some(step)
    }

    /// Forgets what is left, e.g. when another tab or article is shown.
    pub fn clear(&mut self) {
        self.pending = None;
        self.next_tick = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(dy: f32) -> Wheel {
        Wheel {
            dx: 0.0,
            dy,
            x: 10.0,
            y: 20.0,
        }
    }

    #[test]
    fn short_steps_go_at_once() {
        let mut scroll = SmoothScroll::default();
        let now = Instant::now();
        assert_eq!(scroll.add(wheel(-10.0), now), Some(wheel(-10.0)));
        assert_eq!(scroll.next_tick(), None);
    }

    #[test]
    fn long_steps_are_spread() {
        let mut scroll = SmoothScroll::default();
        let mut now = Instant::now();
        assert_eq!(scroll.add(wheel(-120.0), now), None);
        let mut total = 0.0;
        let mut ticks = 0;
        while let Some(due) = scroll.next_tick() {
            now = due;
            let step = scroll.tick(now).unwrap();
            assert!(step.dy < 0.0 && step.x == 10.0);
            total += step.dy;
            ticks += 1;
            assert!(ticks < 20);
        }
        assert!((total + 120.0).abs() < 0.01, "{total}");
        assert!(ticks > 3, "{ticks} ticks");
        assert_eq!(scroll.tick(now + SMOOTH_TICK), None);
    }

    #[test]
    fn steps_add_up_or_start_over() {
        let mut scroll = SmoothScroll::default();
        let now = Instant::now();
        scroll.add(wheel(-120.0), now);
        scroll.add(wheel(-120.0), now);
        assert_eq!(scroll.tick(now).unwrap().dy, -240.0 * SMOOTH_PART);
        scroll.add(wheel(120.0), now);
        assert_eq!(
            scroll.tick(now + SMOOTH_TICK).unwrap().dy,
            120.0 * SMOOTH_PART
        );
        // A short step takes the rest along.
        let rest = scroll.add(wheel(5.0), now).unwrap();
        assert_eq!(rest.dy, 120.0 * (1.0 - SMOOTH_PART) + 5.0);
        assert_eq!(scroll.next_tick(), None);
    }

    #[test]
    fn steps_without_waiting() {
        let mut scroll = SmoothScroll::default();
        let now = Instant::now();
        assert_eq!(scroll.step(now), None);
        scroll.add(wheel(-120.0), now);
        let mut total = 0.0;
        while scroll.next_tick().is_some() {
            total += scroll.step(now).unwrap().dy;
        }
        assert!((total + 120.0).abs() < 0.01, "{total}");
        assert_eq!(scroll.step(now), None);
    }

    #[test]
    fn not_before_the_tick() {
        let mut scroll = SmoothScroll::default();
        let now = Instant::now();
        scroll.add(wheel(-120.0), now);
        assert!(scroll.tick(now).is_some());
        assert_eq!(scroll.tick(now), None);
        scroll.clear();
        assert_eq!(scroll.tick(now + SMOOTH_TICK), None);
    }
}
