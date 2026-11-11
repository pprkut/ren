// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Servo's memory reports, for measurements (`REN_SERVO_MEMORY_REPORT`):
//! what `about:memory` shows, written to a directory a while after each
//! load completes, to compare where the memory goes from one Servo (or
//! page) to the next.
//!
//! Each report is a file `memory-NNN.tsv`: comment lines (`#`) with the
//! milliseconds since the helper started, the tab whose load completed
//! and the number of tabs open, then a line per measurement, sorted by
//! path: the process id, the kind (`ExplicitJemallocHeapSize`, …), the
//! size in bytes and the path, its parts joined by `/`. Servo measures
//! all of its web views together, so a report is the whole helper's.

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ren_tabs::TabId;
use servo::Servo;
use servo::profile_traits::mem::MemoryReportResult;
use servo_base::generic_channel::GenericCallback;

/// How long after a load completes its report is made: what the page
/// loads and decodes after its `load` event (images, fonts, scripts'
/// work) counts too.
const DELAY: Duration = Duration::from_secs(2);

pub struct MemoryReports {
    dir: PathBuf,
    started: Instant,
    next: Cell<u32>,
    /// When each report is due, for which tab.
    due: RefCell<Vec<(Instant, Option<TabId>)>>,
}

impl MemoryReports {
    /// From `REN_SERVO_MEMORY_REPORT`, the directory (made if missing).
    pub fn from_env() -> Option<Self> {
        let dir = PathBuf::from(std::env::var_os("REN_SERVO_MEMORY_REPORT")?);
        std::fs::create_dir_all(&dir)
            .inspect_err(|err| eprintln!("ren-servo: the memory reports: {err}"))
            .ok()?;
        Some(Self {
            dir,
            started: Instant::now(),
            next: Cell::new(1),
            due: RefCell::default(),
        })
    }

    /// A tab's load completed: its report is due in a while.
    pub fn load_complete(&self, tab: Option<TabId>) {
        self.due.borrow_mut().push((Instant::now() + DELAY, tab));
    }

    /// When the next report is due, for the event loop's wait.
    pub fn next_due(&self) -> Option<Instant> {
        self.due.borrow().iter().map(|&(at, _)| at).min()
    }

    /// Asks Servo for the reports that are due; each is written when
    /// Servo has collected it.
    pub fn request_due(&self, servo: &Servo, tabs: usize) {
        let now = Instant::now();
        let due: Vec<_> = {
            let mut pending = self.due.borrow_mut();
            let (due, later) = pending.drain(..).partition(|&(at, _)| at <= now);
            *pending = later;
            due
        };
        for (_, tab) in due {
            let number = self.next.replace(self.next.get() + 1);
            let path = self.dir.join(format!("memory-{number:03}.tsv"));
            let mut header = String::new();
            let _ = writeln!(header, "# ms\t{}", self.started.elapsed().as_millis());
            let tab = tab.map_or_else(|| "-".to_owned(), |tab| tab.to_string());
            let _ = writeln!(header, "# loaded tab\t{tab}");
            let _ = writeln!(header, "# tabs open\t{tabs}");
            let callback = GenericCallback::new(move |result| match result {
                Ok(result) => write(&path, &header, &result),
                Err(err) => eprintln!("ren-servo: no memory report: {err:?}"),
            });
            match callback {
                Ok(callback) => servo.create_memory_report(callback),
                Err(err) => eprintln!("ren-servo: no memory report: {err:?}"),
            }
        }
    }
}

fn write(path: &std::path::Path, header: &str, result: &MemoryReportResult) {
    let mut lines: Vec<_> = result
        .results
        .iter()
        .flat_map(|process| {
            process.reports.iter().map(move |report| {
                (
                    report.path.join("/"),
                    format!("{}\t{:?}\t{}", process.pid, report.kind, report.size),
                )
            })
        })
        .collect();
    lines.sort();
    let mut text = header.to_owned();
    for (path, fields) in lines {
        let _ = writeln!(text, "{fields}\t{path}");
    }
    if let Err(err) = std::fs::write(path, text) {
        eprintln!("ren-servo: {}: {err}", path.display());
    }
}
