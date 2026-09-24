// TEMP: later steps (metrics, themes, plugins, settings overlay) use the remaining items.
#![allow(dead_code)]

mod app;
mod cli;
mod commands;
mod config;
mod event;
mod event_loop;
mod format;
mod metrics;
mod plugins;
mod term;

use std::process::ExitCode;

use cli::{Command, Flags};
use config::{Config, ConfigStatus, LoadOutcome};

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
        Command::Tui => dashboard(&cli.flags),
    }
}

fn dashboard(flags: &Flags) -> ExitCode {
    let path = config::resolve_path(flags.config.as_deref());
    let (mut cfg, status) = match config::load(&path) {
        LoadOutcome::Loaded { config, warnings } if warnings.is_empty() => {
            (config, ConfigStatus::Ok)
        }
        LoadOutcome::Loaded { config, warnings } => (config, ConfigStatus::Warnings(warnings)),
        LoadOutcome::Syntax { line, message } => {
            (Config::default(), ConfigStatus::Syntax { line, message })
        }
        LoadOutcome::Missing => (Config::default(), ConfigStatus::Ok),
    };
    config::apply_flags(&mut cfg, flags);
    match event_loop::run(cfg, status, flags) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("telemetrix: {e}");
            ExitCode::FAILURE
        }
    }
}
