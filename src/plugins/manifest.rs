use mlua::{Function, Table, Value};

use super::MetricItem;

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
    pub update: Function,
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
        let update = match t.get::<Value>("update").map_err(|e| e.to_string())? {
            Value::Function(f) => f,
            _ => return Err("the returned table needs an update function".into()),
        };
        Ok(Self {
            id,
            title,
            interval,
            run_key,
            call_timeout,
            update,
        })
    }
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
            .load("return { metrics = { { label = 'a' } } }")
            .eval()
            .unwrap();
        assert!(CardUpdate::from_value(v).is_err());
        assert!(CardUpdate::from_value(Value::Nil).is_err());
    }
}
