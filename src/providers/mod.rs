mod codex;
mod cursor;

pub use codex::{fetch_codex, CodexError};
pub use cursor::{fetch_cursor, CursorError};

use crate::model::{unix_secs_to_system_time, UsageSnapshot};
use crate::models::UsageData;

pub fn snapshot_to_usage(snapshot: &UsageSnapshot) -> UsageData {
    let mut data = UsageData::default();
    if let Some(q) = snapshot.quotas.first() {
        if let Some(used) = q.used_percent {
            data.session.percentage = used as f64;
        }
        data.session.resets_at = q.resets_at.and_then(unix_secs_to_system_time);
    }
    if let Some(q) = snapshot.quotas.get(1) {
        if let Some(used) = q.used_percent {
            data.weekly.percentage = used as f64;
        }
        data.weekly.resets_at = q.resets_at.and_then(unix_secs_to_system_time);
    }
    data
}

pub fn empty_usage() -> UsageData {
    UsageData::default()
}
