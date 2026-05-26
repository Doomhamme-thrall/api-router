use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use reqwest::Client;
use tokio::sync::RwLock;

use crate::config::RouterConfig;
use crate::usage::{CallRecord, GroupUsageRecord};

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
    pub group_usage: Arc<RwLock<HashMap<String, VecDeque<GroupUsageRecord>>>>,
    pub group_usage_log_dir: PathBuf,
}
