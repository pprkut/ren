// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Memory and CPU use of this process, from `/proc` (Linux), for the
//! measurement modes, and giving freed memory back to the system.

/// A `/proc/self/status` field in KiB, e.g. `VmHWM:` (peak RSS).
pub fn proc_status_kib(field: &str) -> Option<u64> {
    status_kib("/proc/self/status", field)
}

/// A `/proc/<pid>/status` field of another process in KiB.
#[cfg_attr(not(feature = "servo"), allow(dead_code))]
pub fn proc_status_kib_of(pid: u32, field: &str) -> Option<u64> {
    status_kib(&format!("/proc/{pid}/status"), field)
}

fn status_kib(path: &str, field: &str) -> Option<u64> {
    let status = std::fs::read_to_string(path).ok()?;
    let line = status.lines().find(|l| l.starts_with(field))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// User and system CPU time of this process so far, in seconds, from
/// `/proc/self/stat`. Its clock ticks are USER_HZ, which is 100 on Linux.
pub fn cpu_seconds() -> Option<(f64, f64)> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Fields after "pid (comm) " start with field 3; utime and stime are
    // fields 14 and 15.
    let mut fields = stat.rsplit_once(") ")?.1.split_whitespace().skip(11);
    let user: u64 = fields.next()?.parse().ok()?;
    let sys: u64 = fields.next()?.parse().ok()?;
    Some((user as f64 / 100.0, sys as f64 / 100.0))
}

/// Gives the memory the allocator keeps after freeing back to the system,
/// where that is possible (glibc). Returns whether it was tried.
pub fn trim_heap() -> bool {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: malloc_trim only releases free memory of glibc's heap;
        // it takes no pointers.
        unsafe { malloc_trim(0) };
        true
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process_stats() {
        let (user, sys) = cpu_seconds().unwrap();
        assert!(user >= 0.0 && sys >= 0.0);
        // The kernel updates the peak (VmHWM) lazily, so it may briefly be
        // below the current RSS; only check that both are there.
        assert!(proc_status_kib("VmRSS:").unwrap() > 0);
        assert!(proc_status_kib("VmHWM:").unwrap() > 0);
        assert!(proc_status_kib_of(std::process::id(), "VmRSS:").unwrap() > 0);
        assert_eq!(proc_status_kib_of(u32::MAX, "VmRSS:"), None);
    }
}
