use mlua::{Function, Table, Value};

use super::MetricItem;

/// What a plugin file returns: `{ id?, title, interval?, update = function() ... end }`.
pub struct PluginManifest {
    pub id: String,
    pub title: String,
    pub interval: Option<u64>,
    pub update: Function,
}

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
        let update = match t.get::<Value>("update").map_err(|e| e.to_string())? {
            Value::Function(f) => f,
            _ => return Err("the returned table needs an update function".into()),
        };
        Ok(Self {
            id,
            title,
            interval,
            update,
        })
    }
}

/// What `update()` returns: `{ metrics = { { label = "...", value = ... }, ... } }`.
pub struct CardUpdate {
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

impl CardUpdate {
    pub fn from_value(v: Value) -> Result<Self, String> {
        let Value::Table(t) = v else {
            return Err("update() must return a table".into());
        };
        let list: Table = match t.get::<Value>("metrics").map_err(|e| e.to_string())? {
            Value::Table(list) => list,
            _ => return Err("update() must return { metrics = { ... } }".into()),
        };
        let mut metrics = Vec::new();
        for (i, item) in list.sequence_values::<Value>().enumerate() {
            let Ok(Value::Table(item)) = item else {
                return Err(format!("metrics[{}] must be a table", i + 1));
            };
            let label = item.get::<Value>("label").ok().as_ref().and_then(to_text);
            let value = item.get::<Value>("value").ok().as_ref().and_then(to_text);
            match (label, value) {
                (Some(label), Some(value)) => metrics.push(MetricItem { label, value }),
                _ => return Err(format!("metrics[{}] needs a label and a value", i + 1)),
            }
        }
        Ok(Self { metrics })
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
    fn card_update_reads_metrics() {
        let lua = Lua::new();
        let v: Value = lua
            .load("return { metrics = { { label = 'a', value = 1.5 }, { label = 'b', value = 'x' } } }")
            .eval()
            .unwrap();
        let card = CardUpdate::from_value(v).unwrap();
        assert_eq!(card.metrics[0].value, "1.5");
        assert_eq!(card.metrics[1].label, "b");
        let v: Value = lua
            .load("return { metrics = { { label = 'a' } } }")
            .eval()
            .unwrap();
        assert!(CardUpdate::from_value(v).is_err());
        assert!(CardUpdate::from_value(Value::Nil).is_err());
    }
}
