use std::borrow::Cow;
use std::env;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SizeUnit {
    Binary,
    Decimal,
    Bytes,
}

impl SizeUnit {
    pub fn format(self, bytes: u64) -> String {
        if self == Self::Bytes {
            return format!("{bytes} B");
        }

        let base = if self == Self::Decimal {
            1000u64
        } else {
            1024u64
        };
        let units = if self == Self::Decimal {
            ["B", "kB", "MB", "GB", "TB", "PB", "EB"]
        } else {
            ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"]
        };

        if bytes < base {
            return format!("{bytes} B");
        }

        let mut value = bytes as f64;
        let mut unit = 0;
        while value >= base as f64 && unit < units.len() - 1 {
            value /= base as f64;
            unit += 1;
        }

        if unit < units.len() - 1 && value >= 100.0 && value >= base as f64 - 0.5 {
            value /= base as f64;
            unit += 1;
        }

        let number = if value >= 100.0 {
            format!("{value:.0}")
        } else if value >= 10.0 {
            format!("{value:.1}")
        } else {
            format!("{value:.2}")
        };
        format!("{number} {}", units[unit])
    }
}

pub fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn count_noun(count: u64, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{} {noun}", group_digits(count))
}

pub fn percent(size: u64, total: u64) -> String {
    if total == 0 || size == 0 {
        return "0%".to_string();
    }
    let value = size as f64 / total as f64 * 100.0;
    if value < 0.5 {
        "<1%".to_string()
    } else if size >= total {
        "100%".to_string()
    } else {
        format!("{:.0}%", value.round().min(99.0))
    }
}

pub fn bar_units(size: u64, total: u64, width: usize) -> usize {
    if width == 0 || total == 0 || size == 0 {
        return 0;
    }
    let units = ((size as f64 / total as f64) * width as f64 * 8.0).round() as usize;
    units.clamp(1, width * 8)
}

pub fn bar_text(units: usize, width: usize) -> String {
    const PARTIALS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let units = units.min(width * 8);
    let full = units / 8;
    let remainder = units % 8;
    let mut bar = "█".repeat(full);
    if remainder > 0 {
        bar.push_str(PARTIALS[remainder]);
    }
    bar
}

pub fn bar_cells(units: usize) -> usize {
    units.div_ceil(8)
}

pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub fn sanitize_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_control() {
            out.extend(ch.escape_default());
        } else if is_bidi_control(ch) {
            out.push_str(&format!("\\u{{{:x}}}", ch as u32));
        } else {
            out.push(ch);
        }
    }
    out
}

fn is_bidi_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

pub fn truncate_right(text: &str, max_width: usize) -> Cow<'_, str> {
    if display_width(text) <= max_width {
        return Cow::Borrowed(text);
    }
    if max_width == 0 {
        return Cow::Borrowed("");
    }
    if max_width == 1 {
        return Cow::Borrowed("…");
    }

    let target = max_width - 1;
    let mut used = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let width = ch.width().unwrap_or(0);
        if used + width > target {
            break;
        }
        out.push(ch);
        used += width;
    }
    out.push('…');
    Cow::Owned(out)
}

pub fn truncate_left(text: &str, max_width: usize) -> Cow<'_, str> {
    if display_width(text) <= max_width {
        return Cow::Borrowed(text);
    }
    if max_width == 0 {
        return Cow::Borrowed("");
    }
    if max_width == 1 {
        return Cow::Borrowed("…");
    }

    let target = max_width - 1;
    let mut used = 0;
    let mut reversed = Vec::new();
    for ch in text.chars().rev() {
        let width = ch.width().unwrap_or(0);
        if used + width > target {
            break;
        }
        reversed.push(ch);
        used += width;
    }
    reversed.reverse();
    let mut out = String::with_capacity(reversed.len() + 1);
    out.push('…');
    out.extend(reversed);
    Cow::Owned(out)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorMode {
    None,
    Basic,
    Ansi256,
    TrueColor,
}

impl ColorMode {
    pub fn detect(enabled: bool) -> Self {
        if !enabled {
            return Self::None;
        }
        let color_term = env::var("COLORTERM")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if color_term.contains("truecolor") || color_term.contains("24bit") {
            return Self::TrueColor;
        }
        if env::var("TERM")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("256color")
        {
            return Self::Ansi256;
        }
        Self::Basic
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    mode: ColorMode,
}

impl Palette {
    pub fn new(enabled: bool) -> Self {
        Self {
            mode: ColorMode::detect(enabled),
        }
    }

    fn paint(self, text: &str, basic: u8, rgb: (u8, u8, u8), bold: bool) -> String {
        if self.mode == ColorMode::None || text.is_empty() {
            return text.to_string();
        }
        let bold_prefix = if bold { "1;" } else { "" };
        match self.mode {
            ColorMode::Basic => format!("\x1b[{bold_prefix}{basic}m{text}\x1b[0m"),
            ColorMode::Ansi256 => {
                let index = rgb_to_ansi256(rgb.0, rgb.1, rgb.2);
                format!("\x1b[{bold_prefix}38;5;{index}m{text}\x1b[0m")
            }
            ColorMode::TrueColor => {
                let (r, g, b) = rgb;
                format!("\x1b[{bold_prefix}38;2;{r};{g};{b}m{text}\x1b[0m")
            }
            ColorMode::None => text.to_string(),
        }
    }

    pub fn bold(self, text: &str) -> String {
        if self.mode == ColorMode::None || text.is_empty() {
            return text.to_string();
        }
        format!("\x1b[1m{text}\x1b[0m")
    }

    pub fn dim(self, text: &str) -> String {
        if self.mode == ColorMode::None || text.is_empty() {
            return text.to_string();
        }
        format!("\x1b[2m{text}\x1b[22m")
    }

    pub fn title(self, text: &str) -> String {
        self.paint(text, 97, (248, 250, 252), true)
    }

    pub fn accent(self, text: &str) -> String {
        self.paint(text, 96, (94, 234, 212), true)
    }

    pub fn directory(self, text: &str) -> String {
        self.paint(text, 36, (125, 211, 252), false)
    }

    pub fn file(self, text: &str) -> String {
        self.paint(text, 37, (226, 232, 240), false)
    }

    pub fn link(self, text: &str) -> String {
        self.paint(text, 35, (244, 114, 182), false)
    }

    pub fn warning(self, text: &str) -> String {
        self.paint(text, 33, (251, 191, 36), false)
    }

    pub fn bar(self, text: &str, fraction: f64) -> String {
        if self.mode == ColorMode::None || text.is_empty() {
            return text.to_string();
        }
        let fraction = fraction.clamp(0.0, 1.0);
        let rgb = gradient(fraction);
        self.paint(text, 36, rgb, false)
    }
}

fn gradient(t: f64) -> (u8, u8, u8) {
    const STOPS: [(f64, (u8, u8, u8)); 3] = [
        (0.0, (45, 212, 191)),
        (0.55, (96, 165, 250)),
        (1.0, (196, 181, 253)),
    ];
    let t = t.clamp(0.0, 1.0);
    for pair in STOPS.windows(2) {
        let (start_t, start) = pair[0];
        let (end_t, end) = pair[1];
        if t <= end_t {
            let local = if end_t > start_t {
                (t - start_t) / (end_t - start_t)
            } else {
                0.0
            };
            return (
                lerp(start.0, end.0, local),
                lerp(start.1, end.1, local),
                lerp(start.2, end.2, local),
            );
        }
    }
    STOPS[STOPS.len() - 1].1
}

fn lerp(start: u8, end: u8, t: f64) -> u8 {
    (start as f64 + (end as f64 - start as f64) * t).round() as u8
}

fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    if r == g && g == b {
        if r < 8 {
            return 16;
        }
        if r > 248 {
            return 231;
        }
        return 232 + ((r as u16 - 8) * 24 / 247) as u8;
    }
    let to_cube = |value: u8| -> u16 {
        let value = value as u16;
        (value * 5 + 127) / 255
    };
    (16 + 36 * to_cube(r) + 6 * to_cube(g) + to_cube(b)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_binary_sizes() {
        assert_eq!(SizeUnit::Binary.format(0), "0 B");
        assert_eq!(SizeUnit::Binary.format(999), "999 B");
        assert_eq!(SizeUnit::Binary.format(1024), "1.00 KiB");
        assert_eq!(SizeUnit::Binary.format(10 * 1024), "10.0 KiB");
        assert_eq!(SizeUnit::Binary.format(150 * 1024), "150 KiB");
        assert_eq!(SizeUnit::Binary.format(1024 * 1024 - 1), "1.00 MiB");
        assert_eq!(SizeUnit::Binary.format(5 * 1024 * 1024 * 1024), "5.00 GiB");
    }

    #[test]
    fn formats_decimal_and_raw_sizes() {
        assert_eq!(SizeUnit::Decimal.format(1000), "1.00 kB");
        assert_eq!(SizeUnit::Decimal.format(1_500_000), "1.50 MB");
        assert_eq!(SizeUnit::Decimal.format(999_500), "1.00 MB");
        assert_eq!(SizeUnit::Bytes.format(1_500_000), "1500000 B");
    }

    #[test]
    fn groups_digits() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(1_234_567_890), "1,234,567,890");
    }

    #[test]
    fn pluralizes_counted_nouns() {
        assert_eq!(count_noun(0, "file", "files"), "0 files");
        assert_eq!(count_noun(1, "file", "files"), "1 file");
        assert_eq!(count_noun(2, "file", "files"), "2 files");
        assert_eq!(count_noun(1_234, "dir", "dirs"), "1,234 dirs");
    }

    #[test]
    fn formats_percentages() {
        assert_eq!(percent(0, 100), "0%");
        assert_eq!(percent(1, 1000), "<1%");
        assert_eq!(percent(1, 2), "50%");
        assert_eq!(percent(999, 1000), "99%");
        assert_eq!(percent(5, 5), "100%");
    }

    #[test]
    fn calculates_fractional_bars() {
        assert_eq!(bar_units(0, 100, 20), 0);
        assert_eq!(bar_units(1, 1000, 20), 1);
        assert_eq!(bar_text(1, 20), "▏");
        assert_eq!(bar_cells(1), 1);
        assert_eq!(bar_units(50, 100, 20), 80);
        assert_eq!(bar_text(80, 20), "██████████");
        assert_eq!(bar_units(100, 100, 20), 160);
        assert_eq!(bar_text(160, 20), "████████████████████");
        assert_eq!(bar_units(100, 100, 0), 0);
    }

    #[test]
    fn sanitizes_control_characters() {
        assert_eq!(sanitize_text("a\nb\tc"), "a\\nb\\tc");
        assert_eq!(sanitize_text("safe-名前"), "safe-名前");
        assert!(!sanitize_text("bad\x1b[31m").contains('\x1b'));
        assert_eq!(sanitize_text("\u{202e}evil"), "\\u{202e}evil");
    }

    #[test]
    fn truncates_wide_text_by_display_width() {
        assert_eq!(truncate_right("abcdef", 4), "abc…");
        assert_eq!(truncate_right("abcdef", 1), "…");
        assert_eq!(truncate_right("abcdef", 0), "");
        assert_eq!(truncate_right("名前名前", 5), "名前…");
        assert_eq!(truncate_left("abcdef", 4), "…def");
        assert_eq!(truncate_left("名前名前", 5), "…名前");
    }

    #[test]
    fn maps_rgb_to_xterm_palette() {
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16);
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231);
    }
}
