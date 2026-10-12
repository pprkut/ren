// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Watching the settings file, so that edits by hand apply when they are
//! saved, as in Zed.
//!
//! A thread sleeps on inotify for the file's directory: editors save by
//! writing a new file and renaming it over the old one, which replaces the
//! inode a watch on the file itself would follow. If the file is a
//! symbolic link (dotfiles kept elsewhere), the directory of its target is
//! watched too. The thread wakes only for events of the directory, and
//! calls back only for those of the file.

use std::ffi::{CString, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The events that mean the file was written, replaced or removed.
const MASK: u32 = libc::IN_CLOSE_WRITE
    | libc::IN_MOVED_TO
    | libc::IN_MOVED_FROM
    | libc::IN_CREATE
    | libc::IN_DELETE;

/// The watcher's thread does nothing but wait.
const STACK_SIZE: usize = 64 * 1024;

/// Watches a file until dropped.
pub struct Watch {
    /// Closing it (on drop) wakes the thread, which then ends.
    _stop: OwnedFd,
}

/// Calls `changed` on a thread of its own whenever the file at `path` was
/// written, replaced, created or removed; several events read at once
/// call it once. The file's directory is created if it is missing.
pub fn watch(path: &Path, changed: impl Fn() + Send + 'static) -> io::Result<Watch> {
    let watched = watched(path);
    let inotify = cvt(unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) })?;
    // SAFETY: a new descriptor that nothing else owns.
    let inotify = unsafe { OwnedFd::from_raw_fd(inotify) };
    for (dir, _) in &watched {
        std::fs::create_dir_all(dir)?;
        let dir = CString::new(dir.as_os_str().as_bytes())
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
        cvt(unsafe { libc::inotify_add_watch(inotify.as_raw_fd(), dir.as_ptr(), MASK) })?;
    }
    let mut pipe = [0; 2];
    cvt(unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) })?;
    // SAFETY: two new descriptors that nothing else owns.
    let (stopped, stop) = unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
    let names: Vec<OsString> = watched.into_iter().map(|(_, name)| name).collect();
    std::thread::Builder::new()
        .name("settings-watch".to_owned())
        .stack_size(STACK_SIZE)
        .spawn(move || run(&inotify, &stopped, &names, &changed))?;
    Ok(Watch { _stop: stop })
}

/// The directories to watch, each with the name of the file in it: the
/// file's, and its target's if it is a symbolic link.
fn watched(path: &Path) -> Vec<(PathBuf, OsString)> {
    let mut watched = Vec::new();
    for path in [Some(path.to_owned()), std::fs::canonicalize(path).ok()]
        .into_iter()
        .flatten()
    {
        let dir = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.to_owned(),
            _ => PathBuf::from("."),
        };
        let Some(name) = path.file_name() else {
            continue;
        };
        if !watched.iter().any(|(d, n)| *d == dir && n == name) {
            watched.push((dir, name.to_owned()));
        }
    }
    watched
}

fn run(inotify: &OwnedFd, stopped: &OwnedFd, names: &[OsString], changed: &dyn Fn()) {
    // Aligned for `inotify_event`.
    let mut buffer = [0u64; 512];
    loop {
        let mut fds = [
            libc::pollfd {
                fd: inotify.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: stopped.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        // The `Watch` was dropped: its end of the pipe closed.
        if fds[1].revents != 0 {
            return;
        }
        let bytes = std::mem::size_of_val(&buffer);
        let read = unsafe { libc::read(inotify.as_raw_fd(), buffer.as_mut_ptr().cast(), bytes) };
        let Ok(read) = usize::try_from(read) else {
            match io::Error::last_os_error().kind() {
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => continue,
                _ => return,
            }
        };
        // SAFETY: the kernel wrote `read` bytes of events.
        let events =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), read.min(bytes)) };
        if concerns(events, names) {
            changed();
        }
    }
}

/// Whether the events read concern one of `names`, or some were lost.
fn concerns(mut events: &[u8], names: &[OsString]) -> bool {
    const HEADER: usize = std::mem::size_of::<libc::inotify_event>();
    while events.len() >= HEADER {
        // SAFETY: the kernel writes whole events; read unaligned anyway.
        let event: libc::inotify_event =
            unsafe { std::ptr::read_unaligned(events.as_ptr().cast()) };
        if event.mask & libc::IN_Q_OVERFLOW != 0 {
            return true;
        }
        let end = (HEADER + event.len as usize).min(events.len());
        let name = &events[HEADER..end];
        let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
        if names.iter().any(|n| n.as_bytes() == name) {
            return true;
        }
        events = &events[end..];
    }
    false
}

fn cvt(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::file::tests::test_dir;
    use crate::write_atomically;

    const WAIT: Duration = Duration::from_secs(5);
    const QUIET: Duration = Duration::from_millis(200);

    fn watching(path: &Path) -> (Watch, mpsc::Receiver<()>) {
        let (sender, receiver) = mpsc::channel();
        let watch = watch(path, move || {
            let _ = sender.send(());
        })
        .unwrap();
        (watch, receiver)
    }

    /// Takes the events of one change.
    fn drain(receiver: &mpsc::Receiver<()>) {
        receiver.recv_timeout(WAIT).expect("no event");
        while receiver.recv_timeout(QUIET).is_ok() {}
    }

    #[test]
    fn changes_of_the_file() {
        let dir = test_dir("watch");
        // The directory is created.
        let path = dir.join("ren").join("settings.toml");
        let (watch, events) = watching(&path);

        write_atomically(&path, "[sync]\n").unwrap();
        drain(&events);
        std::fs::write(&path, "[sync]\ninterval-minutes = 5\n").unwrap();
        drain(&events);
        // Other files in the directory don't count.
        std::fs::write(dir.join("ren").join("state.toml"), "").unwrap();
        assert!(events.recv_timeout(QUIET).is_err());
        std::fs::remove_file(&path).unwrap();
        drain(&events);

        // No more calls once dropped.
        drop(watch);
        std::thread::sleep(QUIET);
        std::fs::write(&path, "").unwrap();
        assert!(events.recv_timeout(QUIET).is_err());
    }

    #[test]
    fn the_target_of_a_link() {
        let dir = test_dir("watch-link");
        let target = dir.join("dotfiles").join("ren.toml");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "").unwrap();
        let link = dir.join("config").join("settings.toml");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let (_watch, events) = watching(&link);

        // Saved in the dotfiles by an editor.
        write_atomically(&target, "[sync]\n").unwrap();
        drain(&events);
    }

    #[test]
    fn names_in_events() {
        let names = [OsString::from("settings.toml")];
        let event = |name: &[u8], len: u32, mask: u32| {
            let header = libc::inotify_event {
                wd: 1,
                mask,
                cookie: 0,
                len,
            };
            let mut bytes = unsafe {
                std::slice::from_raw_parts(
                    (&raw const header).cast::<u8>(),
                    std::mem::size_of::<libc::inotify_event>(),
                )
            }
            .to_vec();
            let mut name = name.to_vec();
            name.resize(len as usize, 0);
            bytes.extend(name);
            bytes
        };
        assert!(concerns(&event(b"settings.toml", 16, 0), &names));
        assert!(!concerns(&event(b"settings.toml~", 16, 0), &names));
        assert!(!concerns(&event(b"state.toml", 16, 0), &names));
        assert!(concerns(&event(b"", 0, libc::IN_Q_OVERFLOW), &names));
        let mut two = event(b"other", 16, 0);
        two.extend(event(b"settings.toml", 16, 0));
        assert!(concerns(&two, &names));
        // Cut short.
        assert!(!concerns(&event(b"settings.toml", 16, 0)[..10], &names));
    }
}
