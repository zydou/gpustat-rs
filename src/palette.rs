//! The SGR colour codes the renderer emits.
//!
//! Every accessor returns the empty string when styling is off, so callers can
//! wrap unconditionally and never have to branch.

/// A resolved set of styling codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    enabled: bool,
}

// Terminal control sequences, used by the watch loop to redraw a screen.
/// Move to the first cell; terminals address it as 1,1.
pub const MOVE_HOME: &str = "\u{1b}[1;1H";
/// Clear from the cursor to the end of the screen.
pub const CLEAR_EOS: &str = "\u{1b}[J";
/// Clear to the end of the line and wrap. The watch loop puts this at the end
/// of every table line so a shrinking table leaves no residue.
pub const CLEAR_LINE: &str = "\u{1b}[K\n";

impl Palette {
    /// Every accessor returns `""`.
    pub const fn disabled() -> Self {
        Self { enabled: false }
    }

    /// Codes are emitted.
    pub const fn enabled() -> Self {
        Self { enabled: true }
    }

    /// `code`, or `""` when styling is off.
    fn code(&self, code: &'static str) -> &'static str {
        if self.enabled { code } else { "" }
    }

    /// Reset every SGR attribute.
    pub fn reset(&self) -> &'static str {
        self.code("\u{1b}[0m")
    }

    /// Attenuated text, used for process paths and arguments.
    pub fn dim(&self) -> &'static str {
        self.code("\u{1b}[2m")
    }

    pub fn red(&self) -> &'static str {
        self.code("\u{1b}[31m")
    }

    pub fn green(&self) -> &'static str {
        self.code("\u{1b}[32m")
    }

    pub fn yellow(&self) -> &'static str {
        self.code("\u{1b}[33m")
    }

    pub fn blue(&self) -> &'static str {
        self.code("\u{1b}[34m")
    }

    pub fn magenta(&self) -> &'static str {
        self.code("\u{1b}[35m")
    }

    pub fn cyan(&self) -> &'static str {
        self.code("\u{1b}[36m")
    }

    /// Grey, used for usernames and the `--` placeholders.
    pub fn bold_black(&self) -> &'static str {
        self.code("\u{1b}[1;30m")
    }

    pub fn bold_red(&self) -> &'static str {
        self.code("\u{1b}[1;31m")
    }

    pub fn bold_green(&self) -> &'static str {
        self.code("\u{1b}[1;32m")
    }

    pub fn bold_yellow(&self) -> &'static str {
        self.code("\u{1b}[1;33m")
    }

    pub fn bold_magenta(&self) -> &'static str {
        self.code("\u{1b}[1;35m")
    }

    pub fn bold_cyan(&self) -> &'static str {
        self.code("\u{1b}[1;36m")
    }

    pub fn bold_white(&self) -> &'static str {
        self.code("\u{1b}[1;37m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every accessor, so a new one cannot be added without a test.
    fn all_codes(pal: Palette) -> [(&'static str, &'static str); 15] {
        [
            ("reset", pal.reset()),
            ("dim", pal.dim()),
            ("red", pal.red()),
            ("green", pal.green()),
            ("yellow", pal.yellow()),
            ("blue", pal.blue()),
            ("magenta", pal.magenta()),
            ("cyan", pal.cyan()),
            ("bold_black", pal.bold_black()),
            ("bold_red", pal.bold_red()),
            ("bold_green", pal.bold_green()),
            ("bold_yellow", pal.bold_yellow()),
            ("bold_magenta", pal.bold_magenta()),
            ("bold_cyan", pal.bold_cyan()),
            ("bold_white", pal.bold_white()),
        ]
    }

    #[test]
    fn codes_are_sgr_sequences() {
        let pal = Palette::enabled();
        for (name, code) in all_codes(pal) {
            assert!(
                code.starts_with("\u{1b}[") && code.ends_with('m'),
                "{name} = {code:?}"
            );
        }
        assert_eq!(all_codes(pal)[0].1, "\u{1b}[0m");
        assert_eq!(all_codes(pal)[1].1, "\u{1b}[2m");
        assert_eq!(all_codes(pal)[13].1, "\u{1b}[1;36m");
        assert_eq!(Palette::enabled().bold_white(), "\u{1b}[1;37m");
        assert_ne!(Palette::enabled().red(), Palette::enabled().bold_red());
    }

    #[test]
    fn a_disabled_palette_emits_nothing() {
        let pal = Palette::disabled();
        for (name, code) in all_codes(pal) {
            assert!(code.is_empty(), "{name} = {code:?}");
        }
    }

    #[test]
    fn a_disabled_palette_is_not_an_enabled_one() {
        assert_ne!(Palette::disabled(), Palette::enabled());
        assert_eq!(Palette::enabled(), Palette::enabled());
    }

    #[test]
    fn the_watch_escape_sequences_are_terminal_controls() {
        assert_eq!(MOVE_HOME, "\u{1b}[1;1H");
        assert_eq!(CLEAR_EOS, "\u{1b}[J");
        assert_eq!(CLEAR_LINE, "\u{1b}[K\n");
    }
}
