use crate::metrics::SystemSnapshot;
use crate::metrics::network::NetDrive;
use crate::plugins::PluginData;
use crate::plugins::manifest::SearchOption;
use crate::plugins::schema::SchemaEntry;

/// Messages from worker threads to the main loop.
#[derive(Debug)]
pub enum AppEvent {
    Metrics(SystemSnapshot),
    Plugin(PluginData),
    PluginRemoved(String),
    Log(String),
    /// Sent once after a plugin file loaded: what the main loop must know about it.
    PluginMeta {
        id: String,
        /// The title from the file, for the help overlay.
        title: String,
        run_key: Option<char>,
        schema: Vec<SchemaEntry>,
    },
    /// One round of the Windows network-drive thread.
    Network(Vec<NetDrive>),
    /// A plugin's answer to `WorkerCmd::Search`.
    SearchResults {
        id: String,
        query: String,
        result: Result<Vec<SearchOption>, String>,
    },
}

/// Messages from the main loop to one worker; `R` is that worker's settings.
#[derive(Debug)]
pub enum WorkerCmd<R> {
    Reconfigure(R),
    /// Do the work now instead of at the next interval (plugin run keys).
    RunNow,
    /// Run the plugin's `search(query)` for a search setting in the `s` box.
    Search(String),
    Stop,
}
