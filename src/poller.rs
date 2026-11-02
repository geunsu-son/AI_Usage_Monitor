use std::time::{Duration, SystemTime};

use crate::localization::Strings;
use crate::models::{UsageData, UsageSection};
use crate::native_interop;
use crate::providers::{self, CodexError, CursorError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollError {
    AuthRequired,
    NoCredentials,
    TokenExpired,
    NetworkUnavailable,
    RateLimited(Option<u32>),
    ServerError,
    RequestFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageWindowKind {
    Session,
    Weekly,
}

impl PollError {
    pub fn category(self) -> &'static str {
        match self {
            Self::AuthRequired => "login_required",
            Self::NoCredentials => "login_required",
            Self::TokenExpired => "login_required",
            Self::NetworkUnavailable => "unavailable",
            Self::RateLimited(_) => "rate_limited",
            Self::ServerError => "unavailable",
            Self::RequestFailed => "unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialWatchMode {
    ActiveSource,
    AllSources,
    Codex,
    Cursor,
}

pub type CredentialWatchSnapshot = Vec<String>;

pub fn poll_provider(id: usize) -> Result<UsageData, PollError> {
    match id {
        0 => providers::fetch_codex()
            .map(|snapshot| providers::snapshot_to_usage(&snapshot))
            .map_err(map_codex_error),
        1 => providers::fetch_cursor()
            .map(|snapshot| providers::snapshot_to_usage(&snapshot))
            .map_err(map_cursor_error),
        _ => Err(PollError::RequestFailed),
    }
}

fn map_codex_error(error: CodexError) -> PollError {
    match error {
        CodexError::CliMissing => PollError::NoCredentials,
        CodexError::LoginRequired => PollError::AuthRequired,
        CodexError::Timeout => PollError::NetworkUnavailable,
        CodexError::Unavailable => PollError::RequestFailed,
    }
}

fn map_cursor_error(error: CursorError) -> PollError {
    match error {
        CursorError::LoginRequired => PollError::AuthRequired,
        CursorError::RateLimited => PollError::RateLimited(None),
        CursorError::Timeout => PollError::NetworkUnavailable,
        CursorError::Unavailable => PollError::RequestFailed,
    }
}

pub fn claude_code_credentials_available() -> bool {
    false
}

pub fn credential_watch_snapshot(_mode: CredentialWatchMode) -> CredentialWatchSnapshot {
    Vec::new()
}

pub fn format_codex_line(
    section: &UsageSection,
    strings: Strings,
    _show_remaining_in_chinese: bool,
    window: UsageWindowKind,
) -> String {
    if section.percentage == 0.0 && section.resets_at.is_none() {
        return "--".to_string();
    }
    format_line(section, strings, window)
}

pub fn format_line(
    section: &UsageSection,
    strings: Strings,
    _window: UsageWindowKind,
) -> String {
    let remaining = remaining_percentage(section.percentage);
    let pct = format!("{remaining:.0}%");
    let cd = format_countdown(section.resets_at, strings);
    if cd.is_empty() {
        pct
    } else {
        format!("{pct} · {cd}")
    }
}

pub fn format_remaining_only(section: &UsageSection) -> String {
    format!("{:.0}%", remaining_percentage(section.percentage))
}

pub fn remaining_percentage(used_percentage: f64) -> f64 {
    (100.0 - used_percentage).clamp(0.0, 100.0)
}

fn format_countdown(resets_at: Option<SystemTime>, strings: Strings) -> String {
    let reset = match resets_at {
        Some(t) => t,
        None => return String::new(),
    };

    let remaining = match reset.duration_since(SystemTime::now()) {
        Ok(d) => d,
        Err(_) => return strings.now.to_string(),
    };

    format_countdown_from_secs(remaining.as_secs(), strings)
}

pub fn time_until_display_change(resets_at: Option<SystemTime>) -> Option<Duration> {
    let reset = resets_at?;
    let remaining = reset.duration_since(SystemTime::now()).ok()?;
    Some(time_until_display_change_from_secs(remaining.as_secs()))
}

fn format_countdown_from_secs(total_secs: u64, strings: Strings) -> String {
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3600;
    let total_days = total_secs / 86400;

    if total_days >= 1 {
        format!("{total_days}{}", strings.day_suffix)
    } else if total_hours >= 1 {
        format!("{total_hours}{}", strings.hour_suffix)
    } else if total_mins >= 1 {
        format!("{total_mins}{}", strings.minute_suffix)
    } else {
        format!("{total_secs}{}", strings.second_suffix)
    }
}

fn time_until_display_change_from_secs(total_secs: u64) -> Duration {
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3600;
    let total_days = total_secs / 86400;

    let current_bucket_start = if total_days >= 1 {
        total_days * 86400
    } else if total_hours >= 1 {
        total_hours * 3600
    } else if total_mins >= 1 {
        total_mins * 60
    } else {
        total_secs
    };

    Duration::from_secs(total_secs.saturating_sub(current_bucket_start) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_percentage_is_clamped() {
        assert_eq!(remaining_percentage(30.0), 70.0);
        assert_eq!(remaining_percentage(-5.0), 100.0);
        assert_eq!(remaining_percentage(120.0), 0.0);
    }
}
