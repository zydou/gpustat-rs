//! Rendering: the table, and its JSON form.
//!
//! `render` builds the text; `print_formatted` and `print_json` write it.

use std::fmt;
use std::io::{self, IsTerminal, Write};

use chrono::Local;

use crate::model::{GpuStat, GpuStatCollection, Process, NOT_SUPPORTED};
use crate::palette::Palette;
use crate::util;

/// What `-e/--show-codec` selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Codec {
    /// Encoder utilization.
    #[value(name = "enc")]
    Enc,
    /// Decoder utilization.
    #[value(name = "dec")]
    Dec,
    /// Encoder and decoder utilization.
    #[value(name = "enc,dec", alias = "dec,enc")]
    Both,
}

impl Codec {
    pub fn shows_enc(self) -> bool {
        matches!(self, Self::Enc | Self::Both)
    }

    pub fn shows_dec(self) -> bool {
        matches!(self, Self::Dec | Self::Both)
    }
}

/// What `-P/--show-power` selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PowerMode {
    /// The measured draw.
    #[value(name = "draw")]
    Draw,
    /// The enforced limit.
    #[value(name = "limit")]
    Limit,
    /// The measured draw and the enforced limit.
    #[value(name = "draw,limit", alias = "limit,draw")]
    Both,
}

impl PowerMode {
    pub fn shows_draw(self) -> bool {
        matches!(self, Self::Draw | Self::Both)
    }

    pub fn shows_limit(self) -> bool {
        matches!(self, Self::Limit | Self::Both)
    }
}

/// Which parts of the table to render.
#[derive(Debug, Clone, Copy)]
pub struct PrintOptions {
    pub show_cmd: bool,
    pub show_full_cmd: bool,
    pub show_user: bool,
    pub show_pid: bool,
    pub show_fan_speed: bool,
    pub show_codec: Option<Codec>,
    pub show_power: Option<PowerMode>,
    pub show_header: bool,
    pub show_processes: bool,
    /// Force colour even when stdout is not a terminal.
    pub force_color: bool,
    /// Never colour, even when stdout is a terminal.
    pub no_color: bool,
    /// Fixed GPU name width; `0` hides the column, `None` sizes it to the
    /// longest name.
    pub name_width: Option<usize>,
    /// Line ending. Watch mode passes `"\u{1b}[K\n"` to clear each line.
    pub eol: &'static str,
}

impl Default for PrintOptions {
    fn default() -> Self {
        Self {
            show_cmd: false,
            show_full_cmd: false,
            show_user: false,
            show_pid: false,
            show_fan_speed: false,
            show_codec: None,
            show_power: None,
            show_header: true,
            show_processes: true,
            force_color: false,
            no_color: false,
            name_width: None,
            eol: "\n",
        }
    }
}

/// The palette for one stream.
///
/// An explicit flag wins over the environment, which wins over the tty check.
pub fn palette(opts: &PrintOptions, tty: bool) -> Palette {
    choose(
        opts.force_color,
        opts.no_color,
        env_non_empty("FORCE_COLOR") || env_non_empty("CLICOLOR_FORCE"),
        env_non_empty("NO_COLOR"),
        tty,
    )
}

/// The palette for stdout.
pub fn palette_for(opts: &PrintOptions) -> Palette {
    palette(opts, io::stdout().is_terminal())
}

/// The colour decision, split out so it can be exercised without touching the
/// process environment.
fn choose(force: bool, no_color: bool, force_env: bool, no_color_env: bool, tty: bool) -> Palette {
    if force {
        Palette::enabled()
    } else if no_color || no_color_env {
        Palette::disabled()
    } else if force_env || tty {
        Palette::enabled()
    } else {
        Palette::disabled()
    }
}

fn env_non_empty(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.is_empty())
}

/// The colour of each column, chosen from the device's state.
#[derive(Debug, Clone, Copy)]
struct Colors {
    name: &'static str,
    temp: &'static str,
    fan: &'static str,
    util: &'static str,
    enc: &'static str,
    dec: &'static str,
    power_draw: &'static str,
    power_limit: &'static str,
    mem_used: &'static str,
    mem_total: &'static str,
}

fn colors_for(stat: &GpuStat, pal: Palette) -> Colors {
    // Plain under the threshold, bold above it, grey where there is no figure.
    let level = |under: &'static str,
                 over: &'static str,
                 value: Option<u32>,
                 threshold: u32|
     -> &'static str {
        match value {
            Some(value) if value < threshold => under,
            Some(_) => over,
            None => pal.bold_black(),
        }
    };

    // Grey when there is no limit to compare the draw against.
    let power_draw = match (stat.info.power_limit, stat.info.power_draw) {
        (Some(limit), Some(draw)) if limit > 0 => {
            let ratio = draw as f64 / limit as f64;
            if ratio < 0.4 {
                pal.magenta()
            } else {
                pal.bold_magenta()
            }
        }
        _ => pal.bold_black(),
    };

    Colors {
        name: if stat.available {
            pal.blue()
        } else {
            pal.red()
        },
        temp: level(pal.red(), pal.bold_red(), stat.info.temperature, 50),
        fan: level(pal.cyan(), pal.bold_cyan(), stat.info.fan_speed, 30),
        util: level(pal.green(), pal.bold_green(), stat.info.utilization, 30),
        enc: level(pal.green(), pal.bold_green(), stat.info.utilization_enc, 50),
        dec: level(pal.green(), pal.bold_green(), stat.info.utilization_dec, 50),
        power_draw,
        power_limit: pal.magenta(),
        mem_used: if stat.available {
            pal.bold_yellow()
        } else {
            pal.bold_black()
        },
        mem_total: if stat.available {
            pal.yellow()
        } else {
            pal.bold_black()
        },
    }
}

/// Wrap `text` in `color`. A disabled palette makes both codes empty, so there
/// is no need to branch.
fn colored(pal: Palette, color: &'static str, text: impl fmt::Display) -> String {
    format!("{color}{text}{}", pal.reset())
}

/// A metric, or `?` where NVML had none.
fn metric<T: fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "?".to_string(), |value| value.to_string())
}

/// The header line.
fn header(stats: &GpuStatCollection, pal: Palette, name_width: usize) -> String {
    let host = format!("{:<width$}", stats.hostname, width = name_width + 3);
    format!(
        "{}{}{}  {}  {}{}{}",
        pal.bold_white(),
        host,
        pal.reset(),
        header_time(&stats.query_time),
        pal.bold_black(),
        stats.driver_version.as_deref().unwrap_or("unknown"),
        pal.reset(),
    )
}

/// The header timestamp.
///
/// `chrono`'s format tables are English regardless of `LANG`, which is what a
/// shared dashboard wants.
fn header_time(query_time: &chrono::DateTime<Local>) -> String {
    query_time.format("%a %b %e %H:%M:%S %Y").to_string()
}

/// One row of the table.
fn stat_to_string(
    stat: &GpuStat,
    opts: &PrintOptions,
    pal: Palette,
    name_width: Option<usize>,
) -> String {
    let info = &stat.info;
    let c = colors_for(stat, pal);
    let mut out = String::new();

    out.push_str(&colored(pal, pal.cyan(), format!("[{}]", info.index)));
    out.push(' ');

    if let Some(width) = name_width {
        let name = util::shorten_left(&info.name, width);
        out.push_str(&colored(pal, c.name, format!("{name:<width$}")));
        out.push_str(" |");
    }

    out.push_str(&colored(
        pal,
        c.temp,
        format!("{:>3}°C", metric(info.temperature)),
    ));
    out.push_str(", ");

    if opts.show_fan_speed {
        out.push_str(&colored(
            pal,
            c.fan,
            format!("{:>3} %", metric(info.fan_speed)),
        ));
        out.push_str(", ");
    }

    out.push_str(&colored(
        pal,
        c.util,
        format!("{:>3} %", metric(info.utilization)),
    ));

    if let Some(codec) = opts.show_codec {
        let mut halves: Vec<String> = Vec::new();
        if codec.shows_enc() {
            halves.push(colored(
                pal,
                c.enc,
                format!("E: {:>3} %", metric(info.utilization_enc)),
            ));
        }
        if codec.shows_dec() {
            halves.push(colored(
                pal,
                c.dec,
                format!("D: {:>3} %", metric(info.utilization_dec)),
            ));
        }
        if !halves.is_empty() {
            out.push(' ');
            out.push_str(&format!("({})", halves.join("  ")));
        }
    }

    if let Some(power) = opts.show_power {
        out.push_str(",  ");
        let draw = power.shows_draw();
        let limit = power.shows_limit();
        if draw {
            out.push_str(&colored(
                pal,
                c.power_draw,
                format!("{:>3}", metric(info.power_draw)),
            ));
        }
        if draw && limit {
            out.push_str(" / ");
        }
        if limit {
            out.push_str(&colored(
                pal,
                c.power_limit,
                format!("{:>3} W", metric(info.power_limit)),
            ));
        }
    }

    out.push_str(" | ");
    out.push_str(&colored(
        pal,
        c.mem_used,
        format!("{:>5}", metric(info.memory_used)),
    ));
    out.push_str(" / ");
    out.push_str(&colored(
        pal,
        c.mem_total,
        format!("{:>5}", metric(info.memory_total)),
    ));
    out.push_str(" MB");

    if !opts.show_processes {
        return out;
    }
    // The gutter is only worth drawing when something follows it.
    out.push_str(" |");

    match &info.processes {
        // NVML could not enumerate processes at all.
        None => out.push_str(&format!(" ({NOT_SUPPORTED})")),
        Some(processes) => {
            for process in processes {
                out.push_str(&format!(" {}", process_repr(process, opts, pal)));
            }
            if opts.show_full_cmd && !processes.is_empty() {
                let mut lines: Vec<String> = processes
                    .iter()
                    .map(|process| full_process_info(process, pal))
                    .collect();
                if let Some(last) = lines.last_mut() {
                    // The bottom branch of the tree is drawn closed.
                    *last = last.replacen('\u{251c}', "\u{2514}", 1);
                }
                out.push_str(opts.eol);
                out.push_str(&lines.join(opts.eol));
            }
        }
    }

    out
}

/// The inline per-process summary: `user:cmd/pid(21M)`.
fn process_repr(process: &Process, opts: &PrintOptions, pal: Palette) -> String {
    // The username shows whenever the command would stand alone.
    let show_user = opts.show_user || !opts.show_cmd;
    let mut out = String::new();
    if show_user {
        out.push_str(&colored(
            pal,
            pal.bold_black(),
            process.username.as_deref().unwrap_or("--"),
        ));
    }
    if opts.show_cmd {
        if !out.is_empty() {
            out.push(':');
        }
        out.push_str(&colored(
            pal,
            pal.cyan(),
            process.command.as_deref().unwrap_or("--"),
        ));
    }
    if opts.show_pid {
        out.push('/');
        out.push_str(&process.pid.to_string());
    }
    out.push('(');
    let memory = match process.gpu_memory_usage {
        Some(megabytes) => format!("{megabytes}M"),
        None => "?".to_string(),
    };
    out.push_str(&colored(pal, pal.yellow(), memory));
    out.push(')');
    out
}

/// The tree line under a process: ` ├─ 12345 ( 109%, 2567MB): /bin/python x.py`.
fn full_process_info(process: &Process, pal: Palette) -> String {
    format!(
        "{reset} \u{251c}─ {pid:>6} {reset}({reset}{green}{cpu:>4.0}%{reset}, {yellow}{mem:>6}{reset}): {reset}{dim}{command}{reset}",
        reset = pal.reset(),
        green = pal.green(),
        yellow = pal.yellow(),
        dim = pal.dim(),
        pid = process.pid,
        cpu = process.cpu_percent,
        mem = util::bytes2human(process.cpu_memory_usage),
        command = util::prettify_commandline(&process.full_command, pal.cyan(), pal.dim()),
    )
}

/// The table as text.
pub fn render(stats: &GpuStatCollection, opts: &PrintOptions) -> String {
    let pal = palette_for(opts);
    let name_width = match opts.name_width {
        Some(0) => None,                  // hide the column
        Some(width) => Some(width),       // fixed
        None => Some(stats.name_width()), // size to the longest name
    };

    let mut out = String::new();
    if opts.show_header {
        out.push_str(&header(stats, pal, name_width.unwrap_or(0)));
        out.push_str(opts.eol);
    }
    for stat in &stats.gpus {
        out.push_str(&stat_to_string(stat, opts, pal, name_width));
        out.push_str(opts.eol);
    }
    if stats.gpus.is_empty() {
        out.push_str(&format!(
            "{}(no GPUs are available){}",
            pal.yellow(),
            pal.reset()
        ));
        out.push_str(opts.eol);
    }
    out
}

/// Write the table.
pub fn print_formatted(
    out: &mut impl Write,
    stats: &GpuStatCollection,
    opts: &PrintOptions,
) -> io::Result<()> {
    let text = render(stats, opts);
    out.write_all(text.as_bytes())?;
    out.flush()
}

/// A query as pretty-printed JSON.
pub fn json(stats: &GpuStatCollection) -> serde_json::Result<String> {
    serde_json::to_string_pretty(stats)
}

/// Write a query as JSON followed by a newline.
pub fn print_json(out: &mut impl Write, stats: &GpuStatCollection) -> io::Result<()> {
    let mut text = json(stats).map_err(io::Error::other)?;
    text.push('\n');
    out.write_all(text.as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GpuInfo, GpuStat};
    use chrono::{NaiveDate, TimeZone};

    fn stat(info: GpuInfo, available: bool) -> GpuStat {
        GpuStat { info, available }
    }

    fn full_info() -> GpuInfo {
        GpuInfo {
            index: 0,
            name: "NVIDIA GeForce RTX 4090".to_string(),
            uuid: Some("GPU-0000".to_string()),
            temperature: Some(61),
            fan_speed: Some(70),
            memory_used: Some(21575),
            memory_total: Some(24564),
            memory_unified: false,
            utilization: Some(100),
            utilization_enc: Some(0),
            utilization_dec: Some(0),
            power_draw: Some(209),
            power_limit: Some(450),
            processes: Some(vec![Process {
                pid: 461343,
                gpu_memory_usage: Some(21552),
                username: Some("root".to_string()),
                command: Some("VLLM::Worker_TP0".to_string()),
                full_command: vec!["VLLM::Worker_TP0".to_string()],
                cpu_percent: 109.3,
                cpu_memory_usage: 2_691_743_744,
            }]),
        }
    }

    fn fixture_time() -> chrono::DateTime<Local> {
        Local
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2026, 10, 26)
                    .unwrap()
                    .and_hms_opt(12, 0, 35)
                    .unwrap(),
            )
            .single()
            .unwrap()
    }

    fn fixture(opts: PrintOptions, driver: Option<&str>) -> (String, GpuStatCollection) {
        let stats = GpuStatCollection {
            gpus: vec![stat(full_info(), true)],
            hostname: "dnkt".to_string(),
            query_time: fixture_time(),
            driver_version: driver.map(str::to_string),
        };
        (render(&stats, &opts), stats)
    }

    /// The option set with colours off, so text comparisons stay readable.
    fn plain(mut opts: PrintOptions) -> PrintOptions {
        opts.no_color = true;
        opts
    }

    #[test]
    fn codec_and_power_select_their_columns() {
        assert!(Codec::Enc.shows_enc() && !Codec::Enc.shows_dec());
        assert!(Codec::Dec.shows_dec() && !Codec::Dec.shows_enc());
        assert!(Codec::Both.shows_enc() && Codec::Both.shows_dec());

        assert!(PowerMode::Draw.shows_draw() && !PowerMode::Draw.shows_limit());
        assert!(PowerMode::Limit.shows_limit() && !PowerMode::Limit.shows_draw());
        assert!(PowerMode::Both.shows_draw() && PowerMode::Both.shows_limit());
    }

    #[test]
    fn the_colour_decision_follows_the_documented_precedence() {
        let coloured = |palette: Palette| -> bool { !palette.reset().is_empty() };
        // An explicit --force-color wins over everything, even NO_COLOR.
        assert!(coloured(choose(true, false, false, true, false)));
        // --no-color and NO_COLOR both beat FORCE_COLOR and the tty check.
        assert!(!coloured(choose(false, true, true, false, true)));
        assert!(!coloured(choose(false, false, true, true, true)));
        // A terminal is coloured unless told otherwise.
        assert!(coloured(choose(false, false, false, false, true)));
        assert!(!coloured(choose(false, false, false, false, false)));
        // FORCE_COLOR colours a pipe.
        assert!(coloured(choose(false, false, true, false, false)));
    }

    #[test]
    fn a_forced_palette_wraps_every_field() {
        let opts = PrintOptions {
            show_fan_speed: true,
            show_codec: Some(Codec::Both),
            show_power: Some(PowerMode::Both),
            show_user: true,
            show_cmd: true,
            show_pid: true,
            show_header: false,
            ..Default::default()
        };
        let text = stat_to_string(
            &stat(full_info(), true),
            &opts,
            Palette::enabled(),
            Some(30),
        );

        assert!(text.contains("\u{1b}[36m[0]\u{1b}[0m"), "text = {text:?}");
        assert!(
            text.contains("\u{1b}[34mNVIDIA GeForce RTX 4090"),
            "text = {text:?}"
        );
        assert!(text.contains("\u{1b}[1;31m 61°C\u{1b}[0m"));
        assert!(text.contains("\u{1b}[1;36m 70 %\u{1b}[0m"));
        assert!(text.contains("\u{1b}[1;32m100 %\u{1b}[0m"));
        assert!(
            text.contains("\u{1b}[1;35m209\u{1b}[0m / \u{1b}[35m450 W\u{1b}[0m"),
            "text = {text:?}"
        );
        assert!(text.contains("\u{1b}[1;33m21575\u{1b}[0m / \u{1b}[33m24564\u{1b}[0m MB"));
        assert!(
            text.contains("\u{1b}[1;30mroot\u{1b}[0m:\u{1b}[36mVLLM::Worker_TP0\u{1b}[0m/461343")
        );
        assert!(text.contains("\u{1b}[33m21552M\u{1b}[0m"));
    }

    #[test]
    fn a_disabled_palette_emits_no_escapes() {
        let opts = plain(PrintOptions::default());
        let (text, _) = fixture(opts, Some("575.57.08"));
        assert!(!text.contains('\u{1b}'), "text = {text:?}");
    }

    #[test]
    fn plain_text_layout() {
        let (text, _) = fixture(plain(PrintOptions::default()), Some("575.57.08"));
        let row = text.lines().nth(1).unwrap_or("");
        assert_eq!(
            row,
            "[0] NVIDIA GeForce RTX 4090 | 61°C, 100 % | 21575 / 24564 MB | root(21552M)"
        );
    }

    #[test]
    fn width_zero_hides_the_name_column() {
        let opts = plain(PrintOptions {
            name_width: Some(0),
            show_header: false,
            show_processes: false,
            ..Default::default()
        });
        let (text, _) = fixture(opts, None);
        assert!(text.starts_with("[0]  61°C"), "text = {text:?}");
        assert!(text.trim_end().ends_with("24564 MB"), "text = {text:?}");
    }

    #[test]
    fn a_wide_name_column_pads_to_the_requested_width() {
        let opts = plain(PrintOptions {
            name_width: Some(30),
            show_header: false,
            show_processes: false,
            ..Default::default()
        });
        let (text, _) = fixture(opts, None);
        // "[0] " plus the 30-character name column.
        assert_eq!(text.find(" |").unwrap(), 34);
    }

    #[test]
    fn a_narrow_name_column_truncates_from_the_left() {
        let opts = plain(PrintOptions {
            name_width: Some(8),
            show_header: false,
            show_processes: false,
            ..Default::default()
        });
        let (text, _) = fixture(opts, None);
        assert!(text.starts_with("[0] …TX 4090 |"), "text = {text:?}");
    }

    #[test]
    fn every_optional_column_is_appended_in_order() {
        let opts = plain(PrintOptions {
            show_fan_speed: true,
            show_codec: Some(Codec::Both),
            show_power: Some(PowerMode::Both),
            show_cmd: true,
            show_user: true,
            show_pid: true,
            show_header: false,
            show_processes: true,
            ..Default::default()
        });
        let (text, _) = fixture(opts, None);
        assert_eq!(
            text.trim_end(),
            "[0] NVIDIA GeForce RTX 4090 | 61°C,  70 %, 100 % (E:   0 %  D:   0 %),  209 / 450 W | 21575 / 24564 MB | root:VLLM::Worker_TP0/461343(21552M)"
        );
    }

    #[test]
    fn power_limit_alone_omits_the_draw() {
        let opts = plain(PrintOptions {
            show_power: Some(PowerMode::Limit),
            show_header: false,
            show_processes: false,
            ..Default::default()
        });
        let (text, _) = fixture(opts, None);
        assert!(text.contains(",  450 W"), "text = {text:?}");
        assert!(!text.contains("209"), "text = {text:?}");
    }

    #[test]
    fn power_draw_alone_omits_the_limit() {
        let opts = plain(PrintOptions {
            show_power: Some(PowerMode::Draw),
            show_header: false,
            show_processes: false,
            ..Default::default()
        });
        let (text, _) = fixture(opts, None);
        assert!(text.contains(",  209 |"), "text = {text:?}");
        assert!(!text.contains("450 W"), "text = {text:?}");
    }

    #[test]
    fn a_single_codec_half_leaves_no_empty_slot() {
        for codec in [Codec::Enc, Codec::Dec] {
            let opts = plain(PrintOptions {
                show_codec: Some(codec),
                show_header: false,
                show_processes: false,
                ..Default::default()
            });
            let (text, _) = fixture(opts, None);
            let want = if codec == Codec::Enc {
                " (E:   0 %)"
            } else {
                " (D:   0 %)"
            };
            assert!(text.contains(want), "text = {text:?}");
            assert!(!text.contains("  )"), "text = {text:?}");
        }
    }

    #[test]
    fn hiding_processes_drops_the_pipe_too() {
        let opts = plain(PrintOptions {
            show_processes: false,
            show_header: false,
            ..Default::default()
        });
        let (text, _) = fixture(opts, None);
        assert!(text.trim_end().ends_with("24564 MB"), "text = {text:?}");
        assert!(!text.contains("root"), "text = {text:?}");
    }

    #[test]
    fn a_missing_process_list_is_marked() {
        let stats = GpuStatCollection {
            gpus: vec![stat(
                GpuInfo {
                    index: 0,
                    name: "GPU".to_string(),
                    ..Default::default()
                },
                true,
            )],
            hostname: "h".to_string(),
            query_time: fixture_time(),
            driver_version: None,
        };
        let opts = plain(PrintOptions {
            show_header: false,
            ..Default::default()
        });
        let text = render(&stats, &opts);
        // Missing figures become `?`, and a device that cannot list its
        // processes says so instead of showing an empty pipe.
        assert!(text.starts_with("[0] GPU |"), "text = {text:?}");
        assert!(text.contains("?°C"), "text = {text:?}");
        assert!(
            text.trim_end().ends_with("| (Not Supported)"),
            "text = {text:?}"
        );
    }

    #[test]
    fn full_command_lines_form_a_tree() {
        let opts = plain(PrintOptions {
            show_full_cmd: true,
            show_header: false,
            ..Default::default()
        });
        let (text, _) = fixture(opts, None);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "text = {text:?}");
        // A single process is the last, closed branch.
        assert_eq!(lines[1], " └─ 461343 ( 109%, 2567MB): VLLM::Worker_TP0");
    }

    #[test]
    fn full_command_colours_only_the_base_name() {
        let text = full_process_info(
            &Process {
                pid: 461343,
                gpu_memory_usage: None,
                username: None,
                command: None,
                full_command: vec!["VLLM::Worker_TP0".to_string()],
                cpu_percent: 109.3,
                cpu_memory_usage: 2_691_743_744,
            },
            Palette::enabled(),
        );
        // The base name is cyan, and the dim wrapper is opened exactly once.
        assert!(
            text.contains("\u{1b}[2m\u{1b}[36mVLLM::Worker_TP0\u{1b}[2m"),
            "text = {text:?}"
        );
        assert!(text.contains("\u{1b}[32m 109%\u{1b}[0m"));
        assert!(text.contains("\u{1b}[33m2567MB\u{1b}[0m"));
    }

    #[test]
    fn a_path_in_full_command_keeps_its_directory_dim() {
        let info = GpuInfo {
            index: 0,
            name: "GPU".to_string(),
            processes: Some(vec![Process {
                pid: 7,
                gpu_memory_usage: None,
                username: None,
                command: None,
                full_command: vec!["/usr/bin/python3".to_string(), "train.py".to_string()],
                cpu_percent: 0.0,
                cpu_memory_usage: 0,
            }]),
            ..Default::default()
        };
        let stats = GpuStatCollection {
            gpus: vec![stat(info, true)],
            hostname: "h".to_string(),
            query_time: fixture_time(),
            driver_version: None,
        };
        let opts = plain(PrintOptions {
            show_full_cmd: true,
            show_header: false,
            ..Default::default()
        });
        let text = render(&stats, &opts);
        assert!(text.contains("└─"), "text = {text:?}");
        assert!(
            text.contains("/usr/bin/python3 train.py"),
            "text = {text:?}"
        );
    }

    #[test]
    fn placeholders_fill_missing_fields() {
        let info = GpuInfo {
            index: 0,
            name: "GPU".to_string(),
            processes: Some(vec![Process {
                pid: 1,
                gpu_memory_usage: None,
                username: None,
                command: None,
                full_command: vec!["?".to_string()],
                cpu_percent: 0.0,
                cpu_memory_usage: 0,
            }]),
            ..Default::default()
        };
        let stats = GpuStatCollection {
            gpus: vec![stat(info, true)],
            hostname: "h".to_string(),
            query_time: fixture_time(),
            driver_version: None,
        };
        let opts = plain(PrintOptions {
            show_header: false,
            ..Default::default()
        });
        assert!(render(&stats, &opts).contains(" | --(?)"));
    }

    #[test]
    fn a_failed_device_keeps_its_error_as_its_name() {
        let stats = GpuStatCollection {
            gpus: vec![stat(
                GpuInfo {
                    index: 7,
                    name: "NVML Device Not Found".to_string(),
                    ..Default::default()
                },
                false,
            )],
            hostname: "h".to_string(),
            query_time: fixture_time(),
            driver_version: None,
        };
        let opts = plain(PrintOptions {
            show_header: false,
            ..Default::default()
        });
        let text = render(&stats, &opts);
        assert!(
            text.starts_with("[7] NVML Device Not Found |  ?°C"),
            "text = {text:?}"
        );

        let json = json(&stats).unwrap();
        assert!(json.contains("\"index\": 7"));
        assert!(json.contains("\"name\": \"NVML Device Not Found\""));
    }

    #[test]
    fn json_keeps_the_gpustat_key_names() {
        let (_, stats) = fixture(plain(PrintOptions::default()), Some("575.57.08"));
        let text = json(&stats).unwrap();

        for key in [
            "\"hostname\"",
            "\"driver_version\"",
            "\"query_time\"",
            "\"gpus\"",
            "\"index\"",
            "\"name\"",
            "\"uuid\"",
            "\"temperature.gpu\"",
            "\"fan.speed\"",
            "\"memory.used\"",
            "\"memory.total\"",
            "\"memory.unified\"",
            "\"utilization.gpu\"",
            "\"utilization.enc\"",
            "\"utilization.dec\"",
            "\"power.draw\"",
            "\"enforced.power.limit\"",
            "\"processes\"",
            "\"cpu_percent\"",
            "\"cpu_memory_usage\"",
        ] {
            assert!(text.contains(key), "missing {key} in {text}");
        }
        assert!(text.contains("\"memory.unified\": false"));
        assert!(text.contains("\"cpu_percent\": 109.3"));
        assert!(text.contains("\"cpu_memory_usage\": 2691743744"));
    }

    #[test]
    fn json_is_valid_and_round_trips() {
        let (_, stats) = fixture(plain(PrintOptions::default()), Some("575.57.08"));
        let text = json(&stats).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["hostname"], "dnkt");
        assert_eq!(value["gpus"][0]["memory.used"], 21575);
        assert_eq!(
            value["gpus"][0]["processes"][0]["command"],
            "VLLM::Worker_TP0"
        );
    }

    #[test]
    fn json_round_trips_non_ascii_and_quotes() {
        let stats = GpuStatCollection {
            gpus: vec![stat(
                GpuInfo {
                    index: 0,
                    name: "naïve \"gpu\"".to_string(),
                    ..Default::default()
                },
                true,
            )],
            hostname: "h".to_string(),
            query_time: fixture_time(),
            driver_version: None,
        };
        let text = json(&stats).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["gpus"][0]["name"], "naïve \"gpu\"");
    }

    #[test]
    fn an_empty_collection_reports_itself() {
        let stats = GpuStatCollection {
            gpus: Vec::new(),
            hostname: "h".to_string(),
            query_time: fixture_time(),
            driver_version: None,
        };
        let opts = plain(PrintOptions {
            show_header: false,
            ..Default::default()
        });
        assert_eq!(render(&stats, &opts), "(no GPUs are available)\n");
    }

    #[test]
    fn the_header_shows_host_time_and_driver() {
        let (text, _) = fixture(plain(PrintOptions::default()), Some("575.57.08"));
        let header = text.lines().next().unwrap_or("");
        assert!(header.contains("dnkt"), "header = {header:?}");
        assert!(
            header.contains("Mon Oct 26 12:00:35 2026"),
            "header = {header:?}"
        );
        assert!(header.ends_with("575.57.08"), "header = {header:?}");
        assert!(!header.ends_with(' '), "header = {header:?}");
    }

    #[test]
    fn a_missing_driver_version_reads_unknown() {
        let (text, _) = fixture(plain(PrintOptions::default()), None);
        let header = text.lines().next().unwrap_or("");
        assert!(header.ends_with("unknown"), "header = {header:?}");
    }

    #[test]
    fn the_header_pads_the_hostname_to_the_name_width() {
        let (text, _) = fixture(
            plain(PrintOptions {
                name_width: Some(10),
                ..Default::default()
            }),
            Some("575.57.08"),
        );
        // name_width plus the "[n]" gutter, then two spaces before the time.
        assert_eq!(text.lines().next().unwrap_or("").find("Mon"), Some(15));
    }

    #[test]
    fn the_header_can_be_suppressed() {
        let (text, _) = fixture(
            plain(PrintOptions {
                show_header: false,
                ..Default::default()
            }),
            Some("575.57.08"),
        );
        assert!(text.starts_with("[0]"), "text = {text:?}");
    }

    #[test]
    fn watch_mode_uses_a_clearing_line_ending() {
        let (text, _) = fixture(
            plain(PrintOptions {
                eol: "\u{1b}[K\n",
                show_header: false,
                ..Default::default()
            }),
            Some("575.57.08"),
        );
        assert!(text.ends_with("\u{1b}[K\n"), "text = {text:?}");
        assert!(!text.contains("\u{1b}[K\u{1b}[K\n"));
    }
}
