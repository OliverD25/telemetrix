//! `settings_schema`: the settings a plugin declares, so the `s` overlay can
//! show them as rows and wrong values in the file fall back to a default.

use mlua::{Table, Value as Lua};

use crate::config::Value;

/// The longest text the overlay input accepts.
pub const TEXT_MAX: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum SchemaKind {
    Text,
    /// Text chosen from a list the plugin's `search(query)` function
    /// returns; picking one saves all of its `values`.
    Search,
    Enum(Vec<String>),
    Bool,
    Int {
        min: i64,
        max: i64,
        step: i64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchemaEntry {
    pub key: String,
    pub label: String,
    pub kind: SchemaKind,
    pub default: Value,
}

impl SchemaEntry {
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            SchemaKind::Text => "text",
            SchemaKind::Search => "search",
            SchemaKind::Enum(_) => "enum",
            SchemaKind::Bool => "bool",
            SchemaKind::Int { .. } => "int",
        }
    }

    /// The file value when it fits this entry, else `Err` with the reason.
    pub fn check(&self, v: &toml_edit::Value) -> Result<Value, String> {
        match (&self.kind, v) {
            (SchemaKind::Text | SchemaKind::Search, toml_edit::Value::String(s)) => {
                Ok(Value::Str(s.value().clone().into()))
            }
            (SchemaKind::Text | SchemaKind::Search, _) => Err("is not text".into()),
            (SchemaKind::Bool, toml_edit::Value::Boolean(b)) => Ok(Value::Bool(*b.value())),
            (SchemaKind::Bool, _) => Err("is not true or false".into()),
            (SchemaKind::Enum(options), toml_edit::Value::String(s))
                if options.iter().any(|o| o == s.value()) =>
            {
                Ok(Value::Str(s.value().clone().into()))
            }
            (SchemaKind::Enum(options), _) => Err(format!("is not one of {}", options.join(", "))),
            (SchemaKind::Int { min, max, .. }, toml_edit::Value::Integer(n))
                if (*min..=*max).contains(n.value()) =>
            {
                Ok(Value::Int(*n.value()))
            }
            (SchemaKind::Int { min, max, .. }, _) => {
                Err(format!("is not a whole number in {min}..{max}"))
            }
        }
    }

    /// The value in effect: the file's when it is valid, else the default.
    pub fn current(&self, settings: Option<&toml_edit::Table>) -> Value {
        settings
            .and_then(|t| t.get(&self.key))
            .and_then(toml_edit::Item::as_value)
            .and_then(|v| self.check(v).ok())
            .unwrap_or_else(|| self.default.clone())
    }
}

pub fn is_key(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn int_of(v: &Lua) -> Option<i64> {
    match v {
        Lua::Integer(n) => Some(*n),
        Lua::Number(n) if n.fract() == 0.0 && n.is_finite() => Some(*n as i64),
        _ => None,
    }
}

fn text_of(v: &Lua) -> Option<String> {
    match v {
        Lua::String(s) => Some(s.to_string_lossy()),
        _ => None,
    }
}

/// Reads `settings_schema = { key = { kind = ..., ... } }`, sorted by key.
pub fn parse(t: &Table) -> Result<Vec<SchemaEntry>, String> {
    let mut out = Vec::new();
    for pair in t.pairs::<Lua, Lua>() {
        let (k, v) = pair.map_err(|e| e.to_string())?;
        let key = text_of(&k).ok_or("settings_schema keys must be setting names")?;
        if !is_key(&key) {
            return Err(format!(
                "settings_schema: {key:?} is not a setting name (a-z, 0-9, _ and -)"
            ));
        }
        if key == "enabled" || key == "interval" {
            return Err(format!("settings_schema: {key} is reserved"));
        }
        let Lua::Table(spec) = v else {
            return Err(format!("settings_schema.{key} must be a table"));
        };
        out.push(entry(&key, &spec).map_err(|e| format!("settings_schema.{key}: {e}"))?);
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

fn entry(key: &str, spec: &Table) -> Result<SchemaEntry, String> {
    let get = |name: &str| spec.get::<Lua>(name).map_err(|e| e.to_string());
    let kind_name = text_of(&get("kind")?).ok_or("kind must be text, search, enum, bool or int")?;
    let label = match get("label")? {
        Lua::Nil => key.to_string(),
        v => text_of(&v).ok_or("label must be text")?,
    };
    let default = get("default")?;
    let (kind, default) = match kind_name.as_str() {
        "text" | "search" => {
            let d = match default {
                Lua::Nil => String::new(),
                v => text_of(&v).ok_or("default must be text")?,
            };
            let kind = if kind_name == "text" {
                SchemaKind::Text
            } else {
                SchemaKind::Search
            };
            (kind, Value::Str(d.into()))
        }
        "bool" => {
            let d = match default {
                Lua::Nil => false,
                Lua::Boolean(b) => b,
                _ => return Err("default must be true or false".into()),
            };
            (SchemaKind::Bool, Value::Bool(d))
        }
        "enum" => {
            let Lua::Table(list) = get("options")? else {
                return Err("an enum needs options = { \"a\", \"b\", ... }".into());
            };
            let options: Vec<String> = list
                .sequence_values::<Lua>()
                .map(|v| v.ok().as_ref().and_then(text_of))
                .collect::<Option<_>>()
                .ok_or("options must be a list of text")?;
            if options.is_empty() {
                return Err("options must not be empty".into());
            }
            let d = match default {
                Lua::Nil => options[0].clone(),
                v => text_of(&v)
                    .filter(|d| options.contains(d))
                    .ok_or("default must be one of the options")?,
            };
            (SchemaKind::Enum(options), Value::Str(d.into()))
        }
        "int" => {
            let min = int_of(&get("min")?).ok_or("an int needs a whole number min")?;
            let max = int_of(&get("max")?).ok_or("an int needs a whole number max")?;
            if min > max {
                return Err("min is above max".into());
            }
            let step = match get("step")? {
                Lua::Nil => 1,
                v => int_of(&v)
                    .filter(|s| *s >= 1)
                    .ok_or("step must be a whole number, 1 or more")?,
            };
            let d = match default {
                Lua::Nil => min,
                v => int_of(&v)
                    .filter(|d| (min..=max).contains(d))
                    .ok_or("default must be a whole number in min..max")?,
            };
            (SchemaKind::Int { min, max, step }, Value::Int(d))
        }
        other => {
            return Err(format!(
                "unknown kind {other:?}, use text, search, enum, bool or int"
            ));
        }
    };
    Ok(SchemaEntry {
        key: key.to_string(),
        label,
        kind,
        default,
    })
}

/// The table the plugin sees: schema keys checked (a missing or wrong value
/// becomes the default), other keys unchanged. `warn` gets one message per
/// wrong value.
pub fn apply(
    schema: &[SchemaEntry],
    file: &toml_edit::Table,
    mut warn: impl FnMut(String),
) -> toml_edit::Table {
    let mut out = file.clone();
    for e in schema {
        let value = match file.get(&e.key).and_then(toml_edit::Item::as_value) {
            None => e.default.clone(),
            Some(v) => e.check(v).unwrap_or_else(|why| {
                warn(format!(
                    "{} = {} {why}, using {}",
                    e.key,
                    v.to_string().trim(),
                    e.default
                ));
                e.default.clone()
            }),
        };
        out.insert(&e.key, toml_edit::Item::Value(value.to_toml()));
    }
    out
}

/// The next value one press gives an int row: `step` (ten steps with
/// Shift), wrapping at both ends like the other overlay rows.
pub fn step_int(v: i64, min: i64, max: i64, step: i64, dir: i32, big: bool) -> i64 {
    let step = step * if big { 10 } else { 1 };
    if dir > 0 {
        if v >= max { min } else { (v + step).min(max) }
    } else if v <= min {
        max
    } else {
        (v - step).max(min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua as State;

    fn schema(src: &str) -> Result<Vec<SchemaEntry>, String> {
        let lua = State::new();
        let t: Table = lua.load(format!("return {src}")).eval().unwrap();
        parse(&t)
    }

    #[test]
    fn parses_every_kind_sorted_by_key() {
        let s = schema(
            "{ city = { kind = 'text', label = 'City', default = 'Kyiv' },
               bank = { kind = 'enum', options = { 'mono', 'privat' }, default = 'privat' },
               month = { kind = 'bool', default = true },
               mb = { kind = 'int', min = 5, max = 100, step = 5, default = 25 } }",
        )
        .unwrap();
        let keys: Vec<&str> = s.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["bank", "city", "mb", "month"]);
        assert_eq!(s[0].default, Value::Str("privat".into()));
        assert_eq!(s[1].label, "City");
        assert_eq!(
            s[2].kind,
            SchemaKind::Int {
                min: 5,
                max: 100,
                step: 5
            }
        );
        assert_eq!(s[3].default, Value::Bool(true));
        assert_eq!(s[3].label, "month", "the label defaults to the key");
        let d =
            schema("{ t = { kind = 'text' }, e = { kind = 'enum', options = { 'x' } } }").unwrap();
        assert_eq!(d[0].default, Value::Str("x".into()));
        assert_eq!(d[1].default, Value::Str("".into()));
    }

    #[test]
    fn rejects_broken_schemas() {
        for bad in [
            "{ a = { kind = 'color' } }",
            "{ a = { } }",
            "{ a = 5 }",
            "{ enabled = { kind = 'bool' } }",
            "{ ['a.b'] = { kind = 'bool' } }",
            "{ a = { kind = 'enum', options = {} } }",
            "{ a = { kind = 'enum', options = { 'x' }, default = 'y' } }",
            "{ a = { kind = 'int', min = 1 } }",
            "{ a = { kind = 'int', min = 5, max = 1 } }",
            "{ a = { kind = 'int', min = 1, max = 5, default = 9 } }",
            "{ a = { kind = 'int', min = 1, max = 5, step = 0 } }",
            "{ a = { kind = 'bool', default = 'yes' } }",
        ] {
            assert!(schema(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn file_values_are_checked_and_fall_back() {
        let s = schema(
            "{ bank = { kind = 'enum', options = { 'mono', 'privat' } },
               mb = { kind = 'int', min = 5, max = 100, default = 25 },
               month = { kind = 'bool', default = true },
               city = { kind = 'text', default = 'Kyiv' } }",
        )
        .unwrap();
        let file: toml_edit::DocumentMut =
            "bank = 'sber'\nmb = 500\ncity = 'Lviv'\ncoins = ['BTC']\n"
                .parse()
                .unwrap();
        let mut warnings = Vec::new();
        let out = apply(&s, file.as_table(), |w| warnings.push(w));
        assert_eq!(out["bank"].as_str(), Some("mono"), "wrong: the default");
        assert_eq!(
            out["mb"].as_integer(),
            Some(25),
            "out of range: the default"
        );
        assert_eq!(out["month"].as_bool(), Some(true), "missing: the default");
        assert_eq!(out["city"].as_str(), Some("Lviv"), "valid: kept");
        assert_eq!(
            out["coins"].as_array().map(|a| a.len()),
            Some(1),
            "not in the schema: passed through"
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(
            warnings[0],
            "bank = 'sber' is not one of mono, privat, using \"mono\""
        );
        assert!(warnings[1].starts_with("mb = 500 is not a whole number in 5..100"));
    }

    #[test]
    fn int_rows_step_by_the_schema_step_and_wrap() {
        assert_eq!(step_int(25, 5, 100, 5, 1, false), 30);
        assert_eq!(step_int(100, 5, 100, 5, 1, false), 5);
        assert_eq!(step_int(5, 5, 100, 5, -1, false), 100);
        assert_eq!(step_int(25, 5, 100, 5, 1, true), 75);
        assert_eq!(step_int(98, 5, 100, 5, 1, false), 100);
    }
}
