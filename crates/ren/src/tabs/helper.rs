// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Servo helper process (`ren-servo`): started with the first tab,
//! ended with the last, so all of Servo's memory goes back to the system.
//! See `ren_tabs` for what the two say to each other.

use std::io;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ren_tabs::frames::{SharedFrames, frame_len};
use ren_tabs::wire::{self, Receiver};
use ren_tabs::{Command, Event, Input, Size, TabId};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use super::{FrameStats, Waker};

/// The helper's file name, next to ren's.
pub const HELPER_NAME: &str = "ren-servo";
/// Overrides where the helper is.
const HELPER_VAR: &str = "REN_SERVO";

/// How long the helper may take to shut Servo down (it saves cookies and
/// site data then) before it's killed. It ends itself after 3 s.
const EXIT_TIMEOUT: Duration = Duration::from_secs(4);

/// Frame buffers are made for sizes rounded up to this, so resizing the
/// window doesn't make a new one for every pixel.
const BUFFER_STEP: u32 = 128;

/// What the reader thread received and the UI thread hasn't taken yet.
#[derive(Default)]
struct Inbox {
    events: Vec<Event>,
    /// Why the helper is gone.
    failed: Option<String>,
}

/// The helper process, seen from the window.
pub struct Helper {
    child: Option<Child>,
    socket: UnixStream,
    inbox: Arc<Mutex<Inbox>>,
    next_tab: TabId,
    active: Option<TabId>,
    /// The frame buffer, and the one before it, which frames may still
    /// come in until the helper has the new one.
    buffer: SharedFrames,
    old_buffer: Option<SharedFrames>,
    /// Two pixel buffers: Slint still holds the one shown, the other can
    /// be copied into without a copy of its own.
    pixels: [Option<SharedPixelBuffer<Rgba8Pixel>>; 2],
    next_pixels: usize,
    frame: Option<Image>,
    stats: FrameStats,
}

/// Where the helper is: `$REN_SERVO`, else next to ren, else in
/// `../libexec/ren/` from ren's directory.
fn helper_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(HELPER_VAR) {
        return Ok(path.into());
    }
    let exe = std::env::current_exe().map_err(|err| format!("no path to ren: {err}"))?;
    let dir = exe.parent().unwrap_or(Path::new("/"));
    let candidates = [
        dir.join(HELPER_NAME),
        dir.join("../libexec/ren").join(HELPER_NAME),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "the Servo helper ({HELPER_NAME}) isn't installed next to ren (just build-servo)"
            )
        })
}

/// The frame buffer's length for `size`.
fn buffer_len(size: Size) -> usize {
    let round = |n: u32| n.max(1).div_ceil(BUFFER_STEP) * BUFFER_STEP;
    frame_len(round(size.width), round(size.height)).unwrap_or(usize::MAX)
}

/// Reaps helpers that are shutting down, so ren can wait for them when it
/// ends.
static REAPERS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// Waits until the helpers that were told to end have ended (at most
/// `EXIT_TIMEOUT` each), so they can save their site data before ren
/// exits and its exit kills them.
pub fn wait_for_helpers() {
    let reapers = std::mem::take(&mut *REAPERS.lock().unwrap_or_else(|e| e.into_inner()));
    for reaper in reapers {
        let _ = reaper.join();
    }
}

fn lock(inbox: &Mutex<Inbox>) -> std::sync::MutexGuard<'_, Inbox> {
    inbox.lock().unwrap_or_else(|e| e.into_inner())
}

impl Helper {
    /// Starts the helper. With `profile`, Servo keeps cookies and site
    /// data there.
    pub fn start(
        waker: Waker,
        size: Size,
        dark: bool,
        profile: Option<PathBuf>,
    ) -> Result<Self, String> {
        let path = helper_path()?;
        let (socket, theirs) =
            UnixStream::pair().map_err(|err| format!("no socket for the Servo helper: {err}"))?;
        let child = Process::new(&path)
            .arg("--parent")
            .arg(std::process::id().to_string())
            .stdin(Stdio::from(std::os::fd::OwnedFd::from(theirs)))
            .spawn()
            .map_err(|err| format!("starting {} failed: {err}", path.display()))?;
        let reader = socket.try_clone().map_err(|err| err.to_string())?;
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let reader_inbox = inbox.clone();
        std::thread::Builder::new()
            .name("ren-servo-events".to_owned())
            .spawn(move || {
                let failed = read_events(reader, &reader_inbox, &waker);
                lock(&reader_inbox).failed = Some(failed);
                waker();
            })
            .map_err(|err| err.to_string())?;
        let buffer = SharedFrames::create(0, buffer_len(size))
            .map_err(|err| format!("no frame buffer: {err}"))?;
        let mut helper = Self {
            child: Some(child),
            socket,
            inbox,
            next_tab: 0,
            active: None,
            buffer,
            old_buffer: None,
            pixels: [None, None],
            next_pixels: 0,
            frame: None,
            stats: FrameStats::default(),
        };
        helper.send(&Command::Start {
            size,
            dark,
            profile,
        });
        helper.send_buffer();
        Ok(helper)
    }

    fn send(&mut self, command: &Command) {
        // A failure shows as the end of the reader thread.
        let _ = wire::send(&self.socket, command, None);
    }

    fn send_buffer(&mut self) {
        let command = Command::Buffer {
            id: self.buffer.id(),
            len: self.buffer.len() as u64,
        };
        if let Some(fd) = self.buffer.fd() {
            let _ = wire::send(&self.socket, &command, Some(fd));
        }
        self.buffer.sent();
    }

    pub fn open(&mut self, url: &str) -> TabId {
        let tab = self.next_tab;
        self.next_tab += 1;
        self.send(&Command::Open {
            tab,
            url: url.to_owned(),
        });
        tab
    }

    pub fn load(&mut self, tab: TabId, url: &str) {
        self.send(&Command::Load {
            tab,
            url: url.to_owned(),
        });
    }

    pub fn close(&mut self, tab: TabId) {
        self.send(&Command::Close { tab });
    }

    pub fn activate(&mut self, tab: Option<TabId>) {
        self.active = tab;
        self.send(&Command::Activate { tab });
    }

    pub fn back(&mut self, tab: TabId) {
        self.send(&Command::Back { tab });
    }

    pub fn forward(&mut self, tab: TabId) {
        self.send(&Command::Forward { tab });
    }

    pub fn reload(&mut self, tab: TabId) {
        self.send(&Command::Reload { tab });
    }

    pub fn resize(&mut self, size: Size) {
        let len = buffer_len(size);
        if len > self.buffer.len() || len < self.buffer.len() / 4 {
            match SharedFrames::create(self.buffer.id().wrapping_add(1), len) {
                Ok(buffer) => {
                    self.old_buffer = Some(std::mem::replace(&mut self.buffer, buffer));
                    self.send_buffer();
                }
                Err(err) => eprintln!("ren: no frame buffer: {err}"),
            }
        }
        self.send(&Command::Resize(size));
    }

    pub fn input(&mut self, input: Input) {
        self.send(&Command::Input(input));
    }

    pub fn set_dark(&mut self, dark: bool) {
        self.send(&Command::Dark(dark));
    }

    /// Takes what the helper sent: copies a new frame out of the shared
    /// buffer, and returns the other events; an error if the helper is
    /// gone.
    pub fn pump(&mut self) -> Result<Vec<Event>, String> {
        let (events, failed) = {
            let mut inbox = lock(&self.inbox);
            (std::mem::take(&mut inbox.events), inbox.failed.take())
        };
        if let Some(failed) = failed {
            return Err(failed);
        }
        let mut other = Vec::with_capacity(events.len());
        for event in events {
            match event {
                Event::Frame {
                    tab,
                    buffer,
                    width,
                    height,
                    readback_us,
                } => {
                    let started = Instant::now();
                    if Some(tab) == self.active {
                        self.copy_frame(buffer, width, height);
                    }
                    self.send(&Command::FrameTaken);
                    self.stats
                        .add(Duration::from_micros(readback_us.into()) + started.elapsed());
                }
                event => other.push(event),
            }
        }
        Ok(other)
    }

    /// Copies a frame out of buffer `id`, if it fits.
    fn copy_frame(&mut self, id: u32, width: u32, height: u32) {
        if id == self.buffer.id() {
            // The helper has the new buffer.
            self.old_buffer = None;
        }
        let buffer = if id == self.buffer.id() {
            &self.buffer
        } else {
            match &self.old_buffer {
                Some(old) if old.id() == id => old,
                _ => return,
            }
        };
        let Some(frame) = buffer.frame(width, height) else {
            return;
        };
        let slot = self.next_pixels;
        let mut pixels = match self.pixels[slot].take() {
            Some(p) if p.width() == width && p.height() == height => p,
            _ => SharedPixelBuffer::new(width, height),
        };
        pixels.make_mut_bytes().copy_from_slice(frame);
        self.frame = Some(Image::from_rgba8_premultiplied(pixels.clone()));
        self.pixels[slot] = Some(pixels);
        self.next_pixels = 1 - slot;
    }

    /// The newest frame of the active tab, if it changed since the last
    /// call.
    pub fn take_frame(&mut self) -> Option<Image> {
        self.frame.take()
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Frames so far, and the mean and longest time from a painted frame
    /// to a Slint image.
    pub fn frame_stats(&self) -> Option<(usize, Duration, Duration)> {
        self.stats.summary()
    }
}

/// Reads the helper's events until it closes the socket; returns why it
/// ended.
fn read_events(socket: UnixStream, inbox: &Mutex<Inbox>, waker: &Waker) -> String {
    let mut receiver = Receiver::new(socket, false);
    loop {
        match receiver.recv::<Event>() {
            Ok(Some(event)) => {
                lock(inbox).events.push(event);
                waker();
            }
            Ok(None) => return "the Servo helper exited".to_owned(),
            Err(err) => return format!("reading from the Servo helper failed: {err}"),
        }
    }
}

impl Drop for Helper {
    /// Tells the helper to end, and waits for it on another thread.
    fn drop(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
        let Some(mut child) = self.child.take() else {
            return;
        };
        let reaper = std::thread::Builder::new()
            .name("ren-servo-reaper".to_owned())
            .spawn(move || {
                if let Err(err) = reap(&mut child) {
                    eprintln!("ren: ending the Servo helper: {err}");
                }
            });
        match reaper {
            Ok(reaper) => {
                let mut reapers = REAPERS.lock().unwrap_or_else(|e| e.into_inner());
                reapers.retain(|r| !r.is_finished());
                reapers.push(reaper);
            }
            Err(err) => eprintln!("ren: no thread to end the Servo helper: {err}"),
        }
    }
}

/// Waits for the helper to exit, and kills it after `EXIT_TIMEOUT`.
fn reap(child: &mut Child) -> io::Result<()> {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    eprintln!("ren: the Servo helper didn't exit, killing it");
    child.kill()?;
    child.wait().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_are_rounded_up() {
        let size = |width, height| Size {
            width,
            height,
            scale: 1.0,
        };
        assert_eq!(buffer_len(size(800, 600)), 896 * 640 * 4);
        assert_eq!(buffer_len(size(1024, 768)), 1024 * 768 * 4);
        assert_eq!(buffer_len(size(0, 0)), 128 * 128 * 4);
    }
}
