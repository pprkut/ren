// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Statistics of the frame path on both sides, for measurements.

use std::time::{Duration, Instant};

/// Per-second statistics of the tab pipeline, printed to stderr when
/// `REN_TAB_STATS` is set: counters and the total and longest time spent
/// in named stages.
pub struct StageStats {
    label: &'static str,
    enabled: bool,
    since: Instant,
    counts: Vec<(&'static str, u32)>,
    stages: Vec<(&'static str, Duration, Duration, u32)>,
}

impl StageStats {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            enabled: std::env::var_os("REN_TAB_STATS").is_some(),
            since: Instant::now(),
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
        self.since = Instant::now();
        self.counts.clear();
        self.stages.clear();
    }
}
