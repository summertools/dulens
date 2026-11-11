use std::env;
use std::error::Error;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::ValueEnum;

mod cleanup;
mod cleanup_ui;
pub mod cli;
mod format;
mod render;
mod scan;
mod term;
mod tui;

use cleanup_ui::CleanupOptions;
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
    if cli.cleanup && (!stdout_is_terminal || !io::stdin().is_terminal()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cleanup mode requires a terminal on stdin and stdout; use it interactively",
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

    if cli.cleanup {
        let (settings, mut cleanup_warnings) =
            cleanup::load_settings(cli.config.as_deref(), cli.clean_profile.as_deref())
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
        let detection = cleanup::detect(&tree, &settings);
        cleanup_warnings.extend(detection.warnings.iter().cloned());
        cleanup_warnings.extend(tree.warnings.iter().cloned());
        if detection.candidates.is_empty() {
            for warning in &cleanup_warnings {
                eprintln!("dulens: warning: {}", format::sanitize_text(warning));
            }
            eprintln!("dulens: no build cache or dependency candidates found");
            return Ok(if tree.error_count > 0 {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            });
        }
        let report = cleanup_ui::run(
            detection.candidates,
            CleanupOptions {
                color,
                unit,
                profile: settings.profile.clone(),
            },
        )?;
        for warning in &cleanup_warnings {
            eprintln!("dulens: warning: {}", format::sanitize_text(warning));
        }
        if report.failed() {
            for error in &report.errors {
                eprintln!("dulens: cleanup: {}", format::sanitize_text(error));
            }
        }
        return Ok(if tree.error_count > 0 || report.failed() {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        });
    }

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
