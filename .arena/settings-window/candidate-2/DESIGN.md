# Candidate 2 — Property sheet over a pure settings view-model

Dedicated Settings window for AI Usage Monitor. The tray menu and the new window both change preferences by committing one validated snapshot. `window.rs` owns the live `AppState` and is the only code that applies side effects or calls `save_state_settings`.

## Problem

Every preference the user can change today is a `WM_COMMAND` arm on the taskbar widget: `show_context_menu` builds the menu, and `wnd_proc` locks `AppState`, mutates one field, drops the lock, calls `save_state_settings`, then runs a field-specific side effect (timer, `position_at_taskbar`, `render_layered`, tray sync, forced poll, quota balloon, or the Run-key registry). The widget class is `AIUsageMonitor`. After embed it is a child of `Shell_TrayWnd`, with `WS_EX_LAYERED` and `WS_EX_NOACTIVATE`. A settings window cannot be that class, cannot be a child of the taskbar, and cannot enter `wnd_proc`.

The durable copy is `SettingsFile` JSON under `%APPDATA%\AIUsageMonitor\settings.json`, written only by `save_state_settings`. That function locks `SAVES` and then `STATE`, and it copies `last_update_check_unix` and `notified_quota_windows` from `AppState`. Startup-with-Windows is not in the file; it is the `AIUsageMonitor` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. The file is not watched. `AppState` behind `STATE` is the live copy.

Guards that already exist and must stay one policy: at least one of Claude / Codex / Cursor stays on; Claude is refused when `claude_code_available` is false (the menu greys it); at least one of the session and weekly rows stays on; `focused_provider` snaps back onto an enabled provider in Codex, Cursor, Claude order; poll values are the four menu steps (1 / 5 / 15 / 60 minutes); alert values are 0 / 10 / 20 / 30. Display changes go through `attach_to_taskbar(widget)` and only then record `monitor_device` and zero `tray_offset`. `normalize_settings` already snaps corrupt poll and alert values on load.

`quota_tooltip.rs` documents that the process has no comctl32 v6 manifest and sizes `TOOLINFO` for the v5 struct. This design uses the classic `PropertySheetW` API and does not add a v6 manifest.

## Usage (caller's view)

`window.rs` is the only caller. The sheet never sees `AppState`, command ids, or `save_state_settings`. The view-model never sees an `HWND`.

Open the sheet from the tray menu. `TrackPopupMenu` uses `TPM_RETURNCMD`, so the Settings item is handled here and is not a new `match` arm in `wnd_proc`. Every other menu id is forwarded as `WM_COMMAND` unchanged.

```rust
// window.rs — show_context_menu, after AppendMenu of the existing items
let settings_label = native_interop::wide_str(strings.settings);
let _ = AppendMenuW(menu, MF_STRING, IDM_OPEN_SETTINGS as usize, PCWSTR(settings_label.as_ptr()));

let picked = TrackPopupMenu(
    menu,
    TPM_RIGHTBUTTON | TPM_RETURNCMD,
    pt.x,
    pt.y,
    0,
    hwnd,
    None,
);
let _ = DestroyMenu(menu);
let id = picked.0 as u16;
if id == 0 {
    return;
}
if id == IDM_OPEN_SETTINGS {
    open_settings_window(hwnd); // snapshot, then settings_sheet::present
    return;
}
let _ = SendMessageW(hwnd, WM_COMMAND, WPARAM(id as usize), LPARAM(0));
```

`open_settings_window` builds a draft, shows the modal sheet, and passes `commit` as the apply callback. The callback receives a patch and no window handle, so the sheet cannot embed itself in the taskbar.

```rust
// window.rs
fn open_settings_window(widget: HWND) {
    let vm = snapshot_view_model(); // locks only long enough to copy fields
    settings_sheet::present(widget, vm, |patch| commit(patch));
}

fn snapshot_view_model() -> settings_vm::SettingsViewModel {
    unimplemented!("copy AppState fields, is_startup_enabled, find_taskbars display keys")
}
```

Menu toggles, the left-click focus cycle, and the tray “show widget” command stop writing `AppState` themselves. `wnd_proc` asks one function whether the id is a preference. Refresh (`1`), Exit (`2`), and version-check stay in the existing match.

```rust
// window.rs — WM_COMMAND, before the existing match
let id = wparam.0 as u16;
if apply_menu_command(hwnd, id) {
    return LRESULT(0);
}

fn apply_menu_command(widget: HWND, id: u16) -> bool {
    let Some(change) = menu_change(id) else {
        return false;
    };
    let mut vm = snapshot_view_model();
    vm.edit(change);
    commit(vm.patch());
    let _ = widget; // side effects read AppState.hwnd, which is this widget
    true
}

// left-click on the usage area, and tray_icon::TrayAction::ToggleWidget
fn cycle_focus_from_widget() {
    apply_menu_command(widget_hwnd(), IDM_CYCLE_FOCUS_SENTINEL);
}
```

`IDM_CYCLE_FOCUS_SENTINEL` is not appended to the menu. It exists so the click path and `menu_change` share `SettingsChange::CycleFocus`. The tray toggle keeps `tray_icon::IDM_TOGGLE_WIDGET` and maps it to `SettingsChange::ToggleWidget`.

What the caller gets back: `present` returns when the sheet closes. Apply and OK have already invoked `commit` zero or more times. Cancel returns without a further commit. `commit` returns `()`; a failed disk write is logged inside `save_state_settings`, the same as the menu.

## Shape

Three modules. Knowledge is split by who is allowed to be wrong.

| Module | Owns | Forbidden |
| --- | --- | --- |
| `src/settings_vm.rs` | Draft, guards, diff of two snapshots | `windows::*`, `HWND`, `AppState`, disk, registry, timers |
| `src/settings_sheet.rs` | Modal property sheet, page dialogs, control binding | `AppState`, `save_state_settings`, `wnd_proc`, taskbar embed |
| `src/window.rs` | `snapshot_view_model`, `commit`, menu id table | Page control ids, dialog procs, a second copy of the guards |

`main.rs` gains `mod settings_vm;` and `mod settings_sheet;`. No re-export of either from the other. `settings_sheet` may use `settings_vm`. `settings_vm` does not use `settings_sheet`.

### Data

The draft is a `SettingsViewModel`. Controls do not write `AppState` and do not write each other. They send a `SettingsChange`. `edit` applies the guard policy and returns the full control binding, so the sheet paints from one value.

The committed value is a `SettingsPatch`: every editable field, already guarded, with private fields. It is a replacement snapshot of the editable surface, not a bag of `Option`s. Equality is the idempotency key. `ApplyPlan` is the only description of side effects. `commit` copies the patch into `AppState`, then runs the plan. It does not re-decide policy.

`last_update_check_unix` and `notified_quota_windows` are absent from both structs. The alert-off clear of `notified_quota_windows` is an `ApplyPlan` effect, performed on `AppState` before `save_state_settings`, which then persists whatever the set currently is.

Domain enums move into `settings_vm` and keep their serde names (`snake_case` appearance and focus, numeric milliseconds and alert percents on the wire). `SettingsFile` stays a private struct in `window.rs`. JSON fields stay `poll_interval_ms: u32` and `alert_threshold_percent: u8`. Load and save convert at that boundary through `PollStep::from_stored` and `AlertThreshold::from_stored`. `AppState.poll_interval_ms` becomes `PollStep`. `AppState.alert_threshold_percent` becomes `AlertThreshold`. After load, memory cannot hold an out-of-range interval or alert. `Appearance::row_height` and `bar_height` stay in `window.rs` as functions of `Appearance` plus `sc()`, because they read `CURRENT_DPI`. The view-model returns unscaled design pixels only.

`LanguageId` moves into `settings_vm` (`ALL`, `code`, `from_code`, `native_name`). `localization` keeps the string tables and `detect_system_language`, and implements `LanguageId::strings` there. One language list.

Display identity is the same string the menu already stores: `taskbar_monitor_device`, or `format!("{}", index + 1)` when the device name is missing. The view-model treats it as an opaque key. `HWND`s never enter the draft. Snapshot and commit both call `find_taskbars` in `window.rs` so the key function lives once (`display_key`).

### Call chain

1. User edits a control. The page proc sends one `SettingsChange` and paints `ControlState`.
2. Apply / OK. The sheet session calls `commit(vm.patch())` once per distinct patch.
3. `commit` sanitizes against live `claude_code_available`, diffs with `plan`, writes `AppState`, drops `STATE`, calls `save_state_settings`, then runs HWND effects.

That is three files: `settings_sheet.rs`, `window.rs`, `settings_vm.rs`.

### Invariants encoded in types

- `PollStep` and `AlertThreshold` are closed enums. The sheet creates one radio per `PollStep::ALL` and `AlertThreshold::ALL`.
- `SettingsPatch` fields are private. Constructors are `from_live` (snapshot / load) and `SettingsViewModel::patch`. Both run `sanitize`.
- `edit` returns `ControlState` with `enabled: false` on Claude when credentials are missing, on a provider checkbox that is the last one still on, and on a usage-row checkbox in the same situation. Focus radios carry their own enabled flags. The sheet does not recompute this.
- `AppearancePreset` is derived from the value (`recommended`, `default`, otherwise `custom`). It is not stored.
- `ApplyPlan` uses enums for alternatives (`PollPlan`, `AlertPlan`, `DisplayPlan`, `WidgetPlan`, `StartupPlan`) and bools only for effects that combine (render, reposition, tray, force-poll, language).
- `commit` takes no `HWND`. Effects use `AppState.hwnd`, which is the widget.

### Validation

`sanitize(patch, claude_available)` is the only guard implementation. `edit` and `commit` both call it. `commit` uses the live flag so a draft opened before credentials were known cannot persist `show_claude_code`. Last-provider, last-row, and focus normalization live in `sanitize`, in the menu’s order: Codex, then Cursor, then Claude.

`plan(before, after, facts)` lists every patch field in its body with no `..` wildcard. `facts.has_poll_result` gates timer reconfigure only. Provider changes set `force_poll` even when the first poll has not returned, matching the menu, which still calls `request_poll(true)` from the provider arm.

### What the public surface hides

- `settings_vm`: last-provider policy, Claude greying, focus normalization, preset derivation, and which side effects a diff produces.
- `settings_sheet`: dialog templates, tab control, OK / Cancel / Apply, DPI-safe dialog units, and deduping the four `PSN_APPLY` notifications comctl32 sends for one click.
- `commit`: lock order, `save_state_settings`, registry writes, `attach_to_taskbar` success gating the display write, timers, paint, balloons, and forced poll.

What stays exposed to the caller: which `SettingsChange` a menu id means, and the order of HWND effects inside `commit`. That order is the existing menu behavior, and it belongs next to the functions that already perform it.

### Win32 shell

`present` calls `PropertySheetW` with `hwndParent = widget`. The sheet is the standard overlapped dialog `#32770` with the property-sheet frame. Pages are dialog procs, not `wnd_proc`, and not class `AIUsageMonitor`. No custom class is registered.

Flags: `PSH_PROPSHEETPAGE | PSH_NOCONTEXTHELP`. `PSH_MODELESS` is off. `PSH_NOAPPLYNOW` is off, so Apply exists. The owner is disabled for the modal lifetime, which means the tray menu cannot edit `AppState` while a draft is open. `PropertySheetW` runs its own loop and dispatches the widget’s timers; `present` must be called with `STATE` released, and `commit` must not hold `STATE` across that loop. `IsDialogMessage` is not added to `window::run`. Tab and Enter are handled inside the modal property-sheet loop. Constraint 11 applies to a modeless dialog; this window is modal on purpose.

In `PSCB_INITIALIZED`, strip `WS_EX_LAYERED` and `WS_EX_NOACTIVATE` if present, and do not add `WS_CHILD`. Never call `embed_in_taskbar` or `SetParent` on the sheet or its pages. Never call `refresh_dpi` or store into `CURRENT_DPI`. Pages are in-memory `DLGTEMPLATE`s (`PSP_DLGINDIRECT`) with `DS_SETFONT` and Segoe UI 9pt, in dialog units, so the dialog manager scales them under the process’s existing per-monitor v2 awareness. Page procs return `FALSE` for `WM_DPICHANGED` and let the dialog manager handle it. If a later control needs pixels, it reads `GetDpiForWindow` of that page into a local.

Page control ids start at `1000` and stay private to `settings_sheet`. They are not `WM_COMMAND` ids of the widget. The procs handle `WM_COMMAND` and return without `SendMessage` to the owner. `IDOK` / `IDCANCEL` (1 and 2) belong to the property-sheet frame, which is a different window, so they do not collide with Refresh / Exit on the widget.

One `SheetSession` on `present`’s stack is passed as `lParam` to every page. `on_apply` commits only when `vm.patch() != last_committed`, then `mark_applied`. The other three `PSN_APPLY` calls in the same click see an equal patch and return. A second Apply after another edit commits again. `plan` of two equal patches is empty, so a duplicate callback does not `SetTimer`, poll, or balloon.

`PSM_CHANGED` / `PSM_UNCHANGED` follow `ControlState.dirty`.

Pages and the changes they emit:

| Page | Controls | `SettingsChange` |
| --- | --- | --- |
| Providers | Claude, Codex, Cursor checkboxes; focus radios; session and weekly checkboxes; alert radios | `SetClaude`, `SetCodex`, `SetCursor`, `SetFocus`, `SetSessionWindow`, `SetWeeklyWindow`, `SetAlert` |
| Refresh | One radio per `PollStep`; adaptive checkbox | `SetPoll`, `SetAdaptive` |
| Appearance | Recommended and Default buttons; palette, bar style, thickness, font radios | `SetPreset`, `SetPalette`, `SetBarStyle`, `SetBarThickness`, `SetFontSize` |
| System | Startup, widget visible, anchor-left checkboxes; Reset position button; language combo (`None` plus `LanguageId::ALL`); display combo hidden when `displays.len() <= 1` | `SetStartup`, `SetWidgetVisible`, `SetAnchorLeft`, `ResetPosition`, `SetLanguage`, `SetDisplay` |

Labels come from `LanguageId::strings` and `native_name` at the resolved language (`language_override` or the system language captured in the draft). After `SetLanguage`, the active page re-reads strings. The other pages refresh on `PSN_SETACTIVE`.

### `commit` (the only writer)

```rust
fn commit(patch: SettingsPatch) {
    // 1. Lock STATE. Sanitize against live claude_code_available.
    //    before = patch from AppState (startup and display key re-read outside
    //    this lock, before it is taken: registry and find_taskbars).
    //    plan = settings_vm::plan(&before, &after, EffectFacts { has_poll_result }).
    //    If plan.is_empty() { return }  // no save, no effects
    //    Write every non-display field from `after`.
    //    If plan.zero_offset_from_reset { tray_offset = 0 }.
    //    If plan.alerts is ClearNotified { notified_quota_windows.clear() }.
    //    If plan.force_poll { set the six quota texts to "..." }.
    //    If plan.refresh_language { apply_language_to_state(...) }.
    //    If plan.alerts is Notify { alerts = collect_low_quota_alerts(...) }.
    //    Drop STATE before any of the steps below.
    // 2. Display switch: resolve `display_key` against a fresh find_taskbars.
    //    On match, attach_to_taskbar(widget, index). Only if that returns true,
    //    lock again and set monitor_device and tray_offset = 0.
    //    A failed attach leaves the previous device and offset in place.
    // 3. save_state_settings() exactly once. It is the only persistence call.
    //    It still copies last_update_check_unix and notified_quota_windows.
    // 4. Effects, widget HWND from AppState, lock not held across SetTimer:
    //    startup registry, poll reconfigure + SetTimer, position_at_taskbar,
    //    ShowWindow for widget visibility, render_layered, sync_tray_icons,
    //    balloons, request_poll(widget, true).
}
```

Effect mapping, matching the current arms:

| Diff | Plan | Effect |
| --- | --- | --- |
| `PollStep` or adaptive changed | `Reconfigure` only when `has_poll_result` | `monitor.reconfigure` + `SetTimer(TIMER_POLL, effective_poll_interval)`. Adaptive also sets `render`. Interval alone does not. |
| Appearance changed | `reposition`, `render` | `position_at_taskbar` + `render_layered` |
| Session/weekly rows | `render`, `sync_tray` | `render_layered` + `sync_tray_icons` |
| Provider visibility | `force_poll`, `reposition`, `render`, `sync_tray` | clear texts, save, position, render, tray, `request_poll(true)` |
| Focus only | `reposition`, `render` | save, `position_at_taskbar`, `render_layered`. No poll. |
| Language | `refresh_language`, `render` | `apply_language_to_state`, save, render. No reposition (same as the menu). |
| Alert to Off | `ClearNotified` | clear the set, no balloon |
| Alert to 10/20/30 | `Notify` | `collect_low_quota_alerts` + `notify_balloon` |
| Display key | `Switch { device }` | attach, then device + zero offset, then position + render |
| `ResetPosition` | `zero_offset_from_reset`, anchor true, `reposition` | `tray_offset = 0`, `anchor_left = true`, position |
| Anchor only | `reposition` | `position_at_taskbar` |
| Widget visibility | `Show` or `Hide` | show: position, `SW_SHOWNOACTIVATE`, render. hide: `SW_HIDE` |
| Startup | `Enable` or `Disable` | `set_startup_enabled` only. Not a JSON field. |

`save_state_settings` still runs for a startup-only change so there is one persist call site. The JSON rewrite is the same bytes plus `MoveFileEx`.

Lock order stays `SAVES` then `STATE`. `commit` never calls `save_state_settings` while holding `lock_state`. `present` never runs while holding it. Side-effect helpers (`position_at_taskbar`, `attach_to_taskbar`, `render_layered`) keep their internal short locks and do not save.

Running `commit` twice with the same patch: the second `plan` is empty and returns before save and before effects. A crash after step 3 and before step 4 leaves the new JSON in place; startup already applies `poll_interval_ms`, visibility, and display from that file. Balloon-after-save means a crash cannot notify a window whose notified-key was not written. The menu balloons before save; this order is the one `commit` uses for every field so mixed Applies have one sequence.

Menu ids that become `menu_change` rows, and then disappear from the `wnd_proc` match: frequency, adaptive, providers, session/weekly, appearance, alerts, language, startup, anchor, reset position, monitor ids (`IDM_MONITOR_FIRST` range), `IDM_TOGGLE_WIDGET`. `menu_change` and the menu builder share one table per group (`POLL_MENU`, `ALERT_MENU`, `LANG_MENU`, appearance tables). A unit test fails the build when a `PollStep`, `AlertThreshold`, or `LanguageId::ALL` entry is missing from its table. The sheet does not read those tables; it iterates the enums.

`IDM_OPEN_SETTINGS` is `210` (outside `1`, `2`, and `200..300`). `menu_change` returns `None` for it. A leaked `WM_COMMAND` hits the existing `_ => {}` arm.

### Type sketch

```rust
// src/settings_vm.rs
// No `windows` import. No HWND. No AppState.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageId {
    English, Dutch, Spanish, French, German, Japanese, Korean,
    SimplifiedChinese, TraditionalChinese, Russian, PortugueseBrazil,
}

impl LanguageId {
    pub const ALL: [LanguageId; 11] = [/* same order as today */];
    pub fn code(self) -> &'static str { unimplemented!() }
    pub fn from_code(code: &str) -> Option<Self> { unimplemented!() }
    pub fn native_name(self) -> &'static str { unimplemented!() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollStep { OneMinute, FiveMinutes, FifteenMinutes, OneHour }

impl PollStep {
    pub const ALL: [PollStep; 4] = [
        Self::OneMinute, Self::FiveMinutes, Self::FifteenMinutes, Self::OneHour,
    ];
    pub fn ms(self) -> u32 { unimplemented!() } // 60_000, 300_000, 900_000, 3_600_000
    /// Exact step, otherwise FifteenMinutes. Load uses this so a hand-edited
    /// in-range value becomes a radio selection.
    pub fn from_stored(ms: u32) -> Self { unimplemented!() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertThreshold { Off, Ten, Twenty, Thirty }

impl AlertThreshold {
    pub const ALL: [AlertThreshold; 4] = [Self::Off, Self::Ten, Self::Twenty, Self::Thirty];
    pub fn percent(self) -> u8 { unimplemented!() }
    pub fn from_stored(percent: u8) -> Self { unimplemented!() } // anything else → Off
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusedProvider { Codex, Cursor, ClaudeCode } // serde default Codex

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Palette { #[default] System, HighContrastDark, HighContrastLight }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarStyle { #[default] Continuous, Segmented }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarThickness { Standard, #[default] Slim }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontSize { #[default] Standard, Large }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub palette: Palette,
    pub bar_style: BarStyle,
    pub bar_thickness: BarThickness,
    pub font_size: FontSize,
}

impl Appearance {
    pub fn recommended() -> Self { unimplemented!("today's translucent_dark_taskbar") }
    pub fn is_dark(self, system_is_dark: bool) -> bool { unimplemented!() }
    pub fn font_px(self) -> i32 { unimplemented!() }          // 12 or 13, unscaled
    pub fn row_height_base(self) -> i32 { unimplemented!() }  // window applies sc()
    pub fn bar_height_base(self) -> i32 { unimplemented!() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppearancePreset { Recommended, Default, Custom }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayOffset { Keep, Zero }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayOption { pub key: String, pub label: String }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsPatch { /* private */ }

impl SettingsPatch {
    pub(crate) fn from_live(input: LiveSettings, claude_available: bool) -> Self {
        unimplemented!("sanitize")
    }
    pub(crate) fn poll(&self) -> PollStep { unimplemented!() }
    pub(crate) fn adaptive(&self) -> bool { unimplemented!() }
    pub(crate) fn show_claude(&self) -> bool { unimplemented!() }
    pub(crate) fn show_codex(&self) -> bool { unimplemented!() }
    pub(crate) fn show_cursor(&self) -> bool { unimplemented!() }
    pub(crate) fn focus(&self) -> FocusedProvider { unimplemented!() }
    pub(crate) fn show_session(&self) -> bool { unimplemented!() }
    pub(crate) fn show_weekly(&self) -> bool { unimplemented!() }
    pub(crate) fn alert(&self) -> AlertThreshold { unimplemented!() }
    pub(crate) fn appearance(&self) -> Appearance { unimplemented!() }
    pub(crate) fn language_override(&self) -> Option<LanguageId> { unimplemented!() }
    pub(crate) fn anchor_left(&self) -> bool { unimplemented!() }
    pub(crate) fn tray_offset(&self) -> TrayOffset { unimplemented!() }
    pub(crate) fn display_key(&self) -> Option<&str> { unimplemented!() }
    pub(crate) fn widget_visible(&self) -> bool { unimplemented!() }
    pub(crate) fn startup(&self) -> bool { unimplemented!() }
}

pub(crate) struct LiveSettings {
    pub show_claude: bool,
    pub show_codex: bool,
    pub show_cursor: bool,
    pub focus: FocusedProvider,
    pub show_session: bool,
    pub show_weekly: bool,
    pub alert: AlertThreshold,
    pub poll: PollStep,
    pub adaptive: bool,
    pub appearance: Appearance,
    pub language_override: Option<LanguageId>,
    pub anchor_left: bool,
    pub tray_offset: TrayOffset,
    pub display_key: Option<String>,
    pub widget_visible: bool,
    pub startup: bool,
}

pub(crate) fn sanitize(patch: SettingsPatch, claude_available: bool) -> SettingsPatch {
    unimplemented!("drop claude when unavailable; keep one provider; keep one usage row; normalize focus")
}

#[derive(Clone, Copy, Debug)]
pub enum SettingsChange {
    SetClaude(bool), SetCodex(bool), SetCursor(bool),
    SetFocus(FocusedProvider), CycleFocus,
    SetSessionWindow(bool), SetWeeklyWindow(bool),
    SetAlert(AlertThreshold),
    SetPoll(PollStep), SetAdaptive(bool), ToggleAdaptive,
    SetPreset(AppearancePreset), // Recommended or Default; Custom is not a command
    SetPalette(Palette), SetBarStyle(BarStyle),
    SetBarThickness(BarThickness), SetFontSize(FontSize),
    SetLanguage(Option<LanguageId>),
    SetStartup(bool), ToggleStartup,
    SetAnchorLeft(bool), ToggleAnchorLeft,
    ResetPosition,
    SetDisplay(String),
    SetWidgetVisible(bool), ToggleWidget,
    ToggleClaude, ToggleCodex, ToggleCursor,
    ToggleSessionWindow, ToggleWeeklyWindow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check { pub checked: bool, pub enabled: bool }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusState {
    pub selected: FocusedProvider,
    pub codex: Check,
    pub cursor: Check,
    pub claude: Check,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlState {
    pub claude: Check,
    pub codex: Check,
    pub cursor: Check,
    pub focus: FocusState,
    pub session: Check,
    pub weekly: Check,
    pub alert: AlertThreshold,
    pub poll: PollStep,
    pub adaptive: bool,
    pub appearance: Appearance,
    pub preset: AppearancePreset,
    pub language_override: Option<LanguageId>,
    pub startup: bool,
    pub anchor_left: bool,
    pub widget_visible: bool,
    pub displays: Vec<DisplayOption>,
    pub selected_display: Option<String>,
    pub dirty: bool,
}

pub struct SettingsViewModel { /* private: current, baseline, claude_available, displays, system_language */ }

impl SettingsViewModel {
    pub(crate) fn new(
        baseline: SettingsPatch,
        claude_available: bool,
        displays: Vec<DisplayOption>,
        system_language: LanguageId,
    ) -> Self { unimplemented!() }

    /// Sole mutator. Returns the binding the sheet paints. `false`-equivalent
    /// edits (last provider, greyed Claude, unknown display key, cycle with
    /// one target) return the previous binding and leave `dirty` unchanged.
    pub fn edit(&mut self, change: SettingsChange) -> ControlState { unimplemented!() }

    pub fn controls(&self) -> ControlState { unimplemented!() }
    pub fn patch(&self) -> SettingsPatch { unimplemented!() }
    pub fn mark_applied(&mut self) { unimplemented!("baseline = current") }
    pub fn system_language(&self) -> LanguageId { unimplemented!() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectFacts { pub has_poll_result: bool }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PollPlan { Keep, Reconfigure }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlertPlan { Keep, ClearNotified, Notify }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayPlan { Keep, Switch { device: String } }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetPlan { Keep, Show, Hide }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupPlan { Keep, Enable, Disable }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyPlan {
    pub poll: PollPlan,
    pub render: bool,
    pub reposition: bool,
    pub sync_tray: bool,
    pub force_poll: bool,
    pub refresh_language: bool,
    pub alerts: AlertPlan,
    pub display: DisplayPlan,
    pub zero_offset_from_reset: bool,
    pub widget: WidgetPlan,
    pub startup: StartupPlan,
}

impl ApplyPlan {
    pub const EMPTY: ApplyPlan = ApplyPlan {
        poll: PollPlan::Keep,
        render: false,
        reposition: false,
        sync_tray: false,
        force_poll: false,
        refresh_language: false,
        alerts: AlertPlan::Keep,
        display: DisplayPlan::Keep,
        zero_offset_from_reset: false,
        widget: WidgetPlan::Keep,
        startup: StartupPlan::Keep,
    };
    pub fn is_empty(&self) -> bool { unimplemented!() }
}

pub fn plan(before: &SettingsPatch, after: &SettingsPatch, facts: &EffectFacts) -> ApplyPlan {
    unimplemented!("compare every field; no wildcard")
}
```

```rust
// src/settings_sheet.rs
use windows::Win32::Foundation::HWND;
use crate::settings_vm::{ControlState, SettingsChange, SettingsPatch, SettingsViewModel};

/// Modal. `owner` is the widget HWND and becomes hwndParent only.
/// `commit` is invoked from PSN_APPLY with a patch and no HWND.
/// Caller must not hold STATE.
pub fn present(owner: HWND, vm: SettingsViewModel, commit: impl FnMut(SettingsPatch) + 'static) {
    unimplemented!("PropertySheetW, four DLGTEMPLATE pages, one page_proc")
}

fn page_proc(/* hwnd, msg, wparam, lparam */) -> isize {
    unimplemented!(
        "WM_INITDIALOG / PSN_SETACTIVE: push controls().
         WM_COMMAND: one SettingsChange from the control id, then edit().
         PSN_APPLY: session.on_apply() once per distinct patch.
         WM_DPICHANGED: return FALSE.
         Never SendMessage the owner."
    )
}
```

```rust
// src/window.rs — signatures added or narrowed. Bodies stay the existing helpers.
fn open_settings_window(widget: HWND) { unimplemented!() }
fn snapshot_view_model() -> settings_vm::SettingsViewModel { unimplemented!() }
fn menu_change(id: u16) -> Option<settings_vm::SettingsChange> { unimplemented!() }
fn apply_menu_command(widget: HWND, id: u16) -> bool { unimplemented!() }

/// The only writer of preference fields on AppState, and the only new caller
/// of save_state_settings. Takes no HWND.
fn commit(patch: settings_vm::SettingsPatch) { unimplemented!("see commit steps in Shape") }

fn display_key(taskbar: &native_interop::TaskbarWindow, index: usize) -> String {
    unimplemented!("taskbar_monitor_device or index+1")
}
```

### Tests (`settings_vm`, pure)

`unimplemented` until implementation, then these cases:

- Unchecking the last provider or the last usage row leaves the patch unchanged.
- `SetClaude(true)` with `claude_available == false` stays off and the check is disabled.
- Disabling the focused provider selects the next target in Codex, Cursor, Claude order.
- `CycleFocus` with one target is a no-op and is not dirty.
- `plan(a, a, _)` is `ApplyPlan::EMPTY` with and without `has_poll_result`.
- A poll change sets `Reconfigure` only when `has_poll_result`; it does not set `force_poll`.
- A provider change sets `force_poll` even when `has_poll_result` is false.
- Alert Off sets `ClearNotified`. A change from Ten to Twenty sets `Notify`. An equal alert stays `Keep`.
- A new display key sets `Switch` and does not itself set `zero_offset_from_reset` (commit zeros the offset only after `attach_to_taskbar` succeeds). `ResetPosition` sets `zero_offset_from_reset` and `anchor_left`.
- Startup and widget changes do not set `render` except widget `Show`, which repositions.
- `from_stored` maps 900_000 to fifteen minutes, 120_000 to fifteen minutes, and 0 to Off for alerts.
- Menu tables in `window.rs` cover `PollStep::ALL`, `AlertThreshold::ALL`, and `LanguageId::ALL`.

## Synthesis decision

This file is one candidate. Arena fills this section with the base it picked, what it grafts, and what it rejects.

## Tradeoffs accepted

- We accept a modal nested loop inside `PropertySheetW` in exchange for an owner-disabled draft and for keeping `IsDialogMessage` out of the widget loop. Widget timers still run, on the same thread, with the same short `STATE` locks they use today.
- We accept a classic (v5) property-sheet look in exchange for leaving the comctl32 v6 manifest absent, which `quota_tooltip.rs` depends on.
- We accept snapping non-step poll intervals to fifteen minutes in `PollStep::from_stored` in exchange for a closed enum in memory and on the sheet. The menu could only check the four steps anyway.
- We accept balloon-after-save in exchange for one effect order when several fields change in a single Apply. A crash after save does not re-notify a key that was written.
- We accept a JSON rewrite on a startup-only change in exchange for `commit` having one persist call.
- We accept an in-memory `DLGTEMPLATE` (DWORD alignment is fiddly) in exchange for not adding a resource script to a project that has never had one.
- We accept `LanguageId` living in `settings_vm` while `strings()` is implemented in `localization` in exchange for one language enum in a single crate. Purity is the module boundary (`settings_vm.rs` has no `windows` path), not a second package.
- We accept keeping the immediate tray-menu items in exchange for muscle memory. They are not a second apply implementation; they are `SettingsChange` values fed to `commit`.
- We accept `SettingsPatch` accessors as a wide but dumb read surface in exchange for `commit` copying fields without a second policy function.

## Alternatives considered

- **Modeless property sheet plus `IsDialogMessage` in `window::run`.** The widget loop would own a dialog `HWND` and Tab routing, and the owner would stay enabled, so the menu could write the same fields while the draft is open. Two writers. The modal sheet hides the pump and the exclusion inside `PropertySheetW` and the disabled owner. Rejected because the caller would have to learn the sheet’s lifetime.
- **Page procs `PostMessage` `IDM_*` to the widget.** That reuses the arms, grows `wnd_proc`, and makes command ids 1 and 2 a collision risk. Side effects would stay scattered in the widget proc. The sheet would be a remote control for the menu. Rejected because it hides nothing: the caller still has to know every command id and the menu’s toggle semantics.
- **Controls mutate `AppState` on each click.** Either the lock is held across the sheet’s pump, or every notification re-enters `commit` with a one-field write and the draft disappears. Guards would live in the dialog proc. Rejected because the Win32 layer would own the policy.
- **Sparse `SettingsPatch` of `Option` fields.** Unset would mean “leave it alone”, so a full Apply and a partial menu toggle are different shapes, and “clear the display key” is inexpressible versus “leave it”. A full snapshot plus `plan` makes “run twice” observable: the second plan is empty. Rejected because the sparse form leaks merge policy to every caller.

## Open questions and risks

- Should a later pass delete the per-field tray submenu entries and leave a single Settings item? This sketch keeps both entry points on `commit`. Removing the submenus is a product cut, not a second design.
- Is snapping an in-range, non-step `poll_interval_ms` at the next load acceptable for people who hand-edit `settings.json`? `from_stored` does that.
- `PropertySheetW` without a v6 manifest is the intended look. Adding the manifest later is an app-wide control-version change, not a sheet-local fix.
- `TPM_RETURNCMD` means `show_context_menu` must `SendMessage` every non-settings id. Missing that forward drops Refresh, Exit, and the rest.
- In-memory dialog templates must be DWORD-aligned. That helper is the risky Win32 code; the view-model tests will not catch it.
- A timer that opens a message box (the update prompt) can appear above the sheet. `present` must not be called from a timer. Nothing in this sketch adds that call.

## Next implementation step

Write `settings_vm` (`sanitize`, `edit`, `plan`) and its unit tests, then point `commit` and `menu_change` at it before any dialog exists.

## Red flags

Screened against `design-red-flags.md`.

- **Shallow module.** Avoided. Callers do not sequence guards, saves, and effects. They send a `SettingsChange` or a `SettingsPatch`. `controls()` is one read. `present` and `commit` are one call each. The policy those calls hide is the current menu behavior, which is large relative to that surface.
- **Information leakage.** Avoided. `SettingsFile`, command ids, `HWND`, and `TOOLINFO` stay out of the view-model. The sheet does not see `AppState`. Display keys are opaque strings. JSON field names stay inside `save_state_settings` / `load_settings`.
- **Temporal decomposition.** Avoided. Load, edit, and diff share `sanitize` and `SettingsPatch` in one module because they protect the same invariants. `commit` is the apply stage because it is the only code allowed to touch `AppState` and the widget. It is not a pass-through validate-then-save pipeline.
- **Pass-through method.** Avoided. `apply_menu_command` builds a draft and commits it. `present` owns the modal loop and dedups `PSN_APPLY`. `commit` is the effect runner. None of them forward the same arguments unchanged.
- **Split ownership.** Avoided. `AppState` is the live store. The view-model is a per-dialog draft dropped on close. The menu does not keep a second live copy. Registry startup is written only inside `commit`. The owner-disabled modal sheet keeps the menu from writing while the draft exists. Left-click focus and the tray visibility command go through `apply_menu_command` too, so they are not a leftover writer.
- **Two ways to do one task.** Avoided for apply. Menu and sheet both end in `commit`. Page procs do not post `IDM_*`. `wnd_proc` loses the preference arms instead of gaining dialog arms. Opening the sheet is the `TPM_RETURNCMD` branch in `show_context_menu`, not an `IDM` arm.
- **Importable internals.** Avoided. Patch fields are private. `sanitize`, `LiveSettings`, and `from_live` are `pub(crate)` inside `settings_vm`. Page procs, control ids, and `SheetSession` are private to `settings_sheet`. Neither module re-exports the other. `settings_vm.rs` does not reference `windows`, so a dialog type cannot be named there without an obvious break.
- **Hand-synced list.** Avoided for the domain lists by deriving radios and combos from `PollStep::ALL`, `AlertThreshold::ALL`, and `LanguageId::ALL`. The menu’s id tables are the one parallel list; a unit test fails when an enum entry is missing. `ApplyPlan` is produced by comparing every patch field explicitly so a new field does not silently skip effects. Control ids live in one private enum used both to build the template and to handle `WM_COMMAND`.
