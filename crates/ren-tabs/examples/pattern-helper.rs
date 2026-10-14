// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! A stand-in for the Servo helper (`ren-servo`) that paints a synthetic
//! page: lines of "text" and coloured blocks that scroll with the wheel.
//! It speaks the same protocol and paces frames the same way, so the
//! window's side of the tabs can be tried and measured without building
//! Servo, and the window's cost per frame compared between renderers
//! without the page's or the GPU's.
//!
//! ```sh
//! cargo build --release -p ren-tabs --example pattern-helper
//! REN_SERVO=target/release/examples/pattern-helper ren --measure-tabs --tab-url https://example.org/
//! ```
//!
//! Environment:
//! - `REN_PATTERN_GPU_MS`: how long the "GPU" takes per frame (default 0),
//!   between painting a frame and it being ready to be written.
//! - `REN_PATTERN_ONE_FRAME`: paint the next frame only after the window
//!   took the last, as the helper did before M10 (and does with
//!   `REN_SERVO_ONE_FRAME`).

use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ren_tabs::frames::FrameWriter;
use ren_tabs::scroll::{SmoothScroll, Wheel};
use ren_tabs::stats::StageStats;
use ren_tabs::wire::{self, Receiver};
use ren_tabs::{Command, Event, Input, Size, TabId};

/// The synthetic page's height in physical pixels at scale 1.
const PAGE_HEIGHT: f32 = 20_000.0;

enum Message {
    Command(Command, Option<OwnedFd>),
    Quit,
}

fn main() -> io::Result<()> {
    let socket = UnixStream::from(io::stdin().as_fd().try_clone_to_owned()?);
    let out = socket.try_clone()?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut receiver = Receiver::new(socket, true);
        while let Ok(Some(command)) = receiver.recv::<Command>() {
            let fd = match command {
                Command::Buffer { .. } => receiver.take_fd(),
                _ => None,
            };
            if sender.send(Message::Command(command, fd)).is_err() {
                return;
            }
        }
        let _ = sender.send(Message::Quit);
    });

    let gpu = Duration::from_millis(
        std::env::var("REN_PATTERN_GPU_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .unwrap_or(0),
    );
    let one_frame = std::env::var_os("REN_PATTERN_ONE_FRAME").is_some();
    let mut helper = Pattern {
        size: Size {
            width: 1,
            height: 1,
            scale: 1.0,
        },
        buffer: None,
        active: None,
        tabs: Vec::new(),
        offset: 0.0,
        dirty: false,
        painted: None,
        taken: true,
        gpu,
        one_frame,
        scroll: SmoothScroll::default(),
        stats: StageStats::new("helper"),
    };
    loop {
        // A frame that is ready waits for the window's answer, not a time.
        let now = Instant::now();
        let wait = [
            helper.painted.map(|(ready, _)| ready).filter(|&r| r > now),
            helper.scroll.next_tick(),
        ]
        .into_iter()
        .flatten()
        .min();
        let message = match wait {
            Some(until) => {
                match receiver.recv_timeout(until.saturating_duration_since(Instant::now())) {
                    Ok(message) => Some(message),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match receiver.recv() {
                Ok(message) => Some(message),
                Err(_) => break,
            },
        };
        let mut next = message;
        while let Some(message) = next {
            match message {
                Message::Quit => return Ok(()),
                Message::Command(command, fd) => helper.handle(command, fd, &out)?,
            }
            next = receiver.try_recv().ok();
        }
        if let Some(step) = helper.scroll.tick(Instant::now()) {
            helper.wheel(step.dy);
        }
        helper.finish(&out)?;
        helper.paint();
        helper.stats.report();
    }
    Ok(())
}

struct Pattern {
    size: Size,
    buffer: Option<FrameWriter>,
    active: Option<TabId>,
    tabs: Vec<TabId>,
    /// Scroll position in physical pixels.
    offset: f32,
    /// The page changed since the last frame was painted.
    dirty: bool,
    /// A painted frame: when the "GPU" is done with it, and its offset.
    painted: Option<(Instant, f32)>,
    /// The window took the last frame.
    taken: bool,
    gpu: Duration,
    one_frame: bool,
    scroll: SmoothScroll,
    stats: StageStats,
}

impl Pattern {
    fn handle(
        &mut self,
        command: Command,
        fd: Option<OwnedFd>,
        out: &UnixStream,
    ) -> io::Result<()> {
        match command {
            Command::Start { size, .. } | Command::Resize(size) => {
                self.size = size;
                self.dirty = true;
            }
            Command::Buffer { id, len } => {
                if let Some(fd) = fd {
                    self.buffer = Some(FrameWriter::open(id, fd, len)?);
                    self.dirty = true;
                }
            }
            Command::Open { tab, url } | Command::Load { tab, url } => {
                if !self.tabs.contains(&tab) {
                    self.tabs.push(tab);
                }
                let events = [
                    Event::Url {
                        tab,
                        url: url.clone(),
                    },
                    Event::Title {
                        tab,
                        title: format!("Pattern: {url}"),
                    },
                    Event::Loading {
                        tab,
                        loading: false,
                    },
                ];
                for event in events {
                    wire::send(out, &event, None)?;
                }
            }
            Command::Close { tab } => self.tabs.retain(|&t| t != tab),
            Command::Activate { tab } => {
                self.active = tab;
                self.scroll.clear();
                self.dirty = true;
            }
            Command::Input(Input::Wheel { dx, dy, x, y }) => {
                self.stats.count("wheel");
                if let Some(now) = self.scroll.add(Wheel { dx, dy, x, y }, Instant::now()) {
                    self.wheel(now.dy);
                }
            }
            Command::Dark(_) => self.dirty = true,
            Command::FrameTaken => self.taken = true,
            _ => {}
        }
        Ok(())
    }

    fn wheel(&mut self, dy: f32) {
        let max = (PAGE_HEIGHT * self.size.scale - self.size.height as f32).max(0.0);
        let offset = (self.offset - dy).clamp(0.0, max);
        if offset != self.offset {
            self.offset = offset;
            self.dirty = true;
        }
    }

    /// "Paints" a frame if the page changed: it is ready after the GPU's
    /// time.
    fn paint(&mut self) {
        if !self.dirty || self.painted.is_some() || self.active.is_none() {
            return;
        }
        if self.one_frame && !self.taken {
            return;
        }
        self.dirty = false;
        self.painted = Some((Instant::now() + self.gpu, self.offset));
    }

    /// Writes a ready frame into the buffer once the window took the last.
    fn finish(&mut self, out: &UnixStream) -> io::Result<()> {
        let Some((ready, offset)) = self.painted else {
            return Ok(());
        };
        if Instant::now() < ready || !self.taken {
            return Ok(());
        }
        self.painted = None;
        let (Some(tab), Some(buffer)) = (self.active, &mut self.buffer) else {
            return Ok(());
        };
        let Size { width, height, .. } = self.size;
        let Some(pixels) = buffer.frame_mut(width, height) else {
            self.dirty = true;
            return Ok(());
        };
        let started = Instant::now();
        draw(pixels, width as usize, offset as usize, self.size.scale);
        self.stats.time("draw", started.elapsed());
        self.stats.count("frames");
        let event = Event::Frame {
            tab,
            buffer: buffer.id(),
            width,
            height,
            readback_us: u32::try_from(self.gpu.as_micros()).unwrap_or(u32::MAX),
        };
        wire::send(out, &event, None)?;
        self.taken = false;
        Ok(())
    }
}

/// Lines of grey blocks ("words") on white, with a coloured block
/// ("image") every few lines; the page is `offset` pixels down.
fn draw(pixels: &mut [u8], width: usize, offset: usize, scale: f32) {
    let line = ((24.0 * scale) as usize).max(4);
    let text = line * 3 / 5;
    let word = ((40.0 * scale) as usize).max(4);
    for (y, row) in pixels.chunks_exact_mut(width * 3).enumerate() {
        let page_y = y + offset;
        let line_no = page_y / line;
        let colour = if line_no % 12 >= 9 {
            // An "image": a colour per block.
            let h = (line_no / 12).wrapping_mul(2_654_435_761) as u32;
            Some([(h >> 16) as u8, (h >> 8) as u8, h as u8])
        } else {
            None
        };
        if let Some(colour) = colour {
            for px in row.chunks_exact_mut(3) {
                px.copy_from_slice(&colour);
            }
            continue;
        }
        row.fill(0xff);
        if page_y % line >= text {
            continue;
        }
        for (x, px) in row.chunks_exact_mut(3).enumerate() {
            let w = x / word;
            if !(w.wrapping_mul(7) + line_no).is_multiple_of(5) && x % word < word - word / 5 {
                px.copy_from_slice(&[0x40, 0x40, 0x40]);
            }
        }
    }
}
