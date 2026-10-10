// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Messages on the socket: a little-endian u32 length, then JSON. File
//! descriptors travel with the first byte of the message they belong to.

use std::collections::VecDeque;
use std::io::{self, IoSlice, IoSliceMut, Write};
use std::mem::MaybeUninit;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;

use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::MAX_MESSAGE_BYTES;

/// The message as it goes on the socket.
pub fn encode(message: &impl Serialize) -> Vec<u8> {
    let mut bytes = vec![0; 4];
    serde_json::to_writer(&mut bytes, message).expect("messages serialise");
    let len = u32::try_from(bytes.len() - 4).expect("messages are small");
    bytes[..4].copy_from_slice(&len.to_le_bytes());
    bytes
}

/// Sends a message, with a file descriptor if given.
pub fn send(
    socket: &UnixStream,
    message: &impl Serialize,
    fd: Option<BorrowedFd>,
) -> io::Result<()> {
    let bytes = encode(message);
    let Some(fd) = fd else {
        return (&*socket).write_all(&bytes);
    };
    let fds = [fd];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    control.push(SendAncillaryMessage::ScmRights(&fds));
    let sent = loop {
        match rustix::net::sendmsg(
            socket,
            &[IoSlice::new(&bytes)],
            &mut control,
            SendFlags::NOSIGNAL,
        ) {
            Err(rustix::io::Errno::INTR) => continue,
            result => break result?,
        }
    };
    (&*socket).write_all(&bytes[sent..])
}

/// Splits received bytes into messages.
#[derive(Debug, Default)]
pub struct Decoder {
    buffer: Vec<u8>,
    /// Where the next message starts in `buffer`.
    start: usize,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        if self.start > 0 && self.start == self.buffer.len() {
            self.buffer.clear();
            self.start = 0;
        }
        self.buffer.extend_from_slice(bytes);
    }

    /// The next complete message, if there is one. A message longer than
    /// [`MAX_MESSAGE_BYTES`] or one that doesn't decode is an error.
    pub fn decode<T: DeserializeOwned>(&mut self) -> io::Result<Option<T>> {
        let pending = &self.buffer[self.start..];
        let Some(len) = pending.first_chunk::<4>() else {
            return Ok(None);
        };
        let len = u32::from_le_bytes(*len) as usize;
        if len > MAX_MESSAGE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("message of {len} bytes"),
            ));
        }
        let Some(json) = pending.get(4..4 + len) else {
            return Ok(None);
        };
        let message = serde_json::from_slice(json)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        self.start += 4 + len;
        if self.start == self.buffer.len() {
            self.buffer.clear();
            self.start = 0;
        } else if self.start > MAX_MESSAGE_BYTES {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
        Ok(Some(message))
    }
}

/// The most file descriptors kept until they are taken.
const MAX_FDS: usize = 4;

/// Reads messages from a socket, and file descriptors that come with them.
pub struct Receiver {
    socket: UnixStream,
    decoder: Decoder,
    chunk: Vec<u8>,
    fds: VecDeque<OwnedFd>,
    accept_fds: bool,
}

impl Receiver {
    /// With `accept_fds` false, file descriptors that arrive are closed.
    pub fn new(socket: UnixStream, accept_fds: bool) -> Self {
        Self {
            socket,
            decoder: Decoder::default(),
            chunk: vec![0; 64 * 1024],
            fds: VecDeque::new(),
            accept_fds,
        }
    }

    /// Waits for the next message; `None` when the other side closed the
    /// socket.
    pub fn recv<T: DeserializeOwned>(&mut self) -> io::Result<Option<T>> {
        loop {
            if let Some(message) = self.decoder.decode()? {
                return Ok(Some(message));
            }
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
            let mut control = RecvAncillaryBuffer::new(&mut space);
            let received = match rustix::net::recvmsg(
                &self.socket,
                &mut [IoSliceMut::new(&mut self.chunk)],
                &mut control,
                RecvFlags::CMSG_CLOEXEC,
            ) {
                Ok(received) => received,
                Err(rustix::io::Errno::INTR) => continue,
                Err(err) => return Err(err.into()),
            };
            for message in control.drain() {
                if let RecvAncillaryMessage::ScmRights(fds) = message {
                    for fd in fds {
                        if self.accept_fds && self.fds.len() < MAX_FDS {
                            self.fds.push_back(fd);
                        }
                    }
                }
            }
            if received.bytes == 0 {
                return Ok(None);
            }
            self.decoder.push(&self.chunk[..received.bytes]);
        }
    }

    /// The oldest file descriptor received and not taken yet.
    pub fn take_fd(&mut self) -> Option<OwnedFd> {
        self.fds.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Event};
    use std::os::fd::AsFd;

    #[test]
    fn decodes_split_messages() {
        let first = Event::Title {
            tab: 1,
            title: "Ä title".to_owned(),
        };
        let second = Event::Closed { tab: 2 };
        let mut bytes = encode(&first);
        bytes.extend(encode(&second));

        // Byte by byte, then all at once.
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        for byte in &bytes {
            decoder.push(&[*byte]);
            if let Some(event) = decoder.decode::<Event>().unwrap() {
                events.push(event);
            }
        }
        assert_eq!(events, [first.clone(), second.clone()]);
        assert_eq!(decoder.decode::<Event>().unwrap(), None);

        decoder.push(&bytes);
        assert_eq!(decoder.decode::<Event>().unwrap(), Some(first));
        assert_eq!(decoder.decode::<Event>().unwrap(), Some(second));
        assert_eq!(decoder.decode::<Event>().unwrap(), None);
    }

    #[test]
    fn rejects_long_and_broken_messages() {
        let mut decoder = Decoder::default();
        decoder.push(&u32::MAX.to_le_bytes());
        let err = decoder.decode::<Event>().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        let mut decoder = Decoder::default();
        decoder.push(&3u32.to_le_bytes());
        decoder.push(b"{x}");
        assert!(decoder.decode::<Event>().is_err());
    }

    #[test]
    fn sends_and_receives_with_fds() {
        let (a, b) = UnixStream::pair().unwrap();
        let file = rustix::fs::memfd_create("test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
        rustix::fs::ftruncate(&file, 123).unwrap();
        send(&a, &Command::FrameTaken, None).unwrap();
        send(&a, &Command::Buffer { id: 7, len: 123 }, Some(file.as_fd())).unwrap();
        send(&a, &Command::Dark(true), None).unwrap();
        drop(a);

        let mut receiver = Receiver::new(b, true);
        assert_eq!(
            receiver.recv::<Command>().unwrap(),
            Some(Command::FrameTaken)
        );
        assert_eq!(
            receiver.recv::<Command>().unwrap(),
            Some(Command::Buffer { id: 7, len: 123 })
        );
        let fd = receiver.take_fd().unwrap();
        assert_eq!(rustix::fs::fstat(&fd).unwrap().st_size, 123);
        assert_eq!(
            receiver.recv::<Command>().unwrap(),
            Some(Command::Dark(true))
        );
        assert_eq!(receiver.recv::<Command>().unwrap(), None);
        assert!(receiver.take_fd().is_none());
    }

    #[test]
    fn drops_fds_unless_accepted() {
        let (a, b) = UnixStream::pair().unwrap();
        let file = rustix::fs::memfd_create("test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
        send(&a, &Event::Closed { tab: 1 }, Some(file.as_fd())).unwrap();
        drop(a);
        let mut receiver = Receiver::new(b, false);
        assert_eq!(
            receiver.recv::<Event>().unwrap(),
            Some(Event::Closed { tab: 1 })
        );
        assert!(receiver.take_fd().is_none());
    }
}
