mod app;
mod cli;
mod commands;
mod config;
mod event;
mod event_loop;
mod format;
mod metrics;
mod plugins;
mod selfmem;
mod term;
mod themes;
mod ui;

use std::process::ExitCode;

use cli::{Command, Flags};

fn main() -> ExitCode {
    let cli = match cli::parse(std::env::args_os().skip(1)) {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("telemetrix: {e}");
            return ExitCode::from(2);
        }
    };
    plugins::store::set_data_dir(cli.flags.data_dir.clone());
    match cli.command {
        Command::Help => {
            print!("{}", cli::HELP);
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("telemetrix {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Selftest { seconds, json } => commands::selftest::run(seconds, json, &cli.flags),
        Command::ProbeTemps => {
            if metrics::worker::any_temperature() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Command::Themes => {
            for theme in themes::all() {
                println!("{}", theme.name());
            }
            ExitCode::SUCCESS
        }
        Command::Config(cmd) => commands::config_cmd::run(cmd, &cli.flags),
        Command::Plugin(cmd) => commands::plugin_cmd::run(cmd, &cli.flags),
        Command::Snapshot { json, plugins } => commands::snapshot::run(json, plugins, &cli.flags),
        Command::Tui => dashboard(&cli.flags),
    }
}

fn dashboard(flags: &Flags) -> ExitCode {
    let (_, cfg, status) = config::load_effective(flags);
    match event_loop::run(cfg, status, flags) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("telemetrix: {e}");
            ExitCode::FAILURE
        }
    }
}
