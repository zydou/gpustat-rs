//! Watch mode: redraw the table on a steady cadence.

use std::io::Write;
use std::thread;
use std::time::{Duration, Instant};

use crate::format::{self, PrintOptions};
use crate::model::GpuStatCollection;
use crate::nvml::Querier;
use crate::palette;

/// The shortest period, so an interval of `0` cannot spin.
const MINIMUM_INTERVAL: f64 = 0.1;

/// Redraw until the process is terminated, or until every named index is
/// missing.
///
/// Each frame rewrites its own lines rather than using the alternate screen,
/// so an interrupt leaves the last table on screen and needs no restoring.
pub fn run(
    interval: f64,
    querier: &dyn Querier,
    options: PrintOptions,
    ids: Option<&[i32]>,
    debug: bool,
) -> i32 {
    let period = Duration::from_secs_f64(interval.max(MINIMUM_INTERVAL));

    let mut options = options;
    options.eol = palette::CLEAR_LINE;

    let mut due = Instant::now();
    loop {
        if let Ok(stats) = GpuStatCollection::new_query(querier, ids, debug) {
            // The same rule as a one-shot query: a named index never turns into
            // a device for the rest of this process, so fail rather than
            // redrawing an all-missing table forever.
            if let Some(message) = crate::cli::all_named_failed(ids, &stats) {
                eprintln!("{message}");
                return 1;
            }
            draw(&stats, &options);
        }

        // A steady cadence, whatever the query took.
        due += period;
        let now = Instant::now();
        if now < due {
            thread::sleep(due - now);
        }
    }
}

/// One frame: to the top-left corner, the table, then whatever the previous
/// frame left below it.
fn draw(stats: &GpuStatCollection, options: &PrintOptions) {
    let text = format::render(stats, options);
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(palette::MOVE_HOME.as_bytes());
    let _ = out.write_all(text.as_bytes());
    let _ = out.write_all(palette::CLEAR_EOS.as_bytes());
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GpuInfo, GpuStat};

    #[test]
    fn watch_frames_clear_each_line_and_dont_double_clear() {
        let stats = GpuStatCollection {
            gpus: vec![GpuStat {
                info: GpuInfo {
                    index: 0,
                    name: "GPU".to_string(),
                    ..Default::default()
                },
                available: true,
            }],
            hostname: "h".to_string(),
            query_time: chrono::Local::now(),
            driver_version: None,
        };
        let mut options = PrintOptions {
            show_header: false,
            show_processes: false,
            no_color: true,
            ..Default::default()
        };
        options.eol = palette::CLEAR_LINE;

        let line = palette::CLEAR_LINE;
        let text = format::render(&stats, &options);
        // Every line is cleared, and the clearing escape is not repeated.
        assert!(text.ends_with(line), "text = {text:?}");
        assert!(!text.contains(&format!("{line}{line}")), "text = {text:?}");
    }

    #[test]
    fn a_steady_cadence_never_lags_behind() {
        let period = Duration::from_secs_f64(1.0);
        let mut due = Instant::now() - period;
        // The query took longer than the period, so the next frame is immediate.
        due += period;
        let now = Instant::now();
        assert!(now >= due);

        due += period;
        assert!(now < due);
        assert!(due - now > Duration::ZERO);
    }

    #[test]
    fn the_minimum_interval_clamps_short_periods() {
        assert_eq!(
            Duration::from_secs_f64(0.0f64.max(MINIMUM_INTERVAL)),
            Duration::from_millis(100)
        );
        assert_eq!(
            Duration::from_secs_f64(0.05f64.max(MINIMUM_INTERVAL)),
            Duration::from_millis(100)
        );
        assert_eq!(
            Duration::from_secs_f64(1.0f64.max(MINIMUM_INTERVAL)),
            Duration::from_secs(1)
        );
    }
}
