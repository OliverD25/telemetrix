use std::ffi::OsString;
use std::path::PathBuf;

use crate::config::THEME_NAMES;

pub const HELP: &str = "\
telemetrix (TermSaver): terminal screensaver and live system telemetry dashboard

Usage:
  telemetrix [flags]                      dashboard
  telemetrix snapshot [--json] [--plugins]  read the metrics once and print them
  telemetrix plugin check <file> [--json] [--run]
                                          load a plugin, run update() once, print the card
                                          (--run: as if its run key was pressed)
  telemetrix plugin list                  list the plugins that would run
  telemetrix plugin install [--force] [name...]
                                          install or update the built-in plugins
                                          (--force: also over your edits and deletions)
  telemetrix selftest --memory [--seconds N] [--json]
                                          measure memory against the budgets, exit 1 if over
  telemetrix themes                       list theme names
  telemetrix config init [--force]        write the default settings file
  telemetrix config path                  print where the settings file is
  telemetrix config check [--json]        list every problem in the settings file
  telemetrix config show                  print effective settings and their source
  telemetrix config reference             print every setting as a Markdown table

Flags:
  --config <path>       settings file to use
  --theme <name>        minimalist | matrix
  --fps <n>             frames per second for animated themes, 1..60
  --plugins-dir <dir>   folder with .lua plugins
  --no-plugins          run no plugins
  --exit-on-any-key     screensaver mode: any key quits
  --log <path>          also write the log to this file
  --data-dir <dir>      folder for plugin stores
  -h, --help            this text
  -V, --version         version
";

/// Overrides from the command line. They never write to the settings file.
#[derive(Clone, Debug, Default)]
pub struct Flags {
    pub config: Option<PathBuf>,
    pub theme: Option<String>,
    pub fps: Option<u32>,
    pub plugins_dir: Option<PathBuf>,
    pub no_plugins: bool,
    pub exit_on_any_key: bool,
    pub log: Option<PathBuf>,
    pub panic_test: bool,
    /// Hidden, used by `selftest`: write a memory report here after `selftest_seconds`, then quit.
    pub selftest_report: Option<PathBuf>,
    pub selftest_seconds: u64,
    /// Where plugin stores live; the OS data folder when not set.
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigCmd {
    Init { force: bool },
    Path,
    Check { json: bool },
    Show,
    Reference,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PluginCmd {
    Check {
        file: PathBuf,
        json: bool,
        /// `update()` sees the trigger "manual" instead of "start".
        run: bool,
    },
    List,
    /// Built-in plugin names, or none for all of them.
    Install {
        force: bool,
        names: Vec<String>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Tui,
    Snapshot {
        json: bool,
        plugins: bool,
    },
    Themes,
    Selftest {
        seconds: u64,
        json: bool,
    },
    /// Hidden: exits 0 when a temperature sensor answers (used by the metrics thread).
    ProbeTemps,
    Config(ConfigCmd),
    Plugin(PluginCmd),
    Help,
    Version,
}

#[derive(Debug)]
pub struct Cli {
    pub flags: Flags,
    pub command: Command,
}

#[derive(Default)]
struct Switches {
    json: bool,
    force: bool,
    plugins: bool,
    memory: bool,
    run: bool,
    seconds: Option<u64>,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Cli, String> {
    use lexopt::prelude::*;
    let mut parser =
        lexopt::Parser::from_iter(std::iter::once(OsString::from("telemetrix")).chain(args));
    let mut flags = Flags::default();
    let mut sw = Switches::default();
    let mut words: Vec<String> = Vec::new();
    let mut help = false;
    let mut version = false;
    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Long("config") => {
                flags.config = Some(parser.value().map_err(|e| e.to_string())?.into())
            }
            Long("theme") => {
                let name = text(parser.value())?;
                if !THEME_NAMES.contains(&name.as_str()) {
                    return Err(format!(
                        "unknown theme {name:?}, use one of {}",
                        THEME_NAMES.join(", ")
                    ));
                }
                flags.theme = Some(name);
            }
            Long("fps") => {
                let fps: u32 = text(parser.value())?
                    .parse()
                    .map_err(|_| "--fps needs a whole number")?;
                if !(1..=60).contains(&fps) {
                    return Err("--fps must be 1..60".into());
                }
                flags.fps = Some(fps);
            }
            Long("plugins-dir") => {
                flags.plugins_dir = Some(parser.value().map_err(|e| e.to_string())?.into())
            }
            Long("no-plugins") => flags.no_plugins = true,
            Long("exit-on-any-key") => flags.exit_on_any_key = true,
            Long("log") => flags.log = Some(parser.value().map_err(|e| e.to_string())?.into()),
            Long("panic-test") => flags.panic_test = true,
            Long("json") => sw.json = true,
            Long("force") => sw.force = true,
            Long("plugins") => sw.plugins = true,
            Long("memory") => sw.memory = true,
            Long("run") => sw.run = true,
            Long("seconds") => {
                let n: u64 = text(parser.value())?
                    .parse()
                    .map_err(|_| "--seconds needs a whole number")?;
                if !(5..=3600).contains(&n) {
                    return Err("--seconds must be 5..3600".into());
                }
                sw.seconds = Some(n);
            }
            Long("data-dir") => {
                flags.data_dir = Some(parser.value().map_err(|e| e.to_string())?.into())
            }
            Long("selftest-report") => {
                flags.selftest_report = Some(parser.value().map_err(|e| e.to_string())?.into())
            }
            Long("selftest-seconds") => {
                flags.selftest_seconds = text(parser.value())?
                    .parse()
                    .map_err(|_| "--selftest-seconds needs a whole number")?
            }
            Short('h') | Long("help") => help = true,
            Short('V') | Long("version") => version = true,
            Value(v) => words.push(v.string().map_err(|_| "arguments must be valid text")?),
            _ => return Err(arg.unexpected().to_string()),
        }
    }
    let command = if help {
        Command::Help
    } else if version {
        Command::Version
    } else {
        command(&words, &sw)?
    };
    Ok(Cli { flags, command })
}

fn text(v: Result<OsString, lexopt::Error>) -> Result<String, String> {
    v.map_err(|e| e.to_string())?
        .into_string()
        .map_err(|_| "arguments must be valid text".to_string())
}

fn command(words: &[String], sw: &Switches) -> Result<Command, String> {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let cmd = match words.as_slice() {
        [] => Command::Tui,
        ["themes"] => Command::Themes,
        ["selftest"] if sw.memory => Command::Selftest {
            seconds: sw.seconds.unwrap_or(30),
            json: sw.json,
        },
        ["selftest"] => return Err("selftest needs --memory".into()),
        ["probe-temps"] => Command::ProbeTemps,
        ["plugin", "check", file] => Command::Plugin(PluginCmd::Check {
            file: PathBuf::from(file),
            json: sw.json,
            run: sw.run,
        }),
        ["plugin", "list"] => Command::Plugin(PluginCmd::List),
        ["plugin", "install", names @ ..] => Command::Plugin(PluginCmd::Install {
            force: sw.force,
            names: names.iter().map(|n| n.to_string()).collect(),
        }),
        ["snapshot"] => Command::Snapshot {
            json: sw.json,
            plugins: sw.plugins,
        },
        ["config", "init"] => Command::Config(ConfigCmd::Init { force: sw.force }),
        ["config", "path"] => Command::Config(ConfigCmd::Path),
        ["config", "check"] => Command::Config(ConfigCmd::Check { json: sw.json }),
        ["config", "show"] => Command::Config(ConfigCmd::Show),
        ["config", "reference"] => Command::Config(ConfigCmd::Reference),
        _ => return Err(format!("unknown command {:?}, see --help", words.join(" "))),
    };
    let takes_json = matches!(
        cmd,
        Command::Config(ConfigCmd::Check { .. })
            | Command::Snapshot { .. }
            | Command::Plugin(PluginCmd::Check { .. })
            | Command::Selftest { .. }
    );
    if sw.plugins && !matches!(cmd, Command::Snapshot { .. }) {
        return Err("--plugins only applies to snapshot".into());
    }
    if sw.run && !matches!(cmd, Command::Plugin(PluginCmd::Check { .. })) {
        return Err("--run only applies to plugin check".into());
    }
    if sw.json && !takes_json {
        return Err("--json does not apply to this command".into());
    }
    if sw.force
        && !matches!(
            cmd,
            Command::Config(ConfigCmd::Init { .. }) | Command::Plugin(PluginCmd::Install { .. })
        )
    {
        return Err("--force only applies to config init and plugin install".into());
    }
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Cli, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn subcommands_and_flags() {
        let cli = run(&["--theme", "minimalist", "--fps", "30", "--no-plugins"]).unwrap();
        assert_eq!(cli.command, Command::Tui);
        assert_eq!(cli.flags.theme.as_deref(), Some("minimalist"));
        assert_eq!(cli.flags.fps, Some(30));
        assert!(cli.flags.no_plugins);
        let cli = run(&["config", "check", "--json"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Config(ConfigCmd::Check { json: true })
        );
        let cli = run(&["--config", "x.toml", "config", "init", "--force"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Config(ConfigCmd::Init { force: true })
        );
        assert_eq!(cli.flags.config, Some(PathBuf::from("x.toml")));
        assert_eq!(run(&["probe-temps"]).unwrap().command, Command::ProbeTemps);
        assert_eq!(
            run(&["plugin", "install", "--force", "weather", "clock"])
                .unwrap()
                .command,
            Command::Plugin(PluginCmd::Install {
                force: true,
                names: vec!["weather".into(), "clock".into()]
            })
        );
        assert_eq!(
            run(&["plugin", "check", "a.lua", "--run"]).unwrap().command,
            Command::Plugin(PluginCmd::Check {
                file: PathBuf::from("a.lua"),
                json: false,
                run: true
            })
        );
        let cli = run(&["selftest", "--memory", "--json"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Selftest {
                seconds: 30,
                json: true
            }
        );
        assert!(
            !HELP.contains("probe-temps"),
            "internal command stays out of --help"
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(run(&["--theme", "neon"]).is_err());
        assert!(run(&["--fps", "0"]).is_err());
        assert!(run(&["config", "show", "--json"]).is_err());
        assert!(run(&["frobnicate"]).is_err());
        assert!(run(&["plugin", "check"]).is_err());
        assert!(run(&["selftest"]).is_err());
        assert!(run(&["snapshot", "--run"]).is_err());
        assert!(run(&["plugin", "list", "--force"]).is_err());
        assert!(run(&["selftest", "--memory", "--seconds", "1"]).is_err());
        assert!(run(&["--bogus"]).is_err());
    }
}
