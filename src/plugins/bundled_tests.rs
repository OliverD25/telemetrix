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
    /// A status with response headers (lower-case names, as `http_get` gives them).
    Headers(u16, &'static [(&'static str, &'static str)]),
    Offline,
}

pub struct Harness {
    pub plugin: Plugin,
    pub settings: RunnerSettings,
    pub routes: Rc<RefCell<Vec<(String, Answer)>>>,
    /// Every URL the plugin asked for.
    pub asked: Rc<RefCell<Vec<String>>>,
    pub log: Rc<RefCell<Vec<String>>>,
    /// Cards sent with `telemetrix.emit` during `update()`.
    pub emitted: Rc<RefCell<Vec<PluginData>>>,
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
        let emitted = Rc::new(RefCell::new(Vec::new()));
        let out = emitted.clone();
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins")
            .join(format!("{name}.lua"));
        let plugin = Plugin::load(
            &path,
            &settings,
            Arc::new(AtomicBool::new(false)),
            Rc::new(move |m: &str| sink.borrow_mut().push(m.to_string())),
            Rc::new(move |d| out.borrow_mut().push(d)),
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
            emitted,
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
                let headers = |list: &[(&str, &str)]| -> mlua::Result<Value> {
                    Ok(Value::Table(lua.create_table_from(list.iter().copied())?))
                };
                Ok(match answer {
                    Some(Answer::File(name)) => (
                        Value::String(lua.create_string(net_fixture(name))?),
                        Value::Integer(200),
                        headers(&[])?,
                    ),
                    Some(Answer::Status(code)) => (
                        Value::String(lua.create_string("")?),
                        Value::Integer(code.into()),
                        headers(&[])?,
                    ),
                    Some(Answer::Headers(code, list)) => (
                        Value::String(lua.create_string("")?),
                        Value::Integer(code.into()),
                        headers(list)?,
                    ),
                    Some(Answer::Offline) | None => (
                        Value::Nil,
                        Value::String(lua.create_string("connection refused")?),
                        Value::Nil,
                    ),
                })
            })
            .unwrap();
        let t: Table = lua.globals().get("telemetrix").unwrap();
        t.set("http_get", get).unwrap();
    }

    /// Replaces one host function with Lua code, like a fake speed test.
    pub fn stub(&self, name: &str, lua_fn: &str) {
        let lua = self.plugin.lua();
        let f: mlua::Function = lua.load(lua_fn).eval().unwrap();
        let t: Table = lua.globals().get("telemetrix").unwrap();
        t.set(name, f).unwrap();
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

    /// Runs the plugin's `search(query)`, as the `s` box does.
    pub fn search(&self, query: &str) -> Result<Vec<super::manifest::SearchOption>, String> {
        self.plugin.search(&self.settings, query)
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
    use crate::plugins::MetricStyle;

    fn routes() -> Vec<(&'static str, Answer)> {
        vec![
            ("api.monobank.ua", Answer::File("mono.json")),
            ("api.privatbank.ua", Answer::File("privat.json")),
            ("valcode=usd", Answer::File("nbu_usd.json")),
            ("valcode=eur", Answer::File("nbu_eur.json")),
            ("valcode=gbp", Answer::File("nbu_gbp.json")),
        ]
    }

    fn rows(d: &PluginData) -> Vec<(&str, &str)> {
        d.metrics
            .iter()
            .map(|m| (m.label.as_str(), m.value.as_str()))
            .collect()
    }

    #[test]
    fn monobank_table_with_quiet_graph_lines() {
        let h = Harness::new("currency", "", &routes());
        let d = h.run(Trigger::Start);
        assert_eq!(d.error, None, "{:?}", h.log.borrow());
        assert_eq!(d.title, "Currency · monobank");
        let r = rows(&d);
        assert_eq!(r[0], ("", "     buy      sell"));
        assert_eq!(r[1], ("USD", "   44.80     45.20"));
        // The 7 NBU points of the fixture, scaled to 8 levels, and 30 points
        // resampled to 7 glyphs.
        assert_eq!(r[2], ("  7d ▁▁▁▂▄▅█ +0.69%", "30d ▃▂▅▂▃▄█ +0.90%"));
        assert_eq!(r[3], ("EUR", "   50.87     51.53"));
        assert_eq!(r[5], ("GBP", "   59.94     cross"));
        assert!(r[7].0.starts_with("updated "));
        assert_eq!(d.metrics.len(), 8);
        let style = |i: usize| d.metrics[i].style;
        assert_eq!(style(0), Some(MetricStyle::Header));
        assert_eq!(style(1), None);
        assert_eq!(style(2), Some(MetricStyle::Dim));
        assert_eq!(style(7), Some(MetricStyle::Dim));
        assert!(
            d.metrics.iter().all(|m| m.trend.is_none()),
            "no full-width graphs"
        );
        println!("{}", render_card(&d, 45));
    }

    #[test]
    fn privatbank_shows_gbp_from_the_nbu() {
        let h = Harness::new("currency", "primary_bank = 'privat'", &routes());
        let d = h.run(Trigger::Start);
        assert_eq!(d.title, "Currency · privatbank");
        let r = rows(&d);
        assert_eq!(r[1], ("USD", "   44.60     45.05"));
        assert_eq!(r[5], ("GBP", "   59.46       NBU"));
        assert!(
            h.log.borrow().is_empty(),
            "no backup note: {:?}",
            h.log.borrow()
        );
        println!("{}", render_card(&d, 45));
    }

    #[test]
    fn the_other_bank_is_the_backup_when_the_primary_has_nothing() {
        let mut routes = routes();
        routes.insert(0, ("api.monobank.ua", Answer::Status(429)));
        let h = Harness::new("currency", "", &routes);
        let d = h.run(Trigger::Start);
        assert_eq!(d.title, "Currency · privatbank (backup)");
        assert_eq!(rows(&d)[1], ("USD", "   44.60     45.05"));
        assert_eq!(
            *h.log.borrow(),
            ["using privatbank as the backup: monobank HTTP 429"]
        );
        h.run(Trigger::Interval);
        assert_eq!(h.log.borrow().len(), 1, "the note is logged once");
        println!("{}", render_card(&d, 45));
    }

    #[test]
    fn a_recent_failure_shows_the_stored_rates_as_stale() {
        let h = Harness::new("currency", "", &routes());
        h.run(Trigger::Start);
        h.age_store("s.mono.tried = s.mono.tried - 301");
        h.route("api.monobank.ua", Answer::Status(429));
        let d = h.run(Trigger::Interval);
        assert_eq!(
            d.title, "Currency · monobank",
            "rates from the last hour stay"
        );
        assert_eq!(rows(&d)[1], ("USD", "   44.80     45.20"));
        let footer = d.metrics.last().unwrap();
        assert!(
            footer.label.starts_with("stale since ") && footer.label.ends_with(" (HTTP 429)"),
            "{footer:?}"
        );
        assert_eq!(footer.style, Some(MetricStyle::Bad));
        println!("{}", render_card(&d, 45));
        // An hour later the backup takes over.
        h.age_store("s.mono.ok = s.mono.ok - 3600 s.mono.tried = s.mono.tried - 301");
        let d = h.run(Trigger::Interval);
        assert_eq!(d.title, "Currency · privatbank (backup)");
        assert!(
            h.log
                .borrow()
                .last()
                .unwrap()
                .contains("monobank HTTP 429, last rates")
        );
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
    fn nothing_stored_and_no_answer_is_an_error() {
        let h = Harness::new("currency", "", &[("", Answer::Status(500))]);
        let d = h.run(Trigger::Start);
        assert_eq!(
            d.error.as_deref(),
            Some("no rates yet: mono HTTP 500, privat HTTP 500")
        );
    }

    #[test]
    fn old_settings_are_ignored_and_a_narrow_card_drops_the_30_days() {
        let h = Harness::new("currency", "compact = true\nshow_month = false", &routes());
        let d = h.run(Trigger::Start);
        assert_eq!(d.error, None);
        assert!(h.log.borrow().is_empty(), "{:?}", h.log.borrow());
        assert_eq!(d.metrics.len(), 8, "the old keys change nothing");
        let narrow = render_card(&d, 40);
        assert!(
            narrow.contains("7d ▁▁▁▂▄▅█ +0.69%") && !narrow.contains("30d"),
            "{narrow}"
        );
        println!("{narrow}");
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
        h.route("ticker/price", Answer::Status(500));
        let d = h.run(Trigger::Interval);
        assert_eq!(h.asked_for("klines"), 6, "an hour later: fetched again");
        assert_eq!(d.error, None);
        assert_eq!(d.metrics[0].value, "84,853 USDT (stale)");
    }

    #[test]
    fn a_429_or_418_stops_all_requests_until_the_next_update() {
        for status in [429, 418] {
            let h = Harness::new("crypto", "", &routes());
            h.run(Trigger::Start);
            h.age_store("for _, x in pairs(s.history) do x.ts = x.ts - 3600 end");
            h.route("ticker/price", Answer::Status(status));
            let before = h.asked.borrow().len();
            let d = h.run(Trigger::Interval);
            assert_eq!(
                h.asked.borrow().len(),
                before + 1,
                "{status}: no klines after it"
            );
            assert_eq!(d.metrics[0].value, "84,853 USDT (stale)", "{status}");
            let why = if status == 429 {
                "slow down"
            } else {
                "HTTP 418"
            };
            assert!(h.log.borrow().last().unwrap().contains(why), "{status}");
            // A klines 429 stops the other coins' klines too.
            let h = Harness::new("crypto", "", &routes());
            h.route("symbol=BTCUSDT", Answer::Status(status));
            h.run(Trigger::Start);
            assert_eq!(h.asked_for("klines"), 1, "{status}: ETH and SOL wait");
        }
    }

    fn lua_int(h: &Harness, code: &str) -> i64 {
        h.plugin.lua().load(code).eval().unwrap()
    }

    /// Seconds from now until the stored ban ends.
    fn ban_left(h: &Harness) -> i64 {
        lua_int(
            h,
            "return (telemetrix.store_get().ban_until or 0) - os.time()",
        )
    }

    fn pause_row(d: &PluginData) -> Option<&crate::plugins::MetricItem> {
        d.metrics
            .iter()
            .find(|m| m.label.starts_with("paused by Binance until "))
    }

    #[test]
    fn a_418_waits_as_long_as_retry_after_says() {
        use crate::plugins::MetricStyle;
        let h = Harness::new("crypto", "", &routes());
        h.run(Trigger::Start);
        h.route(
            "ticker/price",
            Answer::Headers(418, &[("retry-after", "120")]),
        );
        let d = h.run(Trigger::Interval);
        assert!((118..=120).contains(&ban_left(&h)), "{}", ban_left(&h));
        let row = pause_row(&d).expect("a pause row");
        let until: String = h
            .plugin
            .lua()
            .load("return os.date('%H:%M', telemetrix.store_get().ban_until)")
            .eval()
            .unwrap();
        assert_eq!(row.label, format!("paused by Binance until {until}"));
        assert_eq!(
            (row.value.as_str(), row.style),
            ("", Some(MetricStyle::Bad))
        );
        assert_eq!(d.metrics[0].value, "84,853 USDT (stale)");
        assert!(h.log.borrow().last().unwrap().contains("Retry-After 120 s"));
        println!("{}", render_card(&d, 45));

        // While paused, nothing is asked at all.
        let before = h.asked.borrow().len();
        let d = h.run(Trigger::Interval);
        assert_eq!(h.asked.borrow().len(), before);
        assert!(pause_row(&d).is_some());

        // After the wait, one success ends the pause and resets the streak.
        h.age_store("s.ban_until = s.ban_until - 121");
        h.route("ticker/price", Answer::File("binance_price.json"));
        let d = h.run(Trigger::Interval);
        assert_eq!(h.asked.borrow().len(), before + 1);
        assert!(pause_row(&d).is_none());
        assert_eq!(d.metrics[0].value, "84,853 USDT");
        assert_eq!(
            lua_int(&h, "return telemetrix.store_get().ban_streak or 0"),
            0
        );
    }

    #[test]
    fn repeated_418_doubles_the_wait_from_10_minutes_up_to_a_day() {
        let h = Harness::new("crypto", "", &routes());
        h.run(Trigger::Start);
        h.route("ticker/price", Answer::Status(418));
        let mut waits = Vec::new();
        for _ in 0..9 {
            h.run(Trigger::Interval);
            waits.push(ban_left(&h));
            h.age_store("s.ban_until = os.time() - 1");
        }
        let expected = [600, 1200, 2400, 4800, 9600, 19200, 38400, 76800, 86400];
        for (got, want) in waits.iter().zip(expected) {
            assert!(want - 2 <= *got && *got <= want, "{waits:?}");
        }
        assert!(
            h.log
                .borrow()
                .last()
                .unwrap()
                .contains("418 number 9 in a row")
        );
    }

    #[test]
    fn a_418_before_any_price_shows_only_the_pause() {
        let h = Harness::new(
            "crypto",
            "",
            &[("", Answer::Headers(418, &[("retry-after", "7200")]))],
        );
        let d = h.run(Trigger::Start);
        assert_eq!(d.error, None);
        assert_eq!(d.metrics.len(), 1);
        assert!(pause_row(&d).is_some());
        assert!((7198..=7200).contains(&ban_left(&h)));
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

mod weather {
    use super::*;

    fn routes() -> Vec<(&'static str, Answer)> {
        vec![
            ("geocoding-api", Answer::File("geo_kyiv.json")),
            (
                "api.open-meteo.com/v1/forecast",
                Answer::File("forecast.json"),
            ),
        ]
    }

    #[test]
    fn now_and_two_days_for_the_typed_city() {
        let h = Harness::new("weather", "", &routes());
        let d = h.run(Trigger::Start);
        assert_eq!(d.error, None, "{:?}", h.log.borrow());
        assert_eq!(d.title, "Weather · Kyiv, UA");
        let rows: Vec<(&str, &str)> = d
            .metrics
            .iter()
            .map(|m| (m.label.as_str(), m.value.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("now", "15.9 °C  wind 9 km/h  cloudy"),
                ("Sat", "10..19 °C  cloudy  rain 0%"),
                ("Sun", "10..19 °C  cloudy  rain 0%"),
                ("data: Open-Meteo.com", ""),
            ]
        );
        assert_eq!(
            d.metrics[3].style,
            Some(crate::plugins::MetricStyle::Dim),
            "the CC BY 4.0 attribution is a quiet last line"
        );
        assert!(h.asked.borrow()[1].contains("latitude=50.45466&longitude=30.5238"));
        println!("{}", render_card(&d, 45));
    }

    #[test]
    fn fahrenheit_follows_the_units_setting() {
        let mut h = Harness::new("weather", "", &routes());
        h.settings.units.temperature = crate::config::TempUnit::Fahrenheit;
        let d = h.run(Trigger::Settings);
        let values: Vec<&str> = d.metrics.iter().map(|m| m.value.as_str()).collect();
        assert_eq!(
            values[..3],
            [
                "60.6 °F  wind 9 km/h  cloudy",
                "51..66 °F  cloudy  rain 0%",
                "50..65 °F  cloudy  rain 0%"
            ]
        );
    }

    #[test]
    fn geocoding_runs_once_per_city() {
        let mut h = Harness::new("weather", "", &routes());
        h.run(Trigger::Start);
        h.run(Trigger::Interval);
        assert_eq!(
            h.asked_for("geocoding"),
            1,
            "the place comes from the store"
        );
        h.set("city", "'Київ'");
        h.run(Trigger::Key);
        assert_eq!(h.asked_for("geocoding"), 2);
        assert!(
            h.asked_for("name=%D0%9A%D0%B8%D1%97%D0%B2&") == 1,
            "UTF-8 is percent-encoded: {:?}",
            h.asked.borrow()
        );
    }

    #[test]
    fn unknown_city_is_a_card_error() {
        let mut h = Harness::new("weather", "", &routes());
        h.route("geocoding-api", Answer::File("geo_none.json"));
        h.set("city", "'Qwxzzy'");
        let d = h.run(Trigger::Key);
        assert_eq!(d.error.as_deref(), Some("place not found: Qwxzzy"));
        println!("{}", render_card(&d, 45));
        assert_eq!(
            h.asked_for("geocoding"),
            2,
            "as typed, then the prefix qwxz: {:?}",
            h.asked.borrow()
        );
        h.run(Trigger::Interval);
        assert_eq!(h.asked_for("geocoding"), 2, "a miss is cached too");
    }

    const KHM_UK: &str =
        "name=%D0%A5%D0%BC%D0%B5%D0%BB%D1%8C%D0%BD%D0%B8%D1%86%D1%8C%D0%BA%D0%B8%D0%B9&";

    /// Recorded answers (2026-09-27) for the search cases; anything else
    /// finds nothing, as those spellings did live.
    fn search_routes() -> Vec<(&'static str, Answer)> {
        vec![
            ("name=khmelnytskyi&", Answer::File("geo_khmelnytskyi.json")),
            ("name=Khmelnytskyi&", Answer::File("geo_khmelnytskyi.json")),
            (KHM_UK, Answer::File("geo_khmelnytskyi_uk.json")),
            ("name=kiev&", Answer::File("geo_kiev.json")),
            ("name=Kyiv&", Answer::File("geo_kyiv_list.json")),
            ("name=lvov&", Answer::File("geo_lvov.json")),
            ("name=Lviv&", Answer::File("geo_lviv_list.json")),
            ("name=exampletow&", Answer::File("geo_exampletown.json")),
            ("geocoding-api", Answer::File("geo_none.json")),
            (
                "api.open-meteo.com/v1/forecast",
                Answer::File("forecast.json"),
            ),
        ]
    }

    fn labels(options: &[crate::plugins::manifest::SearchOption]) -> Vec<&str> {
        options.iter().map(|o| o.label.as_str()).collect()
    }

    /// The search box as the `s` overlay draws it, `width` columns wide.
    fn render_search(b: &crate::app::SearchBox, width: u16) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let height = 16;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| crate::ui::settings_overlay::draw_search(f, f.area(), b))
            .unwrap();
        let buf = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_misspelled_latin_name_finds_the_official_one() {
        let h = Harness::new("weather", "", &search_routes());
        let found = h.search("hmelnitskiy").unwrap();
        assert_eq!(
            labels(&found),
            [
                "Khmelnytskyi, Khmelnytskyi Oblast, UA",
                "Khmelnytskyi, Luhansk Oblast, UA",
                "Khmelnytskyi, Kirovohrad Oblast, UA",
            ],
            "the airport and the heliport are left out, the biggest place first"
        );
        let values = &found[0].values;
        let get = |k: &str| {
            values
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        use crate::config::Value as V;
        assert_eq!(get("city"), Some(V::Str("Khmelnytskyi".into())));
        assert_eq!(get("lat"), Some(V::Float(49.41835)));
        assert_eq!(get("lon"), Some(V::Float(26.97936)));
        assert_eq!(
            get("place"),
            Some(V::Str("Khmelnytskyi, Khmelnytskyi Oblast, UA".into()))
        );
        let asked: Vec<String> = h.asked.borrow().clone();
        assert_eq!(asked.len(), 3, "typed, kh + yi, then i -> y: {asked:?}");
        assert!(asked[0].contains("name=hmelnitskiy&count=10&language=en"));
        assert!(asked[1].contains("name=khmelnitskyi&"));
        assert!(asked[2].contains("name=khmelnytskyi&"));

        let mut b = crate::app::SearchBox::new("weather", "city", "city");
        let t0 = std::time::Instant::now();
        for c in "hmelnitskiy".chars() {
            b.key(crate::app::InputKey::Char(c), t0);
        }
        let query = b.due(t0 + crate::app::SEARCH_PAUSE).unwrap();
        b.answer(&query, Ok(found));
        let text = render_search(&b, 62);
        println!("{text}");
        assert!(text.contains("city: hmelnitskiy"));
        assert!(text.contains("3 found"));
        assert!(text.contains("> Khmelnytskyi, Khmelnytskyi Oblast, UA"));
        assert!(text.contains("Up/Down choose · Enter save · Esc cancel"));
    }

    #[test]
    fn old_russian_names_find_todays_place_first() {
        let h = Harness::new("weather", "", &search_routes());
        let kiev = h.search("kiev").unwrap();
        assert_eq!(kiev.len(), 8);
        assert_eq!(kiev[0].label, "Kyiv, Kyiv City, UA");
        assert!(
            labels(&kiev).iter().all(|l| l.ends_with(", UA")),
            "the preferred country fills the list: {:?}",
            labels(&kiev)
        );
        let lvov = h.search("lvov").unwrap();
        assert_eq!(
            labels(&lvov)[..2],
            ["Lviv, Lviv Oblast, UA", "Lvove, Kherson Oblast, UA"]
        );
        assert!(!labels(&lvov).iter().any(|l| l.contains("Airport")));
    }

    #[test]
    fn without_a_country_the_biggest_places_come_first() {
        let mut h = Harness::new("weather", "", &search_routes());
        h.set("country", "''");
        let kiev = h.search("kiev").unwrap();
        let list = labels(&kiev);
        assert_eq!(list[0], "Kyiv, Kyiv City, UA");
        assert!(list.contains(&"Kievskiy, Moscow Oblast, RU"), "{list:?}");
        assert!(
            list.iter()
                .position(|l| *l == "Kievskiy, Moscow Oblast, RU")
                < list.iter().position(|l| *l == "Kyivka, Kherson Oblast, UA"),
            "9,700 people before 325: {list:?}"
        );
    }

    #[test]
    fn russian_cyrillic_is_asked_in_ukrainian() {
        let h = Harness::new("weather", "", &search_routes());
        let found = h.search("Хмельницкий").unwrap();
        assert_eq!(labels(&found), ["Хмельницький, Хмельницька область, UA"]);
        let asked = h.asked.borrow();
        assert_eq!(
            asked.len(),
            2,
            "as typed, then the Ukrainian name: {asked:?}"
        );
        assert!(asked.iter().all(|u| u.contains("language=uk")));
    }

    #[test]
    fn nothing_found_tries_shorter_prefixes() {
        let h = Harness::new("weather", "", &search_routes());
        let found = h.search("Exampletownz").unwrap();
        assert_eq!(labels(&found), ["Exampletown, Example Region, XX"]);
        assert!(h.asked.borrow().len() <= 6);
        assert_eq!(h.asked_for("name=exampletow&"), 1);
        assert_eq!(h.search("x").unwrap(), [], "one letter asks nothing");
        let asked = h.asked.borrow().len();
        h.search("Exampletownz").unwrap();
        assert_eq!(h.asked.borrow().len(), asked, "the answers are cached");
    }

    #[test]
    fn a_picked_place_uses_its_coordinates() {
        let h = Harness::new(
            "weather",
            "city = \"Lviv\"\nlat = 49.83826\nlon = 24.02324\nplace = \"Lviv, Lviv Oblast, UA\"",
            &search_routes(),
        );
        let d = h.run(Trigger::Settings);
        assert_eq!(d.error, None);
        assert_eq!(d.title, "Weather · Lviv, UA");
        assert_eq!(h.asked_for("geocoding"), 0);
        assert!(h.asked.borrow()[0].contains("latitude=49.83826&longitude=24.02324"));
        assert!(h.log.borrow().is_empty(), "{:?}", h.log.borrow());
    }

    #[test]
    fn a_city_typed_by_hand_wins_over_an_old_pick() {
        let mut h = Harness::new(
            "weather",
            "city = \"lvov\"\nlat = 50.45\nlon = 30.52\nplace = \"Kyiv, Kyiv City, UA\"",
            &search_routes(),
        );
        let d = h.run(Trigger::Settings);
        assert_eq!(d.error, None);
        assert_eq!(d.title, "Weather · Lviv, UA", "the alias finds Lviv");
        assert_eq!(
            h.log.borrow().as_slice(),
            ["using city lvov; lat/lon are ignored"]
        );
        h.set("city", "'hmelnitskiy'");
        h.set("place", "''");
        let d = h.run(Trigger::Settings);
        assert_eq!(d.title, "Weather · Khmelnytskyi, UA");
    }

    #[test]
    fn lat_lon_apply_only_without_a_city() {
        let h = Harness::new("weather", "lat = 49.84\nlon = 24.03", &routes());
        let d = h.run(Trigger::Start);
        assert_eq!(h.asked_for("geocoding"), 0);
        assert_eq!(d.title, "Weather · 49.84, 24.03");
        assert_eq!(d.metrics[0].value, "49.84, 24.03 from lat/lon");
        assert!(h.asked.borrow()[0].contains("latitude=49.84&longitude=24.03"));
        assert_eq!(
            *h.log.borrow(),
            ["using lat/lon 49.84, 24.03; city is not set"]
        );
    }

    #[test]
    fn a_v01_settings_file_keeps_working_until_a_city_is_typed() {
        // What `config init` wrote in v0.1.
        let mut h = Harness::new(
            "weather",
            "lat = 50.45\nlon = 30.52\nlabel = \"Kyiv\"",
            &[
                ("name=Lviv", Answer::File("geo_kyiv.json")),
                ("geocoding-api", Answer::File("geo_none.json")),
                (
                    "api.open-meteo.com/v1/forecast",
                    Answer::File("forecast.json"),
                ),
            ],
        );
        let d = h.run(Trigger::Start);
        assert_eq!(d.title, "Weather · Kyiv", "label names the coordinates");
        h.run(Trigger::Interval);
        assert_eq!(h.log.borrow().len(), 1, "the note is logged once");
        h.set("city", "'Lviv'");
        let d = h.run(Trigger::Key);
        assert_eq!(d.error, None);
        assert_eq!(h.asked_for("name=Lviv"), 1, "the typed city wins");
        assert!(!d.metrics.iter().any(|m| m.label == "place"));
        assert_eq!(
            d.title, "Weather · Kyiv, UA",
            "label is not used for a city"
        );
        assert_eq!(
            h.log.borrow().last().map(String::as_str),
            Some("using city Lviv; lat/lon are ignored")
        );
    }

    #[test]
    fn wmo_codes_and_weekdays() {
        let h = Harness::new("weather", "", &routes());
        h.run(Trigger::Start);
        let lua = h.plugin.lua();
        lua.globals()
            .set("UPDATE", h.plugin.update_fn().clone())
            .unwrap();
        let words: String = lua
            .load(
                "local w = {} \
                 for _, c in ipairs({ 0, 1, 2, 3, 45, 48, 51, 57, 61, 65, 66, 67, 71, 77, 80, 82, 85, 86, 95, 99 }) do \
                   local t = { current = { temperature_2m = 1, wind_speed_10m = 1, weather_code = c }, \
                     daily = { time = { '2026-09-25', '2000-02-29', '2026-01-01' }, weather_code = { 0, 0, 0 }, \
                     temperature_2m_min = { 0, 0, 0 }, temperature_2m_max = { 1, 1, 1 } } } \
                   telemetrix.json_decode = function() return t end \
                   local card = UPDATE() \
                   w[#w + 1] = card.metrics[1].value:match('km/h  (.*)$') \
                   if c == 0 then w[#w + 1] = card.metrics[2].label .. ' ' .. card.metrics[3].label end \
                 end \
                 return table.concat(w, ',')",
            )
            .eval()
            .unwrap();
        assert_eq!(
            words,
            "clear,Tue Thu,mostly clear,cloudy,cloudy,fog,fog,drizzle,drizzle,rain,rain,\
             freezing rain,freezing rain,snow,snow,showers,showers,snow showers,snow showers,storm,storm"
        );
    }
}

mod speedtest {
    use super::*;

    fn routes() -> Vec<(&'static str, Answer)> {
        vec![("api/js/servers", Answer::File("ookla_servers.json"))]
    }

    /// Fake pings (spt.example-b.net is nearest) and a fake speed_multi that
    /// records its calls, reports progress and fails where FAIL says.
    fn fake(h: &Harness) {
        h.stub(
            "tcp_ping_ms",
            "PINGS = { ['speedtest1.example-a.net'] = 10, \
               ['spt.example-b.net'] = 4, ['speedtest.example-c.net'] = 12, \
               ['speed.cloudflare.com'] = 56 } \
             PINGED = {} \
             return function(host, port) PINGED[#PINGED + 1] = host .. ':' .. port \
               local ms = PINGS[host] if ms then return ms end return nil, 'timed out' end",
        );
        h.stub(
            "speed_multi",
            "CALLS = {} FAIL = {} \
             return function(o) \
               CALLS[#CALLS + 1] = { o.direction, o.url, o.streams, o.seconds, o.warmup } \
               for _, f in ipairs(FAIL) do \
                 if o.direction == f[1] and o.url:find(f[2], 1, true) then return nil, f[3] end \
               end \
               local down = o.direction == 'down' \
               o.progress(down and 912 or 905, 1.5) \
               return { mbps = down and 935 or 933, bytes = 1, seconds = 2.5 } \
             end",
        );
    }

    fn lua(h: &Harness, code: &str) {
        h.plugin.lua().load(code).exec().unwrap();
    }

    fn calls(h: &Harness) -> Vec<(String, String)> {
        h.plugin
            .lua()
            .load("local out = {} for i, c in ipairs(CALLS) do out[i] = c[1] .. ' ' .. c[2] end return out")
            .eval::<Vec<String>>()
            .unwrap()
            .into_iter()
            .map(|c| {
                let (dir, url) = c.split_once(' ').unwrap();
                (dir.to_string(), url.to_string())
            })
            .collect()
    }

    fn call(dir: &str, url: &str) -> (String, String) {
        (dir.to_string(), url.to_string())
    }

    fn row(d: &PluginData, label: &str) -> String {
        d.metrics
            .iter()
            .find(|m| m.label == label)
            .map(|m| m.value.clone())
            .unwrap_or_default()
    }

    #[test]
    fn start_never_tests_and_shows_the_hint() {
        let h = Harness::new("speedtest", "", &routes());
        fake(&h);
        let d = h.run(Trigger::Start);
        assert_eq!(d.metrics.len(), 1);
        assert_eq!(d.metrics[0].label, "press g to test");
        assert!(h.emitted.borrow().is_empty() && h.asked.borrow().is_empty());
        println!("{}", render_card(&d, 45));
    }

    #[test]
    fn nearest_ookla_server_by_ping() {
        let h = Harness::new("speedtest", "", &routes());
        fake(&h);
        let d = h.run(Trigger::Key);
        assert_eq!(d.error, None, "{:?}", h.log.borrow());
        let labels: Vec<&str> = d.metrics.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["down", "up", "ping", "server", "last"]);
        assert_eq!(row(&d, "down"), "935 Mbps");
        assert_eq!(row(&d, "up"), "933 Mbps");
        assert_eq!(row(&d, "ping"), "4 ms");
        assert_eq!(
            row(&d, "server"),
            "Example Broadband Ltd, Springfiel…",
            "cut to fit"
        );
        assert_eq!(
            calls(&h),
            [
                call(
                    "down",
                    "https://spt.example-b.net:8080/download?size=25000000"
                ),
                call("up", "https://spt.example-b.net:8080/upload"),
            ]
        );
        let args: (i64, i64, f64) = h
            .plugin
            .lua()
            .load("return CALLS[1][3], CALLS[1][4], CALLS[1][5]")
            .eval()
            .unwrap();
        assert_eq!(args, (2, 3, 0.5), "streams, seconds and warm-up");
        let pings: Vec<String> = h.plugin.lua().load("return PINGED").eval().unwrap();
        assert_eq!(
            pings.len(),
            5 * 3 + 5,
            "3 per listed server, then 5 to the chosen one"
        );
        assert!(pings.iter().all(|p| p.ends_with(":8080")));
        println!("{}", render_card(&d, 45));
    }

    #[test]
    fn progress_rows_during_the_test() {
        let h = Harness::new("speedtest", "", &routes());
        fake(&h);
        h.run(Trigger::Key);
        let progress: Vec<String> = h
            .emitted
            .borrow()
            .iter()
            .map(|c| format!("{} {}", c.metrics[0].label, c.metrics[0].value))
            .collect();
        for want in ["testing download... 912 Mbps", "testing upload... 905 Mbps"] {
            assert!(progress.contains(&want.to_string()), "{progress:?}");
        }
    }

    #[test]
    fn the_server_is_kept_for_24_hours() {
        let h = Harness::new("speedtest", "", &routes());
        fake(&h);
        h.run(Trigger::Key);
        h.run(Trigger::Interval);
        assert_eq!(
            h.asked_for("api/js/servers"),
            1,
            "the second test reuses the choice"
        );
        h.age_store("s.server.chosen = s.server.chosen - 86401");
        h.run(Trigger::Interval);
        assert_eq!(
            h.asked_for("api/js/servers"),
            2,
            "a day later it chooses again"
        );
    }

    #[test]
    fn a_forced_server_and_forced_cloudflare() {
        let mut h = Harness::new("speedtest", "server = 'my.host:8443'", &routes());
        fake(&h);
        lua(&h, "PINGS['my.host'] = 7");
        let d = h.run(Trigger::Key);
        assert_eq!(h.asked_for("api/js/servers"), 0);
        assert_eq!(row(&d, "server"), "my.host:8443");
        assert_eq!(row(&d, "ping"), "7 ms");
        assert_eq!(
            calls(&h)[0].1,
            "https://my.host:8443/download?size=25000000"
        );
        h.set("server", "'cloudflare'");
        let d = h.run(Trigger::Key);
        assert_eq!(row(&d, "server"), "Cloudflare");
        assert_eq!(row(&d, "ping"), "56 ms");
        assert_eq!(
            calls(&h)[2..],
            [
                call("down", "https://speed.cloudflare.com/__down?bytes=10000000"),
                call("up", "https://speed.cloudflare.com/__up"),
            ]
        );
        assert!(
            h.log.borrow().is_empty(),
            "no backup message: {:?}",
            h.log.borrow()
        );
    }

    #[test]
    fn cloudflare_is_the_backup_when_the_list_fails() {
        let h = Harness::new("speedtest", "", &routes());
        fake(&h);
        h.route("api/js/servers", Answer::Status(503));
        let d = h.run(Trigger::Key);
        assert_eq!(d.error, None);
        assert_eq!(row(&d, "server"), "Cloudflare (backup)");
        assert_eq!(
            *h.log.borrow(),
            ["using Cloudflare as the backup: the Ookla server list failed: HTTP 503"]
        );
        println!("{}", render_card(&d, 45));
    }

    #[test]
    fn cloudflare_is_the_backup_when_the_download_fails() {
        let h = Harness::new("speedtest", "", &routes());
        fake(&h);
        lua(
            &h,
            "FAIL[1] = { 'down', 'spt.example-b.net', 'the server answered HTTP 403' }",
        );
        let d = h.run(Trigger::Key);
        assert_eq!(row(&d, "server"), "Cloudflare (backup)");
        let c = calls(&h);
        assert_eq!(c.len(), 3, "Ookla down, then Cloudflare down and up: {c:?}");
        assert!(h.log.borrow()[0].contains("download from spt.example-b.net:8080 failed"));
        let chosen: bool = h
            .plugin
            .lua()
            .load("return telemetrix.store_get().server ~= nil")
            .eval()
            .unwrap();
        assert!(!chosen, "a failing server is chosen again next time");
    }

    #[test]
    fn upload_failure_alone_does_not_switch_servers() {
        let h = Harness::new("speedtest", "", &routes());
        fake(&h);
        lua(
            &h,
            "FAIL[1] = { 'up', 'spt.example-b.net', 'the server answered HTTP 500' }",
        );
        let d = h.run(Trigger::Key);
        assert_eq!(row(&d, "up"), "failed: the server answered HTTP 500");
        assert_eq!(row(&d, "down"), "935 Mbps");
        assert_eq!(calls(&h).len(), 2, "no Cloudflare repeat");
    }

    #[test]
    fn everything_failing_is_a_card_error() {
        let h = Harness::new("speedtest", "", &routes());
        fake(&h);
        h.route("api/js/servers", Answer::Offline);
        lua(
            &h,
            "FAIL[1] = { 'down', 'cloudflare', 'the server answered HTTP 403' }",
        );
        let d = h.run(Trigger::Key);
        assert_eq!(
            d.error.as_deref(),
            Some(
                "download from speed.cloudflare.com:443 failed: the server answered HTTP 403 \
                 (after: the Ookla server list failed: connection refused)"
            )
        );
    }

    #[test]
    fn old_size_settings_are_ignored_and_history_keeps_30() {
        let h = Harness::new(
            "speedtest",
            "download_mb = 15\nupload_mb = 5\nmax_seconds = 8\nstreams = 99",
            &routes(),
        );
        fake(&h);
        for _ in 0..32 {
            h.run(Trigger::Manual);
        }
        let d = h.run(Trigger::Start);
        assert_eq!(d.error, None);
        assert_eq!(d.metrics.last().unwrap().label, "30 runs");
        let streams: i64 = h.plugin.lua().load("return CALLS[1][3]").eval().unwrap();
        assert_eq!(streams, 2, "out of range falls back to the default");
        let log = h.log.borrow();
        assert_eq!(log.len(), 1, "one warning, only for streams: {log:?}");
        assert!(log[0].starts_with("warning: streams = 99"));
        drop(log);
        assert_eq!(
            h.run(Trigger::Settings).metrics.len(),
            6,
            "a settings change does not test"
        );
    }
}

/// `cargo test --features alloc-stats soak -- --test-threads=1`: runs each
/// plugin thousands of times against recorded answers and checks that
/// neither the Lua heap nor the Rust heap keeps growing.
#[cfg(feature = "alloc-stats")]
mod soak {
    use super::*;
    use crate::alloc_stats::snapshot;

    fn gc(h: &Harness) -> usize {
        let lua = h.plugin.lua();
        lua.gc_collect().unwrap();
        lua.gc_collect().unwrap();
        lua.used_memory()
    }

    fn soak(name: &str, routes: &[(&str, Answer)], trigger: Trigger) {
        let h = Harness::new(name, "", routes);
        let mut state =
            crate::app::AppState::new(Config::default(), crate::config::ConfigStatus::Ok);
        let mut step = |n: usize| {
            for _ in 0..n {
                let d = h.run(trigger);
                h.emitted.borrow_mut().clear();
                // The harness records every URL; that list is not the plugin's memory.
                h.asked.borrow_mut().clear();
                state.apply(crate::event::AppEvent::Plugin(d));
            }
        };
        step(300);
        let (lua0, heap0) = (gc(&h), snapshot().0);
        step(3000);
        let (lua1, heap1) = (gc(&h), snapshot().0);
        println!(
            "{name}: lua {lua0} -> {lua1} ({:+}), heap {heap0} -> {heap1} ({:+}) over 3000 updates",
            lua1 as i64 - lua0 as i64,
            heap1 - heap0
        );
        assert!(lua1 <= lua0 + 4096, "{name}: Lua heap grows");
        assert!(heap1 - heap0 < 64 * 1024, "{name}: Rust heap grows");
    }

    #[test]
    fn soak_each_plugin() {
        soak("clock", &[], Trigger::Interval);
        soak("uptime", &[], Trigger::Interval);
        soak(
            "crypto",
            &[
                ("ticker/price", Answer::File("binance_price.json")),
                ("symbol=BTCUSDT", Answer::File("klines_BTC.json")),
                ("symbol=ETHUSDT", Answer::File("klines_ETH.json")),
                ("symbol=SOLUSDT", Answer::File("klines_SOL.json")),
            ],
            Trigger::Interval,
        );
        soak(
            "currency",
            &[
                ("api.monobank.ua", Answer::File("mono.json")),
                ("api.privatbank.ua", Answer::File("privat.json")),
                ("valcode=usd", Answer::File("nbu_usd.json")),
                ("valcode=eur", Answer::File("nbu_eur.json")),
                ("valcode=gbp", Answer::File("nbu_gbp.json")),
            ],
            Trigger::Interval,
        );
        soak(
            "weather",
            &[
                ("geocoding-api", Answer::File("geo_kyiv.json")),
                (
                    "api.open-meteo.com/v1/forecast",
                    Answer::File("forecast.json"),
                ),
            ],
            Trigger::Interval,
        );
    }
}

#[cfg(feature = "alloc-stats")]
mod soak_parts {
    use super::*;
    use crate::alloc_stats::snapshot;

    fn measure(h: &Harness, what: &str, code: &str) {
        let lua = h.plugin.lua();
        let f: mlua::Function = lua.load(code).eval().unwrap();
        for _ in 0..200 {
            f.call::<()>(()).unwrap();
            h.asked.borrow_mut().clear();
        }
        lua.gc_collect().unwrap();
        let before = snapshot().0;
        for _ in 0..3000 {
            f.call::<()>(()).unwrap();
            h.asked.borrow_mut().clear();
        }
        lua.gc_collect().unwrap();
        lua.gc_collect().unwrap();
        h.asked.borrow_mut().clear();
        println!(
            "{what}: heap {:+} bytes over 3000 calls",
            snapshot().0 - before
        );
    }

    #[test]
    fn soak_host_functions() {
        let h = Harness::new(
            "crypto",
            "",
            &[("ticker/price", Answer::File("binance_price.json"))],
        );
        h.run(Trigger::Start);
        measure(&h, "empty", "return function() end");
        measure(
            &h,
            "http_get (fake)",
            "return function() telemetrix.http_get('https://x/ticker/price') end",
        );
        measure(
            &h,
            "json_decode",
            "local t = '[{\"a\":1,\"b\":\"x\"},{\"a\":2}]' return function() telemetrix.json_decode(t) end",
        );
        measure(
            &h,
            "store_get",
            "return function() telemetrix.store_get() end",
        );
        measure(
            &h,
            "store_set",
            "local s = telemetrix.store_get() return function() telemetrix.store_set(s) end",
        );
        measure(
            &h,
            "os.time/os.date",
            "return function() os.date('%H:%M', os.time()) end",
        );
        measure(
            &h,
            "string.format",
            "return function() string.format('%.2f', 1.5) end",
        );
    }
}
