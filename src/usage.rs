use std::collections::HashMap;
use std::path::Path;

use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub target_id: String,
    pub target_name: String,
    pub timestamp: i64,
    pub success: bool,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct TargetStats {
    pub target_id: String,
    pub target_name: String,
    pub total_calls: u64,
    pub success_count: u64,
    pub error_count: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
}

// --- token extraction helpers ---

pub fn extract_tokens_from_bytes(bytes: &[u8]) -> (u64, u64, u64) {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
        if let Some(usage) = v.get("usage") {
            let prompt = usage.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let completion = usage.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let total = usage.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            return (prompt, completion, total);
        }
    }
    (0, 0, 0)
}

pub fn extract_tokens_from_sse_bytes(bytes: &[u8]) -> (u64, u64, u64) {
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return (0, 0, 0),
    };

    let mut last_usage: Option<(u64, u64, u64)> = None;
    for line in text.lines() {
        let data = line.strip_prefix("data: ").unwrap_or(line).trim();
        if data == "[DONE]" || data.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            if let Some(usage) = v.get("usage") {
                let prompt = usage.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let completion = usage.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let total = usage.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                last_usage = Some((prompt, completion, total));
            }
        }
    }
    last_usage.unwrap_or((0, 0, 0))
}

pub fn extract_tokens_from_value(v: &serde_json::Value) -> (u64, u64, u64) {
    let usage = v.get("usage").and_then(|v| v.as_object());
    let prompt = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total = usage
        .and_then(|u| u.get("total_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    (prompt, completion, total)
}

// --- disk persistence ---

fn day_key_from_timestamp(ts: i64) -> String {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".to_string())
}

fn usage_log_file_for_day(dir: &Path, day_key: &str) -> std::path::PathBuf {
    dir.join(format!("usage-{}.jsonl", day_key))
}

fn usage_log_file_for_timestamp(dir: &Path, ts: i64) -> std::path::PathBuf {
    usage_log_file_for_day(dir, &day_key_from_timestamp(ts))
}

async fn list_all_usage_log_files(dir: &Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut items = Vec::new();
    let mut rd = match fs::read_dir(dir).await {
        Ok(v) => v,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(items),
        Err(err) => return Err(err.into()),
    };

    while let Some(entry) = rd.next_entry().await? {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with("usage-") && name.ends_with(".jsonl") {
            items.push(path);
        }
    }

    items.sort();
    Ok(items)
}

fn try_day_key(ts: i64) -> Option<String> {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
}

fn day_keys_in_range(from: i64, to: i64) -> Option<Vec<String>> {
    let start = try_day_key(from)?;
    let end = try_day_key(to)?;
    let mut day = chrono::NaiveDate::parse_from_str(&start, "%Y-%m-%d").ok()?;
    let end_day = chrono::NaiveDate::parse_from_str(&end, "%Y-%m-%d").ok()?;
    if day > end_day {
        return Some(Vec::new());
    }

    let mut keys = Vec::new();
    while day <= end_day {
        keys.push(day.format("%Y-%m-%d").to_string());
        day = match day.succ_opt() {
            Some(v) => v,
            None => break,
        };
    }
    Some(keys)
}

async fn usage_log_files_for_range(dir: &Path, from: i64, to: i64) -> anyhow::Result<Vec<std::path::PathBuf>> {
    if from <= 0 || to == i64::MAX {
        return list_all_usage_log_files(dir).await;
    }

    let Some(day_keys) = day_keys_in_range(from, to) else {
        return list_all_usage_log_files(dir).await;
    };
    let mut files = Vec::new();
    for day_key in day_keys {
        let path = usage_log_file_for_day(dir, &day_key);
        if fs::metadata(&path).await.is_ok() {
            files.push(path);
        }
    }
    Ok(files)
}

pub async fn append_call_record_to_disk(dir: &Path, record: &CallRecord) -> anyhow::Result<()> {
    fs::create_dir_all(dir).await?;

    let path = usage_log_file_for_timestamp(dir, record.timestamp);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await?;
    let line = serde_json::to_string(record)?;
    file.write_all(line.as_bytes()).await?;
    file.write_all(b"\n").await?;
    Ok(())
}

pub async fn load_call_records_from_disk(dir: &Path, max_records: usize) -> Vec<CallRecord> {
    let files = match list_all_usage_log_files(dir).await {
        Ok(v) => v,
        Err(err) => {
            error!("failed to list usage logs in {}: {}", dir.display(), err);
            return Vec::new();
        }
    };

    let mut items = Vec::new();
    for path in files {
        let body = match fs::read_to_string(&path).await {
            Ok(v) => v,
            Err(err) => {
                error!("failed to read usage log from {}: {}", path.display(), err);
                continue;
            }
        };

        for (idx, line) in body.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<CallRecord>(line) {
                Ok(record) => items.push(record),
                Err(err) => error!(
                    "invalid usage log line {} in {}: {}",
                    idx + 1,
                    path.display(),
                    err
                ),
            }
        }
    }

    if items.len() > max_records {
        let drop_count = items.len() - max_records;
        items.drain(0..drop_count);
    }
    items
}

pub fn apply_record_to_agg(record: &CallRecord, agg: &mut HashMap<String, TargetStats>) {
    let entry = agg.entry(record.target_id.clone()).or_insert(TargetStats {
        target_id: record.target_id.clone(),
        target_name: record.target_name.clone(),
        total_calls: 0,
        success_count: 0,
        error_count: 0,
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
    });
    entry.total_calls += 1;
    if record.success {
        entry.success_count += 1;
    } else {
        entry.error_count += 1;
    }
    entry.prompt_tokens += record.prompt_tokens;
    entry.completion_tokens += record.completion_tokens;
    entry.total_tokens += record.total_tokens;
}

pub async fn aggregate_usage_from_disk(
    dir: &Path,
    from: i64,
    to: i64,
) -> anyhow::Result<HashMap<String, TargetStats>> {
    let files = usage_log_files_for_range(dir, from, to).await?;
    let mut agg: HashMap<String, TargetStats> = HashMap::new();
    for path in files {
        let body = match fs::read_to_string(&path).await {
            Ok(v) => v,
            Err(err) => {
                error!("failed to read usage log from {}: {}", path.display(), err);
                continue;
            }
        };

        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(record) = serde_json::from_str::<CallRecord>(line) {
                if record.timestamp >= from && record.timestamp <= to {
                    apply_record_to_agg(&record, &mut agg);
                }
            }
        }
    }
    Ok(agg)
}
