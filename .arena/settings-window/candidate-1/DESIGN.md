# Candidate 1: typed `SettingsCommand` + one `apply`, menu returns choices, modeless settings class

## Problem

AI Usage Monitor's preferences are edited only through `show_context_menu` + ~12 `WM_COMMAND` arms in `wnd_proc` (window.rs). Each arm hand-repeats the same protocol: lock `STATE`, mutate with an ad-hoc guard (last provider, last quota row, Claude availability), drop the lock, `save_state_settings()`, then a field-specific tail of side effects (`SetTimer`, `position_at_taskbar`, `render_layered`, `sync_tray_icons`, balloons, forced poll). Menu items are *relative* toggles (`s.show_codex = !s.show_codex`), the IDM id → value mapping is listed twice (menu builder arrays and the `match id` decode), and appearance/display labels are hardcoded inside the menu builder.

A second editor (a settings window) multiplies all of that unless the protocol gets one owner. Constraints from grounding that shape the design:

- The dialog needs its own class and wndproc. It must be a normal activatable top-level window, never `WS_CHILD` of the taskbar, owned by the widget HWND, and not layered or no-activate.
- Lock order is `SAVES` then `STATE`. Never call `save_state_settings` while holding `lock_state`, and never hold the lock across a message pump.
- `save_state_settings` is the only persistence path, because it preserves `last_update_check_unix` and `notified_quota_windows`.
- Menu guards are reused. Display changes go through `attach_to_taskbar` + `position_at_taskbar`.
- Command ids 1 and 2 are not reused on the widget. `IsDialogMessageW` goes into the run loop. The dialog scales from `GetDpiForWindow(dialog)` and never writes `CURRENT_DPI`.

Grounding fact this design leans on: **the context menu is the only producer of `WM_COMMAND` on the widget** (`rg WM_COMMAND|PostMessageW` finds no other poster). So the menu can switch to `TPM_RETURNCMD | TPM_NONOTIFY` and dispatch its choice in place. That lets us delete the widget's `WM_COMMAND` arm and every `IDM_*` constant, which makes "post an IDM to the widget" impossible rather than merely discouraged.

## Usage (caller's view)

There are three files, and each has one way in:

```rust
// window.rs — the only things the rest of window.rs needs from the two new child modules
use settings::{SettingsCommand, SettingsView};
//   settings::apply(widget, cmd)       — the one write path for user preferences
//   SettingsView::read()               — the one read model (menu + dialog render from it)
//   settings_dialog::open(widget)      — idempotent: create or re-activate
//   settings_dialog::pre_translate(&m) — run-loop hook for Tab/Enter/Esc
```

**Call site 1: context menu (window.rs).** The menu renders from `SettingsView` and returns a typed choice, so there are no IDM decode tables.

```rust
fn show_context_menu(hwnd: HWND) {
    let view = SettingsView::read();
    let chrome = MenuChrome::read();               // strings, install_channel, update_status
    let menu = build_context_menu(&view, &chrome); // MenuBuilder records (id -> MenuChoice) as it appends
    let picked = menu.track(hwnd);                 // TrackPopupMenu(TPM_RETURNCMD | TPM_NONOTIFY)
    match picked {
        Some(MenuChoice::Setting(cmd)) => settings::apply(hwnd, cmd),
        Some(MenuChoice::OpenSettings) => settings_dialog::open(hwnd),
        Some(MenuChoice::Refresh) => refresh_now(hwnd),
        Some(MenuChoice::VersionAction) => run_version_action(hwnd),
        Some(MenuChoice::Exit) => exit_app(),
        None => {}
    }
}

// inside build_context_menu — a checked item carries the *absolute* target value
for interval in PollInterval::ALL {
    b.item(interval.label(view.strings()), view.poll_interval == Some(interval), true,
           MenuChoice::Setting(SettingsCommand::SetPollInterval(interval)));
}
let codex = view.provider(FocusedProvider::Codex);
b.item(strings.codex_model, codex.checked, codex.enabled,
       MenuChoice::Setting(SettingsCommand::SetProviderShown(FocusedProvider::Codex, !codex.checked)));
```

**Call site 2: settings dialog control handler (window/settings_dialog.rs).**

```rust
WM_COMMAND => {
    let Some(field) = Field::from_control_id(loword(wparam)) else { return close_on_ok_cancel(...) };
    if !state.syncing && field.is_change_notification(hiword(wparam)) {
        let view = SettingsView::read();
        if let Some(cmd) = field.command(&view, control_hwnd) {
            settings::apply(state.widget, cmd);    // apply() ends with settings_dialog::sync()
        }
    }
}
```

**Call site 3: tray left-click and run loop (window.rs).**

```rust
tray_icon::TrayAction::ToggleWidget => {
    let visible = lock_state().as_ref().is_some_and(|s| s.widget_visible);
    settings::apply(hwnd, SettingsCommand::SetWidgetVisible(!visible));
}

while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
    if settings_dialog::pre_translate(&msg) { continue; }
    let _ = TranslateMessage(&msg);
    DispatchMessageW(&msg);
}
```

## Shape

### Module map

```
src/window.rs                     (existing, shrinks)
  mod settings;                   // new child module: window/settings.rs
  mod settings_dialog;            // new child module: window/settings_dialog.rs
  - delete: all IDM_* consts, the WM_COMMAND arm, toggle_widget_visibility,
            the 17-tuple snapshot in show_context_menu
  + MenuBuilder / MenuChoice (private), refresh_now / exit_app / run_version_action
    (bodies lifted verbatim from the deleted arms 1, 2, IDM_VERSION_ACTION)
  + run loop calls settings_dialog::pre_translate

src/window/settings.rs            (new) — owns "what a preference change means"
  SettingsCommand, PollInterval, AlertThreshold, QuotaWindow, DisplayChoice
  SettingsView (+ Toggle), Visibility guard predicates
  impl ALL/label for Palette, BarStyle, BarThickness, FontSize (moved out of menu builder)
  pub(super) fn apply ; private Change, Effects, prepare, mutate, run_effects

src/window/settings_dialog.rs     (new) — owns the Win32 dialog
  class "AIUsageMonitorSettings", wndproc, Field table, layout, per-window DPI
  pub(super) fn open, pre_translate, sync

src/tray_icon.rs                  delete IDM_TOGGLE_WIDGET (no longer a menu id)
src/localization/*                new Strings fields (see open questions)
src/settings_store.rs, native_interop.rs   unchanged
```

`window.rs` + `src/window/*.rs` is the Rust 2018 non-`mod.rs` layout. Because the two new modules are **children** of `window`, they can reach `AppState`, `lock_state`, `save_state_settings`, `position_at_taskbar`, `render_layered`, `attach_to_taskbar`, and the rest through `super::` while those stay private to the `window` subtree. Nothing gets widened to `pub(crate)`, so outside modules can't import the internals (per *importable internals*). Reading any flow takes at most three files: `window.rs` → `settings.rs` → back into `window.rs` helpers.

### Data structures (window/settings.rs)

```rust
/// A user preference change. Every variant is an absolute target value, never a
/// toggle, so applying the same command twice is a no-op (per make-operations-idempotent).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum SettingsCommand {
    SetPollInterval(PollInterval),
    SetAdaptiveRefresh(bool),
    SetProviderShown(FocusedProvider, bool),   // reuse the existing provider identity enum
    SetQuotaWindowShown(QuotaWindow, bool),
    SetAlertThreshold(AlertThreshold),
    SetAppearance(Appearance),                  // presets are just values: translucent_dark_taskbar(), default()
    SetLanguage(Option<LanguageId>),            // None = follow system
    SetAnchorLeft(bool),
    ResetPosition,
    SetDisplay(DisplayChoice),
    SetWidgetVisible(bool),
    SetStartWithWindows(bool),                  // registry is its own source of truth, not AppState
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PollInterval { OneMinute, FiveMinutes, FifteenMinutes, OneHour }
impl PollInterval {
    pub(super) const ALL: [Self; 4] = [Self::OneMinute, Self::FiveMinutes, Self::FifteenMinutes, Self::OneHour];
    pub(super) fn ms(self) -> u32 { todo!("not implemented") }              // exhaustive match → POLL_* consts
    pub(super) fn from_ms(ms: u32) -> Option<Self> { todo!("not implemented") }
    pub(super) fn label(self, strings: Strings) -> &'static str { todo!("not implemented") }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AlertThreshold { Off, Ten, Twenty, Thirty }
impl AlertThreshold {
    pub(super) const ALL: [Self; 4] = [Self::Off, Self::Ten, Self::Twenty, Self::Thirty];
    pub(super) fn percent(self) -> u8 { todo!("not implemented") }
    pub(super) fn from_percent(p: u8) -> Option<Self> { todo!("not implemented") }
    pub(super) fn label(self, language: LanguageId) -> String { todo!("not implemented") }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuotaWindow { Session, Weekly }

/// Identifies a taskbar by monitor device, with the enumeration index as fallback
/// for taskbars whose device can't be resolved. Resolved at apply time through
/// the existing `preferred_taskbar_index`, so a stale index from an open dialog
/// still lands on the right monitor.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DisplayChoice { pub index: usize, pub device: Option<String> }

// Option lists the menu and the dialog both iterate. One list per enum, and the
// exhaustive `match` in label() fails the build when a variant is added.
impl Palette      { pub(super) const ALL: [Self; 3] = [/*..*/]; pub(super) fn label(self, l: LanguageId) -> &'static str { todo!("not implemented") } }
impl BarStyle     { pub(super) const ALL: [Self; 2] = [/*..*/]; pub(super) fn label(self, l: LanguageId) -> &'static str { todo!("not implemented") } }
impl BarThickness { pub(super) const ALL: [Self; 2] = [/*..*/]; pub(super) fn label(self, l: LanguageId) -> &'static str { todo!("not implemented") } }
impl FontSize     { pub(super) const ALL: [Self; 2] = [/*..*/]; pub(super) fn label(self, l: LanguageId) -> &'static str { todo!("not implemented") } }
```

### The guard rules, which both views and the writer share

```rust
/// The guard rules in one place. `mutate` uses them to reject a command, and
/// `SettingsView` uses them to grey out the control that would be rejected
/// (the last shown provider, the last quota row, Claude when unavailable).
#[derive(Clone, Copy)]
struct Visibility {
    claude: bool, codex: bool, cursor: bool, claude_available: bool,
    session: bool, weekly: bool,
}
impl Visibility {
    fn of(state: &AppState) -> Self { todo!("not implemented") }
    /// Turning on is allowed (Claude: only if available); turning off needs another provider shown.
    fn can_set_provider(self, p: FocusedProvider, shown: bool) -> bool { todo!("not implemented") }
    /// Turning off needs the other row shown.
    fn can_set_window(self, w: QuotaWindow, shown: bool) -> bool { todo!("not implemented") }
}
```

### Read model

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Toggle { pub checked: bool, pub enabled: bool }

/// Snapshot of every user-editable preference plus whether each control may
/// change. The menu and the dialog both render from this; neither keeps a copy
/// of settings between renders (AppState stays the single owner; per split ownership).
pub(super) struct SettingsView {
    pub language: LanguageId,                     // resolved, for labels
    pub language_override: Option<LanguageId>,
    pub poll_interval: Option<PollInterval>,      // None if settings.json holds a non-preset value
    pub adaptive_refresh: bool,
    pub alert_threshold: Option<AlertThreshold>,
    pub appearance: Appearance,
    pub anchor_left: bool,
    pub widget_visible: bool,
    pub start_with_windows: bool,                 // is_startup_enabled(), read after the lock is dropped
    pub displays: Vec<(DisplayChoice, String)>,   // find_taskbars() + label, read after the lock is dropped
    pub current_display: Option<usize>,           // index into `displays`
    visibility: Visibility,
}
impl SettingsView {
    /// Holds lock_state only to copy fields; registry + taskbar enumeration happen after the drop.
    pub(super) fn read() -> Self { todo!("not implemented") }
    pub(super) fn strings(&self) -> Strings { self.language.strings() }
    pub(super) fn provider(&self, p: FocusedProvider) -> Toggle { todo!("not implemented") }   // enabled via Visibility
    pub(super) fn quota_window(&self, w: QuotaWindow) -> Toggle { todo!("not implemented") }
}
```

### The write path

```rust
/// The single write path for user preferences. It applies the change to the live
/// AppState, persists through save_state_settings, runs the same side effects the
/// menu arms ran, and finally re-syncs an open settings dialog.
///
/// Invariants:
/// - No STATE lock is held across save_state_settings or any Win32 call that can
///   pump messages (SAVES -> STATE order is preserved).
/// - A command rejected by a guard or equal to current state changes nothing and
///   does not save. The dialog is still synced so a refused checkbox snaps back.
/// - Must be called on the UI thread (it touches the widget and dialog HWNDs).
pub(super) fn apply(widget: HWND, command: SettingsCommand) {
    // let Some(change) = prepare(widget, command) else { settings_dialog::sync(); return };
    // let effects = { let mut g = lock_state(); let Some(s) = g.as_mut() else { return }; mutate(s, change) };
    // run_effects(widget, effects);           // lock already dropped
    // settings_dialog::sync();
    todo!("not implemented")
}

/// The command after any Win32 preconditions have run outside the lock.
enum Change {
    Pure(SettingsCommand),                    // every variant except the two below
    DisplayAttached { device: Option<String> },   // attach_to_taskbar already succeeded
    Done,                                     // SetStartWithWindows: registry written, nothing to mutate
}

/// Steps that must happen before AppState is touched, all outside the lock:
/// - SetDisplay: resolve with preferred_taskbar_index(find_taskbars(), device, index),
///   then attach_to_taskbar(widget, i). On failure return None (state untouched, as today).
/// - SetStartWithWindows(b): set_startup_enabled(b) → Change::Done.
fn prepare(widget: HWND, command: SettingsCommand) -> Option<Change> { todo!("not implemented") }

/// Side effects to run after the lock is dropped, executed in a fixed order by
/// run_effects. The sequencing policy lives only there.
#[derive(Default)]
struct Effects {
    save: bool,
    retime_poll_ms: Option<u32>,      // SetTimer(TIMER_POLL, ..) — moved out from under the lock
    show_widget: Option<bool>,        // Some(true): position + SW_SHOWNOACTIVATE + render; Some(false): SW_HIDE
    reposition: bool,                 // position_at_taskbar()
    render: bool,                     // render_layered()
    sync_tray: bool,                  // sync_tray_icons(widget)
    balloons: Vec<QuotaAlert>,        // tray_icon::notify_balloon
    force_poll: bool,                 // request_poll(widget, true)
}

/// Pure state transition under the lock. No Win32 calls and no save.
/// Applies the Visibility guards, and returns Effects::default() for rejects and no-ops.
///
/// TODO effect table (copied from today's arms):
///   SetPollInterval       set ms; if has_poll_result { monitor.reconfigure(..); retime }      ; save
///   SetAdaptiveRefresh    set; if has_poll_result { reconfigure; retime }                      ; save, render
///   SetProviderShown      guard; normalize_focused_provider; all six texts = "..."             ; save, reposition, render, sync_tray, force_poll
///   SetQuotaWindowShown   guard                                                                ; save, render, sync_tray
///   SetAlertThreshold     set; Off → notified_quota_windows.clear(); else collect_low_quota_alerts → balloons ; save
///   SetAppearance         set whole struct                                                     ; save, reposition, render
///   SetLanguage           apply_language_to_state                                              ; save, render
///   SetAnchorLeft         set                                                                  ; save, reposition
///   ResetPosition         tray_offset = 0, anchor_left = true                                  ; save, reposition
///   DisplayAttached       monitor_device = device, tray_offset = 0                             ; save, reposition, render
///   SetWidgetVisible      set                                                                  ; save, show_widget
///   Done                  —                                                                    ; (nothing)
fn mutate(state: &mut AppState, change: Change) -> Effects { todo!("not implemented") }

/// Fixed order: save → retime → show/hide → reposition → render → sync_tray → balloons → force_poll.
/// position_at_taskbar may save again on its own; that's fine because no lock is held.
fn run_effects(widget: HWND, effects: Effects) { todo!("not implemented") }
```

### The dialog (window/settings_dialog.rs)

```rust
const CLASS_NAME: &str = "AIUsageMonitorSettings";
const CONTROL_ID_BASE: u16 = 100;   // never 1/2; IDOK/IDCANCEL here mean "close the dialog"

thread_local! { static DIALOG: Cell<HWND> = Cell::new(HWND::default()); }  // UI-thread singleton

/// Creates the settings window, or restores and activates it if it is already open.
/// - Registers CLASS_NAME once, reusing load_embedded_app_icons.
/// - Style WS_POPUP|WS_CAPTION|WS_SYSMENU|WS_CLIPCHILDREN,
///   ex WS_EX_CONTROLPARENT|WS_EX_DLGMODALFRAME. Never LAYERED, NOACTIVATE, or CHILD.
/// - Owner = widget (resolves to Shell_TrayWnd while embedded). Never SetParent/embed.
/// - Placed centered on MonitorFromWindow(widget)'s work area, then sync().
pub(super) fn open(widget: HWND) { todo!("not implemented") }

/// Run-loop hook. Returns true if the message was a dialog-navigation key
/// (Tab, arrows, Enter, Esc) for the open settings window, and was consumed by IsDialogMessageW.
pub(super) fn pre_translate(msg: &MSG) -> bool { todo!("not implemented") }

/// Pushes SettingsView::read() into the controls when the dialog is open, and does nothing otherwise.
/// Also runs on WM_ACTIVATE, which catches writers outside apply() (drag to another
/// display, system language change, startup toggled by another tool).
/// Re-labels and re-lays out when view.language differs from the last sync.
/// Sets `syncing` so BM_SETCHECK/CB_SETCURSEL echoes aren't treated as user edits.
pub(super) fn sync() { todo!("not implemented") }

/// Per-window state boxed in GWLP_USERDATA, freed in WM_NCDESTROY.
struct DialogState {
    widget: HWND,
    dpi: u32,                          // GetDpiForWindow(dialog); updated on WM_DPICHANGED
    font: HFONT,                       // message-box font scaled to `dpi`; recreated on DPI change
    controls: [HWND; Field::COUNT],
    labels: [HWND; Field::COUNT],      // static text beside checkboxes and combos
    laid_out_for: Option<LanguageId>,
    syncing: bool,
}

/// One row in the dialog. Field::ALL is the single list that drives control
/// creation, control ids (CONTROL_ID_BASE + index), layout order, sync, and decoding.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    PollInterval, AdaptiveRefresh,
    Provider(FocusedProvider), QuotaWindow(QuotaWindow), AlertThreshold,
    Palette, BarStyle, BarThickness, FontSize, AppearanceRecommended, AppearanceReset,
    Language,
    Display, AnchorLeft, ResetPosition,
    WidgetVisible, StartWithWindows,
    Close,
}
impl Field {
    const ALL: [Field; 20] = [/* layout order */];
    const COUNT: usize = Self::ALL.len();
    fn control_id(self) -> u16 { todo!("not implemented") }
    fn from_control_id(id: u16) -> Option<Self> { todo!("not implemented") }
    fn kind(self) -> ControlKind { todo!("not implemented") }   // Check | Choice | Button
    fn section(self) -> Section { todo!("not implemented") }   // Refresh | Providers | Alerts | Appearance | Language | Position | General
    fn label(self, view: &SettingsView) -> String { todo!("not implemented") }
    /// What the control should show: checked/enabled, choice labels + selection, or button enabled.
    fn present(self, view: &SettingsView) -> Presented { todo!("not implemented") }
    /// Turn the control's current state into a command. Appearance parts compose
    /// onto view.appearance (e.g. Palette → SetAppearance(Appearance { palette, ..view.appearance })).
    /// Close returns None (the wndproc handles it).
    fn command(self, view: &SettingsView, control: HWND) -> Option<SettingsCommand> { todo!("not implemented") }
}

enum ControlKind { Check, Choice, Button }
enum Presented {
    Check(Toggle),
    Choice { options: Vec<String>, selected: Option<usize>, enabled: bool },
    Button { enabled: bool },
}

/// Message handling:
/// WM_CREATE → create controls from Field::ALL; WM_COMMAND → Field::command → settings::apply;
/// IDOK/IDCANCEL/WM_CLOSE → DestroyWindow; WM_DPICHANGED → new dpi/font, relayout, SetWindowPos(suggested rect);
/// WM_ACTIVATE(active) → sync(); WM_DESTROY → DIALOG = null, DeleteObject(font).
unsafe extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    todo!("not implemented")
}

/// Scale from the dialog's own DPI. Never sc() or CURRENT_DPI, which track the widget.
fn px(dpi: u32, logical: i32) -> i32 { todo!("not implemented") }
fn layout(hwnd: HWND, state: &DialogState, view: &SettingsView) { todo!("not implemented") }
```

### Menu side (window.rs, private)

```rust
enum MenuChoice { Setting(SettingsCommand), OpenSettings, Refresh, VersionAction, Exit }

/// Allocates ids sequentially from 0x100 as items are appended and records each
/// id's choice. The id is meaningless outside one track() call, so no IDM_* table exists.
struct MenuBuilder { root: HMENU, next_id: u16, choices: Vec<(u16, MenuChoice)> }
impl MenuBuilder {
    fn item(&mut self, label: &str, checked: bool, enabled: bool, choice: MenuChoice) { todo!("not implemented") }
    fn submenu(&mut self, label: &str, build: impl FnOnce(&mut MenuBuilder)) { todo!("not implemented") }
    fn separator(&mut self) { todo!("not implemented") }
    /// SetForegroundWindow + TrackPopupMenu(TPM_RIGHTBUTTON|TPM_RETURNCMD|TPM_NONOTIFY) + DestroyMenu.
    fn track(self, hwnd: HWND) -> Option<MenuChoice> { todo!("not implemented") }
}
```

### Load-bearing decisions

1. **Absolute commands, not toggles.** The dialog naturally produces target values (checkbox state, combo selection), and the menu produces them at build time from the same `SettingsView` it renders. Repeating a command is a no-op, so menu and dialog can't flip each other's state on a double-apply (per *make-operations-idempotent*).
2. **One deep `apply` instead of N `apply_*` functions.** The public write surface is a single function plus an enum. Behind it sit the lock discipline, guards, persistence, the per-command effect sequencing, and dialog resync. An agent adding a preference adds a variant, and the exhaustive `match` in `mutate` and `Field::command` makes the build fail until every site handles it (per *encode-lessons-in-structure*).
3. **Prepare / mutate / run split, all private.** `prepare` runs the Win32 preconditions that exist today (attach before recording `monitor_device`, the registry write). `mutate` is the only code that runs under `STATE`, and it can't save because it has no way to. `run_effects` holds the ordering policy. This is one module that owns one decision ("what a preference change does"), not temporal layers exposed to callers (per *boundary-discipline*). Callers see only `apply`.
4. **Guards and greying come from one predicate.** `Visibility` is used by `mutate` to reject and by `SettingsView` to disable. The menu and dialog can't drift from the writer.
5. **No settings copy in the dialog.** It re-reads `SettingsView` on every sync. `AppState` stays the single owner, and a rejected edit snaps the control back because `apply` always syncs (per *separate-before-serializing-shared-state*: one writer, readers derive).
6. **Menu returns choices synchronously.** `TPM_RETURNCMD` removes the widget's `WM_COMMAND` arm, every `IDM_*` constant, and `tray_icon::IDM_TOGGLE_WIDGET`. Constraint 6 holds structurally, because the widget no longer interprets command ids at all.
7. **Dialog geometry is self-contained.** It has its own DPI, own font, and own class. It never calls `sc()` and never reads or writes `CURRENT_DPI`.

### What it deliberately does not do

- **Focused provider.** It isn't exposed. Left-click keeps cycling it, and `mutate` keeps normalizing it on provider changes.
- **Dark-mode theming of the dialog.** It uses standard system colors.
- **OK/Cancel/Apply semantics.** Changes apply live, so the only button is Close, mapped to Enter and Esc via `IsDialogMessageW`.
- **Watching the settings file.** The file is still not watched.
- **Rerouting non-preference writers through `apply`.** Drag-to-display, `position_at_taskbar`'s own offset save, and the system-language follow in `update_language_change` keep writing `AppState` directly. The dialog catches those changes on `WM_ACTIVATE`.

### Interface depth

The public surface across both new modules is `apply`, `SettingsCommand` and its value enums, `SettingsView::read` with two accessors, and `open` / `pre_translate` / `sync`. That surface hides the lock order, guards, persistence path, per-field side-effect table, display resolution and attach, registry handling, dialog class registration, DPI handling, control layout, and re-entrancy suppression. Callers can't express "mutate and forget to save" or "save under the lock".

## Synthesis decision

*(Filled in by arena.)*

## Tradeoffs accepted

- We accept rewriting `show_context_menu` (`TPM_RETURNCMD` + `MenuBuilder`) in exchange for deleting both IDM tables and the entire widget `WM_COMMAND` arm. This is a larger diff than the minimum, but it makes the old path disappear instead of coexisting (per *two ways to do one task*).
- We accept that a no-op or rejected command does not save. Today a rejected menu toggle still rewrote the file, so this is a behavior change, but a harmless one.
- We accept that `SetTimer` now runs after the lock is dropped rather than inside it. A poll result could land in between, but `request_poll` re-sets the timer from the current state either way.
- We accept that `apply_language_to_state` still calls `set_window_title` (`SetWindowTextW` on the widget) under the lock, as today. Moving it into `Effects` is a cheap follow-up but touches `update_language_change`.
- We accept a `SettingsView::read()` that enumerates taskbars and reads the registry on every dialog sync and menu open. That's a handful of syscalls per user action, in exchange for no cached display or startup state to go stale.
- We accept that `SetAppearance` takes the whole `Appearance` struct, so the dialog composes `palette` onto the current value read on the same thread. A concurrent appearance change can't interleave because both editors run on the UI thread.

## Alternatives considered

- **Dialog posts `IDM_*` to the widget.** This is the smallest diff, but it keeps relative toggles, so a checkbox and the state can disagree. It makes the dialog depend on the menu's private id scheme, keeps the IDM decode table as the de-facto API, and leaves two producers of the same `WM_COMMAND` arms. It's a shallow coupling that leaks the menu's representation into a second module.
- **A family of `apply_poll_interval`, `apply_providers`, … functions (one per field).** Each would be small, but the interface is wide and every function re-implements lock → guard → drop → save → effects → resync. The next agent copies the nearest one and forgets `save_state_settings` or the dialog resync. That's the shallow-module and hand-synced-protocol flag. One `apply(cmd)` hides the protocol once.
- **Draft-and-commit dialog (edit a `SettingsFile` copy; OK writes it).** It introduces a second owner of settings plus a merge against `last_update_check_unix`, `notified_quota_windows`, and live drag/offset changes. It also contradicts "live apply" and "persist only through `save_state_settings`".
- **`CreateDialogIndirectParamW` with an in-memory `DLGTEMPLATE`.** This gets `DefDlgProc` behavior for free, but template units fight per-monitor DPI and per-language relayout, and it still needs the same command layer underneath. A plain class with `WS_EX_CONTROLPARENT` + `IsDialogMessageW` gives the same keyboard behavior with explicit DPI control.

## Red-flag screen

- **Shallow module (avoided):** one `apply` hides the whole change protocol, and `settings_dialog` exposes three functions.
- **Information leakage (avoided):**
  - Effect ordering lives only in `run_effects`.
  - Option lists and labels live on the value enums (`ALL` + `label`).
  - Menu ids exist only inside one `MenuBuilder::track` call.
  - Wire types (`SettingsFile`) never appear in commands or the view.
- **Temporal decomposition (watched and avoided):** prepare/mutate/run are stages, but they're private to the one module that owns the decision, and no caller sequences them.
- **Pass-through (avoided):** `apply` adds policy (guards, ordering, resync). The deleted `toggle_widget_visibility` was the only would-be pass-through.
- **Split ownership (avoided for preferences):** `AppState` is the only copy and the dialog holds none. The residual is that drag/position code also writes `tray_offset` and `monitor_device` directly; that's noted in open questions.
- **Two ways to do one task (avoided):** the `WM_COMMAND` arms and `toggle_widget_visibility` are deleted in the same change, and `apply` is the only write path for preferences.
- **Importable internals (avoided):** the child modules use `pub(super)`, and `AppState` and the helpers stay private to the `window` subtree.
- **Hand-synced list (avoided):**
  - `PollInterval::ALL`, `AlertThreshold::ALL`, the appearance `ALL`s, and `LanguageId::ALL` feed both the menu and the dialog.
  - `Field::ALL` drives control ids, layout, sync, and decode.
  - Exhaustive matches in `label`, `mutate`, and `Field::{present, command}` fail the build on a new variant.
  - A unit test asserts each `ALL` round-trips through `from_ms` / `from_percent` / `from_control_id` and has unique entries.

## Open questions and risks

- Should the new labels be added to `Strings` across all 11 locale files, or ship English-only first with a fallback? This covers the settings window title, section headers, Display, Anchor left, Alert, Close, and the appearance part names that today are hardcoded en/zh in the menu builder.
- When embedded, the widget's owner chain resolves to the topmost `Shell_TrayWnd`, so the owned dialog will likely inherit topmost behavior. Is that acceptable, or should the owner be `None` with the singleton tied to the widget's lifetime instead? Grounding says owner = widget, but this is the visible consequence.
- Should the dialog appear in the taskbar and Alt-Tab (`WS_EX_APPWINDOW`)? It's easy to lose behind other windows without it, unless the topmost inheritance above holds.
- Should drag-to-another-display route through `apply(SetDisplay)` with an offset parameter, so `monitor_device` has exactly one writer?
- If `settings.json` holds a non-preset `poll_interval_ms` (which `normalize_settings` accepts as anything in the 1 min–1 h range), the combo shows no selection. Is that fine, or should `normalize_settings` snap to the nearest preset?
- Does the `windows` crate version in use return the `TPM_RETURNCMD` id through `TrackPopupMenu`'s `BOOL` (`.0`)? This needs verifying before the menu rewrite.
- Testing `mutate` needs an `AppState` fixture, because `provider_poll::Monitor` is inside it. Does the existing `window.rs` test module have one, or should `Visibility` and the value-enum round-trips carry the unit tests while `mutate` is covered manually?

## Next implementation step

Create `src/window/settings.rs` with `SettingsCommand`, `Visibility`, `prepare`/`mutate`/`run_effects`, and `apply`. Port every settings `WM_COMMAND` arm onto it via `MenuBuilder` + `TPM_RETURNCMD`, and delete the arms and `IDM_*` constants, as a behavior-preserving commit before any dialog code exists.
