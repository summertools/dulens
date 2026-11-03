use std::path::PathBuf;

use clap::{Parser, ValueEnum};

use crate::SortKey;
use crate::format::SizeUnit;
use crate::scan::Metric;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Parser)]
#[command(
    name = "dulens",
    version,
    about = "Explore disk usage with a readable tree or an interactive terminal UI",
    after_help = "INTERACTIVE KEYS:\n  ↑/k, ↓/j      move          Enter/l   open directory\n  Backspace/h  go up         g/G       first/last\n  PgUp/PgDn    page\n  s            sort by size/name\n  S            reverse order  a         toggle disk/apparent size\n  q            quit          Esc       back, then quit\n\nNOTES:\n  Sizes use allocated disk blocks unless --apparent-size is used.\n  Hard links are counted once. Symbolic links are not followed.\n  Interactive mode always shows all entries; --depth and --all do not apply.\n  Plain mode uses a clean size + name layout without colors or decorations.\n\nExamples:\n  dulens\n  dulens -d 3 -a ~/projects\n  dulens --plain -d 3 ~/projects\n  dulens -i --apparent-size /var\n  dulens --sort name --reverse -E target -E node_modules ."
)]
pub struct Cli {
    #[arg(value_name = "PATH", help = "Paths to inspect (default: .)")]
    pub paths: Vec<PathBuf>,

    #[arg(
        short = 'd',
        long,
        default_value_t = 2,
        value_name = "N",
        help = "Tree depth to display"
    )]
    pub depth: usize,

    #[arg(
        short = 'a',
        long,
        visible_alias = "files",
        help = "Show files as well as directories"
    )]
    pub all: bool,

    #[arg(short = 'i', long, help = "Open the interactive directory browser")]
    pub interactive: bool,

    #[arg(
        short = 'p',
        long,
        visible_aliases = ["simple", "clean"],
        help = "Plain monochrome layout without bars, percentages, or color"
    )]
    pub plain: bool,

    #[arg(
        long = "apparent-size",
        help = "Count file lengths instead of allocated disk blocks"
    )]
    pub apparent_size: bool,

    #[arg(long, conflicts_with = "bytes", help = "Use powers of 1000")]
    pub si: bool,

    #[arg(long, conflicts_with = "si", help = "Print exact byte counts")]
    pub bytes: bool,

    #[arg(
        long,
        value_enum,
        default_value_t = ColorChoice::Auto,
        help = "Color output: auto, always, or never"
    )]
    pub color: ColorChoice,

    #[arg(long, help = "Disable color output")]
    pub no_color: bool,

    #[arg(
        short = 's',
        long,
        value_enum,
        default_value_t = SortKey::Size,
        help = "Sort by size or name"
    )]
    pub sort: SortKey,

    #[arg(short = 'r', long, help = "Reverse the sort order")]
    pub reverse: bool,

    #[arg(short = 'x', long, help = "Stay on the starting filesystem")]
    pub one_file_system: bool,

    #[arg(
        short = 'E',
        long = "exclude",
        value_name = "GLOB",
        help = "Skip entries matching a name or path glob (repeatable)"
    )]
    pub exclude: Vec<String>,

    #[arg(short = 'q', long, help = "Suppress scan warnings")]
    pub quiet: bool,
}

impl Cli {
    pub fn metric(&self) -> Metric {
        if self.apparent_size {
            Metric::Apparent
        } else {
            Metric::Disk
        }
    }

    pub fn size_unit(&self) -> SizeUnit {
        if self.bytes {
            SizeUnit::Bytes
        } else if self.si {
            SizeUnit::Decimal
        } else {
            SizeUnit::Binary
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_flags() {
        let cli = Cli::try_parse_from([
            "dulens",
            "-d",
            "4",
            "-a",
            "--apparent-size",
            "--si",
            "-E",
            "target",
            "src",
        ])
        .unwrap();

        assert_eq!(cli.depth, 4);
        assert!(cli.all);
        assert_eq!(cli.metric(), Metric::Apparent);
        assert_eq!(cli.size_unit(), SizeUnit::Decimal);
        assert_eq!(cli.paths, [PathBuf::from("src")]);
        assert_eq!(cli.exclude, ["target"]);
    }

    #[test]
    fn parses_plain_mode_and_aliases() {
        let cli = Cli::try_parse_from(["dulens", "--plain"]).unwrap();
        assert!(cli.plain);
        assert!(Cli::try_parse_from(["dulens", "-p"]).unwrap().plain);
        assert!(Cli::try_parse_from(["dulens", "--simple"]).unwrap().plain);
        assert!(Cli::try_parse_from(["dulens", "--clean"]).unwrap().plain);
        assert!(Cli::try_parse_from(["dulens", "--plain", "-i"]).is_ok());
    }

    #[test]
    fn rejects_conflicting_size_units() {
        assert!(Cli::try_parse_from(["dulens", "--si", "--bytes"]).is_err());
    }
}
