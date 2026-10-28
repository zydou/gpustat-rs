//! The NVIDIA backend, over `nvml-wrapper`.
//!
//! Every field that can be missing on its own degrades to `None` and is logged
//! for `--debug`; only the lookups that make a device unnameable fail the whole
//! query.

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use nvml_wrapper::enums::device::UsedGpuMemory;
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::struct_wrappers::device::ProcessInfo;

use crate::model::{GpuInfo, Log, Process, is_unified_memory_device};
use crate::process;
use crate::util::{MB, basename};

/// What a device backend must provide to fill the model.
pub trait Querier {
    fn device_count(&self) -> Result<u32, NvmlError>;
    fn gpu_info(&self, index: i32, log: &mut Log) -> Result<GpuInfo, NvmlError>;
    fn driver_version(&self, log: &mut Log) -> Option<String>;
}

/// The only backend: NVIDIA, over `nvml-wrapper`.
pub struct NvmlQuerier {
    nvml: Nvml,
}

impl NvmlQuerier {
    /// `nvmlInit()`: any failure is fatal for the whole invocation.
    pub fn new() -> Result<Self, NvmlError> {
        Ok(Self {
            nvml: Nvml::init()?,
        })
    }
}

/// Run one NVML query, logging the failure and degrading to `None`.
fn safe<T: Copy>(log: &mut Log, call: &str, value: Result<T, NvmlError>) -> Option<T> {
    match value {
        Ok(value) => Some(value),
        Err(error) => {
            log.add(format!("{call}: {error}"));
            None
        }
    }
}

/// One NVML process entry, enriched from `/proc`.
fn process_info(entry: ProcessInfo) -> Option<Process> {
    let pid = entry.pid;
    if !process::pid_exists(pid) {
        // The process exited between the NVML query and this read.
        return None;
    }

    // NVML reports device memory in bytes; zero and `Unavailable` both mean
    // there is no usable figure.
    let gpu_memory_usage = match entry.used_gpu_memory {
        UsedGpuMemory::Used(bytes) if bytes != 0 => Some(bytes / MB),
        _ => None,
    };

    // NVML does not expose a command line, so every string below comes from
    // `/proc`; an unreadable one leaves the whole entry as `?`.
    let cmdline = process::cmdline(pid).unwrap_or_default();
    let full_command = if cmdline.is_empty() {
        vec!["?".to_string()]
    } else {
        cmdline
    };

    Some(Process {
        pid,
        gpu_memory_usage,
        username: process::username(pid),
        command: Some(basename(&full_command[0]).to_string()),
        full_command,
        cpu_percent: process::cpu_percent(pid),
        cpu_memory_usage: process::rss(pid).unwrap_or(0),
    })
}

/// The used total for a device that shares memory with the host, summed from
/// the processes on it.
///
/// An empty list means nobody is on the device, so zero is the right answer. A
/// list in which the driver reported no figure for anyone means there is no
/// total to give, and defaulting the missing figures to zero must not turn that
/// into a confident zero.
fn unified_memory_used(list: &[Process]) -> Option<u64> {
    if list.is_empty() {
        Some(0)
    } else {
        list.iter()
            .filter_map(|process| process.gpu_memory_usage)
            .reduce(u64::saturating_add)
    }
}

impl Querier for NvmlQuerier {
    fn device_count(&self) -> Result<u32, NvmlError> {
        self.nvml.device_count()
    }

    fn gpu_info(&self, index: i32, log: &mut Log) -> Result<GpuInfo, NvmlError> {
        let device = self.nvml.device_by_index(index as u32)?;

        // Without a name or a UUID there is no device to describe, so these
        // fail the query; everything else degrades to `None`.
        let name = device.name()?;
        let uuid = device.uuid()?;

        let temperature = safe(
            log,
            "nvmlDeviceGetTemperature",
            device.temperature(TemperatureSensor::Gpu),
        );
        let fan_speed = safe(log, "nvmlDeviceGetFanSpeed", device.fan_speed(0));

        let (memory_used, memory_total, memory_unified) = match device.memory_info() {
            Ok(memory) => (Some(memory.used / MB), Some(memory.total / MB), false),
            Err(error) => {
                log.add(format!("nvmlDeviceGetMemoryInfo: {error}"));
                if matches!(error, NvmlError::NotSupported) && is_unified_memory_device(&name) {
                    // Integrated memory: there is no separate device pool, so
                    // the total is system RAM and the used figure has to come
                    // from the process list.
                    (None, Some(process::system_ram_bytes() / MB), true)
                } else {
                    (None, None, false)
                }
            }
        };

        let utilization = safe(log, "nvmlDeviceGetUtilizationRates", {
            device.utilization_rates().map(|rates| rates.gpu)
        });
        let utilization_enc = safe(log, "nvmlDeviceGetEncoderUtilization", {
            device.encoder_utilization().map(|info| info.utilization)
        });
        let utilization_dec = safe(log, "nvmlDeviceGetDecoderUtilization", {
            device.decoder_utilization().map(|info| info.utilization)
        });

        let power_draw = safe(log, "nvmlDeviceGetPowerUsage", device.power_usage())
            .map(|milliwatts| milliwatts / 1000);
        let power_limit = safe(log, "nvmlDeviceGetEnforcedPowerLimit", {
            device.enforced_power_limit()
        })
        .map(|milliwatts| milliwatts / 1000);

        // A process may show up in both the compute and the graphics list, and
        // it is displayed once.
        let mut seen = Vec::new();
        let mut listed = false;
        let mut processes = Vec::new();
        for (call, result) in [
            (
                "nvmlDeviceGetComputeRunningProcesses",
                device.running_compute_processes(),
            ),
            (
                "nvmlDeviceGetGraphicsRunningProcesses",
                device.running_graphics_processes(),
            ),
        ] {
            match result {
                Ok(entries) => {
                    listed = true;
                    for entry in entries {
                        if seen.contains(&entry.pid) {
                            continue;
                        }
                        seen.push(entry.pid);
                        if let Some(info) = process_info(entry) {
                            processes.push(info);
                        }
                    }
                }
                Err(error) => log.add(format!("{call}: {error}")),
            }
        }
        let processes = listed.then_some(processes);

        // On integrated memory the per-process figures are the only source of a
        // used total.
        let memory_used = if let Some(list) = processes.as_deref().filter(|_| memory_unified) {
            unified_memory_used(list)
        } else {
            memory_used
        };

        Ok(GpuInfo {
            index,
            name,
            uuid: Some(uuid),
            temperature,
            fan_speed,
            memory_used,
            memory_total,
            memory_unified,
            utilization,
            utilization_enc,
            utilization_dec,
            power_draw,
            power_limit,
            processes,
        })
    }

    fn driver_version(&self, log: &mut Log) -> Option<String> {
        match self.nvml.sys_driver_version() {
            Ok(version) => Some(version),
            Err(error) => {
                log.add(format!("driver_version: {error}"));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Process, unified_memory_used};

    fn process(memory: Option<u64>) -> Process {
        Process {
            pid: 1,
            gpu_memory_usage: memory,
            username: None,
            command: None,
            full_command: Vec::new(),
            cpu_percent: 0.0,
            cpu_memory_usage: 0,
        }
    }

    #[test]
    fn an_idle_unified_device_reports_zero_bytes() {
        assert_eq!(unified_memory_used(&[]), Some(0));
    }

    #[test]
    fn a_process_that_reports_no_figure_does_not_become_zero() {
        let list = [process(None), process(None)];
        assert_eq!(unified_memory_used(&list), None);
    }

    #[test]
    fn the_known_figures_are_summed_over_the_unknown_ones() {
        let list = [process(None), process(Some(42)), process(None)];
        assert_eq!(unified_memory_used(&list), Some(42));
    }
}
