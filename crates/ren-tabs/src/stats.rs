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
        Self::with(label, std::env::var_os("REN_TAB_STATS").is_some())
    }

    fn with(label: &'static str, enabled: bool) -> Self {
        Self {
            label,
            enabled,
            since: Instant::now(),
            counts: Vec::new(),
            stages: Vec::new(),
        }
    }

    pub fn count(&mut self, name: &'static str) {
        self.add(name, 1);
    }

    pub fn add(&mut self, name: &'static str, count: u32) {
        if !self.enabled || count == 0 {
            return;
        }
        match self.counts.iter_mut().find(|(n, _)| *n == name) {
            Some((_, c)) => *c += count,
            None => self.counts.push((name, count)),
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
        eprintln!("{}", self.line(self.since.elapsed().as_secs_f64()));
        self.since = Instant::now();
        self.counts.clear();
        self.stages.clear();
    }

    /// The statistics of `secs` seconds as one line.
    fn line(&self, secs: f64) -> String {
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
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_stages() {
        let mut stats = StageStats::with("helper", true);
        stats.count("frames");
        stats.add("frames", 2);
        stats.add("dropped", 0);
        stats.time("paint", Duration::from_millis(4));
        stats.time("paint", Duration::from_millis(8));
        stats.time("read", Duration::from_millis(10));
        assert_eq!(
            stats.line(2.0),
            "ren: helper stats over 2.00 s: frames=3 paint=6.0ms/2x(max 8.0, 1%) \
             read=10.0ms/1x(max 10.0, 0%)"
        );
        // Kept until a second passed.
        stats.report();
        assert_eq!(stats.counts, [("frames", 3)]);
        stats.since -= Duration::from_secs(1);
        stats.report();
        assert!(stats.counts.is_empty() && stats.stages.is_empty());
    }

    #[test]
    fn nothing_when_off() {
        let mut stats = StageStats::with("ui", false);
        stats.count("frames");
        stats.time("paint", Duration::from_millis(4));
        assert_eq!(stats.line(1.0), "ren: ui stats over 1.00 s:");
    }
}
