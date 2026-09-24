mod cli;
mod commands;
mod config;

use std::process::ExitCode;

use cli::Command;

fn main() -> ExitCode {
    let cli = match cli::parse(std::env::args_os().skip(1)) {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("telemetrix: {e}");
            return ExitCode::from(2);
        }
    };
    match cli.command {
        Command::Help => {
            print!("{}", cli::HELP);
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("telemetrix {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Config(cmd) => commands::config_cmd::run(cmd, &cli.flags),
        Command::Tui => {
            eprintln!("telemetrix: the dashboard is not built yet");
            ExitCode::FAILURE
        }
    }
}
