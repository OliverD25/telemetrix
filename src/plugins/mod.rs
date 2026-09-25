pub mod bundled;
#[cfg(test)]
mod bundled_tests;
pub mod host_api;
pub mod manager;
pub mod manifest;
pub mod runner;
pub mod sandbox;
pub mod schema;
pub mod speed;
pub mod store;

#[derive(Clone, Debug, PartialEq)]
pub struct MetricItem {
    pub label: String,
    pub value: String,
    /// Points for a sparkline between the label and the value (2..=400).
    pub trend: Option<Vec<f32>>,
    /// The plugin sent something invalid for this row; the value explains it.
    pub bad: bool,
    /// How the row looks, from the metric's optional `style` field.
    pub style: Option<MetricStyle>,
}

/// `style = "dim" | "header" | "good" | "bad"` on a metric.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricStyle {
    /// Quieter than normal rows; dropped value when the row is too narrow.
    Dim,
    /// A column header above other rows.
    Header,
    /// The value (or, without a value, the label) in the rising colour.
    Good,
    /// The value (or, without a value, the label) in the warning colour.
    Bad,
}

impl MetricStyle {
    /// `None` for unknown names: a newer plugin must still work here.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "dim" => Some(Self::Dim),
            "header" => Some(Self::Header),
            "good" => Some(Self::Good),
            "bad" => Some(Self::Bad),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Dim => "dim",
            Self::Header => "header",
            Self::Good => "good",
            Self::Bad => "bad",
        }
    }
}

impl MetricItem {
    pub fn text(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            trend: None,
            bad: false,
            style: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginData {
    pub id: String,
    pub title: String,
    pub metrics: Vec<MetricItem>,
    /// Set when loading or `update()` failed; `metrics` is then empty.
    pub error: Option<String>,
    /// The plugin's Lua memory when the data was sent.
    pub lua_bytes: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PluginStatus {
    Ok,
    Error(String),
}

/// What the dashboard shows for one plugin: its last good data and status.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginCard {
    pub data: PluginData,
    pub status: PluginStatus,
}
