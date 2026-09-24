pub mod host_api;
pub mod manager;
pub mod manifest;
pub mod runner;
pub mod sandbox;

#[derive(Clone, Debug, PartialEq)]
pub struct MetricItem {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginData {
    pub id: String,
    pub title: String,
    pub metrics: Vec<MetricItem>,
    /// Set when loading or `update()` failed; `metrics` is then empty.
    pub error: Option<String>,
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
