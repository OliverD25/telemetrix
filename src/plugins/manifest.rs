use mlua::{Function, Table, Value};

use super::schema::{self, SchemaEntry, SchemaKind};
use super::{MetricItem, MetricStyle};
use crate::config::Value as Setting;

/// Trend lines outside this length become an error line.
pub const TREND_POINTS: std::ops::RangeInclusive<usize> = 2..=400;

/// What a plugin file returns: `{ id?, title, interval?, update = function() ... end }`.
pub struct PluginManifest {
    pub id: String,
    pub title: String,
    pub interval: Option<u64>,
    /// A key that runs `update()` at once, like `g` for the speed test.
    pub run_key: Option<char>,
    /// Seconds for one `update()` call, capped at `MAX_CALL_TIMEOUT`.
    pub call_timeout: Option<u64>,
    /// Settings the `s` overlay shows and the runner checks, sorted by key.
    pub settings_schema: Vec<SchemaEntry>,
    pub update: Function,
    /// `search(query)` for `kind = "search"` settings.
    pub search: Option<Function>,
}

pub const MAX_CALL_TIMEOUT: u64 = 60;

impl PluginManifest {
    pub fn from_table(t: &Table, file_stem: &str) -> Result<Self, String> {
        let id = match t.get::<Value>("id").map_err(|e| e.to_string())? {
            Value::Nil => file_stem.to_string(),
            Value::String(s) if !s.to_string_lossy().is_empty() => s.to_string_lossy(),
            _ => return Err("id must be a non-empty string".into()),
        };
        let title = match t.get::<Value>("title").map_err(|e| e.to_string())? {
            Value::Nil => id.clone(),
            Value::String(s) => s.to_string_lossy(),
            _ => return Err("title must be a string".into()),
        };
        let interval = match t.get::<Value>("interval").map_err(|e| e.to_string())? {
            Value::Nil => None,
            Value::Integer(n) if n >= 1 => Some(n as u64),
            Value::Number(n) if n >= 1.0 => Some(n as u64),
            _ => return Err("interval must be a number of seconds, 1 or more".into()),
        };
        let run_key = match t.get::<Value>("run_key").map_err(|e| e.to_string())? {
            Value::Nil => None,
            Value::String(s) => {
                let s = s.to_string_lossy();
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if !c.is_control() && c != ' ' => Some(c),
                    _ => return Err("run_key must be one printable character".into()),
                }
            }
            _ => return Err("run_key must be a one-character string".into()),
        };
        let call_timeout = match t.get::<Value>("call_timeout").map_err(|e| e.to_string())? {
            Value::Nil => None,
            Value::Integer(n) if n >= 1 => Some((n as u64).min(MAX_CALL_TIMEOUT)),
            Value::Number(n) if n >= 1.0 => Some((n as u64).min(MAX_CALL_TIMEOUT)),
            _ => return Err("call_timeout must be a number of seconds, 1 or more".into()),
        };
        let settings_schema = match t
            .get::<Value>("settings_schema")
            .map_err(|e| e.to_string())?
        {
            Value::Nil => Vec::new(),
            Value::Table(s) => schema::parse(&s)?,
            _ => return Err("settings_schema must be a table".into()),
        };
        let update = match t.get::<Value>("update").map_err(|e| e.to_string())? {
            Value::Function(f) => f,
            _ => return Err("the returned table needs an update function".into()),
        };
        let search = match t.get::<Value>("search").map_err(|e| e.to_string())? {
            Value::Nil => None,
            Value::Function(f) => Some(f),
            _ => return Err("search must be a function".into()),
        };
        if search.is_none()
            && let Some(e) = settings_schema
                .iter()
                .find(|e| e.kind == SchemaKind::Search)
        {
            return Err(format!(
                "settings_schema.{} is a search setting, so the returned table needs a search function",
                e.key
            ));
        }
        Ok(Self {
            id,
            title,
            interval,
            run_key,
            call_timeout,
            settings_schema,
            update,
            search,
        })
    }
}

/// The most places `search(query)` may offer; more are left out.
pub const SEARCH_MAX: usize = 8;

/// One choice from `search(query)`: what the list shows and the settings
/// that picking it saves.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchOption {
    pub label: String,
    pub values: Vec<(String, Setting)>,
}

/// Reads `{ { label = "...", values = { key = value, ... } }, ... }`.
pub fn search_options(v: Value) -> Result<Vec<SearchOption>, String> {
    let Value::Table(list) = v else {
        return Err("search() must return a list of { label, values }".into());
    };
    let mut out = Vec::new();
    for (i, item) in list.sequence_values::<Value>().take(SEARCH_MAX).enumerate() {
        let n = i + 1;
        let Ok(Value::Table(item)) = item else {
            return Err(format!("search result {n} must be a table"));
        };
        let label = match item.get::<Value>("label").map_err(|e| e.to_string())? {
            Value::String(s) => s.to_string_lossy(),
            _ => return Err(format!("search result {n} needs a text label")),
        };
        let Value::Table(values) = item.get::<Value>("values").map_err(|e| e.to_string())? else {
            return Err(format!("search result {n} needs a values table"));
        };
        let mut settings = Vec::new();
        for pair in values.pairs::<Value, Value>() {
            let (k, v) = pair.map_err(|e| e.to_string())?;
            let key = match k {
                Value::String(s) => s.to_string_lossy(),
                _ => return Err(format!("search result {n}: values keys must be names")),
            };
            if !schema::is_key(&key) || key == "enabled" || key == "interval" {
                return Err(format!(
                    "search result {n}: {key:?} cannot be a setting name"
                ));
            }
            let value = match v {
                Value::String(s) => Setting::Str(s.to_string_lossy().into()),
                Value::Integer(i) => Setting::Int(i),
                Value::Number(f) if f.is_finite() => Setting::Float(f),
                Value::Boolean(b) => Setting::Bool(b),
                _ => {
                    return Err(format!(
                        "search result {n}: {key} must be text, a number or true/false"
                    ));
                }
            };
            settings.push((key, value));
        }
        if settings.is_empty() {
            return Err(format!("search result {n} has no values"));
        }
        settings.sort_by(|a, b| a.0.cmp(&b.0));
        out.push(SearchOption {
            label,
            values: settings,
        });
    }
    Ok(out)
}

/// What `update()` returns: `{ title?, metrics = { { label = "...", value = ... }, ... } }`.
/// `title` lets a card name depend on settings, like "Weather · Kyiv".
pub struct CardUpdate {
    pub title: Option<String>,
    pub metrics: Vec<MetricItem>,
}

fn to_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.to_string_lossy()),
        Value::Integer(n) => Some(n.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A list of 2..=400 finite numbers, or `None`.
fn trend_points(t: &Table) -> Option<Vec<f32>> {
    let points: Option<Vec<f32>> = t
        .sequence_values::<Value>()
        .map(|v| match v.ok()? {
            Value::Integer(n) => Some(n as f32),
            Value::Number(n) if n.is_finite() => Some(n as f32),
            _ => None,
        })
        .collect();
    points.filter(|p| TREND_POINTS.contains(&p.len()))
}

impl CardUpdate {
    pub fn from_value(v: Value) -> Result<Self, String> {
        let Value::Table(t) = v else {
            return Err("update() must return a table".into());
        };
        let list: Table = match t.get::<Value>("metrics").map_err(|e| e.to_string())? {
            Value::Table(list) => list,
            _ => return Err("update() must return { metrics = { ... } }".into()),
        };
        let title = match t.get::<Value>("title").map_err(|e| e.to_string())? {
            Value::Nil => None,
            Value::String(s) => Some(s.to_string_lossy()),
            _ => return Err("title must be a string".into()),
        };
        let mut metrics = Vec::new();
        for (i, item) in list.sequence_values::<Value>().enumerate() {
            let Ok(Value::Table(item)) = item else {
                return Err(format!("metrics[{}] must be a table", i + 1));
            };
            let label = item.get::<Value>("label").ok().as_ref().and_then(to_text);
            let value = item.get::<Value>("value").ok().as_ref().and_then(to_text);
            let (Some(label), Some(value)) = (label, value) else {
                return Err(format!("metrics[{}] needs a label and a value", i + 1));
            };
            let mut metric = MetricItem::text(label, value);
            if let Ok(Value::String(style)) = item.get::<Value>("style") {
                metric.style = MetricStyle::parse(&style.to_string_lossy());
            }
            match item.get::<Value>("trend").map_err(|e| e.to_string())? {
                Value::Nil => {}
                Value::Table(points) => match trend_points(&points) {
                    Some(points) => metric.trend = Some(points),
                    None => {
                        metric.value = format!(
                            "trend needs {}..{} numbers",
                            TREND_POINTS.start(),
                            TREND_POINTS.end()
                        );
                        metric.bad = true;
                    }
                },
                _ => {
                    metric.value = "trend must be a list of numbers".into();
                    metric.bad = true;
                }
            }
            metrics.push(metric);
        }
        Ok(Self { title, metrics })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    #[test]
    fn manifest_defaults_and_errors() {
        let lua = Lua::new();
        let t: Table = lua
            .load("return { update = function() end }")
            .eval()
            .unwrap();
        let m = PluginManifest::from_table(&t, "clock").unwrap();
        assert_eq!(
            (m.id.as_str(), m.title.as_str(), m.interval),
            ("clock", "clock", None)
        );
        let t: Table = lua
            .load("return { id = 'x', title = 'X', interval = 30, update = function() end }")
            .eval()
            .unwrap();
        let m = PluginManifest::from_table(&t, "file").unwrap();
        assert_eq!(
            (m.id.as_str(), m.title.as_str(), m.interval),
            ("x", "X", Some(30))
        );
        let t: Table = lua.load("return { title = 'no update' }").eval().unwrap();
        assert!(PluginManifest::from_table(&t, "f").is_err());
        let t: Table = lua
            .load("return { interval = 0, update = print }")
            .eval()
            .unwrap();
        assert!(PluginManifest::from_table(&t, "f").is_err());
    }

    #[test]
    fn run_key_and_call_timeout() {
        let lua = Lua::new();
        let t: Table = lua
            .load("return { run_key = 'g', call_timeout = 90, update = print }")
            .eval()
            .unwrap();
        let m = PluginManifest::from_table(&t, "speed").unwrap();
        assert_eq!(m.run_key, Some('g'));
        assert_eq!(m.call_timeout, Some(60), "capped at 60 s");
        for bad in [
            "run_key = 'gg'",
            "run_key = ''",
            "run_key = 1",
            "call_timeout = 0",
        ] {
            let t: Table = lua
                .load(format!("return {{ {bad}, update = print }}"))
                .eval()
                .unwrap();
            assert!(PluginManifest::from_table(&t, "x").is_err(), "{bad}");
        }
    }

    #[test]
    fn card_update_reads_metrics() {
        let lua = Lua::new();
        let v: Value = lua
            .load("return { metrics = { { label = 'a', value = 1.5 }, { label = 'b', value = 'x' } } }")
            .eval()
            .unwrap();
        let card = CardUpdate::from_value(v).unwrap();
        assert_eq!(card.metrics[0].value, "1.5");
        assert_eq!(card.metrics[1].label, "b");
        assert_eq!(card.title, None);
        let v: Value = lua
            .load("return { title = 'T', metrics = {} }")
            .eval()
            .unwrap();
        assert_eq!(
            CardUpdate::from_value(v).unwrap().title.as_deref(),
            Some("T")
        );
        let v: Value = lua
            .load("return { metrics = { { label = '7d', value = '+1%', trend = { 1, 2.5, 3 } } } }")
            .eval()
            .unwrap();
        let card = CardUpdate::from_value(v).unwrap();
        assert_eq!(card.metrics[0].trend.as_deref(), Some(&[1.0, 2.5, 3.0][..]));
        assert!(!card.metrics[0].bad);
        let v: Value = lua
            .load("return { metrics = { { label = 'x', value = '1', trend = { 5 } }, { label = 'y', value = 2 } } }")
            .eval()
            .unwrap();
        let card = CardUpdate::from_value(v).unwrap();
        assert!(
            card.metrics[0].bad && card.metrics[0].value.contains("2..400"),
            "one point is too few"
        );
        assert!(!card.metrics[1].bad, "the other rows still work");
        let v: Value = lua
            .load(
                "return { metrics = { { label = '', value = 'buy', style = 'header' }, \
                 { label = 'x', value = 1, style = 'sparkly' }, { label = 'y', value = 2, style = 5 } } }",
            )
            .eval()
            .unwrap();
        let card = CardUpdate::from_value(v).unwrap();
        assert_eq!(card.metrics[0].style, Some(MetricStyle::Header));
        assert_eq!(card.metrics[1].style, None, "unknown styles are ignored");
        assert_eq!(card.metrics[2].style, None);
        let v: Value = lua
            .load("return { metrics = { { label = 'a' } } }")
            .eval()
            .unwrap();
        assert!(CardUpdate::from_value(v).is_err());
        assert!(CardUpdate::from_value(Value::Nil).is_err());
    }

    #[test]
    fn a_search_setting_needs_a_search_function() {
        let lua = Lua::new();
        let schema = "settings_schema = { city = { kind = 'search', label = 'city' } }";
        let t: Table = lua
            .load(format!("return {{ {schema}, update = print }}"))
            .eval()
            .unwrap();
        let err = PluginManifest::from_table(&t, "w").err().unwrap();
        assert!(err.contains("needs a search function"), "{err}");
        let t: Table = lua
            .load(format!(
                "return {{ {schema}, update = print, search = print }}"
            ))
            .eval()
            .unwrap();
        let m = PluginManifest::from_table(&t, "w").unwrap();
        assert!(m.search.is_some());
        assert_eq!(m.settings_schema[0].kind, SchemaKind::Search);
        let t: Table = lua
            .load("return { update = print, search = 5 }")
            .eval()
            .unwrap();
        assert!(PluginManifest::from_table(&t, "w").is_err());
    }

    #[test]
    fn search_options_are_read_and_capped() {
        let lua = Lua::new();
        let v: Value = lua
            .load(
                "local out = {} \
                 for i = 1, 10 do out[i] = { label = 'Place ' .. i, \
                   values = { city = 'Place', lat = 50.5, pop = i, exact = true } } end \
                 return out",
            )
            .eval()
            .unwrap();
        let options = search_options(v).unwrap();
        assert_eq!(options.len(), SEARCH_MAX);
        assert_eq!(options[1].label, "Place 2");
        assert_eq!(
            options[1].values,
            [
                ("city".to_string(), Setting::Str("Place".into())),
                ("exact".to_string(), Setting::Bool(true)),
                ("lat".to_string(), Setting::Float(50.5)),
                ("pop".to_string(), Setting::Int(2)),
            ]
        );
        for bad in [
            "5",
            "{ 5 }",
            "{ { values = { a = 1 } } }",
            "{ { label = 'x' } }",
            "{ { label = 'x', values = {} } }",
            "{ { label = 'x', values = { enabled = false } } }",
            "{ { label = 'x', values = { ['a.b'] = 1 } } }",
            "{ { label = 'x', values = { a = {} } } }",
        ] {
            let v: Value = lua.load(format!("return {bad}")).eval().unwrap();
            assert!(search_options(v).is_err(), "{bad}");
        }
        let v: Value = lua.load("return {}").eval().unwrap();
        assert_eq!(search_options(v).unwrap(), [], "no places is not an error");
    }
}
