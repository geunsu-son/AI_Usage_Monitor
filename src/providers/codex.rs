use crate::model::{
    remaining_percent, window_label, ConnectionStatus, ProviderDetails, ProviderId, QuotaWindow,
    UsageSnapshot,
};
use serde::Deserialize;
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub fn parse_rate_limits_json(value: &serde_json::Value) -> Result<Vec<QuotaWindow>, &'static str> {
    let snapshot = value
        .get("rateLimitsByLimitId")
        .and_then(|m| m.get("codex"))
        .or_else(|| value.get("rateLimits"));
    let Some(snapshot) = snapshot else {
        return Err("missing_limits");
    };
    let primary = snapshot.get("primary");
    let secondary = snapshot.get("secondary");
    let mut quotas = Vec::new();
    if let Some(w) = primary.filter(|w| !w.is_null()) {
        quotas.push(parse_window(w)?);
    }
    if let Some(w) = secondary.filter(|w| !w.is_null()) {
        quotas.push(parse_window(w)?);
    }
    if quotas.is_empty() {
        return Err("empty_windows");
    }
    Ok(quotas)
}

fn parse_window(window: &serde_json::Value) -> Result<QuotaWindow, &'static str> {
    let used = window
        .get("usedPercent")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .filter(|v| v.is_finite() && (0.0..=100.0).contains(v));
    let duration = window
        .get("windowDurationMins")
        .and_then(|v| v.as_u64());
    let resets_at = window
        .get("resetsAt")
        .and_then(|v| v.as_i64())
        .filter(|v| *v >= 0);
    Ok(QuotaWindow {
        label: window_label(duration),
        used_percent: used,
        remaining_percent: used.and_then(remaining_percent),
        resets_at,
    })
}

pub fn snapshot_from_quotas(quotas: Vec<QuotaWindow>) -> UsageSnapshot {
    UsageSnapshot {
        provider: ProviderId::Codex,
        quotas,
        status: ConnectionStatus::Ok,
        fetched_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs() as i64),
        error_category: None,
        details: ProviderDetails::default(),
    }
}

#[derive(Deserialize)]
struct ResponseHeader {
    id: Option<u64>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: i64,
}

#[derive(Deserialize)]
struct AccountResult {
    account: Option<AccountDto>,
}

#[derive(Deserialize)]
struct AccountDto {
    #[serde(rename = "type")]
    account_type: String,
}

#[derive(Deserialize)]
struct RateLimitsEnvelope {
    #[serde(rename = "rateLimits")]
    rate_limits: serde_json::Value,
}

const MAX_JSONL_FRAME: usize = 1024 * 1024;
const SESSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn fetch_codex() -> Result<UsageSnapshot, CodexError> {
    let cli = locate_codex_cli()?;
    run_app_server_session(&cli)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexError {
    CliMissing,
    LoginRequired,
    Unavailable,
    Timeout,
}

impl CodexError {
    pub fn category(self) -> &'static str {
        match self {
            Self::CliMissing => "cli_missing",
            Self::LoginRequired => "login_required",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
        }
    }
}

fn known_codex_bin_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(
            std::path::PathBuf::from(local)
                .join("OpenAI")
                .join("Codex")
                .join("bin"),
        );
    }
    if let Ok(pf) = std::env::var("ProgramFiles") {
        dirs.push(
            std::path::PathBuf::from(pf)
                .join("OpenAI")
                .join("Codex")
                .join("bin"),
        );
    }
    dirs
}

fn known_codex_install_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    for bin in known_codex_bin_dirs() {
        paths.push(bin.join("codex.exe"));
    }
    if let Ok(roaming) = std::env::var("APPDATA") {
        paths.push(
            std::path::PathBuf::from(roaming)
                .join("npm")
                .join("codex.cmd"),
        );
    }
    if let Ok(pf) = std::env::var("ProgramFiles") {
        paths.push(
            std::path::PathBuf::from(pf)
                .join("OpenAI")
                .join("Codex")
                .join("codex.exe"),
        );
    }
    paths
}

fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn push_codex_candidate(
    candidates: &mut Vec<(std::path::PathBuf, Option<std::time::SystemTime>)>,
    path: std::path::PathBuf,
) {
    if path.is_file() {
        let mtime = file_mtime(&path);
        candidates.push((path, mtime));
    }
}

/// Codex desktop installs may place `codex.exe` directly under `bin\` or in a
/// content-addressed subdirectory such as `bin\<hash>\codex.exe`.
fn scan_codex_bin_dir(
    bin_dir: &std::path::Path,
    candidates: &mut Vec<(std::path::PathBuf, Option<std::time::SystemTime>)>,
) {
    push_codex_candidate(candidates, bin_dir.join("codex.exe"));
    let Ok(entries) = std::fs::read_dir(bin_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            push_codex_candidate(candidates, path.join("codex.exe"));
        }
    }
}

fn locate_codex_cli() -> Result<std::path::PathBuf, CodexError> {
    use std::path::PathBuf;
    let mut candidates: Vec<(PathBuf, Option<std::time::SystemTime>)> = Vec::new();
    for path in known_codex_install_paths() {
        push_codex_candidate(&mut candidates, path);
    }
    for bin in known_codex_bin_dirs() {
        scan_codex_bin_dir(&bin, &mut candidates);
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in ["codex.exe", "codex.cmd", "codex.ps1"] {
                push_codex_candidate(&mut candidates, dir.join(name));
            }
        }
    }
    for name in ["codex.exe", "codex", "codex.cmd", "codex.ps1"] {
        let mut cmd = std::process::Command::new("where.exe");
        cmd.arg(name)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        if let Ok(output) = cmd.output() {
            if output.status.success() {
                for line in String::from_utf8_lossy(&output.stdout).lines() {
                    let line = line.trim();
                    if !line.is_empty() {
                        push_codex_candidate(&mut candidates, PathBuf::from(line));
                    }
                }
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_string_lossy().to_ascii_lowercase().cmp(&b.0.to_string_lossy().to_ascii_lowercase()))
    });
    let mut deduped = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (path, _) in candidates {
        let key = path.to_string_lossy().to_ascii_lowercase();
        if seen.insert(key) {
            deduped.push(path);
        }
    }
    deduped.into_iter().next().ok_or(CodexError::CliMissing)
}

#[cfg(test)]
mod locate_tests {
    use super::*;

    #[test]
    fn finds_codex_under_local_appdata_openai() {
        let Some(local) = std::env::var("LOCALAPPDATA").ok().map(std::path::PathBuf::from) else {
            return;
        };
        let bin = local.join("OpenAI").join("Codex").join("bin");
        let flat = bin.join("codex.exe");
        let nested = std::fs::read_dir(&bin).ok().and_then(|entries| {
            entries.flatten().find_map(|entry| {
                let path = entry.path().join("codex.exe");
                path.is_file().then_some(path)
            })
        });
        if !flat.is_file() && nested.is_none() {
            return;
        }
        let resolved = locate_codex_cli().expect("codex should resolve when installed");
        assert!(
            resolved.ends_with("codex.exe") || resolved.ends_with("codex.cmd") || resolved.ends_with("codex.ps1"),
            "unexpected codex path: {}",
            resolved.display()
        );
        assert!(resolved.is_file(), "resolved path missing: {}", resolved.display());
    }
}

fn run_app_server_session(program: &std::path::Path) -> Result<UsageSnapshot, CodexError> {
    let deadline = Instant::now() + SESSION_TIMEOUT;
    let mut cmd = Command::new(program);
    cmd.args(["app-server"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd.spawn().map_err(|_| CodexError::Unavailable)?;
    let mut stdin = child.stdin.take().ok_or(CodexError::Unavailable)?;
    let stdout = child.stdout.take().ok_or(CodexError::Unavailable)?;
    let mut reader = std::io::BufReader::new(stdout);

    let mut next_id = 1u64;
    send_line(
        &mut stdin,
        &serde_json::json!({
            "id": next_id,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "ai-usage-monitor",
                    "title": "AI Usage Monitor",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
    )?;
    read_result_id(&mut reader, next_id, deadline)?;
    next_id += 1;
    send_line(
        &mut stdin,
        &serde_json::json!({"method": "initialized", "params": {}}),
    )?;

    send_line(
        &mut stdin,
        &serde_json::json!({
            "id": next_id,
            "method": "account/read",
            "params": {"refreshToken": false}
        }),
    )?;
    let account = read_json_result(&mut reader, next_id, deadline)?;
    next_id += 1;
    let account_type = account
        .get("account")
        .and_then(|a| a.get("type"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    if account_type.is_empty() {
        let _ = child.kill();
        return Err(CodexError::LoginRequired);
    }

    send_line(
        &mut stdin,
        &serde_json::json!({
            "id": next_id,
            "method": "account/rateLimits/read"
        }),
    )?;
    let limits = read_json_result(&mut reader, next_id, deadline)?;
    let _ = child.kill();

    let quotas = parse_rate_limits_json(&limits).map_err(|_| CodexError::Unavailable)?;
    Ok(snapshot_from_quotas(quotas))
}

fn send_line(
    stdin: &mut impl Write,
    value: &serde_json::Value,
) -> Result<(), CodexError> {
    let mut line = serde_json::to_string(value).map_err(|_| CodexError::Unavailable)?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .map_err(|_| CodexError::Unavailable)?;
    Ok(())
}

fn read_result_id(
    reader: &mut impl BufRead,
    expected_id: u64,
    deadline: Instant,
) -> Result<(), CodexError> {
    loop {
        if Instant::now() >= deadline {
            return Err(CodexError::Timeout);
        }
        let line = read_line(reader)?;
        let header: ResponseHeader =
            serde_json::from_str(&line).map_err(|_| CodexError::Unavailable)?;
        if header.id != Some(expected_id) {
            continue;
        }
        if header.error.is_some() {
            return Err(CodexError::Unavailable);
        }
        return Ok(());
    }
}

fn read_json_result(
    reader: &mut impl BufRead,
    expected_id: u64,
    deadline: Instant,
) -> Result<serde_json::Value, CodexError> {
    loop {
        if Instant::now() >= deadline {
            return Err(CodexError::Timeout);
        }
        let line = read_line(reader)?;
        let v: serde_json::Value =
            serde_json::from_str(&line).map_err(|_| CodexError::Unavailable)?;
        if v.get("id").and_then(|id| id.as_u64()) != Some(expected_id) {
            continue;
        }
        if v.get("error").is_some() {
            return Err(CodexError::Unavailable);
        }
        return v
            .get("result")
            .cloned()
            .ok_or(CodexError::Unavailable);
    }
}

fn read_line(reader: &mut impl BufRead) -> Result<String, CodexError> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|_| CodexError::Unavailable)?;
    if line.len() > MAX_JSONL_FRAME {
        return Err(CodexError::Unavailable);
    }
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_buckets() {
        let json: serde_json::Value = serde_json::json!({
            "rateLimitsByLimitId": {
                "codex": {
                    "primary": {"usedPercent": 36.0, "windowDurationMins": 300, "resetsAt": 1700000000},
                    "secondary": {"usedPercent": 18.0, "windowDurationMins": 10080, "resetsAt": 1700003600}
                }
            }
        });
        let quotas = parse_rate_limits_json(&json).unwrap();
        assert_eq!(quotas[0].label, "5h");
        assert_eq!(quotas[0].remaining_percent, Some(64.0));
        assert_eq!(quotas[1].label, "7d");
    }
}
