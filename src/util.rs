//! Text and path helpers shared by the renderer and the process reader.

use std::fs;
use std::path::Path;

/// Divisor used to turn NVML byte counters into megabytes.
pub const MB: u64 = 1024 * 1024;

/// A byte count as a short human-readable magnitude: `21_552` -> `"21KB"`.
///
/// Divided by 1024 while it still needs four digits, so the figure always fits
/// a narrow column and is never rounded away to a single digit.
pub fn bytes2human(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes;
    let mut unit = 0;
    while value > 9999 && unit + 1 < UNITS.len() {
        value /= 1024;
        unit += 1;
    }
    format!("{value}{}", UNITS[unit])
}

/// Keep the last `width` characters, prefixing an ellipsis when something was
/// cut. The ellipsis counts as one of the `width` characters.
pub fn shorten_left(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let len = text.chars().count();
    if len <= width {
        return text.to_string();
    }
    let mut out = String::from("\u{2026}");
    for c in text.chars().skip(len - (width - 1)) {
        out.push(c);
    }
    out
}

/// `cmdline` with the executable's base name highlighted.
///
/// `color_cmd` highlights the base name and `color_text` the surrounding
/// text. A base name with no directory is wrapped in `color_cmd` only, so no
/// style is opened and left unclosed.
pub fn prettify_commandline(cmdline: &[String], color_cmd: &str, color_text: &str) -> String {
    let Some(first) = cmdline.first() else {
        return String::new();
    };
    let (dir, base) = match first.rsplit_once('/') {
        Some((dir, base)) if !base.is_empty() => (Some(dir), base),
        _ => (None, first.as_str()),
    };

    let mut out = String::new();
    if let Some(dir) = dir {
        out.push_str(color_text);
        out.push_str(dir);
        out.push('/');
    }
    out.push_str(color_cmd);
    out.push_str(base);
    out.push_str(color_text);
    for arg in &cmdline[1..] {
        out.push(' ');
        out.push_str(arg);
    }
    out
}

/// The final component of a path: `"/usr/bin/python3"` -> `"python3"`.
pub fn basename(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, base)| base)
}

/// Read a text file, replacing bytes that are not valid UTF-8.
pub fn read_lossy(path: impl AsRef<Path>) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes2human_keeps_four_digits() {
        for (bytes, want) in [
            (0u64, "0B"),
            (1, "1B"),
            (1023, "1023B"),
            (1024, "1024B"),
            (10_000, "9KB"),
            (1024 * 1024, "1024KB"),
            (21_552 * MB, "21GB"),
            (1u64 << 30, "1024MB"),
            (1u64 << 40, "1024GB"),
            (1u64 << 50, "1024TB"),
            (1u64 << 60, "1024PB"),
            // The figure the renderer feeds for a large RSS.
            (2_691_743_744, "2567MB"),
        ] {
            assert_eq!(bytes2human(bytes), want, "bytes = {bytes}");
        }
    }

    #[test]
    fn shorten_left_truncates_from_the_left() {
        assert_eq!(shorten_left("NVIDIA GeForce RTX 4090", 8), "…TX 4090");
        assert_eq!(
            shorten_left("NVIDIA GeForce RTX 4090", 23),
            "NVIDIA GeForce RTX 4090"
        );
        assert_eq!(shorten_left("abcd", 4), "abcd");
        assert_eq!(shorten_left("abcd", 5), "abcd");
        assert_eq!(shorten_left("abcd", 0), "");
        assert_eq!(shorten_left("abcd", 1), "…");
        assert_eq!(shorten_left("abcd", 2), "…d");
        assert_eq!(shorten_left("", 5), "");
        // Counted by character, not by UTF-8 byte.
        assert_eq!(shorten_left("日本語テスト長い名前", 5), "…長い名前");
    }

    #[test]
    fn prettify_commandline_highlights_only_the_base_name() {
        let cmd = vec![
            "/usr/lib/xorg/Xorg".to_string(),
            "vt1".to_string(),
            "-nolisten".to_string(),
        ];
        assert_eq!(
            prettify_commandline(&cmd, "CMD", "TXT"),
            "TXT/usr/lib/xorg/CMDXorgTXT vt1 -nolisten"
        );

        // A base name without a directory is not wrapped twice.
        let cmd = vec!["Xorg".to_string()];
        assert_eq!(prettify_commandline(&cmd, "CMD", "TXT"), "CMDXorgTXT");

        // With styling off, the arguments are joined verbatim.
        let cmd = vec![
            "/usr/bin/python3".to_string(),
            "train.py".to_string(),
            "--epochs".to_string(),
            "40".to_string(),
        ];
        assert_eq!(
            prettify_commandline(&cmd, "", ""),
            "/usr/bin/python3 train.py --epochs 40"
        );
        assert!(prettify_commandline(&[], "CMD", "TXT").is_empty());
    }

    #[test]
    fn read_lossy_reports_a_missing_file() {
        assert!(read_lossy("/proc/1/stat").is_some());
        assert!(read_lossy("/proc/99999999/stat").is_none());
    }
}
