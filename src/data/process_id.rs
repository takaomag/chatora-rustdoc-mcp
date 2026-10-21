//! Who a process is, beyond its PID, which the OS uses again (requirement R-WS-6).
//!
//! That is when it started, and the machine and the boot it runs in (`doc/m2-plan.md` §6.4, the
//! decisions of C4). A record of a process is compared only on the same machine since the same
//! boot, where the start times of processes are comparable.

use std::sync::OnceLock;

/// The machine and its boot, as text: on Linux, the machine id and the boot id; on macOS, the
/// host name and the time of the boot. `None` when they cannot be read.
#[must_use]
pub fn machine_and_boot() -> Option<&'static (String, String)> {
    static READ: OnceLock<Option<(String, String)>> = OnceLock::new();
    READ.get_or_init(read_machine_and_boot).as_ref()
}

/// When the process `pid` started, or `None` when it does not run or that cannot be read.
///
/// The units are the OS's, which compare among the processes of one boot: clock ticks since the
/// boot on Linux, microseconds since 1970 on macOS.
#[must_use]
pub fn started(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        linux::stat(pid).map(|stat| stat.started)
    }
    #[cfg(target_os = "macos")]
    {
        macos::started(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

fn read_machine_and_boot() -> Option<(String, String)> {
    #[cfg(target_os = "linux")]
    {
        let read = |path: &str| {
            std::fs::read_to_string(path)
                .ok()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
        };
        let machine = read("/etc/machine-id").or_else(|| read("/var/lib/dbus/machine-id"))?;
        Some((machine, read("/proc/sys/kernel/random/boot_id")?))
    }
    #[cfg(target_os = "macos")]
    {
        macos::machine_and_boot()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
mod linux {
    /// What `/proc/<pid>/stat` tells of a process.
    pub(super) struct Stat {
        /// Field 22: clock ticks from the boot to the start.
        pub(super) started: u64,
    }

    /// The stat of the process `pid`, if it runs.
    pub(super) fn stat(pid: u32) -> Option<Stat> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The name, field 2, is in parentheses and may hold spaces and parentheses; the fields
        // after the last `)` start with field 3.
        let (_, rest) = stat.rsplit_once(')')?;
        let fields: Vec<&str> = rest.split_whitespace().collect();
        Some(Stat {
            started: fields.get(22 - 3)?.parse().ok()?,
        })
    }
}

#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "proc_pidinfo, sysctlbyname and gethostname are system calls without a safe wrapper \
              in std"
)]
mod macos {
    use std::{ffi::CStr, mem};

    /// When the process `pid` started, in microseconds since 1970.
    pub(super) fn started(pid: u32) -> Option<u64> {
        let pid = libc::c_int::try_from(pid).ok()?;
        // SAFETY: `proc_bsdinfo` is plain data, for which all zeros is a value.
        let mut info: libc::proc_bsdinfo = unsafe { mem::zeroed() };
        let size = libc::c_int::try_from(mem::size_of::<libc::proc_bsdinfo>()).ok()?;
        // SAFETY: the buffer is a `proc_bsdinfo` of `size` bytes, which PROC_PIDTBSDINFO fills.
        let filled = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        (filled == size).then(|| {
            info.pbi_start_tvsec
                .saturating_mul(1_000_000)
                .saturating_add(info.pbi_start_tvusec)
        })
    }

    /// The host name, and the time of the boot.
    pub(super) fn machine_and_boot() -> Option<(String, String)> {
        let mut name = [0_u8; 256];
        // SAFETY: the buffer is as long as the length given.
        if unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len()) } != 0 {
            return None;
        }
        let host = CStr::from_bytes_until_nul(&name)
            .ok()?
            .to_str()
            .ok()?
            .to_owned();
        // SAFETY: `timeval` is plain data, for which all zeros is a value.
        let mut boot: libc::timeval = unsafe { mem::zeroed() };
        let mut size = mem::size_of::<libc::timeval>();
        // SAFETY: the name ends with NUL, and the buffer is a `timeval` of `size` bytes, which
        // `kern.boottime` fills.
        let read = unsafe {
            libc::sysctlbyname(
                c"kern.boottime".as_ptr(),
                (&raw mut boot).cast(),
                &raw mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (read == 0).then(|| (host, format!("{}.{:06}", boot.tv_sec, boot.tv_usec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_process_has_a_start() {
        let me = started(std::process::id()).expect("this process started");
        // A container may have no machine id; then nothing is compared.
        if let Some((machine, boot)) = machine_and_boot() {
            assert!(!machine.is_empty() && !boot.is_empty());
        }
        // A process started later starts later.
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("sleep starts");
        let later = started(child.id()).expect("the child started");
        assert!(later >= me, "{later} < {me}");
        child.kill().expect("killed");
        child.wait().expect("waited");
        assert_eq!(started(child.id()), None, "gone");
    }
}
