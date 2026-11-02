//! Headless probes for local provider connectivity (no GUI).

use crate::model::UsageSnapshot;
use crate::providers::{self, CodexError, CursorError};
use std::io::Write;

pub fn handle_probe_args(args: &[String]) -> Option<i32> {
    let pos = args.iter().position(|a| a == "--probe")?;
    let target = args.get(pos + 1).map(String::as_str).unwrap_or("all");
    attach_console();
    let mut log = ProbeLog::open();
    let code = match target {
        "codex" => run_codex_probe(&mut log),
        "cursor" => run_cursor_probe(&mut log),
        "all" => {
            let c = run_codex_probe(&mut log);
            let u = run_cursor_probe(&mut log);
            if c != 0 || u != 0 {
                1
            } else {
                0
            }
        }
        other => {
            log.line(format!(
                "Unknown probe target {other:?}. Use: codex, cursor, or all."
            ));
            2
        }
    };
    log.line(format!("probe exit code {code}"));
    Some(code)
}

struct ProbeLog {
    path: std::path::PathBuf,
}

impl ProbeLog {
    fn open() -> Self {
        let path = std::env::temp_dir().join("ai-usage-probe.txt");
        let _ = std::fs::write(&path, "ai-usage-monitor probe\n");
        Self { path }
    }

    fn line(&mut self, message: impl AsRef<str>) {
        let message = message.as_ref();
        let _ = writeln!(std::io::stderr(), "{message}");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{message}");
        }
    }
}

fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            let _ = AllocConsole();
        }
    }
}

fn run_codex_probe(log: &mut ProbeLog) -> i32 {
    log.line("--- codex ---");
    match providers::fetch_codex() {
        Ok(snapshot) => {
            print_snapshot(log, "codex", &snapshot);
            0
        }
        Err(error) => {
            log.line(format!(
                "codex probe failed: {error:?} ({})",
                codex_error_hint(error)
            ));
            1
        }
    }
}

fn run_cursor_probe(log: &mut ProbeLog) -> i32 {
    log.line("--- cursor ---");
    match providers::fetch_cursor() {
        Ok(snapshot) => {
            print_snapshot(log, "cursor", &snapshot);
            0
        }
        Err(error) => {
            log.line(format!(
                "cursor probe failed: {error:?} ({})",
                cursor_error_hint(error)
            ));
            1
        }
    }
}

fn codex_error_hint(error: CodexError) -> &'static str {
    match error {
        CodexError::CliMissing => "install Codex CLI or add it to PATH",
        CodexError::LoginRequired => "run codex in a terminal and sign in",
        CodexError::Timeout => "app-server session timed out",
        CodexError::Unavailable => "app-server error or unexpected response",
    }
}

fn cursor_error_hint(error: CursorError) -> &'static str {
    match error {
        CursorError::LoginRequired => "sign in to Cursor desktop app",
        CursorError::RateLimited => "rate limited by Cursor API",
        CursorError::Timeout => "request timed out",
        CursorError::Unavailable => "missing state.vscdb or API error",
    }
}

fn print_snapshot(log: &mut ProbeLog, label: &str, snapshot: &UsageSnapshot) {
    log.line(format!("{label} ok status={:?}", snapshot.status));
    for (index, quota) in snapshot.quotas.iter().enumerate() {
        log.line(format!(
            "  [{index}] label={} used={:?} remaining={:?} resets_at={:?}",
            quota.label, quota.used_percent, quota.remaining_percent, quota.resets_at
        ));
    }
}
