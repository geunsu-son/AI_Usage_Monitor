# Grounding: settings subsystem for a dedicated settings window

Source: how explorers + [How: settings explainer](f28e255b-ffb9-48a3-b35a-868ddd5df43e). Checked against src/window.rs, settings_store.rs, native_interop.rs.

## Current shape
- SettingsFile JSON at %APPDATA%\AIUsageMonitor\settings.json + Run key for startup.
- AppState behind STATE mutex is the live copy. File is not watched.
- Only editor today: show_context_menu + WM_COMMAND arms in wnd_proc.
- Apply pattern: lock AppState, mutate with guards, drop lock, save_state_settings(), then field-specific side effects.
- Widget HWND (class AIUsageMonitor) is taskbar child after embed. Owns tray, timers, WM_APP messages.

## Constraints a settings window must obey
1. Own class + wndproc; not AIUsageMonitor/wnd_proc.
2. Normal activatable top-level/popup. No WS_EX_LAYERED / WS_EX_NOACTIVATE.
3. Never WS_CHILD of taskbar; never embed_in_taskbar / SetParent onto shell.
4. Owner = widget HWND (resolves to Shell_TrayWnd while embedded; OK given Explorer-restart exit).
5. Lock order: SAVES then STATE. Never save_state_settings while holding lock_state. Never hold lock across message pumps.
6. Do not reuse command ids 1/2 (Refresh/Exit = IDOK/IDCANCEL) on the widget.
7. Preserve last_update_check_unix and notified_quota_windows via save_state_settings.
8. Reuse menu guards (last provider, last usage row, alert values, Claude greyed).
9. Display changes call attach_to_taskbar(widget) + position_at_taskbar; recent fix keeps LAYERED on re-embed and raises HWND_TOP.
10. Prefer extracting named apply_* functions shared by menu and window over posting IDM_* (posts keep force-poll / balloon side effects).
11. Add IsDialogMessage to run loop for Tab/Enter if modeless dialog.
12. Scale from GetDpiForWindow(dialog); never write CURRENT_DPI.

## Fields to expose (minimum useful settings window)
poll_interval_ms, adaptive_refresh, show_codex, show_cursor, show_session_window, show_weekly_window,
appearance, alert_threshold_percent, language_override, anchor_left, monitor_device (display),
widget_visible, startup (registry). Claude greyed. focused_provider optional (left-click cycles).

## Goal
User opens a settings window from the tray menu and can change the same preferences the menu exposes, with live apply matching today's side effects, persisted through save_state_settings.
