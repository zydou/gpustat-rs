//! The data model, and the query that fills it.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Local};
use nvml_wrapper::error::NvmlError;
use serde::Serialize;

use crate::nvml::Querier;
use crate::process;

/// Reported in place of a metric NVML refused to give.
pub const NOT_SUPPORTED: &str = "Not Supported";

/// How long to wait between the two samples a CPU percentage needs.
const RESAMPLE_DELAY: Duration = Duration::from_millis(100);

/// One process using a GPU.
#[derive(Debug, Clone, Serialize)]
pub struct Process {
    pub pid: u32,
    /// Megabytes of device memory, when NVML reports one.
    pub gpu_memory_usage: Option<u64>,
    pub username: Option<String>,
    /// The executable's base name.
    pub command: Option<String>,
    pub full_command: Vec<String>,
    pub cpu_percent: f64,
    /// Bytes of resident set size.
    pub cpu_memory_usage: u64,
}

/// One GPU, in the flat `gpustat` JSON shape.
///
/// Field order is the JSON key order.
#[derive(Debug, Clone, Default, Serialize)]
pub struct GpuInfo {
    pub index: i32,
    pub name: String,
    pub uuid: Option<String>,
    #[serde(rename = "temperature.gpu")]
    pub temperature: Option<u32>,
    #[serde(rename = "fan.speed")]
    pub fan_speed: Option<u32>,
    #[serde(rename = "memory.used")]
    pub memory_used: Option<u64>,
    #[serde(rename = "memory.total")]
    pub memory_total: Option<u64>,
    #[serde(rename = "memory.unified")]
    pub memory_unified: bool,
    #[serde(rename = "utilization.gpu")]
    pub utilization: Option<u32>,
    #[serde(rename = "utilization.enc")]
    pub utilization_enc: Option<u32>,
    #[serde(rename = "utilization.dec")]
    pub utilization_dec: Option<u32>,
    #[serde(rename = "power.draw")]
    pub power_draw: Option<u32>,
    #[serde(rename = "enforced.power.limit")]
    pub power_limit: Option<u32>,
    pub processes: Option<Vec<Process>>,
}

/// One queried GPU. A device that could not be queried at all carries the
/// error message where its name would go.
#[derive(Debug, Clone, Serialize)]
pub struct GpuStat {
    #[serde(flatten)]
    pub info: GpuInfo,
    #[serde(skip)]
    pub available: bool,
}

/// A query result: the host, the driver, when it ran, and the devices.
///
/// The field order is the JSON key order.
#[derive(Debug, Clone, Serialize)]
pub struct GpuStatCollection {
    pub hostname: String,
    pub driver_version: Option<String>,
    pub query_time: DateTime<Local>,
    pub gpus: Vec<GpuStat>,
}

/// Errors swallowed while querying, reported to stderr with `--debug`.
#[derive(Default)]
pub struct Log {
    messages: Vec<String>,
}

impl Log {
    pub fn add(&mut self, message: impl Into<String>) {
        self.messages.push(message.into());
    }

    /// Print each distinct message once, with how often it happened.
    pub fn report(&self) {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for message in &self.messages {
            *counts.entry(message.as_str()).or_insert(0) += 1;
        }
        if counts.is_empty() {
            return;
        }

        let mut out = std::io::stderr().lock();
        for message in &self.messages {
            let Some(times) = counts.remove(message.as_str()) else {
                continue;
            };
            let plural = if times == 1 { "time" } else { "times" };
            let _ = writeln!(out, "warning: {message} ({times} {plural})");
        }
    }
}

impl GpuStatCollection {
    /// Query every requested GPU through `querier`, or every device when
    /// `ids` is `None`.
    pub fn new_query(
        querier: &dyn Querier,
        ids: Option<&[i32]>,
        debug: bool,
    ) -> Result<Self, NvmlError> {
        let mut log = Log::default();
        let mut gpus: Vec<GpuStat> = match ids {
            // Explicit ids are used as given; their validity is per-device.
            Some(ids) => {
                let mut gpus = Vec::with_capacity(ids.len());
                query_devices(querier, ids.iter().copied(), &mut gpus, &mut log);
                gpus
            }
            None => {
                let count = querier.device_count()?;
                let mut gpus = Vec::with_capacity(count as usize);
                query_devices(querier, 0..count as i32, &mut gpus, &mut log);
                gpus
            }
        };

        // A CPU percentage is a delta between two samples, so the second one
        // has to wait a little.
        resample_cpu(&mut gpus);

        process::clean();

        let collection = Self {
            gpus,
            hostname: hostname(),
            query_time: Local::now(),
            driver_version: querier.driver_version(&mut log),
        };
        if debug {
            log.report();
        }
        Ok(collection)
    }

    /// The widest GPU name, used to size the name column.
    pub fn name_width(&self) -> usize {
        self.gpus
            .iter()
            .map(|stat| stat.info.name.chars().count())
            .max()
            .unwrap_or(0)
    }

    /// Whether not a single device could be queried.
    ///
    /// An empty collection is included: a host with no devices has none either,
    /// so callers that care must know an index was actually requested.
    pub fn none_available(&self) -> bool {
        !self.gpus.iter().any(|stat| stat.available)
    }

    /// `index N: <reason>` for every device that could not be queried.
    pub fn failure_report(&self) -> String {
        self.gpus
            .iter()
            .filter(|stat| !stat.available)
            .map(|stat| format!("index {}: {}", stat.info.index, stat.info.name))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Query each index, appending a failed one as a row that carries the error
/// message where the device name would go.
fn query_devices<I>(querier: &dyn Querier, ids: I, gpus: &mut Vec<GpuStat>, log: &mut Log)
where
    I: IntoIterator<Item = i32>,
{
    for index in ids {
        match querier.gpu_info(index, log) {
            Ok(info) => gpus.push(GpuStat {
                info,
                available: true,
            }),
            Err(error) => {
                log.add(format!("GPU {index}: {error}"));
                gpus.push(GpuStat {
                    info: GpuInfo {
                        index,
                        name: error.to_string(),
                        ..Default::default()
                    },
                    available: false,
                });
            }
        }
    }
}

/// The pids that need a second sample, each once, in first-seen order.
///
/// A process that spans several devices is listed once per device, but its CPU
/// percentage is a delta between two samples; sampling the same pid twice
/// within one query would report about zero on every device but the first.
fn resample_pids(gpus: &[GpuStat]) -> Vec<u32> {
    let mut pids = Vec::new();
    for stat in gpus {
        for process in stat.info.processes.iter().flatten() {
            if !pids.contains(&process.pid) {
                pids.push(process.pid);
            }
        }
    }
    pids
}

/// Sample every listed process again so its CPU percentage is a real delta,
/// and share the figure between the devices a process spans.
fn resample_cpu(gpus: &mut [GpuStat]) {
    let pids = resample_pids(gpus);
    if pids.is_empty() {
        return;
    }
    thread::sleep(RESAMPLE_DELAY);

    let mut samples: HashMap<u32, f64> = HashMap::new();
    for pid in pids {
        samples.insert(pid, process::cpu_percent(pid));
    }
    for stat in gpus {
        if let Some(processes) = stat.info.processes.as_mut() {
            for process in processes {
                if let Some(percent) = samples.get(&process.pid) {
                    process.cpu_percent = *percent;
                }
            }
        }
    }
}

/// The host name, from `gethostname(3)`.
fn hostname() -> String {
    let mut buffer = [0u8; 256];
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if result != 0 {
        return String::new();
    }
    let len = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..len]).into_owned()
}

/// Whether the device shares memory with the host, so NVML's memory query is
/// reported as unsupported and total memory comes from system RAM instead.
///
/// A positive signal is required: a known integrated SoC in the device name,
/// or a Linux Tegra/GB10 host marker. A failing memory query alone is not
/// enough — it also happens for a lost GPU or a driver mismatch.
pub fn is_unified_memory_device(name: &str) -> bool {
    const KNOWN: [&str; 5] = ["gb10", "jetson", "orin", "xavier", "tegra"];
    let lower = name.to_ascii_lowercase();
    let mut tokens = lower.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
    tokens.any(|token| KNOWN.contains(&token)) || is_unified_memory_platform()
}

/// Linux host markers for an integrated NVIDIA (Tegra/GB10) SoC.
fn is_unified_memory_platform() -> bool {
    if fs::metadata("/etc/nv_tegra_release").is_ok() {
        return true;
    }
    let Ok(compatible) = fs::read("/proc/device-tree/compatible") else {
        return false;
    };
    let lower = String::from_utf8_lossy(&compatible).to_ascii_lowercase();
    lower.contains("nvidia") && (lower.contains("tegra") || lower.contains("gb10"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invalid(index: i32, message: &str) -> GpuStat {
        GpuStat {
            info: GpuInfo {
                index,
                name: message.to_string(),
                ..Default::default()
            },
            available: false,
        }
    }

    fn available(index: i32) -> GpuStat {
        GpuStat {
            info: GpuInfo {
                index,
                name: "GPU".to_string(),
                ..Default::default()
            },
            available: true,
        }
    }

    fn collection(gpus: Vec<GpuStat>) -> GpuStatCollection {
        GpuStatCollection {
            gpus,
            hostname: "h".to_string(),
            query_time: Local::now(),
            driver_version: None,
        }
    }

    #[test]
    fn a_failed_device_carries_the_error_as_its_name() {
        let stat = invalid(7, "NVML Device Not Found");
        assert!(!stat.available);
        assert_eq!(stat.info.index, 7);
        assert_eq!(stat.info.name, "NVML Device Not Found");
        assert!(stat.info.processes.is_none());
        assert!(!stat.info.memory_unified);
    }

    #[test]
    fn only_a_total_failure_reads_as_nothing_available() {
        assert!(
            collection(Vec::new()).none_available(),
            "a query with no devices has none either"
        );
        assert!(collection(vec![invalid(0, "x")]).none_available());
        assert!(!collection(vec![available(0)]).none_available());
        assert!(
            !collection(vec![available(0), invalid(1, "x")]).none_available(),
            "one good device is enough"
        );
    }

    #[test]
    fn the_failure_report_names_each_missing_index() {
        assert_eq!(
            collection(vec![
                available(0),
                invalid(3, "NVML Device Not Found"),
                invalid(99, "a supplied argument was invalid"),
            ])
            .failure_report(),
            "index 3: NVML Device Not Found; index 99: a supplied argument was invalid"
        );
    }

    #[test]
    fn unified_memory_needs_a_positive_signal() {
        assert!(is_unified_memory_device("NVIDIA Jetson AGX Xavier"));
        assert!(is_unified_memory_device("NVIDIA GB10"));
        assert!(is_unified_memory_device("NVIDIA Tegra"));
        assert!(is_unified_memory_device("orin"));
        assert!(is_unified_memory_device("jetson-xavier"));
        // A desktop GPU is never unified, whatever /proc says.
        assert!(!is_unified_memory_device("NVIDIA GeForce RTX 4090"));
        assert!(!is_unified_memory_device(""));
        // Token boundaries: a longer word containing the name does not match.
        assert!(!is_unified_memory_device("NVIDIA Tegraphon"));
        assert!(!is_unified_memory_device("_jetson"));
    }

    #[test]
    fn log_dedupes_and_counts() {
        let mut log = Log::default();
        log.add("nvmlDeviceGetFanSpeed: not supported");
        log.add("nvmlDeviceGetMemoryInfo: unsupported");
        log.add("nvmlDeviceGetFanSpeed: not supported");
        log.report();
    }

    #[test]
    fn name_width_is_the_longest_name() {
        let empty = GpuStatCollection {
            gpus: Vec::new(),
            hostname: "h".to_string(),
            query_time: Local::now(),
            driver_version: None,
        };
        assert_eq!(empty.name_width(), 0);

        let mut stat = invalid(0, "x");
        stat.info.name = "NVIDIA GeForce RTX 4090".to_string();
        let collection = GpuStatCollection {
            gpus: vec![stat],
            hostname: "h".to_string(),
            query_time: Local::now(),
            driver_version: None,
        };
        assert_eq!(collection.name_width(), 23);
    }

    #[test]
    fn resample_pids_dedupes_a_process_that_spans_devices() {
        let make = |index: i32, pid: u32| -> GpuStat {
            GpuStat {
                info: GpuInfo {
                    index,
                    name: "GPU".to_string(),
                    processes: Some(vec![Process {
                        pid,
                        gpu_memory_usage: None,
                        username: None,
                        command: None,
                        full_command: Vec::new(),
                        cpu_percent: 0.0,
                        cpu_memory_usage: 0,
                    }]),
                    ..Default::default()
                },
                available: true,
            }
        };

        let gpus = vec![
            make(0, 11),
            make(0, 11), // the same process, listed twice on the same device
            make(1, 11), // and again on another device
            make(1, 22),
            make(2, 33),
        ];
        assert_eq!(resample_pids(&gpus), vec![11, 22, 33]);

        let shared = vec![make(0, 11), make(1, 11)];
        assert_eq!(resample_pids(&shared), vec![11]);
    }
}
