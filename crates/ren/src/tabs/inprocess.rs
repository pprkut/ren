// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Servo in the UI process, with frames read back from the GPU into a
//! Slint image.

use std::rc::Rc;
use std::time::Duration;

use servo::RenderingContext;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use super::browser::Browser;
use super::headless::HeadlessContext;
use super::{Engine, Event, FrameStats, Input, Size, TabId, Waker};

pub struct InProcess {
    browser: Browser,
    context: Rc<HeadlessContext>,
    next_tab: TabId,
    /// Two frame buffers: Slint still holds the one shown, the other can be
    /// read into without a copy.
    buffers: [Option<SharedPixelBuffer<Rgba8Pixel>>; 2],
    next: usize,
    frame: Option<Image>,
    stats: FrameStats,
}

impl InProcess {
    pub fn new(waker: Waker, size: Size, dark: bool) -> Result<Self, String> {
        let context = Rc::new(
            HeadlessContext::new(dpi::PhysicalSize::new(size.width, size.height))
                .map_err(|err| format!("no OpenGL context for Servo: {err:?}"))?,
        );
        Ok(Self {
            browser: Browser::new(waker, context.clone(), size, dark),
            context,
            next_tab: 0,
            buffers: [None, None],
            next: 0,
            frame: None,
            stats: FrameStats::default(),
        })
    }

    fn read_frame(&mut self) {
        let started = std::time::Instant::now();
        let size = self.context.size();
        let slot = self.next;
        let mut buffer = match self.buffers[slot].take() {
            Some(b) if b.width() == size.width && b.height() == size.height => b,
            _ => SharedPixelBuffer::new(size.width, size.height),
        };
        self.context.read_into(buffer.make_mut_bytes());
        self.frame = Some(Image::from_rgba8_premultiplied(buffer.clone()));
        self.buffers[slot] = Some(buffer);
        self.next = 1 - slot;
        self.stats.add(started.elapsed());
    }
}

impl Engine for InProcess {
    fn open(&mut self, url: &str) -> TabId {
        let tab = self.next_tab;
        self.next_tab += 1;
        self.browser.open(tab, url);
        tab
    }

    fn close(&mut self, tab: TabId) {
        self.browser.close(tab);
    }

    fn activate(&mut self, tab: Option<TabId>) {
        self.browser.activate(tab);
    }

    fn resize(&mut self, size: Size) {
        self.browser.resize(size);
    }

    fn input(&mut self, input: Input) {
        self.browser.input(input);
    }

    fn set_dark(&mut self, dark: bool) {
        self.browser.set_dark(dark);
    }

    fn pump(&mut self) -> Vec<Event> {
        let events = self.browser.spin();
        if self.browser.paint() {
            self.read_frame();
        }
        events
    }

    fn take_frame(&mut self) -> Option<Image> {
        self.frame.take()
    }

    fn frame_stats(&self) -> Option<(usize, Duration, Duration)> {
        self.stats.summary()
    }
}
