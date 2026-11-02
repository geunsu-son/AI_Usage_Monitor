# AI Usage Monitor

Windows taskbar widget that shows **remaining** Codex and optional Cursor quota (session and weekly windows).

## Build

Requires [Rust](https://rustup.rs/) (MSVC toolchain) and Visual Studio Build Tools with the C++ workload.

```powershell
cargo build --release
```

The binary is `target\release\ai-usage-monitor.exe`.

### Verify providers (no GUI)

```powershell
cargo build
.\target\debug\ai-usage-monitor.exe --probe all
```

Results are appended to `%TEMP%\ai-usage-probe.txt`. Codex needs the OpenAI Codex CLI installed (typically under `%LOCALAPPDATA%\OpenAI\Codex\bin\codex.exe`). Cursor needs the desktop app signed in.

## Usage

- **Codex**: uses `codex app-server --stdio` (same approach as [CodexPeek](https://github.com/lch5518/CodexPeek)); sign in with the Codex CLI if prompted.
- **Cursor**: off by default. Enable it from the tray menu after Cursor is installed and signed in. Reads `%APPDATA%\Cursor\User\globalStorage\state.vscdb` read-only and calls `api2.cursor.sh`.

Settings and mutex names use the `AIUsageMonitor` prefix so this app can run beside the upstream Codex-only monitor.

## Docs

- [DESIGN.md](docs/DESIGN.md)
- [IMPLEMENTATION_PLAN.md](docs/IMPLEMENTATION_PLAN.md)
- [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)

## License

MIT — see [LICENSE](LICENSE).
