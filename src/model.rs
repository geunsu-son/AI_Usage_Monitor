use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderId {
    Codex,
    Cursor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuotaWindow {
    pub label: String,
    pub used_percent: Option<f32>,
    pub remaining_percent: Option<f32>,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Ok,
    Stale,
    LoginRequired,
    Unavailable,
    RateLimited,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderDetails {
    pub auto_percent_used: Option<f32>,
    pub api_percent_used: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageSnapshot {
    pub provider: ProviderId,
    pub quotas: Vec<QuotaWindow>,
    pub status: ConnectionStatus,
    pub fetched_at: Option<i64>,
    pub error_category: Option<String>,
    pub details: ProviderDetails,
}

pub fn remaining_percent(used: f32) -> Option<f32> {
    if !used.is_finite() || !(0.0..=100.0).contains(&used) {
        return None;
    }
    Some((100.0 - used).clamp(0.0, 100.0))
}

pub fn window_label(window_duration_mins: Option<u64>) -> String {
    let Some(mins) = window_duration_mins else {
        return "--".to_string();
    };
    if mins == 0 {
        return "--".to_string();
    }
    if mins % 1440 == 0 {
        return format!("{}d", mins / 1440);
    }
    if mins % 60 == 0 {
        return format!("{}h", mins / 60);
    }
    format!("{mins}m")
}

pub fn unix_secs_to_system_time(secs: i64) -> Option<SystemTime> {
    if secs < 0 {
        return None;
    }
    UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs as u64))
}

pub fn parse_billing_cycle_end(raw: &serde_json::Value) -> Option<i64> {
    let n = match raw {
        serde_json::Value::String(s) => s.parse::<i64>().ok()?,
        serde_json::Value::Number(n) => n.as_i64()?,
        _ => return None,
    };
    let abs = n.unsigned_abs();
    if abs > 10_000_000_000 {
        Some(n / 1000)
    } else {
        Some(n)
    }
}
