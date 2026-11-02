use crate::model::{
    parse_billing_cycle_end, remaining_percent, ConnectionStatus, ProviderDetails, ProviderId,
    QuotaWindow, UsageSnapshot,
};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const CURSOR_USAGE_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";

pub fn fetch_cursor() -> Result<UsageSnapshot, CursorError> {
    let token = read_access_token()?;
    let response = post_usage(&token)?;
    Ok(parse_period_usage(&response))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorError {
    LoginRequired,
    Unavailable,
    RateLimited,
    Timeout,
}

impl CursorError {
    pub fn category(self) -> &'static str {
        match self {
            Self::LoginRequired => "login_required",
            Self::Unavailable => "unavailable",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
        }
    }
}

fn cursor_state_db() -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(
        PathBuf::from(appdata)
            .join("Cursor")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb"),
    )
}

fn read_access_token() -> Result<String, CursorError> {
    let path = cursor_state_db().ok_or(CursorError::LoginRequired)?;
    if !path.is_file() {
        return Err(CursorError::LoginRequired);
    }
    let conn = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| CursorError::Unavailable)?;
    let token: String = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| CursorError::LoginRequired)?;
    if token.trim().is_empty() {
        return Err(CursorError::LoginRequired);
    }
    Ok(token)
}

fn post_usage(token: &str) -> Result<serde_json::Value, CursorError> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10))
        .build();
    let response = agent
        .post(CURSOR_USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .set("Connect-Protocol-Version", "1")
        .send_string("{}")
        .map_err(map_ureq_error)?;
    let text = response
        .into_string()
        .map_err(|_| CursorError::Unavailable)?;
    serde_json::from_str(&text).map_err(|_| CursorError::Unavailable)
}

fn map_ureq_error(error: ureq::Error) -> CursorError {
    if let ureq::Error::Status(code, resp) = error {
        if code == 401 || code == 403 {
            return CursorError::LoginRequired;
        }
        if code == 429 {
            return CursorError::RateLimited;
        }
        let _ = resp;
        return CursorError::Unavailable;
    }
    CursorError::Unavailable
}

fn parse_used_percent(plan: Option<&serde_json::Value>, key: &str) -> Option<f32> {
    plan.and_then(|p| p.get(key))
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
}

pub fn parse_period_usage(value: &serde_json::Value) -> UsageSnapshot {
    let plan = value.get("planUsage");
    let cycle_end = value.get("billingCycleEnd").and_then(parse_billing_cycle_end);

    let auto_raw = parse_used_percent(plan, "autoPercentUsed");
    let api = parse_used_percent(plan, "apiPercentUsed");
    let total = parse_used_percent(plan, "totalPercentUsed").or_else(|| {
        let limit = plan?.get("limit")?.as_f64()? as f32;
        let remaining = plan?.get("remaining")?.as_f64()? as f32;
        if limit > 0.0 && remaining >= 0.0 {
            Some(((limit - remaining) / limit * 100.0).clamp(0.0, 100.0))
        } else {
            None
        }
    });
    let mut auto = auto_raw;
    if auto.is_none() {
        auto = total;
    }

    let has_data = auto.is_some() || api.is_some();
    let status = if has_data {
        ConnectionStatus::Ok
    } else {
        ConnectionStatus::Unavailable
    };

    let auto_window = QuotaWindow {
        label: "auto".to_string(),
        used_percent: auto,
        remaining_percent: auto.and_then(remaining_percent),
        resets_at: cycle_end,
    };
    let api_window = QuotaWindow {
        label: "api".to_string(),
        used_percent: api,
        remaining_percent: api.and_then(remaining_percent),
        resets_at: cycle_end,
    };

    UsageSnapshot {
        provider: ProviderId::Cursor,
        quotas: vec![auto_window, api_window],
        status,
        fetched_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs() as i64),
        error_category: if has_data {
            None
        } else {
            Some("unavailable".to_string())
        },
        details: ProviderDetails {
            auto_percent_used: auto_raw,
            api_percent_used: api,
        },
    }
}

pub fn cursor_error_to_category(error: CursorError) -> &'static str {
    error.category()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_total_percent() {
        let json = serde_json::json!({
            "billingCycleEnd": "1771077734000",
            "planUsage": {"totalPercentUsed": 29.0, "autoPercentUsed": 1.0, "apiPercentUsed": 46.0}
        });
        let snap = parse_period_usage(&json);
        assert_eq!(snap.quotas[0].used_percent, Some(1.0));
        assert_eq!(snap.quotas[0].remaining_percent, Some(99.0));
        assert_eq!(snap.quotas[1].used_percent, Some(46.0));
        assert_eq!(snap.quotas[1].remaining_percent, Some(54.0));
        assert_eq!(snap.quotas[0].resets_at, Some(1771077734));
    }

    #[test]
    fn derives_from_limit_remaining() {
        let json = serde_json::json!({
            "planUsage": {"limit": 40000, "remaining": 16778}
        });
        let snap = parse_period_usage(&json);
        assert!(snap.quotas[0].used_percent.unwrap() > 58.0);
    }

    #[test]
    #[ignore]
    fn live_fetch_cursor() {
        let result = fetch_cursor();
        assert!(
            result.is_ok(),
            "cursor live probe failed: {result:?} (enable Cursor login and TLS)"
        );
    }
}
