//! Argument parsing and dispatch.
//!
//! The flags are the ones gpustat has always accepted; only the layout and
//! wording of `--help` and parse errors come from `clap` now.

use std::io::{self, IsTerminal};

use clap::{ArgAction, Parser, ValueEnum};

use crate::format::{self, Codec, PowerMode, PrintOptions};
use crate::model::GpuStatCollection;
use crate::nvml::NvmlQuerier;

/// The device backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Backend {
    /// Pick the first available backend.
    Auto,
    /// NVIDIA, through NVML.
    Nvidia,
    /// AMD, through rocmi.
    Amd,
    /// Ascend, through npu-smi.
    Ascend,
}

#[derive(Debug, Parser)]
#[command(
    name = "gpustat",
    version,
    disable_version_flag = true,
    about = "Display the status of GPUs"
)]
struct Args {
    /// Target specific GPUs by index, for example `0,1`.
    #[arg(long, value_delimiter = ',', value_parser = clap::value_parser!(i32))]
    id: Vec<i32>,

    /// Device query backend.
    #[arg(long, default_value = "auto")]
    backend: Backend,

    /// Show every property: command, user, pid, fan speed, codec and power.
    #[arg(short = 'a', long)]
    show_all: bool,

    /// Show the command name of each running process.
    #[arg(short = 'c', long)]
    show_cmd: bool,

    /// Show each process' full command line and CPU usage.
    #[arg(short = 'f', long)]
    show_full_cmd: bool,

    /// Show the user of each running process.
    #[arg(short = 'u', long)]
    show_user: bool,

    /// Show the pid of each running process.
    #[arg(short = 'p', long)]
    show_pid: bool,

    /// Show the fan speed.
    #[arg(short = 'F', long, visible_alias = "show-fan")]
    show_fan_speed: bool,

    /// Show encoder and decoder utilization.
    #[arg(short = 'e', long, num_args = 0..=1, default_missing_value = "enc,dec")]
    show_codec: Option<Codec>,

    /// Show the power draw and the enforced limit.
    #[arg(short = 'P', long, num_args = 0..=1, default_missing_value = "draw,limit")]
    show_power: Option<PowerMode>,

    /// Print everything as JSON.
    #[arg(long)]
    json: bool,

    /// Watch mode: the seconds to wait between updates.
    #[arg(
        short = 'i',
        long,
        visible_alias = "watch",
        num_args = 0..=1,
        default_missing_value = "1.0",
        conflicts_with = "json"
    )]
    interval: Option<f64>,

    /// Suppress the header line.
    #[arg(long = "no-header", action = ArgAction::SetFalse, default_value_t = true)]
    show_header: bool,

    /// Width of the GPU name column; `0` hides the column.
    #[arg(long = "gpuname-width", value_name = "N")]
    name_width: Option<usize>,

    /// Trace every NVML call that failed.
    #[arg(long)]
    debug: bool,

    /// Do not show running process information.
    #[arg(long = "no-processes", action = ArgAction::SetFalse, default_value_t = true)]
    show_processes: bool,

    /// Force coloured output.
    #[arg(
        long = "force-color",
        visible_alias = "color",
        conflicts_with = "no_color"
    )]
    force_color: bool,

    /// Suppress coloured output.
    #[arg(long = "no-color", conflicts_with = "force_color")]
    no_color: bool,

    /// Print the version and exit.
    #[arg(short = 'v', long)]
    version: bool,
}

impl Args {
    /// The display switches, with `--show-all` expanded.
    fn options(&self) -> PrintOptions {
        let show_all = self.show_all;
        PrintOptions {
            show_cmd: self.show_cmd || show_all,
            show_full_cmd: self.show_full_cmd,
            show_user: self.show_user || show_all,
            show_pid: self.show_pid || show_all,
            show_fan_speed: self.show_fan_speed || show_all,
            show_codec: self.show_codec.or_else(|| show_all.then_some(Codec::Both)),
            show_power: self
                .show_power
                .or_else(|| show_all.then_some(PowerMode::Both)),
            show_header: self.show_header,
            show_processes: self.show_processes,
            force_color: self.force_color,
            no_color: self.no_color,
            name_width: self.name_width,
            eol: "\n",
        }
    }
}

/// Parse argv and run, returning the exit code.
///
/// Malformed argv makes clap print a usage message and exit on its own.
pub fn run() -> i32 {
    let args = Args::parse();
    if args.version {
        println!("gpustat {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    execute(args)
}

fn execute(mut args: Args) -> i32 {
    // The colour switches govern errors too, including the one that happens
    // before a query can run.
    let options = args.options();

    let querier = match args.backend {
        Backend::Amd => return unsupported("amd"),
        Backend::Ascend => return unsupported("ascend"),
        Backend::Auto | Backend::Nvidia => match NvmlQuerier::new() {
            Ok(querier) => querier,
            Err(error) => return report(format!("could not initialize NVML: {error}"), &options),
        },
    };

    // Without `--interval` the query runs once. An interval of `0` is treated
    // the same way, since a period of zero would spin; positive values are
    // clamped from below in the watch loop.
    let interval = args.interval.filter(|interval| *interval > 0.0);
    let debug = args.debug;
    let json = args.json;
    let ids = (!args.id.is_empty()).then_some(std::mem::take(&mut args.id));

    if let Some(interval) = interval {
        return crate::watch::run(interval, &querier, options, ids.as_deref(), debug);
    }

    let stats = match GpuStatCollection::new_query(&querier, ids.as_deref(), debug) {
        Ok(stats) => stats,
        Err(error) => return report(format!("could not query devices: {error}"), &options),
    };

    if let Some(message) = all_named_failed(ids.as_deref(), &stats) {
        return report(message, &options);
    }

    let mut stdout = io::stdout();
    let written = if json {
        format::print_json(&mut stdout, &stats).is_ok()
    } else {
        format::print_formatted(&mut stdout, &stats, &options).is_ok()
    };
    if written {
        0
    } else {
        1
    }
}

/// The error for a query that named indices and found none of them.
///
/// An index the user named cannot become a device for the rest of this process,
/// so an all-missing query is a hard failure rather than a table of `?`. With no
/// indices given, an unavailable device may turn up again, so that stays a
/// per-row warning. Shared with the watch loop.
pub(crate) fn all_named_failed(ids: Option<&[i32]>, stats: &GpuStatCollection) -> Option<String> {
    if ids.is_none() || !stats.none_available() {
        return None;
    }
    Some(format!("no GPU available: {}", stats.failure_report()))
}

fn unsupported(backend: &str) -> i32 {
    eprintln!("the {backend} backend is not built into this binary");
    1
}

/// Print an error to stderr, coloured according to stderr.
fn report(message: String, options: &PrintOptions) -> i32 {
    let pal = format::palette(options, io::stderr().is_terminal());
    eprintln!(
        "Error querying accelerator devices.\n{red}{message}{reset}",
        red = pal.red(),
        reset = pal.reset(),
    );
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GpuInfo, GpuStat};

    fn args(show_all: bool) -> Args {
        Args {
            id: Vec::new(),
            backend: Backend::Auto,
            show_all,
            show_cmd: false,
            show_full_cmd: false,
            show_user: false,
            show_pid: false,
            show_fan_speed: false,
            show_codec: None,
            show_power: None,
            json: false,
            interval: None,
            show_header: true,
            name_width: None,
            debug: false,
            show_processes: true,
            force_color: false,
            no_color: false,
            version: false,
        }
    }

    #[test]
    fn show_all_expands_to_every_switch() {
        let options = args(true).options();
        assert!(options.show_cmd);
        assert!(options.show_user);
        assert!(options.show_pid);
        assert!(options.show_fan_speed);
        assert!(matches!(options.show_codec, Some(Codec::Both)));
        assert!(matches!(options.show_power, Some(PowerMode::Both)));
        // The full command line is a separate, heavier switch.
        assert!(!options.show_full_cmd);
    }

    #[test]
    fn without_show_all_each_switch_is_independent() {
        let options = args(false).options();
        assert!(!options.show_cmd);
        assert!(!options.show_user);
        assert!(!options.show_pid);
        assert!(!options.show_fan_speed);
        assert!(options.show_codec.is_none());
        assert!(options.show_power.is_none());
        assert!(options.show_header);
        assert!(options.show_processes);
    }

    fn collection(gpus: Vec<GpuStat>) -> GpuStatCollection {
        GpuStatCollection {
            gpus,
            hostname: "h".to_string(),
            query_time: chrono::Local::now(),
            driver_version: None,
        }
    }

    fn missing(index: i32) -> GpuStat {
        GpuStat {
            info: GpuInfo {
                index,
                name: "a supplied argument was invalid".to_string(),
                ..Default::default()
            },
            available: false,
        }
    }

    fn present(index: i32) -> GpuStat {
        GpuStat {
            info: GpuInfo {
                index,
                name: "NVIDIA GeForce RTX 4090".to_string(),
                ..Default::default()
            },
            available: true,
        }
    }

    #[test]
    fn naming_only_missing_indices_fails() {
        let stats = collection(vec![missing(99)]);
        assert_eq!(
            all_named_failed(Some(&[99]), &stats),
            Some("no GPU available: index 99: a supplied argument was invalid".to_string())
        );
    }

    #[test]
    fn naming_missing_indices_alongside_a_good_one_still_shows_a_table() {
        let stats = collection(vec![present(0), missing(99)]);
        assert_eq!(all_named_failed(Some(&[0, 99]), &stats), None);
    }

    #[test]
    fn an_unnamed_missing_index_is_not_a_failure() {
        let stats = collection(vec![missing(0)]);
        assert_eq!(all_named_failed(None, &stats), None);
    }

    #[test]
    fn explicit_switches_survive_show_all() {
        let mut parsed = args(true);
        parsed.name_width = Some(20);
        parsed.show_full_cmd = true;
        let options = parsed.options();
        assert_eq!(options.name_width, Some(20));
        assert!(options.show_full_cmd);
    }
}
