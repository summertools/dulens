use std::process::ExitCode;

use clap::Parser;
use dulens::cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dulens::run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dulens: {error}");
            ExitCode::FAILURE
        }
    }
}
