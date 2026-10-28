//! Process information, read from `/proc`.
//!
//! NVML reports only pids, so a process' user, command line, CPU usage and
//! resident set all come from the same small fixed set of `/proc` files that
//! `psutil` reads. Doing it directly keeps the dependency list short.

use std::collections::HashMap;
use std::ffi::CStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use crate::util::read_lossy;

unsafe extern "C" {
    fn getpwuid_r(
        uid: libc::uid_t,
        pwd: *mut libc::passwd,
        buf: *mut libc::c_char,
        buflen: libc::size_t,
        result: *mut *mut libc::passwd,
    ) -> libc::c_int;
}

/// `utime + stime` in clock ticks, with the wall clock it was read at.
#[derive(Debug, Copy, Clone)]
struct Sample {
    jiffies: u64,
    wall: Instant,
}

/// CPU baselines from the previous query, keyed by pid.
static SAMPLES: LazyLock<Mutex<HashMap<u32, Sample>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn proc(pid: u32) -> PathBuf {
    Path::new("/proc").join(pid.to_string())
}

fn proc_file(pid: u32, name: &str) -> PathBuf {
    proc(pid).join(name)
}

fn lock_samples() -> std::sync::MutexGuard<'static, HashMap<u32, Sample>> {
    SAMPLES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn pid_exists(pid: u32) -> bool {
    fs::metadata(proc(pid)).is_ok()
}

/// Drop cached baselines for processes that have exited.
pub fn clean() {
    lock_samples().retain(|pid, _| pid_exists(*pid));
}

/// The process' command line as argv.
pub fn cmdline(pid: u32) -> Result<Vec<String>, std::io::Error> {
    let bytes = fs::read(proc_file(pid, "cmdline"))?;
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    Ok(split_cmdline(&String::from_utf8_lossy(&bytes)))
}

/// Split a `/proc/<pid>/cmdline` buffer.
///
/// The buffer is NUL-separated, but processes that rewrite their own name
/// (VLLM, `setproctitle`, Chrome) write one space-separated string terminated
/// by a single NUL, so a buffer that yields a single argument is split on
/// whitespace instead.
fn split_cmdline(data: &str) -> Vec<String> {
    let mut args: Vec<String> = data.split_terminator('\0').map(str::to_string).collect();
    if args.len() == 1 && args[0].chars().any(char::is_whitespace) {
        args = args
            .remove(0)
            .split_whitespace()
            .map(str::to_string)
            .collect();
    }
    args
}

/// The user owning the process.
///
/// `None` only when `/proc/<pid>/status` is unreadable. A UID with no
/// `/etc/passwd` entry is a value, not an error: the numeric UID is returned,
/// so a container runtime user prints as `7474` rather than `?`.
pub fn username(pid: u32) -> Option<String> {
    let status = read_lossy(proc_file(pid, "status"))?;
    let uid = status
        .lines()
        .find_map(|line| {
            line.strip_prefix("Uid:")
                .and_then(|rest| rest.split_whitespace().next())
        })?
        .parse::<u32>()
        .ok()?;
    Some(username_by_uid(uid).unwrap_or_else(|| uid.to_string()))
}

fn username_by_uid(uid: u32) -> Option<String> {
    let mut buffer = vec![0u8; 1024];
    let mut pwd = unsafe { std::mem::zeroed::<libc::passwd>() };
    let mut result = std::ptr::null_mut::<libc::passwd>();
    let rc = unsafe {
        getpwuid_r(
            uid,
            &mut pwd,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr((*result).pw_name) }
        .to_str()
        .ok()
        .map(str::to_string)
}

/// Resident set size in bytes.
pub fn rss(pid: u32) -> Option<u64> {
    let statm = read_lossy(proc_file(pid, "statm"))?;
    let pages = statm
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()?;
    Some(pages.saturating_mul(pagesize()))
}

/// Page size in bytes, for turning page counts into bytes.
fn pagesize() -> u64 {
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    u64::try_from(size).unwrap_or(4096)
}

/// Total system memory in bytes.
///
/// Used as the `memory.total` figure on systems where the device shares memory
/// with the host.
pub fn system_ram_bytes() -> u64 {
    let Some(meminfo) = read_lossy("/proc/meminfo") else {
        return 0;
    };
    meminfo
        .lines()
        .find_map(|line| {
            line.strip_prefix("MemTotal:")
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|field| field.parse::<u64>().ok())
        })
        .map_or(0, |kibibytes| kibibytes.saturating_mul(1024))
}

/// Clocks per second, for turning jiffies into seconds. Linux reports 100.
fn clk_tck() -> f64 {
    let clocks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if clocks > 0 { clocks as f64 } else { 100.0 }
}

fn sample(pid: u32) -> Option<Sample> {
    let wall = Instant::now();
    let stat = read_lossy(proc_file(pid, "stat"))?;
    // The `comm` field is parenthesised and may itself contain spaces and
    // parentheses, so the closing paren is located from the right.
    let close = stat.rfind(')')?;
    // After `)` the fields start at `state`; `utime` and `stime` sit at 11
    // and 12.
    let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    let utime = fields.get(11)?.parse::<u64>().ok()?;
    let stime = fields.get(12)?.parse::<u64>().ok()?;
    Some(Sample {
        jiffies: utime.saturating_add(stime),
        wall,
    })
}

/// CPU usage since the previous call for this process, as a percentage.
///
/// Two samples are needed for a delta, so the first call for a pid returns
/// `0.0`. A loop spinning one core reads about `100`. The result is rounded to
/// one decimal place to keep the column from flickering.
pub fn cpu_percent(pid: u32) -> f64 {
    let Some(now) = sample(pid) else {
        return 0.0;
    };
    let mut samples = lock_samples();
    let Some(previous) = samples.remove(&pid) else {
        samples.insert(pid, now);
        return 0.0;
    };
    samples.insert(pid, now);

    let elapsed = now.wall.duration_since(previous.wall).as_secs_f64();
    if elapsed == 0.0 {
        return 0.0;
    }
    let delta = now.jiffies.saturating_sub(previous.jiffies) as f64;
    let percent = 100.0 * delta / clk_tck() / elapsed;
    (percent * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_pid_is_reported() {
        let pid = std::process::id();
        assert!(pid_exists(pid));
        assert!(!cmdline(pid).unwrap().is_empty());
        assert!(username(pid).is_some());
        assert!(rss(pid).unwrap() > 0);
    }

    #[test]
    fn cpu_percent_needs_two_samples_and_clean_keeps_the_live_pid() {
        // Both parts share the process-wide sample cache, so they live in one
        // test: run in parallel they would steal each other's baseline.
        let pid = std::process::id();
        clean();

        assert_eq!(cpu_percent(pid), 0.0);
        assert!(lock_samples().contains_key(&pid));

        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = cpu_percent(pid);
        assert!(second.is_finite() && second >= 0.0);

        // This process still exists, so `clean` keeps its baseline.
        clean();
        assert!(lock_samples().contains_key(&pid));
        assert!(!pid_exists(1 << 29));
    }

    #[test]
    fn split_cmdline_handles_a_rewritten_proctitle() {
        // Normal NUL-separated argv.
        assert_eq!(
            split_cmdline("/usr/bin/python\0train.py\0"),
            vec!["/usr/bin/python", "train.py"]
        );
        // A rewritten argv that keeps its NUL terminator.
        assert_eq!(
            split_cmdline("VLLM::Worker_TP0 --model a\0"),
            vec!["VLLM::Worker_TP0", "--model", "a"]
        );
        // And one with no trailing separator at all.
        assert_eq!(
            split_cmdline("VLLM::Worker_TP0 --model a"),
            vec!["VLLM::Worker_TP0", "--model", "a"]
        );
        // A single rewritten argument is left alone.
        assert_eq!(split_cmdline("VLLM::Worker_TP0"), vec!["VLLM::Worker_TP0"]);
        assert_eq!(split_cmdline("foo bar "), vec!["foo", "bar"]);
    }

    #[test]
    fn system_ram_is_positive() {
        assert!(system_ram_bytes() > 0);
        assert!(clk_tck() > 0.0);
        assert!(pagesize() > 0);
    }
}
