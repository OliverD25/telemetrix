//! Offline tests for the plugins in `plugins/`. Recorded answers from the
//! real services (tests/fixtures/net) stand in for the network.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mlua::{Table, Value};

use super::PluginData;
use super::host_api::Trigger;
use super::runner::{self, Plugin, RunnerSettings};
use crate::config::{Config, PluginConfig};

/// What the fake `http_get` answers for a URL that contains a pattern.
#[derive(Clone)]
pub enum Answer {
    File(&'static str),
    Status(u16),
    Offline,
}

pub struct Harness {
    pub plugin: Plugin,
    pub settings: RunnerSettings,
    pub routes: Rc<RefCell<Vec<(String, Answer)>>>,
    /// Every URL the plugin asked for.
    pub asked: Rc<RefCell<Vec<String>>>,
    pub log: Rc<RefCell<Vec<String>>>,
    dir: PathBuf,
}

fn net_fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/net")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

impl Harness {
    /// Loads `plugins/<name>.lua` with its built-in defaults plus `extra`
    /// settings, a fresh data folder and a fake network.
    pub fn new(name: &str, extra: &str, routes: &[(&str, Answer)]) -> Self {
        let mut settings = RunnerSettings::from_config(&Config::default());
        let dir = std::env::temp_dir().join(format!(
            "telemetrix-bundled-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        settings.data_dir = dir.clone();
        let cfg = settings
            .plugin_cfg
            .entry(name.to_string())
            .or_insert_with(|| PluginConfig {
                enabled: true,
                ..PluginConfig::default()
            });
        let doc: toml_edit::DocumentMut = extra.parse().unwrap();
        for (k, v) in doc.iter() {
            cfg.settings.insert(k, v.clone());
        }
        let log = Rc::new(RefCell::new(Vec::new()));
        let sink = log.clone();
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins")
            .join(format!("{name}.lua"));
        let plugin = Plugin::load(
            &path,
            &settings,
            Arc::new(AtomicBool::new(false)),
            Rc::new(move |m: &str| sink.borrow_mut().push(m.to_string())),
            runner::no_emit(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let h = Self {
            plugin,
            settings,
            routes: Rc::new(RefCell::new(
                routes
                    .iter()
                    .map(|(p, a)| (p.to_string(), a.clone()))
                    .collect(),
            )),
            asked: Rc::new(RefCell::new(Vec::new())),
            log,
            dir,
        };
        h.fake_network();
        h
    }

    fn fake_network(&self) {
        let lua = self.plugin.lua();
        let (routes, asked) = (self.routes.clone(), self.asked.clone());
        let get = lua
            .create_function(move |lua, (url, _t): (String, Option<f64>)| {
                asked.borrow_mut().push(url.clone());
                let answer = routes
                    .borrow()
                    .iter()
                    .find(|(pattern, _)| url.contains(pattern.as_str()))
                    .map(|(_, a)| a.clone());
                Ok(match answer {
                    Some(Answer::File(name)) => (
                        Value::String(lua.create_string(net_fixture(name))?),
                        Value::Integer(200),
                    ),
                    Some(Answer::Status(code)) => (
                        Value::String(lua.create_string("")?),
                        Value::Integer(code.into()),
                    ),
                    Some(Answer::Offline) | None => (
                        Value::Nil,
                        Value::String(lua.create_string("connection refused")?),
                    ),
                })
            })
            .unwrap();
        let t: Table = lua.globals().get("telemetrix").unwrap();
        t.set("http_get", get).unwrap();
    }

    pub fn route(&self, pattern: &str, answer: Answer) {
        let mut routes = self.routes.borrow_mut();
        routes.retain(|(p, _)| p != pattern);
        routes.insert(0, (pattern.to_string(), answer));
    }

    pub fn set(&mut self, key: &str, toml_value: &str) {
        let doc: toml_edit::DocumentMut = format!("{key} = {toml_value}").parse().unwrap();
        let id = self.plugin.id().to_string();
        let cfg = self.settings.plugin_cfg.get_mut(&id).unwrap();
        cfg.settings.insert(key, doc[key].clone());
    }

    pub fn run(&self, trigger: Trigger) -> PluginData {
        self.plugin
            .update(&self.settings, trigger)
            .unwrap_or_else(|e| runner::error_data(self.plugin.id(), self.plugin.title(), e))
    }

    pub fn asked_for(&self, pattern: &str) -> usize {
        self.asked
            .borrow()
            .iter()
            .filter(|u| u.contains(pattern))
            .count()
    }

    /// Makes the store look `seconds` older, as if time passed.
    pub fn age_store(&self, lua_fix: &str) {
        let lua = self.plugin.lua();
        lua.load(format!(
            "local s = telemetrix.store_get() {lua_fix} telemetrix.store_set(s)"
        ))
        .exec()
        .unwrap();
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The card as the minimalist theme draws it in a `width`-wide terminal.
pub fn render_card(data: &PluginData, width: u16) -> String {
    use crate::app::AppState;
    use crate::config::ConfigStatus;
    use crate::plugins::{PluginCard, PluginStatus};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut cfg = Config::default();
    cfg.general.theme = "minimalist".into();
    cfg.disks.show_network = false;
    let mut state = AppState::new(cfg, ConfigStatus::Ok);
    let status = match &data.error {
        Some(e) => PluginStatus::Error(e.clone()),
        None => PluginStatus::Ok,
    };
    state.plugins.insert(
        data.id.clone(),
        PluginCard {
            data: data.clone(),
            status,
        },
    );
    let height = 60 + 2 * data.metrics.len() as u16;
    let mut theme = crate::themes::all().remove(state.theme_idx);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|f| crate::ui::draw(f, &state, theme.as_mut()))
        .unwrap();
    let buf = terminal.backend().buffer();
    let lines: Vec<String> = (0..height)
        .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect();
    let title = format!(" {} ", data.title);
    let start = lines
        .iter()
        .position(|l| l.contains(&title))
        .unwrap_or_else(|| panic!("no card titled {title:?} in:\n{}", lines.join("\n")));
    let mut out = Vec::new();
    for line in &lines[start..] {
        out.push(line.trim_end().to_string());
        if out.len() > 1 && (line.contains('└') || line.contains('╰')) {
            break;
        }
    }
    out.join("\n")
}

mod currency {
    use super::*;

    fn routes() -> Vec<(&'static str, Answer)> {
        vec![
            ("api.monobank.ua", Answer::File("mono.json")),
            ("api.privatbank.ua", Answer::File("privat.json")),
            ("valcode=usd", Answer::File("nbu_usd.json")),
            ("valcode=eur", Answer::File("nbu_eur.json")),
            ("valcode=gbp", Answer::File("nbu_gbp.json")),
        ]
    }

    #[test]
    fn both_banks_nbu_and_graphs() {
        let h = Harness::new("currency", "", &routes());
        let d = h.run(Trigger::Start);
        assert_eq!(d.error, None, "{:?}", h.log.borrow());
        let rows: Vec<(&str, &str)> = d
            .metrics
            .iter()
            .map(|m| (m.label.as_str(), m.value.as_str()))
            .collect();
        assert_eq!(rows[0], ("USD", "mono 44.80/45.20  privat 44.60/45.05"));
        assert_eq!(rows[1], ("7d", "+0.69%"));
        assert_eq!(rows[3].0, "EUR");
        assert_eq!(rows[6], ("GBP", "mono x59.94  NBU 59.46"));
        assert_eq!(d.metrics.len(), 9);
        assert_eq!(d.metrics[1].trend.as_ref().map(Vec::len), Some(7));
        assert_eq!(d.metrics[2].trend.as_ref().map(Vec::len), Some(30));
        println!("{}", render_card(&d, 45));
    }

    #[test]
    fn monobank_is_asked_once_per_five_minutes_and_nbu_once_a_day() {
        let h = Harness::new("currency", "", &routes());
        h.run(Trigger::Start);
        h.run(Trigger::Key);
        h.run(Trigger::Settings);
        assert_eq!(h.asked_for("monobank"), 1);
        assert_eq!(h.asked_for("privatbank"), 3, "PrivatBank has no such limit");
        assert_eq!(h.asked_for("NBU_Exchange"), 3, "one request per currency");
        h.age_store("s.mono.tried = s.mono.tried - 301");
        h.run(Trigger::Interval);
        assert_eq!(h.asked_for("monobank"), 2);
        h.age_store("s.history_date = '2000-01-01'");
        h.run(Trigger::Interval);
        assert_eq!(
            h.asked_for("NBU_Exchange"),
            6,
            "a new day fetches the history"
        );
    }

    #[test]
    fn a_failing_bank_keeps_its_last_rates_marked_stale() {
        let mut h = Harness::new("currency", "", &routes());
        h.run(Trigger::Start);
        h.age_store("s.mono.tried = s.mono.tried - 301");
        h.route("api.monobank.ua", Answer::Status(429));
        h.route("api.privatbank.ua", Answer::Offline);
        let d = h.run(Trigger::Interval);
        assert_eq!(d.error, None);
        assert_eq!(d.metrics[0].value, "mono 44.80/45.20  privat 44.60/45.05");
        let stale: Vec<&str> = d
            .metrics
            .iter()
            .filter(|m| m.label == "stale")
            .map(|m| m.value.as_str())
            .collect();
        assert_eq!(stale.len(), 2);
        assert!(stale[0].starts_with("mono since ") && stale[0].ends_with("(HTTP 429)"));
        assert!(stale[1].starts_with("privat since "), "{stale:?}");
        h.set("compact", "true");
        h.set("primary_bank", "'privat'");
        let d = h.run(Trigger::Settings);
        assert_eq!(d.metrics[0].value, "privat 44.60/45.05 (stale)");
        let gbp = d.metrics.iter().find(|m| m.label == "GBP").unwrap();
        assert_eq!(gbp.value, "NBU 59.46", "PrivatBank has no GBP");
        println!("{}", render_card(&d, 45));
    }

    #[test]
    fn nothing_stored_and_no_answer_is_an_error() {
        let h = Harness::new(
            "currency",
            "show_month = false",
            &[("", Answer::Status(500))],
        );
        let d = h.run(Trigger::Start);
        assert_eq!(
            d.error.as_deref(),
            Some("no rates yet: mono HTTP 500, privat HTTP 500")
        );
    }

    #[test]
    fn show_month_off_hides_the_30_day_graph() {
        let h = Harness::new("currency", "show_month = false", &routes());
        let d = h.run(Trigger::Start);
        assert!(d.metrics.iter().all(|m| m.label != "30d"));
        assert_eq!(d.metrics.len(), 6);
    }
}

mod crypto {
    use super::*;

    fn routes() -> Vec<(&'static str, Answer)> {
        vec![
            ("ticker/price", Answer::File("binance_price.json")),
            ("symbol=BTCUSDT", Answer::File("klines_BTC.json")),
            ("symbol=ETHUSDT", Answer::File("klines_ETH.json")),
            ("symbol=SOLUSDT", Answer::File("klines_SOL.json")),
        ]
    }

    #[test]
    fn prices_and_graphs_from_binance() {
        let h = Harness::new("crypto", "", &routes());
        let d = h.run(Trigger::Start);
        assert_eq!(d.error, None, "{:?}", h.log.borrow());
        let rows: Vec<(&str, &str)> = d
            .metrics
            .iter()
            .map(|m| (m.label.as_str(), m.value.as_str()))
            .collect();
        assert_eq!(rows[0], ("BTC", "84,853 USDT"));
        assert_eq!(rows[3], ("ETH", "2,725 USDT"));
        assert_eq!(rows[6], ("SOL", "121 USDT"));
        assert_eq!((rows[1].0, rows[2].0), ("7d", "30d"));
        assert_eq!(d.metrics[2].trend.as_ref().map(Vec::len), Some(30));
        assert_eq!(
            d.metrics[2].trend.as_ref().and_then(|t| t.last().copied()),
            Some(84853.17),
            "the graph ends at the current price"
        );
        let asked = h.asked.borrow();
        assert!(
            asked[0].ends_with("symbols=%5B%22BTCUSDT%22%2C%22ETHUSDT%22%2C%22SOLUSDT%22%5D"),
            "{}",
            asked[0]
        );
        drop(asked);
        println!("{}", render_card(&d, 45));
    }

    #[test]
    fn history_once_an_hour_and_stale_prices_on_failure() {
        let h = Harness::new("crypto", "", &routes());
        h.run(Trigger::Start);
        h.run(Trigger::Interval);
        assert_eq!(h.asked_for("ticker/price"), 2);
        assert_eq!(h.asked_for("klines"), 3, "one per coin, then cached");
        h.age_store("for _, x in pairs(s.history) do x.ts = x.ts - 3600 end");
        h.route("ticker/price", Answer::Status(429));
        let d = h.run(Trigger::Interval);
        assert_eq!(h.asked_for("klines"), 6, "an hour later: fetched again");
        assert_eq!(d.error, None);
        assert_eq!(d.metrics[0].value, "84,853 USDT (stale)");
    }

    #[test]
    fn old_coingecko_ids_and_other_quotes() {
        let mut h = Harness::new(
            "crypto",
            "coins = ['bitcoin', 'doge']",
            &[("", Answer::Status(400))],
        );
        let d = h.run(Trigger::Start);
        assert_eq!(d.error.as_deref(), Some("no prices yet: HTTP 400"));
        assert!(h.asked.borrow()[0].contains("%22BTCUSDT%22%2C%22DOGEUSDT%22"));
        h.set("quote", "'usdc'");
        h.run(Trigger::Settings);
        assert!(h.asked.borrow().last().unwrap().contains("DOGEUSDC"));
    }
}
