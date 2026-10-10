// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! ren's Servo helper: renders the web pages of ren's tabs. ren starts it
//! with the first tab and ends it with the last, so all of Servo's memory
//! goes back to the system; see `ren_tabs` for how the two talk and
//! `docs/decisions/0011-tabs.md` for why it is a program of its own.
//!
//! Usage: `ren-servo --parent <PID>`, with a Unix socket to ren as
//! standard input.

mod browser;
mod headless;
mod input;

use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use ren_tabs::frames::FrameWriter;
use ren_tabs::stats::StageStats;
use ren_tabs::wire::{self, Receiver};
use ren_tabs::{Command, Event, Input, TabId};
use rustix::process::{Pid, Signal};

use browser::{Browser, Waker};
use headless::{HeadlessContext, Readback};
use input::{SmoothScroll, Wheel};

/// How long Servo may take to shut down before the helper ends itself:
/// Servo's shutdown can hang once ren is gone.
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);
/// How often a readback is checked while the GPU works on it.
const READBACK_POLL: Duration = Duration::from_millis(1);
/// Blocks of at least this size get their own mapping from glibc, as in
/// ren (`docs/decisions/0010-articles.md`).
const MMAP_THRESHOLD: std::ffi::c_int = 1024 * 1024;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let parent = match (args.next().as_deref(), args.next()) {
        (Some("--parent"), Some(pid)) => pid.parse::<i32>().ok().and_then(Pid::from_raw),
        _ => None,
    };
    let Some(parent) = parent else {
        eprintln!("Usage: ren-servo --parent <PID>, started by ren");
        return ExitCode::FAILURE;
    };
    if let Err(err) = die_with(parent) {
        eprintln!("ren-servo: {err}");
        return ExitCode::FAILURE;
    }
    // With glibc's dynamic threshold for comparison (`just measure-tabs`).
    if std::env::var_os("REN_SERVO_DYNAMIC_MMAP_THRESHOLD").is_none() {
        fix_mmap_threshold();
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ren-servo: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Ends the helper when ren ends, even if ren is killed and can't tell it.
fn die_with(parent: Pid) -> Result<(), String> {
    rustix::process::set_parent_process_death_signal(Some(Signal::KILL))
        .map_err(|err| format!("no parent death signal: {err}"))?;
    // ren may have ended before the signal was set.
    if rustix::process::getppid() != Some(parent) {
        return Err("ren has ended".to_owned());
    }
    Ok(())
}

fn fix_mmap_threshold() {
    #[cfg(target_env = "gnu")]
    {
        const M_MMAP_THRESHOLD: std::ffi::c_int = -3;
        unsafe extern "C" {
            fn mallopt(param: std::ffi::c_int, value: std::ffi::c_int) -> std::ffi::c_int;
        }
        // SAFETY: mallopt only changes a parameter of glibc's allocator;
        // no other threads exist yet.
        unsafe {
            mallopt(M_MMAP_THRESHOLD, MMAP_THRESHOLD);
        }
    }
}

enum Message {
    Command(Command, Option<OwnedFd>),
    Wake,
    /// ren closed the socket, or sent something unreadable.
    Quit,
}

/// Reads ren's commands until it closes the socket.
fn read_commands(socket: UnixStream, sender: mpsc::Sender<Message>) {
    let mut receiver = Receiver::new(socket, true);
    loop {
        let command = match receiver.recv::<Command>() {
            Ok(Some(command)) => command,
            Ok(None) => break,
            Err(err) => {
                eprintln!("ren-servo: reading from ren failed: {err}");
                break;
            }
        };
        let fd = match command {
            Command::Buffer { .. } => receiver.take_fd(),
            _ => None,
        };
        if sender.send(Message::Command(command, fd)).is_err() {
            return;
        }
    }
    let _ = sender.send(Message::Quit);
}

/// The frame path: Servo paints the active tab, the frame is read back
/// from the GPU and written into ren's buffer. Only one frame is under
/// way at a time, and the next one is painted only when ren took the
/// last: frames ren couldn't show aren't read back at all.
struct Frames {
    context: Rc<HeadlessContext>,
    buffer: Option<FrameWriter>,
    /// The frame being read back, and its tab.
    readback: Option<(Readback, TabId)>,
    /// ren took the last frame.
    taken: bool,
    /// The active tab must be painted again, e.g. into a new buffer.
    repaint: bool,
}

impl Frames {
    /// Paints a new frame and starts reading it back, if one is due.
    fn paint(&mut self, browser: &mut Browser, stats: &mut StageStats) {
        if self.readback.is_some() || !self.taken || self.buffer.is_none() {
            return;
        }
        if self.repaint {
            self.repaint = false;
            browser.repaint();
        }
        let started = Instant::now();
        if let Some(tab) = browser.active()
            && browser.paint()
        {
            stats.time("paint", started.elapsed());
            self.readback = Some((self.context.start_readback(), tab));
        }
    }

    /// Writes a finished readback into the buffer and tells ren; returns
    /// whether one is still under way.
    fn finish(&mut self, out: &UnixStream, stats: &mut StageStats) -> io::Result<bool> {
        let Some((readback, tab)) = self.readback.take_if(|(r, _)| self.context.is_ready(r)) else {
            return Ok(self.readback.is_some());
        };
        let (width, height) = readback.size();
        let started = readback.started();
        let target = self
            .buffer
            .as_mut()
            .and_then(|buffer| Some((buffer.id(), buffer.frame_mut(width, height)?)));
        let Some((buffer, pixels)) = target else {
            // The buffer is too small for this frame; ren sends a new one
            // with the new size.
            self.context.finish_readback(readback, None);
            self.repaint = true;
            return Ok(false);
        };
        self.context.finish_readback(readback, Some(pixels));
        let took = started.elapsed();
        stats.time("readback", took);
        stats.count("frames");
        let event = Event::Frame {
            tab,
            buffer,
            width,
            height,
            readback_us: u32::try_from(took.as_micros()).unwrap_or(u32::MAX),
        };
        wire::send(out, &event, None)?;
        self.taken = false;
        Ok(false)
    }
}

/// Runs Servo until ren closes the socket.
fn run() -> Result<(), String> {
    let socket = io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map(UnixStream::from)
        .map_err(|err| format!("no socket to ren: {err}"))?;
    let out = socket.try_clone().map_err(|err| err.to_string())?;
    let (sender, receiver) = mpsc::channel();
    let commands = sender.clone();
    std::thread::Builder::new()
        .name("ren-servo-commands".to_owned())
        .spawn(move || read_commands(socket, commands))
        .map_err(|err| err.to_string())?;

    let Ok(Message::Command(
        Command::Start {
            size,
            dark,
            profile,
        },
        _,
    )) = receiver.recv()
    else {
        return Err("ren must start with a Start command".to_owned());
    };
    let context = Rc::new(
        HeadlessContext::new(dpi::PhysicalSize::new(
            size.width.max(1),
            size.height.max(1),
        ))
        .map_err(|err| format!("no OpenGL context for Servo: {err:?}"))?,
    );
    let waker: Waker = Arc::new(move || {
        let _ = sender.send(Message::Wake);
    });
    let mut browser = Browser::new(waker, context.clone(), size, dark, profile);
    let mut frames = Frames {
        context,
        buffer: None,
        readback: None,
        taken: true,
        repaint: false,
    };
    let mut scroll = SmoothScroll::default();
    let mut stats = StageStats::new("helper");

    'run: loop {
        // Wait for ren or Servo, or until the readback or the next part of
        // a wheel step is due.
        let idle = Instant::now();
        let wait = [
            frames.readback.as_ref().map(|_| idle + READBACK_POLL),
            scroll.next_tick(),
        ]
        .into_iter()
        .flatten()
        .min();
        let first = match wait {
            Some(until) => match receiver.recv_timeout(until.saturating_duration_since(idle)) {
                Ok(message) => Some(message),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match receiver.recv() {
                Ok(message) => Some(message),
                Err(_) => break,
            },
        };
        stats.time("idle", idle.elapsed());

        // Everything that is queued before spinning Servo once.
        let mut next = first;
        while let Some(message) = next {
            match message {
                Message::Quit => break 'run,
                Message::Wake => {}
                Message::Command(command, fd) => handle(
                    command,
                    fd,
                    &mut browser,
                    &mut frames,
                    &mut scroll,
                    &mut stats,
                ),
            }
            next = receiver.try_recv().ok();
        }
        let now = Instant::now();
        if let Some(Wheel { dx, dy, x, y }) = scroll.tick(now) {
            browser.input(Input::Wheel { dx, dy, x, y });
        }

        let started = Instant::now();
        let events = browser.spin();
        stats.time("spin", started.elapsed());
        for event in events {
            wire::send(&out, &event, None).map_err(|err| err.to_string())?;
        }
        frames
            .finish(&out, &mut stats)
            .map_err(|err| err.to_string())?;
        frames.paint(&mut browser, &mut stats);
        stats.report();
    }
    // Shut Servo down, so it saves cookies and site data; but not for
    // longer than EXIT_TIMEOUT.
    std::thread::spawn(|| {
        std::thread::sleep(EXIT_TIMEOUT);
        eprintln!("ren-servo: Servo didn't shut down in time");
        let _ = rustix::process::kill_process(rustix::process::getpid(), Signal::KILL);
    });
    drop(frames);
    drop(browser);
    Ok(())
}

fn handle(
    command: Command,
    fd: Option<OwnedFd>,
    browser: &mut Browser,
    frames: &mut Frames,
    scroll: &mut SmoothScroll,
    stats: &mut StageStats,
) {
    match command {
        Command::Start { .. } => {}
        Command::Buffer { id, len } => {
            frames.buffer = None;
            match fd.map(|fd| FrameWriter::open(id, fd, len)) {
                Some(Ok(buffer)) => {
                    frames.buffer = Some(buffer);
                    frames.repaint = true;
                }
                Some(Err(err)) => eprintln!("ren-servo: the frame buffer: {err}"),
                None => eprintln!("ren-servo: a frame buffer without a file descriptor"),
            }
        }
        Command::Open { tab, url } => browser.open(tab, &url),
        Command::Load { tab, url } => browser.load(tab, &url),
        Command::Close { tab } => browser.close(tab),
        Command::Activate { tab } => {
            scroll.clear();
            browser.activate(tab);
        }
        Command::Back { tab } => browser.back(tab),
        Command::Forward { tab } => browser.forward(tab),
        Command::Reload { tab } => browser.reload(tab),
        Command::Resize(size) => browser.resize(size),
        Command::Input(Input::Wheel { dx, dy, x, y }) => {
            stats.count("wheel");
            if let Some(Wheel { dx, dy, x, y }) = scroll.add(Wheel { dx, dy, x, y }, Instant::now())
            {
                browser.input(Input::Wheel { dx, dy, x, y });
            }
        }
        Command::Input(input) => browser.input(input),
        Command::Dark(dark) => browser.set_dark(dark),
        Command::FrameTaken => frames.taken = true,
    }
}
