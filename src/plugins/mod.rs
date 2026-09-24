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
