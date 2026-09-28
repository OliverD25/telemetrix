#[cfg(feature = "alloc-stats")]
mod alloc_stats;
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
mod update;

use std::process::ExitCode;

use cli::{Command, Flags};

#[cfg(feature = "alloc-stats")]
#[global_allocator]
static ALLOC: alloc_stats::Counting = alloc_stats::Counting;

fn main() -> ExitCode {
    let cli = match cli::parse(std::env::args_os().skip(1)) {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("telemetrix: {e}");
            return ExitCode::from(2);
        }
    };
    plugins::store::set_data_dir(cli.flags.data_dir.clone());
    if let Ok(exe) = std::env::current_exe() {
        update::cleanup_old(&exe);
    }
    match cli.command {
        Command::Help => {
            print!("{}", cli::HELP);
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("telemetrix {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Selftest {
            soak: Some(minutes),
            json,
            ..
        } => commands::selftest::soak(minutes, json, &cli.flags),
        Command::Selftest { seconds, json, .. } => {
            commands::selftest::run(seconds, json, &cli.flags)
        }
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
        Command::Screensaver(cmd) => commands::screensaver::run(cmd, &cli.flags),
        Command::Snapshot { json, plugins } => commands::snapshot::run(json, plugins, &cli.flags),
        Command::Update {
            check,
            dry_run,
            force,
        } => commands::update_cmd::run(check, dry_run, force),
        Command::Tui => dashboard(&cli.flags),
    }
}

fn dashboard(flags: &Flags) -> ExitCode {
    let exe = std::env::current_exe().unwrap_or_else(|_| "telemetrix".into());
    let outcome = {
        let _any = commands::screensaver::hold_dashboard();
        let _mark = flags
            .screensaver
            .then(commands::screensaver::mark_dashboard)
            .flatten();
        let (_, cfg, status) = config::load_effective(flags);
        event_loop::run(cfg, status, flags, &exe)
    };
    match outcome {
        Ok(event_loop::Exit::Quit) => ExitCode::SUCCESS,
        Ok(event_loop::Exit::Restart) => update::restart(&exe),
        Err(e) => {
            eprintln!("telemetrix: {e}");
            ExitCode::FAILURE
        }
    }
}
