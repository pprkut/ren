// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Servo in a helper process: the same binary, started with
//! [`HELPER_ARG`] when the first tab opens and ended when the last one
//! closes, so all of Servo's memory goes back to the system.
//!
//! The UI sends one JSON [`Command`] per line on the helper's stdin; when
//! stdin closes, the helper exits. The helper answers on a Unix socket
//! (not stdout, which Servo or its libraries might print to) whose path is
//! its second argument, with messages of a tag byte followed by:
//! `E`: a little-endian u32 length and a JSON [`Event`]; `F`: little-endian
//! u32 width, height and the microseconds the readback took, then the
//! frame as RGBA, top row first.

use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command as Process, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use super::browser::Browser;
use super::headless::HeadlessContext;
use super::{Engine, Event, FrameStats, Input, Size, TabId, Waker};

/// The argument that makes ren run as the helper.
pub const HELPER_ARG: &str = "--servo-helper";

/// How long the helper may take to shut Servo down before it's killed.
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the helper may take to connect after it was started.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum Command {
    /// The first command, once.
    Start {
        size: Size,
        dark: bool,
    },
    Open {
        tab: TabId,
        url: String,
    },
    Close {
        tab: TabId,
    },
    Activate {
        tab: TabId,
    },
    Resize(Size),
    Input(Input),
    Dark(bool),
}

/// What the reader thread received and the UI thread hasn't taken yet.
#[derive(Default)]
struct Inbox {
    events: Vec<Event>,
    /// Only the newest frame matters; older ones are dropped.
    frame: Option<SharedPixelBuffer<Rgba8Pixel>>,
    /// The time the helper took to read the newest frame back from the
    /// GPU plus the time it took to receive it.
    frame_time: Option<Duration>,
}

/// The UI side: an [`Engine`] that forwards to the helper process.
pub struct Helper {
    child: Child,
    stdin: Option<BufWriter<ChildStdin>>,
    inbox: Arc<Mutex<Inbox>>,
    next_tab: TabId,
    frame: Option<Image>,
    stats: FrameStats,
}

impl Helper {
    pub fn start(waker: Waker, size: Size, dark: bool) -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|err| format!("no path to ren: {err}"))?;
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)
            .map_err(|err| format!("no socket for the Servo helper: {err}"))?;
        let mut child = Process::new(exe)
            .arg(HELPER_ARG)
            .arg(&path)
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|err| format!("starting the Servo helper failed: {err}"))?;
        let stream = accept(&listener, &mut child);
        let _ = std::fs::remove_file(&path);
        let stream = match stream {
            Ok(stream) => stream,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the Servo helper didn't connect: {err}"));
            }
        };
        let stdin = child.stdin.take().expect("piped");
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let reader_inbox = inbox.clone();
        std::thread::Builder::new()
            .name("servo-helper-reader".to_owned())
            .spawn(move || {
                let result = read_messages(BufReader::new(stream), &reader_inbox, &waker);
                let reason = match result {
                    Ok(()) => "the Servo helper exited".to_owned(),
                    Err(err) => format!("reading from the Servo helper failed: {err}"),
                };
                lock(&reader_inbox).events.push(Event::Failed(reason));
                waker();
            })
            .map_err(|err| err.to_string())?;
        let mut helper = Self {
            child,
            stdin: Some(BufWriter::new(stdin)),
            inbox,
            next_tab: 0,
            frame: None,
            stats: FrameStats::default(),
        };
        helper.send(&Command::Start { size, dark });
        Ok(helper)
    }

    fn send(&mut self, command: &Command) {
        let Some(stdin) = &mut self.stdin else {
            return;
        };
        let line = serde_json::to_string(command).expect("commands serialise");
        if writeln!(stdin, "{line}")
            .and_then(|()| stdin.flush())
            .is_err()
        {
            // The reader thread reports the helper's exit.
            self.stdin = None;
        }
    }
}

/// A path for the socket, in the user's runtime directory if there is one.
fn socket_path() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    dir.join(format!("ren-servo-{}-{n}.sock", std::process::id()))
}

/// Waits for the helper to connect, unless it exits first.
fn accept(listener: &UnixListener, child: &mut Child) -> io::Result<UnixStream> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                return Ok(stream);
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            Err(err) => return Err(err),
        }
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!("it exited with {status}")));
        }
        if Instant::now() > deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn lock(inbox: &Mutex<Inbox>) -> std::sync::MutexGuard<'_, Inbox> {
    inbox.lock().unwrap_or_else(|e| e.into_inner())
}

fn read_u32(input: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

/// Reads the helper's messages until it exits.
fn read_messages(mut input: impl Read, inbox: &Mutex<Inbox>, waker: &Waker) -> io::Result<()> {
    loop {
        let mut tag = [0];
        if input.read(&mut tag)? == 0 {
            return Ok(());
        }
        match tag[0] {
            b'E' => {
                let len = read_u32(&mut input)? as usize;
                let mut json = vec![0; len];
                input.read_exact(&mut json)?;
                let event = serde_json::from_slice(&json)
                    .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
                lock(inbox).events.push(event);
            }
            b'F' => {
                let width = read_u32(&mut input)?;
                let height = read_u32(&mut input)?;
                let readback = Duration::from_micros(read_u32(&mut input)?.into());
                let started = Instant::now();
                let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
                input.read_exact(buffer.make_mut_bytes())?;
                let mut inbox = lock(inbox);
                inbox.frame = Some(buffer);
                inbox.frame_time = Some(readback + started.elapsed());
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown message {other}"),
                ));
            }
        }
        waker();
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        // Closing stdin tells the helper to shut Servo down and exit.
        self.stdin = None;
        let deadline = Instant::now() + EXIT_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        eprintln!("ren: the Servo helper didn't exit, killing it");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Engine for Helper {
    fn open(&mut self, url: &str) -> TabId {
        let tab = self.next_tab;
        self.next_tab += 1;
        self.send(&Command::Open {
            tab,
            url: url.to_owned(),
        });
        tab
    }

    fn close(&mut self, tab: TabId) {
        self.send(&Command::Close { tab });
    }

    fn activate(&mut self, tab: TabId) {
        self.send(&Command::Activate { tab });
    }

    fn resize(&mut self, size: Size) {
        self.send(&Command::Resize(size));
    }

    fn input(&mut self, input: Input) {
        self.send(&Command::Input(input));
    }

    fn set_dark(&mut self, dark: bool) {
        self.send(&Command::Dark(dark));
    }

    fn pump(&mut self) -> Vec<Event> {
        let mut inbox = lock(&self.inbox);
        if let Some(buffer) = inbox.frame.take() {
            self.frame = Some(Image::from_rgba8_premultiplied(buffer));
            if let Some(took) = inbox.frame_time.take() {
                self.stats.add(took);
            }
        }
        std::mem::take(&mut inbox.events)
    }

    fn take_frame(&mut self) -> Option<Image> {
        self.frame.take()
    }

    fn helper_pid(&self) -> Option<u32> {
        Some(self.child.id())
    }

    fn frame_stats(&self) -> Option<(usize, Duration, Duration)> {
        self.stats.summary()
    }
}

enum Message {
    Command(Command),
    Wake,
    /// The UI closed stdin, or sent something unreadable.
    Quit,
}

/// The helper's main function: runs Servo until stdin is closed. `socket`
/// is where the UI waits for it.
pub fn run(socket: &std::path::Path) -> Result<(), String> {
    let stream = UnixStream::connect(socket)
        .map_err(|err| format!("connecting to {} failed: {err}", socket.display()))?;
    let (sender, receiver) = mpsc::channel();
    let commands = sender.clone();
    std::thread::Builder::new()
        .name("servo-helper-stdin".to_owned())
        .spawn(move || {
            for line in io::stdin().lock().lines() {
                let command = line.ok().and_then(|line| serde_json::from_str(&line).ok());
                let Some(command) = command else {
                    break;
                };
                if commands.send(Message::Command(command)).is_err() {
                    return;
                }
            }
            let _ = commands.send(Message::Quit);
        })
        .map_err(|err| err.to_string())?;

    let Ok(Message::Command(Command::Start { size, dark })) = receiver.recv() else {
        return Err("the helper must be started with a Start command".to_owned());
    };
    let context = Rc::new(
        HeadlessContext::new(dpi::PhysicalSize::new(size.width, size.height))
            .map_err(|err| format!("no OpenGL context for Servo: {err:?}"))?,
    );
    let waker: Waker = Arc::new(move || {
        let _ = sender.send(Message::Wake);
    });
    let mut browser = Browser::new(waker, context.clone(), size, dark);
    let mut out = BufWriter::new(stream);
    let mut pixels = Vec::new();

    'run: loop {
        // Handle everything that is queued before spinning Servo once.
        let mut message = receiver.recv().map_err(|err| err.to_string())?;
        loop {
            match message {
                Message::Quit => break 'run,
                Message::Wake => {}
                Message::Command(command) => match command {
                    Command::Start { .. } => {}
                    Command::Open { tab, url } => browser.open(tab, &url),
                    Command::Close { tab } => browser.close(tab),
                    Command::Activate { tab } => browser.activate(tab),
                    Command::Resize(size) => browser.resize(size),
                    Command::Input(input) => browser.input(input),
                    Command::Dark(dark) => browser.set_dark(dark),
                },
            }
            match receiver.try_recv() {
                Ok(next) => message = next,
                Err(_) => break,
            }
        }

        for event in browser.spin() {
            let json = serde_json::to_vec(&event).expect("events serialise");
            out.write_all(b"E")
                .and_then(|()| out.write_all(&(json.len() as u32).to_le_bytes()))
                .and_then(|()| out.write_all(&json))
                .map_err(|err| err.to_string())?;
        }
        if browser.paint() {
            let started = Instant::now();
            let size = servo::RenderingContext::size(&*context);
            pixels.resize(size.width as usize * size.height as usize * 4, 0);
            context.read_into(&mut pixels);
            let readback = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);
            out.write_all(b"F")
                .and_then(|()| out.write_all(&size.width.to_le_bytes()))
                .and_then(|()| out.write_all(&size.height.to_le_bytes()))
                .and_then(|()| out.write_all(&readback.to_le_bytes()))
                .and_then(|()| out.write_all(&pixels))
                .map_err(|err| err.to_string())?;
        }
        out.flush().map_err(|err| err.to_string())?;
    }
    // Shut Servo down before exiting, so it can clean up its threads.
    drop(browser);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_round_trip() {
        let commands = [
            Command::Start {
                size: Size {
                    width: 800,
                    height: 600,
                    scale: 1.5,
                },
                dark: true,
            },
            Command::Open {
                tab: 3,
                url: "https://example.org/".to_owned(),
            },
            Command::Input(Input::Key {
                text: "a".to_owned(),
                mods: 1,
                down: true,
            }),
        ];
        for command in commands {
            let line = serde_json::to_string(&command).unwrap();
            assert!(!line.contains('\n'));
            assert_eq!(serde_json::from_str::<Command>(&line).unwrap(), command);
        }
    }

    #[test]
    fn reads_messages() {
        let event = serde_json::to_vec(&Event::Title {
            tab: 1,
            title: "T".to_owned(),
        })
        .unwrap();
        let mut stream = Vec::new();
        stream.push(b'E');
        stream.extend_from_slice(&(event.len() as u32).to_le_bytes());
        stream.extend_from_slice(&event);
        stream.push(b'F');
        stream.extend_from_slice(&2u32.to_le_bytes());
        stream.extend_from_slice(&1u32.to_le_bytes());
        stream.extend_from_slice(&1500u32.to_le_bytes());
        stream.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);

        let inbox = Mutex::new(Inbox::default());
        let wakes = Arc::new(Mutex::new(0));
        let counter = wakes.clone();
        let waker: Waker = Arc::new(move || *counter.lock().unwrap() += 1);
        read_messages(stream.as_slice(), &inbox, &waker).unwrap();

        let inbox = inbox.into_inner().unwrap();
        assert_eq!(
            inbox.events,
            [Event::Title {
                tab: 1,
                title: "T".to_owned()
            }]
        );
        let frame = inbox.frame.unwrap();
        assert_eq!((frame.width(), frame.height()), (2, 1));
        assert_eq!(frame.as_bytes(), [1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(inbox.frame_time.unwrap() >= Duration::from_micros(1500));
        assert_eq!(*wakes.lock().unwrap(), 2);

        assert!(read_messages(b"X".as_slice(), &Mutex::default(), &waker).is_err());
    }
}
