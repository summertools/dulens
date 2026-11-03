use std::env;
use std::error::Error;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::ValueEnum;

pub mod cli;
mod format;
mod render;
mod scan;
mod tui;

use cli::{Cli, ColorChoice};
use render::RenderOptions;
use scan::{Excludes, ScanOptions, Scanner, Tree};
use tui::TuiOptions;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum SortKey {
    Size,
    Name,
}

pub fn run(cli: Cli) -> Result<ExitCode, Box<dyn Error>> {
    let stdout_is_terminal = io::stdout().is_terminal();
    if cli.interactive && (!stdout_is_terminal || !io::stdin().is_terminal()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "interactive mode requires a terminal on stdin and stdout",
        )
        .into());
    }

    let paths = if cli.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        cli.paths.clone()
    };

    let excludes = Excludes::new(&cli.exclude)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    let scan_options = ScanOptions {
        one_file_system: cli.one_file_system,
        excludes,
    };
    let tree = Scanner::new(scan_options).scan(&paths);

    if tree.node(tree.root).children.is_empty() {
        if !cli.quiet {
            print_warnings(&tree);
        }
        eprintln!("dulens: no readable paths");
        return Ok(ExitCode::FAILURE);
    }

    let color = if cli.plain || cli.no_color {
        false
    } else {
        match cli.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => color_auto(stdout_is_terminal),
        }
    };
    let metric = cli.metric();
    let unit = cli.size_unit();

    if cli.interactive {
        let options = TuiOptions {
            metric,
            unit,
            color,
            plain: cli.plain,
            sort: cli.sort,
            reverse: cli.reverse,
        };
        tui::run(&tree, options)?;
        if !cli.quiet {
            print_warnings(&tree);
        }
    } else {
        let options = RenderOptions {
            depth: cli.depth,
            all: cli.all,
            metric,
            unit,
            color,
            plain: cli.plain,
            width: output_width(stdout_is_terminal),
            sort: cli.sort,
            reverse: cli.reverse,
        };
        let output = render::render(&tree, &options);
        let mut stdout = io::stdout().lock();
        if let Err(error) = stdout.write_all(output.as_bytes()) {
            if error.kind() == io::ErrorKind::BrokenPipe {
                return Ok(ExitCode::SUCCESS);
            }
            return Err(error.into());
        }
        if let Err(error) = stdout.flush() {
            if error.kind() == io::ErrorKind::BrokenPipe {
                return Ok(ExitCode::SUCCESS);
            }
            return Err(error.into());
        }
        if !cli.quiet {
            print_warnings(&tree);
        }
    }

    if tree.error_count > 0 {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn color_auto(stdout_is_terminal: bool) -> bool {
    if !stdout_is_terminal {
        return false;
    }
    if env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if env::var("CLICOLOR").is_ok_and(|value| value == "0") {
        return false;
    }
    !env::var("TERM").is_ok_and(|value| value.eq_ignore_ascii_case("dumb"))
}

fn output_width(stdout_is_terminal: bool) -> usize {
    if stdout_is_terminal && let Ok((width, _)) = crossterm::terminal::size() {
        return (width as usize).max(1);
    }
    env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| *width >= 20)
        .unwrap_or(100)
}

fn print_warnings(tree: &Tree) {
    for warning in &tree.warnings {
        eprintln!("dulens: warning: {warning}");
    }
    let omitted = tree.error_count.saturating_sub(tree.warnings.len() as u64);
    if omitted > 0 {
        eprintln!("dulens: warning: {omitted} additional error(s) were suppressed");
    }
}
