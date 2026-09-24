use crate::metrics::SystemSnapshot;
use crate::plugins::PluginData;

/// Messages from worker threads to the main loop.
#[derive(Debug)]
pub enum AppEvent {
    Metrics(SystemSnapshot),
    Plugin(PluginData),
    PluginRemoved(String),
    Log(String),
}

/// Messages from the main loop to one worker; `R` is that worker's settings.
#[derive(Debug)]
pub enum WorkerCmd<R> {
    Reconfigure(R),
    Stop,
}
