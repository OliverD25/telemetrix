pub mod host_api;
pub mod manager;
pub mod manifest;
pub mod runner;
pub mod sandbox;

#[derive(Clone, Debug, PartialEq)]
pub struct MetricItem {
    pub label: String,
    pub value: String,
    /// Points for a sparkline between the label and the value (2..=400).
    pub trend: Option<Vec<f32>>,
    /// The plugin sent something invalid for this row; the value explains it.
    pub bad: bool,
}

impl MetricItem {
    pub fn text(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            trend: None,
            bad: false,
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
