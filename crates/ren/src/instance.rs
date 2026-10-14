// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! One Ren at a time: the window listens on a socket in the runtime
//! directory, and a second start hands over to it (the running window
//! comes to the front) instead of opening a second window on the same
//! database.
//!
//! A start sends one line, `activate` and the launcher's activation token
//! if it has one (`XDG_ACTIVATION_TOKEN`, `DESKTOP_STARTUP_ID`), and the
//! running Ren answers `ok`. A socket nobody listens on is left from a
//! Ren that ended without removing it, and is replaced.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long either side waits for the other.
const TIMEOUT: Duration = Duration::from_secs(2);

/// The longest request read.
const MAX_REQUEST: u64 = 4096;

/// What a running Ren is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activate {
    /// The launcher's activation token, for the window manager.
    pub token: Option<String>,
}

/// Whether this Ren runs alone.
pub enum Claim {
    /// No other Ren runs: this one listens.
    First(Instance),
    /// Another Ren runs and was asked to come to the front.
    Running,
}

/// The socket of the running Ren, removed when it is dropped.
pub struct Instance {
    listener: Option<UnixListener>,
    path: PathBuf,
    /// The socket file's inode: another Ren's socket at the same path
    /// (after this one's was replaced) isn't removed.
    inode: u64,
}

/// Claims `path` for this Ren, or hands `activate` over to the Ren that
/// has it.
pub fn claim(path: &Path, activate: &Activate) -> io::Result<Claim> {
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    // Twice: a Ren starting at the same moment may bind first.
    for _ in 0..2 {
        match hand_over(path, activate) {
            Ok(()) => return Ok(Claim::Running),
            Err(err) if gone(&err) => {}
            Err(err) => return Err(err),
        }
        match bind(path) {
            Ok(instance) => return Ok(Claim::First(instance)),
            Err(err) if err.kind() == io::ErrorKind::AddrInUse => {}
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::other(
        "another Ren holds the socket but doesn't answer",
    ))
}

/// No Ren listens: no socket, or one left behind.
fn gone(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

/// Asks the Ren listening on `path` to come to the front.
fn hand_over(path: &Path, activate: &Activate) -> io::Result<()> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let line = match &activate.token {
        Some(token) => format!("activate {token}\n"),
        None => "activate\n".to_owned(),
    };
    stream.write_all(line.as_bytes())?;
    let mut answer = String::new();
    BufReader::new(stream.take(16)).read_line(&mut answer)?;
    if answer.trim_end() == "ok" {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "the running Ren answered {answer:?}"
        )))
    }
}

/// Binds the socket, replacing one left behind.
fn bind(path: &Path) -> io::Result<Instance> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let listener = UnixListener::bind(path)?;
    let inode = std::fs::symlink_metadata(path)?.ino();
    Ok(Instance {
        listener: Some(listener),
        path: path.to_owned(),
        inode,
    })
}

/// Reads a start's request; `None` for anything but one.
fn read_request(stream: &UnixStream) -> Option<Activate> {
    let mut line = String::new();
    BufReader::new(stream.take(MAX_REQUEST))
        .read_line(&mut line)
        .ok()?;
    let line = line.strip_suffix('\n')?;
    let mut words = line.split(' ');
    if words.next()? != "activate" {
        return None;
    }
    let token = words.next().filter(|token| !token.is_empty());
    Some(Activate {
        token: token.map(str::to_owned),
    })
}

impl Activate {
    /// The token the launcher gave this process, if any.
    pub fn from_env() -> Self {
        let token = ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|token| !token.is_empty() && !token.contains(char::is_whitespace));
        Self { token }
    }
}

impl Instance {
    /// Answers the starts that hand over on a thread of its own, calling
    /// `activate` for each.
    pub fn serve(&mut self, activate: impl Fn(Activate) + Send + 'static) -> io::Result<()> {
        let Some(listener) = self.listener.take() else {
            return Ok(());
        };
        std::thread::Builder::new()
            .name("ren-instance".to_owned())
            .stack_size(64 * 1024)
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else {
                        continue;
                    };
                    let _ = stream.set_read_timeout(Some(TIMEOUT));
                    let _ = stream.set_write_timeout(Some(TIMEOUT));
                    if let Some(request) = read_request(&stream) {
                        let _ = stream.write_all(b"ok\n");
                        activate(request);
                    }
                }
            })?;
        Ok(())
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let ours = std::fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.inode);
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::testing::TempDir;

    fn token(token: &str) -> Activate {
        Activate {
            token: Some(token.to_owned()),
        }
    }

    #[test]
    fn a_second_start_hands_over() {
        let dir = TempDir::new("instance");
        let path = dir.path().join("ren/instance");
        let Claim::First(mut first) = claim(&path, &Activate { token: None }).unwrap() else {
            panic!("the first Ren runs alone");
        };
        let (sender, received) = mpsc::channel();
        first
            .serve(move |activate| sender.send(activate).unwrap())
            .unwrap();

        assert!(matches!(
            claim(&path, &token("abc")).unwrap(),
            Claim::Running
        ));
        assert_eq!(received.recv_timeout(TIMEOUT).unwrap(), token("abc"));
        assert!(matches!(
            claim(&path, &Activate { token: None }).unwrap(),
            Claim::Running
        ));
        assert_eq!(
            received.recv_timeout(TIMEOUT).unwrap(),
            Activate { token: None }
        );

        // Gone with the first Ren.
        drop(first);
        assert!(!path.exists());
    }

    #[test]
    fn a_socket_left_behind_is_replaced() {
        let dir = TempDir::new("instance-left");
        let path = dir.path().join("instance");
        // Bound, but nobody listens any more.
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        let Claim::First(first) = claim(&path, &Activate { token: None }).unwrap() else {
            panic!("the socket was left behind");
        };
        // Another Ren replaced the socket: this one leaves it alone.
        std::fs::remove_file(&path).unwrap();
        drop(UnixListener::bind(&path).unwrap());
        drop(first);
        assert!(path.exists());
    }

    #[test]
    fn requests() {
        let request = |text: &[u8]| {
            let (mut a, b) = UnixStream::pair().unwrap();
            a.write_all(text).unwrap();
            drop(a);
            read_request(&b)
        };
        assert_eq!(request(b"activate\n"), Some(Activate { token: None }));
        assert_eq!(request(b"activate t-1\n"), Some(token("t-1")));
        assert_eq!(request(b"activate"), None);
        assert_eq!(request(b"quit\n"), None);
        assert_eq!(request(&[b'x'; 5000]), None);
    }
}
