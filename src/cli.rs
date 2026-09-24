use std::ffi::OsString;
use std::path::PathBuf;

use crate::config::THEME_NAMES;

pub const HELP: &str = "\
telemetrix (TermSaver): terminal screensaver and live system telemetry dashboard

Usage:
  telemetrix [flags]                      dashboard
  telemetrix snapshot [--json] [--plugins]  read the metrics once and print them
  telemetrix plugin check <file> [--json] load a plugin, run update() once, print the card
  telemetrix plugin list                  list the plugins that would run
  telemetrix themes                       list theme names
  telemetrix config init [--force]        write the default settings file
  telemetrix config path                  print where the settings file is
  telemetrix config check [--json]        list every problem in the settings file
  telemetrix config show                  print effective settings and their source

Flags:
  --config <path>       settings file to use
  --theme <name>        minimalist | matrix
  --fps <n>             frames per second for animated themes, 1..60
  --plugins-dir <dir>   folder with .lua plugins
  --no-plugins          run no plugins
  --exit-on-any-key     screensaver mode: any key quits
  --log <path>          also write the log to this file
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
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigCmd {
    Init { force: bool },
    Path,
    Check { json: bool },
    Show,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PluginCmd {
    Check { file: PathBuf, json: bool },
    List,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Tui,
    Snapshot { json: bool, plugins: bool },
    Themes,
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
        ["plugin", "check", file] => Command::Plugin(PluginCmd::Check {
            file: PathBuf::from(file),
            json: sw.json,
        }),
        ["plugin", "list"] => Command::Plugin(PluginCmd::List),
        ["snapshot"] => Command::Snapshot {
            json: sw.json,
            plugins: sw.plugins,
        },
        ["config", "init"] => Command::Config(ConfigCmd::Init { force: sw.force }),
        ["config", "path"] => Command::Config(ConfigCmd::Path),
        ["config", "check"] => Command::Config(ConfigCmd::Check { json: sw.json }),
        ["config", "show"] => Command::Config(ConfigCmd::Show),
        _ => return Err(format!("unknown command {:?}, see --help", words.join(" "))),
    };
    let takes_json = matches!(
        cmd,
        Command::Config(ConfigCmd::Check { .. })
            | Command::Snapshot { .. }
            | Command::Plugin(PluginCmd::Check { .. })
    );
    if sw.plugins && !matches!(cmd, Command::Snapshot { .. }) {
        return Err("--plugins only applies to snapshot".into());
    }
    if sw.json && !takes_json {
        return Err("--json does not apply to this command".into());
    }
    if sw.force && !matches!(cmd, Command::Config(ConfigCmd::Init { .. })) {
        return Err("--force only applies to config init".into());
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
    }

    #[test]
    fn rejects_bad_input() {
        assert!(run(&["--theme", "neon"]).is_err());
        assert!(run(&["--fps", "0"]).is_err());
        assert!(run(&["config", "show", "--json"]).is_err());
        assert!(run(&["frobnicate"]).is_err());
        assert!(run(&["plugin", "check"]).is_err());
        assert!(run(&["--bogus"]).is_err());
    }
}
