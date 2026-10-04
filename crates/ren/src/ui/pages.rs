// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Web pages in tabs next to the article: the tab bar and the page pane,
//! backed by a Servo [`Engine`] that exists only while tabs are open.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, VecModel};

use super::{MainWindow, TabRow};
use crate::cli::TabMode;
use crate::tabs::{Button, Cursor, Engine, Event, Input, Size, TabId, Waker};

pub struct Pages {
    window: slint::Weak<MainWindow>,
    mode: TabMode,
    engine: Option<Box<dyn Engine>>,
    /// Servo has run in this process, so it can't run here again.
    servo_used: bool,
    /// The engine's tab ids, in the order of `rows`.
    tabs: Vec<TabId>,
    rows: Rc<VecModel<TabRow>>,
    measure: bool,
    /// A pump is already queued on the event loop.
    pump_queued: Arc<AtomicBool>,
    stats: crate::tabs::StageStats,
}

impl Pages {
    pub fn new(window: &MainWindow, mode: TabMode, measure: bool) -> Rc<RefCell<Self>> {
        let rows = Rc::new(VecModel::default());
        window.set_tabs(rows.clone().into());
        let pages = Rc::new(RefCell::new(Self {
            window: window.as_weak(),
            mode,
            engine: None,
            servo_used: false,
            tabs: Vec::new(),
            rows,
            measure,
            stats: crate::tabs::StageStats::new("ui"),
            pump_queued: Arc::default(),
        }));
        connect(window, &pages);
        pages
    }

    fn window(&self) -> MainWindow {
        self.window
            .upgrade()
            .expect("the pages only live as long as the window")
    }

    pub fn helper_pid(&self) -> Option<u32> {
        self.engine.as_ref()?.helper_pid()
    }

    /// Whether Servo can still be started after the last tab closed.
    pub fn can_restart(&self) -> bool {
        self.mode == TabMode::Helper
    }

    /// Frames shown so far by the current engine.
    pub fn frame_count(&self) -> usize {
        self.engine
            .as_ref()
            .and_then(|e| e.frame_stats())
            .map_or(0, |(count, _, _)| count)
    }

    /// Whether any open tab is still loading.
    pub fn loading(&self) -> bool {
        self.rows.iter().any(|row| row.loading)
    }

    fn size(&self) -> Size {
        let window = self.window();
        let scale = window.window().scale_factor();
        Size {
            width: (window.get_page_width() * scale).round().max(1.0) as u32,
            height: (window.get_page_height() * scale).round().max(1.0) as u32,
            scale,
        }
    }

    /// Wakes the UI thread to call `pump`, at most once per pending pump.
    fn waker(&self) -> Waker {
        let weak = self.window.clone();
        let queued = self.pump_queued.clone();
        Arc::new(move || {
            if queued.swap(true, Ordering::AcqRel) {
                return;
            }
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    window.invoke_page_pump();
                }
            });
        })
    }

    fn start_engine(&mut self) -> Result<Box<dyn Engine>, String> {
        let window = self.window();
        let size = self.size();
        let dark = window.get_dark();
        let waker = self.waker();
        let started = Instant::now();
        let engine: Box<dyn Engine> = match self.mode {
            TabMode::Helper => Box::new(crate::tabs::helper::Helper::start(waker, size, dark)?),
            _ if self.servo_used => {
                return Err("Servo can't be started again in this process".to_owned());
            }
            TabMode::InProcess => {
                Box::new(crate::tabs::inprocess::InProcess::new(waker, size, dark)?)
            }
            #[cfg(feature = "servo-wgpu")]
            TabMode::Wgpu => Box::new(crate::tabs::wgpu::Wgpu::new(waker, size, dark)?),
            #[cfg(not(feature = "servo-wgpu"))]
            TabMode::Wgpu => return Err("built without the servo-wgpu feature".to_owned()),
        };
        self.servo_used = true;
        if self.measure {
            eprintln!(
                "ren: Servo started ({:?}) in {:.1} ms",
                self.mode,
                started.elapsed().as_secs_f64() * 1_000.0
            );
        }
        Ok(engine)
    }

    pub fn open(&mut self, url: &str) -> Result<(), String> {
        if self.engine.is_none() {
            let engine = self.start_engine();
            match engine {
                Ok(engine) => self.engine = Some(engine),
                Err(err) => {
                    self.window().set_current_tab(-1);
                    return Err(err);
                }
            }
        }
        let engine = self.engine.as_mut().expect("started above");
        let tab = engine.open(url);
        self.tabs.push(tab);
        self.rows.push(TabRow {
            title: url.into(),
            loading: true,
        });
        self.select(self.tabs.len() as i32 - 1);
        Ok(())
    }

    /// Shows the article instead of a page; the tabs stay open.
    pub fn show_article(&mut self) {
        if self.window().get_current_tab() >= 0 {
            self.select(-1);
        }
    }

    /// Shows tab `index`, or the article for -1.
    fn select(&mut self, index: i32) {
        let window = self.window();
        window.set_current_tab(index);
        let tab = usize::try_from(index)
            .ok()
            .and_then(|i| self.tabs.get(i).copied());
        if let Some(engine) = &mut self.engine {
            engine.activate(tab);
            self.request_pump();
        }
    }

    pub fn close(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(index);
        self.rows.remove(index);
        if let Some(engine) = &mut self.engine {
            engine.close(tab);
        }
        if self.tabs.is_empty() {
            self.stop_engine();
            self.select(-1);
            return;
        }
        let current = self.window().get_current_tab();
        if current >= index as i32 {
            self.select((current - 1).max(0).min(self.tabs.len() as i32 - 1));
        }
    }

    pub fn close_all(&mut self) {
        while !self.tabs.is_empty() {
            self.close(self.tabs.len() - 1);
        }
    }

    fn stop_engine(&mut self) {
        let Some(engine) = self.engine.take() else {
            return;
        };
        if self.measure
            && let Some((frames, mean, max)) = engine.frame_stats()
        {
            eprintln!(
                "ren: page frames: {frames}, to the UI in {:.2} ms on average, {:.2} ms at most",
                mean.as_secs_f64() * 1_000.0,
                max.as_secs_f64() * 1_000.0
            );
        }
        let started = Instant::now();
        drop(engine);
        if self.measure {
            eprintln!(
                "ren: Servo stopped in {:.1} ms",
                started.elapsed().as_secs_f64() * 1_000.0
            );
        }
        let window = self.window();
        window.set_page_image(slint::Image::default());
        window.set_page_cursor(0);
    }

    fn request_pump(&self) {
        (self.waker())();
    }

    fn pump(&mut self) {
        self.pump_queued.store(false, Ordering::Release);
        let Some(engine) = &mut self.engine else {
            return;
        };
        let events = engine.pump();
        let frame = engine.take_frame();
        let window = self.window();
        if let Some(frame) = frame {
            self.stats.count("frames");
            window.set_page_image(frame);
        }
        self.stats.report();
        for event in events {
            match event {
                Event::Title { tab, title } => self.update_row(tab, |row| row.title = title.into()),
                Event::Loading { tab, loading } => {
                    self.update_row(tab, |row| row.loading = loading)
                }
                Event::Cursor(cursor) => window.set_page_cursor(match cursor {
                    Cursor::Default => 0,
                    Cursor::Pointer => 1,
                    Cursor::Text => 2,
                }),
                Event::Failed(reason) => {
                    eprintln!("ren: {reason}");
                    window.set_status_text(reason.into());
                    self.tabs.clear();
                    self.rows.clear();
                    self.engine = None;
                    self.select(-1);
                    return;
                }
            }
        }
    }

    fn update_row(&self, tab: TabId, update: impl FnOnce(&mut TabRow)) {
        if let Some(index) = self.tabs.iter().position(|&t| t == tab)
            && let Some(mut row) = self.rows.row_data(index)
        {
            update(&mut row);
            self.rows.set_row_data(index, row);
        }
    }

    fn resized(&mut self) {
        let size = self.size();
        if let Some(engine) = &mut self.engine {
            engine.resize(size);
            self.request_pump();
        }
    }

    pub fn input(&mut self, input: Input) {
        if let Some(engine) = &mut self.engine {
            engine.input(input);
        }
    }

    fn scale(&self) -> f32 {
        self.window().window().scale_factor()
    }

    fn pointer(&mut self, kind: i32, button: i32, x: f32, y: f32) {
        let scale = self.scale();
        let (x, y) = (x * scale, y * scale);
        let button = match button {
            1 => Button::Middle,
            2 => Button::Right,
            _ => Button::Left,
        };
        self.input(match kind {
            0 => Input::Down { button, x, y },
            1 => Input::Up { button, x, y },
            _ => Input::Move { x, y },
        });
    }

    pub fn scroll(&mut self, dx: f32, dy: f32, x: f32, y: f32) {
        let scale = self.scale();
        self.input(Input::Wheel {
            dx: dx * scale,
            dy: dy * scale,
            x: x * scale,
            y: y * scale,
        });
    }

    fn theme_changed(&mut self) {
        let dark = self.window().get_dark();
        if let Some(engine) = &mut self.engine {
            engine.set_dark(dark);
        }
    }
}

/// Forwards the window's tab and page callbacks.
fn connect(window: &MainWindow, pages: &Rc<RefCell<Pages>>) {
    let weak = Rc::downgrade(pages);
    let with = move |f: &dyn Fn(&mut Pages)| {
        if let Some(pages) = weak.upgrade() {
            f(&mut pages.borrow_mut());
        }
    };
    let w = with.clone();
    window.on_tab_selected(move |index| w(&|p| p.select(index)));
    let w = with.clone();
    window.on_tab_closed(move |index| w(&|p| p.close(index as usize)));
    let w = with.clone();
    window.on_page_pointer(move |kind, button, x, y, _mods| w(&|p| p.pointer(kind, button, x, y)));
    let w = with.clone();
    window.on_page_scroll(move |dx, dy, x, y| w(&|p| p.scroll(dx, dy, x, y)));
    let w = with.clone();
    window.on_page_key(move |text, mods, down| {
        w(&|p| {
            p.input(Input::Key {
                text: text.to_string(),
                mods: mods as u8,
                down,
            })
        })
    });
    let w = with.clone();
    window.on_page_resized(move || w(&|p| p.resized()));
    let w = with.clone();
    window.on_page_theme_changed(move || w(&|p| p.theme_changed()));
    window.on_page_pump(move || with(&|p| p.pump()));
}

/// Time before `--measure-tabs` starts.
const MEASURE_DELAY: Duration = Duration::from_secs(2);
/// The longest wait for pages to load.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// Time after loading or closing before memory is measured, for Servo's
/// and the allocator's work to settle.
const SETTLE: Duration = Duration::from_secs(3);
/// How long `--measure-tabs` scrolls the page, and by how much per step
/// (logical pixels at about 60 steps per second).
const SCROLL_TIME: Duration = Duration::from_secs(3);
const SCROLL_STEP: f32 = 40.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Start,
    OneTab,
    Scroll,
    ThreeTabs,
    Closed,
    Reopened,
    ClosedAgain,
}

fn log_memory(when: &str, pages: &Pages) {
    super::log_rss(when);
    if let Some(pid) = pages.helper_pid() {
        let kib = |field| crate::procstat::proc_status_kib_of(pid, field).unwrap_or(0);
        eprintln!(
            "ren: helper rss {when}: {} KiB (anon {}, file {}, shmem {}; peak {} KiB)",
            kib("VmRSS:"),
            kib("RssAnon:"),
            kib("RssFile:"),
            kib("RssShmem:"),
            kib("VmHWM:")
        );
    }
}

/// `--measure-tabs`: opens one page and then three, scrolls the first,
/// closes them all, opens one again if Servo can be restarted, and logs
/// memory use after each step; then quits.
pub fn start_measurement(pages: std::rc::Weak<RefCell<Pages>>, urls: Vec<String>) -> slint::Timer {
    let timer = slint::Timer::default();
    let url = move |i: usize| urls[i % urls.len()].clone();
    let mut step = Step::Start;
    // When the current step's waiting ends, and whether it waits for
    // loading first.
    let mut wait_until = Instant::now() + MEASURE_DELAY;
    let mut wait_for_load = false;
    let mut scroll_started = (Instant::now(), 0);
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(16),
        move || {
            let Some(pages) = pages.upgrade() else {
                return;
            };
            let mut pages = pages.borrow_mut();
            let now = Instant::now();
            if step == Step::Scroll {
                if now < scroll_started.0 + SCROLL_TIME {
                    pages.scroll(0.0, -SCROLL_STEP, 100.0, 100.0);
                    return;
                }
                let frames = pages.frame_count() - scroll_started.1;
                eprintln!(
                    "ren: scrolling: {frames} frames in {:.1} s",
                    scroll_started.0.elapsed().as_secs_f64()
                );
            } else {
                if wait_for_load && pages.loading() && now < wait_until + LOAD_TIMEOUT {
                    return;
                }
                if wait_for_load {
                    if pages.loading() {
                        eprintln!("ren: pages still loading after {LOAD_TIMEOUT:?}");
                    }
                    wait_for_load = false;
                    wait_until = now + SETTLE;
                }
                if now < wait_until {
                    return;
                }
            }
            let open = |pages: &mut Pages, i| {
                if let Err(err) = pages.open(&url(i)) {
                    eprintln!("ren: opening a tab failed: {err}");
                }
            };
            step = match step {
                Step::Start => {
                    log_memory("before tabs", &pages);
                    open(&mut pages, 0);
                    Step::OneTab
                }
                Step::OneTab => {
                    log_memory("with 1 tab", &pages);
                    scroll_started = (now, pages.frame_count());
                    Step::Scroll
                }
                Step::Scroll => {
                    open(&mut pages, 1);
                    open(&mut pages, 2);
                    Step::ThreeTabs
                }
                Step::ThreeTabs => {
                    log_memory("with 3 tabs", &pages);
                    pages.close_all();
                    Step::Closed
                }
                Step::Closed => {
                    log_memory("after closing all tabs", &pages);
                    if !pages.can_restart() {
                        let _ = slint::quit_event_loop();
                        return;
                    }
                    open(&mut pages, 0);
                    Step::Reopened
                }
                Step::Reopened => {
                    log_memory("with 1 tab again", &pages);
                    pages.close_all();
                    Step::ClosedAgain
                }
                Step::ClosedAgain => {
                    log_memory("after closing again", &pages);
                    let _ = slint::quit_event_loop();
                    return;
                }
            };
            wait_for_load = matches!(step, Step::OneTab | Step::ThreeTabs | Step::Reopened);
            wait_until = now + SETTLE;
        },
    );
    timer
}
