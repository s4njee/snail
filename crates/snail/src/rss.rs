//! Resident memory of this process (plan.md E0.8): current and peak, in bytes.
//!
//! macOS reads the task's resident size with `proc_pidinfo`, the peak with `getrusage`, and the
//! physical footprint with `proc_pid_rusage`; Linux reads `VmRSS` and `VmHWM` from
//! `/proc/self/status`. Elsewhere everything is `None`. Adapted from the reference project.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rss {
    pub current_bytes: Option<u64>,
    pub peak_bytes: Option<u64>,
    /// macOS's physical footprint. `None` on Linux.
    pub footprint_bytes: Option<u64>,
}

/// Resident memory now, and the most the process has held.
pub fn sample() -> Rss {
    platform::sample()
}

/// `VmRSS` and `VmHWM` from a `/proc/<pid>/status` text, converted from kB.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_proc_status(status: &str) -> Rss {
    let field = |name: &str| {
        status.lines().find_map(|line| {
            let rest = line.strip_prefix(name)?.strip_prefix(':')?;
            let kb: u64 = rest.trim().strip_suffix("kB")?.trim().parse().ok()?;
            Some(kb * 1024)
        })
    };
    Rss {
        current_bytes: field("VmRSS"),
        peak_bytes: field("VmHWM"),
        ..Rss::default()
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::Rss;

    pub fn sample() -> Rss {
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        // SAFETY: `proc_taskinfo` is plain integers, so all-zero is valid, and `proc_pidinfo`
        // writes at most `size` bytes into it.
        let current = unsafe {
            let mut info: libc::proc_taskinfo = std::mem::zeroed();
            let written = libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTASKINFO,
                0,
                (&mut info as *mut libc::proc_taskinfo).cast(),
                size,
            );
            (written == size).then_some(info.pti_resident_size)
        };
        // SAFETY: `rusage` is plain integers, and `getrusage` fills it.
        let peak = unsafe {
            let mut usage: libc::rusage = std::mem::zeroed();
            // macOS reports `ru_maxrss` in bytes (Linux uses kB).
            (libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0).then_some(usage.ru_maxrss as u64)
        };
        // SAFETY: `rusage_info_v4` is plain integers, and `proc_pid_rusage` fills it for V4.
        let footprint = unsafe {
            let mut info: libc::rusage_info_v4 = std::mem::zeroed();
            let status = libc::proc_pid_rusage(
                libc::getpid(),
                libc::RUSAGE_INFO_V4,
                (&mut info as *mut libc::rusage_info_v4).cast(),
            );
            (status == 0).then_some(info.ri_phys_footprint)
        };
        Rss {
            current_bytes: current,
            peak_bytes: peak.map(|peak| peak.max(current.unwrap_or(0))),
            footprint_bytes: footprint,
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::Rss;

    pub fn sample() -> Rss {
        std::fs::read_to_string("/proc/self/status")
            .map(|status| super::parse_proc_status(&status))
            .unwrap_or_default()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use super::Rss;

    pub fn sample() -> Rss {
        Rss::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{Rss, parse_proc_status, sample};

    #[test]
    fn proc_status_fields_are_read_in_bytes() {
        let status =
            "Name:\tsnail\nVmPeak:\t 900000 kB\nVmHWM:\t  204800 kB\nVmRSS:\t  102400 kB\n";
        assert_eq!(
            parse_proc_status(status),
            Rss {
                current_bytes: Some(102_400 * 1024),
                peak_bytes: Some(204_800 * 1024),
                ..Rss::default()
            }
        );
        assert_eq!(parse_proc_status("Name:\tx\n"), Rss::default());
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn this_process_has_resident_memory() {
        let rss = sample();
        let current = rss.current_bytes.expect("current RSS");
        let peak = rss.peak_bytes.expect("peak RSS");
        assert!(current > 1024 * 1024, "{rss:?}");
        assert!(peak >= current, "{rss:?}");
    }
}
