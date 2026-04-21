use std::{collections::HashMap, path::PathBuf, sync::Arc, sync::atomic::AtomicUsize};

use reqwest::Client;
use tokio::sync::RwLock;

use crate::config::RouterConfig;
use crate::usage::CallRecord;

#[derive(Clone)]
pub struct AppState {
    pub cfg_path: PathBuf,
    pub usage_log_dir: PathBuf,
    pub cfg: Arc<RwLock<RouterConfig>>,
    pub rr_index: Arc<AtomicUsize>,
    pub group_rr_index: Arc<RwLock<HashMap<String, usize>>>,
    pub http_client: Client,
    pub upstream_timeout_secs: u64,
    pub call_records: Arc<RwLock<Vec<CallRecord>>>,
    pub max_call_records: usize,
}
