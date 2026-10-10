// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The shared memory frames go through. The window makes it (a memfd,
//! sealed so that its size can't change: the helper could otherwise
//! shrink it and make the window crash reading it) and maps it read-only;
//! the helper maps it for writing.

use std::fs::File;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use memmap2::{Mmap, MmapMut, MmapOptions};
use rustix::fs::{MemfdFlags, SealFlags};

/// The bytes of an RGBA frame, if they fit a `usize`.
pub fn frame_len(width: u32, height: u32) -> Option<usize> {
    let len = u64::from(width)
        .checked_mul(u64::from(height))?
        .checked_mul(4)?;
    usize::try_from(len).ok()
}

/// The window's side: a buffer it made, read-only.
pub struct SharedFrames {
    id: u32,
    fd: Option<OwnedFd>,
    map: Mmap,
}

impl SharedFrames {
    /// A buffer of `len` bytes. Its pages are only allocated once the
    /// helper writes to them.
    pub fn create(id: u32, len: usize) -> io::Result<Self> {
        let len = len.max(4);
        let fd = rustix::fs::memfd_create(
            "ren-frames",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )?;
        rustix::fs::ftruncate(&fd, len as u64)?;
        rustix::fs::fcntl_add_seals(&fd, SealFlags::SHRINK | SealFlags::GROW | SealFlags::SEAL)?;
        let file = File::from(fd.try_clone()?);
        // SAFETY: the memfd can't shrink (sealed above), so the mapping
        // stays valid; the helper writes to it, but only while the window
        // isn't reading (see the crate documentation).
        let map = unsafe { MmapOptions::new().len(len).map(&file)? };
        Ok(Self {
            id,
            fd: Some(fd),
            map,
        })
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The file descriptor to send to the helper, until it was sent.
    pub fn fd(&self) -> Option<BorrowedFd<'_>> {
        self.fd.as_ref().map(|fd| fd.as_fd())
    }

    /// Closes the file descriptor once it was sent; the mapping stays.
    pub fn sent(&mut self) {
        self.fd = None;
    }

    /// The pixels of a frame of `width` × `height`, if it fits.
    pub fn frame(&self, width: u32, height: u32) -> Option<&[u8]> {
        let len = frame_len(width, height).filter(|&len| len > 0)?;
        self.map.get(..len)
    }
}

/// The helper's side: the window's buffer, for writing.
pub struct FrameWriter {
    id: u32,
    map: MmapMut,
}

impl FrameWriter {
    /// Maps the buffer the window sent with [`crate::Command::Buffer`].
    pub fn open(id: u32, fd: OwnedFd, len: u64) -> io::Result<Self> {
        let size = rustix::fs::fstat(&fd)?.st_size;
        let len = usize::try_from(len)
            .ok()
            .filter(|&len| u64::try_from(size).is_ok_and(|size| len as u64 <= size))
            .ok_or_else(|| io::Error::other(format!("a buffer of {size} bytes, not {len}")))?;
        let file = File::from(fd);
        // SAFETY: the window sealed the memfd against shrinking and only
        // reads it while the helper doesn't write.
        let map = unsafe { MmapOptions::new().len(len).map_mut(&file)? };
        Ok(Self { id, map })
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    /// Where a frame of `width` × `height` goes, if it fits.
    pub fn frame_mut(&mut self, width: u32, height: u32) -> Option<&mut [u8]> {
        let len = frame_len(width, height).filter(|&len| len > 0)?;
        self.map.get_mut(..len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_go_through_shared_memory() {
        let mut shared = SharedFrames::create(1, frame_len(4, 2).unwrap()).unwrap();
        assert_eq!(shared.len(), 32);
        let fd = shared.fd().unwrap().try_clone_to_owned().unwrap();
        shared.sent();
        assert!(shared.fd().is_none());

        let mut writer = FrameWriter::open(1, fd, 32).unwrap();
        writer.frame_mut(4, 2).unwrap().fill(7);
        assert!(writer.frame_mut(5, 2).is_none());
        assert!(writer.frame_mut(0, 2).is_none());
        assert_eq!(shared.frame(4, 2).unwrap(), [7; 32]);
        assert_eq!(shared.frame(2, 2).unwrap(), [7; 16]);
        assert!(shared.frame(4, 3).is_none());
        assert!(shared.frame(u32::MAX, u32::MAX).is_none());
    }

    #[test]
    fn the_size_is_sealed() {
        let shared = SharedFrames::create(1, 64).unwrap();
        let fd = shared.fd().unwrap();
        assert!(rustix::fs::ftruncate(fd, 8).is_err());
        assert!(rustix::fs::ftruncate(fd, 128).is_err());
        assert!(rustix::fs::fcntl_add_seals(fd, SealFlags::empty()).is_err());
    }

    #[test]
    fn refuses_a_buffer_smaller_than_announced() {
        let shared = SharedFrames::create(1, 64).unwrap();
        let fd = shared.fd().unwrap().try_clone_to_owned().unwrap();
        assert!(FrameWriter::open(1, fd, 65).is_err());
    }

    #[test]
    fn frame_lengths() {
        assert_eq!(frame_len(800, 600), Some(1_920_000));
        assert_eq!(frame_len(0, 600), Some(0));
        assert_eq!(frame_len(u32::MAX, u32::MAX), None);
    }
}
