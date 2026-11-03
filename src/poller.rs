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
    precise: bool,
) -> String {
    if section.percentage == 0.0 && section.resets_at.is_none() {
        return "--".to_string();
    }
    format_line(section, strings, window, precise)
}

pub fn format_line(
    section: &UsageSection,
    strings: Strings,
    window: UsageWindowKind,
    precise: bool,
) -> String {
    format_line_at(section, strings, window, precise, SystemTime::now())
}

fn format_line_at(
    section: &UsageSection,
    strings: Strings,
    window: UsageWindowKind,
    precise: bool,
    now: SystemTime,
) -> String {
    let remaining = remaining_percentage(section.percentage);
    let pct = format!("{remaining:.0}%");
    let cd = format_reset_text(section.resets_at, now, window, precise, strings);
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

/// Local civil time. Formatting tests pass this in directly so they do not
/// depend on the machine time zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalClock {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

fn format_reset_text(
    resets_at: Option<SystemTime>,
    now: SystemTime,
    _window: UsageWindowKind,
    precise: bool,
    strings: Strings,
) -> String {
    let reset = match resets_at {
        Some(reset) => reset,
        None => return String::new(),
    };
    let remaining = match reset.duration_since(now) {
        Ok(remaining) => remaining,
        Err(_) => return strings.now.to_string(),
    };
    let total_secs = remaining.as_secs();
    if !precise {
        return format_countdown_from_secs(total_secs, strings);
    }
    // Session/auto and weekly/API both use the absolute local reset clock when
    // precise mode is on (e.g. `15:40` or `11/04 01:05`).
    match (local_clock(reset), local_clock(now)) {
        (Some(reset_clock), Some(now_clock)) => format_precise_clock(reset_clock, now_clock),
        _ => format_countdown_from_secs(total_secs, strings),
    }
}

fn local_clock(value: SystemTime) -> Option<LocalClock> {
    let local = native_interop::system_time_to_local(value)?;
    Some(LocalClock {
        year: local.wYear,
        month: local.wMonth as u8,
        day: local.wDay as u8,
        hour: local.wHour as u8,
        minute: local.wMinute as u8,
        second: local.wSecond as u8,
    })
}

/// Absolute reset clock for every precise row. Today is `HH:MM`. Another day
/// in this year is `MM/DD HH:MM`. A different year keeps the year.
pub fn format_precise_clock(reset: LocalClock, now: LocalClock) -> String {
    let time = format!("{:02}:{:02}", reset.hour, reset.minute);
    if reset.year == now.year && reset.month == now.month && reset.day == now.day {
        time
    } else if reset.year == now.year {
        format!("{:02}/{:02} {time}", reset.month, reset.day)
    } else {
        format!("{:04}-{:02}-{:02} {time}", reset.year, reset.month, reset.day)
    }
}

pub fn time_until_display_change(
    resets_at: Option<SystemTime>,
    window: UsageWindowKind,
    precise: bool,
) -> Option<Duration> {
    let reset = resets_at?;
    let now = SystemTime::now();
    let remaining = reset.duration_since(now).ok()?;
    Some(time_until_reset_change(
        remaining.as_secs(),
        window,
        precise,
        local_clock(reset),
        local_clock(now),
    ))
}

fn time_until_reset_change(
    total_secs: u64,
    _window: UsageWindowKind,
    precise: bool,
    reset_clock: Option<LocalClock>,
    now_clock: Option<LocalClock>,
) -> Duration {
    if !precise {
        return time_until_display_change_from_secs(total_secs);
    }
    match (reset_clock, now_clock) {
        (Some(reset_clock), Some(now_clock)) => {
            time_until_precise_clock_change(total_secs, reset_clock, now_clock)
        }
        // Without a local clock the absolute time cannot be known, so wake
        // on the next minute and try again.
        _ => time_until_minute_boundary(total_secs),
    }
}

/// The absolute clock string changes at local midnight (the date prefix) or
/// when the reset passes. It does not change on other minute boundaries.
fn time_until_precise_clock_change(total_secs: u64, reset: LocalClock, now: LocalClock) -> Duration {
    let same_day = reset.year == now.year && reset.month == now.month && reset.day == now.day;
    let until_reset = total_secs.saturating_add(1).max(1);
    if same_day {
        return Duration::from_secs(until_reset);
    }
    let elapsed = u64::from(now.hour) * 3_600 + u64::from(now.minute) * 60 + u64::from(now.second);
    let until_midnight = (24 * 3_600_u64).saturating_sub(elapsed).max(1);
    Duration::from_secs(until_midnight.min(until_reset).max(1))
}

fn time_until_minute_boundary(total_secs: u64) -> Duration {
    let into_minute = total_secs % 60;
    let until = if into_minute == 0 { 60 } else { into_minute };
    Duration::from_secs(until + 1)
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

    fn clock(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> LocalClock {
        LocalClock {
            year,
            month,
            day,
            hour,
            minute,
            second: 0,
        }
    }

    #[test]
    fn precise_clock_shows_today_as_hours_and_minutes() {
        let now = clock(2026, 11, 3, 11, 24);
        assert_eq!(format_precise_clock(clock(2026, 11, 3, 15, 40), now), "15:40");
        assert_eq!(
            format_precise_clock(clock(2026, 11, 4, 1, 5), now),
            "11/04 01:05"
        );
        assert_eq!(
            format_precise_clock(clock(2027, 1, 2, 9, 0), now),
            "2027-01-02 09:00"
        );
    }

    #[test]
    fn precise_clock_refreshes_at_midnight_or_reset_not_every_minute() {
        let now = clock(2026, 11, 3, 23, 50);
        let tomorrow = clock(2026, 11, 4, 0, 10);
        assert_eq!(
            time_until_precise_clock_change(20 * 60, tomorrow, now),
            Duration::from_secs(10 * 60)
        );
        let later_today = clock(2026, 11, 3, 15, 40);
        assert_eq!(
            time_until_precise_clock_change(4 * 3_600, later_today, clock(2026, 11, 3, 11, 24)),
            Duration::from_secs(4 * 3_600 + 1)
        );
    }

    #[test]
    fn relative_line_keeps_day_buckets_and_precise_weekly_uses_the_clock() {
        let strings = crate::localization::LanguageId::English.strings();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let reset = now + Duration::from_secs(3 * 86_400 + 5 * 3_600);
        let section = UsageSection {
            percentage: 30.0,
            resets_at: Some(reset),
        };
        assert_eq!(
            format_line_at(&section, strings, UsageWindowKind::Weekly, false, now),
            "70% · 3d"
        );
        let expected = match (local_clock(reset), local_clock(now)) {
            (Some(reset_clock), Some(now_clock)) => {
                format!("70% · {}", format_precise_clock(reset_clock, now_clock))
            }
            _ => "70% · 3d".to_string(),
        };
        assert_eq!(
            format_line_at(&section, strings, UsageWindowKind::Weekly, true, now),
            expected
        );
        assert_eq!(
            format_line_at(
                &UsageSection {
                    percentage: 10.0,
                    resets_at: None,
                },
                strings,
                UsageWindowKind::Session,
                true,
                now,
            ),
            "90%"
        );
    }
}
