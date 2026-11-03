use std::time::{Instant, SystemTime};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{SetTimer, ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE};

use crate::localization::{LanguageId, Strings};
use crate::native_interop::{self, TIMER_POLL};
use crate::tray_icon;

use super::{
    apply_language_to_state, attach_to_taskbar, collect_low_quota_alerts, effective_poll_interval,
    is_startup_enabled, lock_state, normalize_focused_provider, position_at_taskbar,
    preferred_taskbar_index, render_layered, request_poll, save_state_settings,
    set_startup_enabled, sync_tray_icons, tr, AppState, Appearance, BarStyle, BarThickness,
    FocusedProvider, FontSize, Palette, QuotaAlert, SendHwnd, MAX_MONITOR_MENU_ITEMS, POLL_15_MIN,
    POLL_1_HOUR, POLL_1_MIN, POLL_5_MIN,
};

/// A user preference change. Every variant is an absolute target, so applying
/// the same command twice leaves state where the first apply left it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SettingsCommand {
    SetPollInterval(PollInterval),
    SetAdaptiveRefresh(bool),
    SetProviderShown(FocusedProvider, bool),
    SetQuotaWindowShown(QuotaWindow, bool),
    SetAlertThreshold(AlertThreshold),
    SetAppearance(Appearance),
    SetLanguage(Option<LanguageId>),
    SetAnchorLeft(bool),
    ResetPosition,
    SetDisplay(DisplayChoice),
    SetWidgetVisible(bool),
    SetStartWithWindows(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PollInterval {
    OneMinute,
    FiveMinutes,
    FifteenMinutes,
    OneHour,
}

impl PollInterval {
    pub(super) const ALL: [Self; 4] = [
        Self::OneMinute,
        Self::FiveMinutes,
        Self::FifteenMinutes,
        Self::OneHour,
    ];

    pub(super) fn ms(self) -> u32 {
        match self {
            Self::OneMinute => POLL_1_MIN,
            Self::FiveMinutes => POLL_5_MIN,
            Self::FifteenMinutes => POLL_15_MIN,
            Self::OneHour => POLL_1_HOUR,
        }
    }

    /// Exact preset, otherwise fifteen minutes. The menu only has four radios,
    /// so a hand-edited interval still has to land on one of them.
    pub(super) fn from_stored(ms: u32) -> Self {
        Self::ALL
            .into_iter()
            .find(|step| step.ms() == ms)
            .unwrap_or(Self::FifteenMinutes)
    }

    pub(super) fn label(self, strings: Strings) -> &'static str {
        match self {
            Self::OneMinute => strings.one_minute,
            Self::FiveMinutes => strings.five_minutes,
            Self::FifteenMinutes => strings.fifteen_minutes,
            Self::OneHour => strings.one_hour,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AlertThreshold {
    Off,
    Ten,
    Twenty,
    Thirty,
}

impl AlertThreshold {
    pub(super) const ALL: [Self; 4] = [Self::Off, Self::Ten, Self::Twenty, Self::Thirty];

    pub(super) fn percent(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Ten => 10,
            Self::Twenty => 20,
            Self::Thirty => 30,
        }
    }

    pub(super) fn from_stored(percent: u8) -> Self {
        Self::ALL
            .into_iter()
            .find(|step| step.percent() == percent)
            .unwrap_or(Self::Off)
    }

    pub(super) fn label(self, language: LanguageId) -> &'static str {
        match self {
            Self::Off => tr(language, "끄기", "Off"),
            Self::Ten => tr(language, "남은 10%", "10% remaining"),
            Self::Twenty => tr(language, "남은 20%", "20% remaining"),
            Self::Thirty => tr(language, "남은 30%", "30% remaining"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuotaWindow {
    Session,
    Weekly,
}

/// Taskbar identity. `device` is the monitor device name when Windows reports one.
/// `index` is the enumeration fallback used when it does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DisplayChoice {
    pub index: usize,
    pub device: Option<String>,
}

impl Palette {
    pub(super) const ALL: [Self; 3] = [
        Self::System,
        Self::HighContrastDark,
        Self::HighContrastLight,
    ];

    pub(super) fn label(self, language: LanguageId) -> &'static str {
        match self {
            Self::System => tr(language, "시스템 설정 따르기", "Follow system"),
            Self::HighContrastDark => tr(language, "고대비 어두운", "High contrast dark"),
            Self::HighContrastLight => tr(language, "고대비 밝은", "High contrast light"),
        }
    }
}

impl BarStyle {
    pub(super) const ALL: [Self; 2] = [Self::Continuous, Self::Segmented];

    pub(super) fn label(self, language: LanguageId) -> &'static str {
        match self {
            Self::Continuous => tr(language, "연속 둥근", "Continuous rounded"),
            Self::Segmented => tr(language, "분할", "Segmented"),
        }
    }
}

impl BarThickness {
    pub(super) const ALL: [Self; 2] = [Self::Standard, Self::Slim];

    pub(super) fn label(self, language: LanguageId) -> &'static str {
        match self {
            Self::Standard => tr(language, "표준", "Standard"),
            Self::Slim => tr(language, "얇게", "Slim"),
        }
    }
}

impl FontSize {
    pub(super) const ALL: [Self; 2] = [Self::Standard, Self::Large];

    pub(super) fn label(self, language: LanguageId) -> &'static str {
        match self {
            Self::Standard => tr(language, "표준", "Standard"),
            Self::Large => tr(language, "더 크게", "Larger"),
        }
    }
}

/// Monitor device name, or `index + 1` when the device name is missing.
/// The menu label and the display resolver both use this string.
pub(super) fn display_key(taskbar: &native_interop::TaskbarWindow, index: usize) -> String {
    native_interop::taskbar_monitor_device(taskbar).unwrap_or_else(|| (index + 1).to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Visibility {
    claude: bool,
    codex: bool,
    cursor: bool,
    claude_available: bool,
    session: bool,
    weekly: bool,
}

impl Visibility {
    fn of(state: &AppState) -> Self {
        Self {
            claude: state.show_claude_code,
            codex: state.show_codex,
            cursor: state.show_cursor,
            claude_available: state.claude_code_available,
            session: state.show_session_window,
            weekly: state.show_weekly_window,
        }
    }

    fn provider_shown(self, provider: FocusedProvider) -> bool {
        match provider {
            FocusedProvider::ClaudeCode => self.claude,
            FocusedProvider::Codex => self.codex,
            FocusedProvider::Cursor => self.cursor,
        }
    }

    /// Turning a provider on is allowed. Claude also needs a logged-in CLI.
    /// Turning one off requires another provider to stay shown.
    fn can_set_provider(self, provider: FocusedProvider, shown: bool) -> bool {
        if shown {
            return provider != FocusedProvider::ClaudeCode || self.claude_available;
        }
        match provider {
            FocusedProvider::ClaudeCode => self.codex || self.cursor,
            FocusedProvider::Codex => self.claude || self.cursor,
            FocusedProvider::Cursor => self.claude || self.codex,
        }
    }

    fn window_shown(self, window: QuotaWindow) -> bool {
        match window {
            QuotaWindow::Session => self.session,
            QuotaWindow::Weekly => self.weekly,
        }
    }

    fn can_set_window(self, window: QuotaWindow, shown: bool) -> bool {
        if shown {
            return true;
        }
        match window {
            QuotaWindow::Session => self.weekly,
            QuotaWindow::Weekly => self.session,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Toggle {
    pub checked: bool,
    pub enabled: bool,
}

pub(super) struct SettingsView {
    pub language: LanguageId,
    pub language_override: Option<LanguageId>,
    pub poll_interval: PollInterval,
    pub adaptive_refresh: bool,
    pub alert_threshold: AlertThreshold,
    pub appearance: Appearance,
    pub anchor_left: bool,
    pub widget_visible: bool,
    pub start_with_windows: bool,
    pub displays: Vec<(DisplayChoice, String)>,
    pub current_display: Option<usize>,
    visibility: Visibility,
}

impl SettingsView {
    pub(super) fn read() -> Self {
        let copied = {
            let state = lock_state();
            state.as_ref().map(|s| {
                (
                    s.language,
                    s.language_override,
                    PollInterval::from_stored(s.poll_interval_ms),
                    s.adaptive_refresh,
                    AlertThreshold::from_stored(s.alert_threshold_percent),
                    s.appearance,
                    s.anchor_left,
                    s.widget_visible,
                    Visibility::of(s),
                    s.taskbar_hwnd,
                )
            })
        };
        let Some((
            language,
            language_override,
            poll_interval,
            adaptive_refresh,
            alert_threshold,
            appearance,
            anchor_left,
            widget_visible,
            visibility,
            taskbar_hwnd,
        )) = copied
        else {
            return Self::detached();
        };
        let start_with_windows = is_startup_enabled();
        let (displays, current_display) = read_displays(language, taskbar_hwnd);
        Self {
            language,
            language_override,
            poll_interval,
            adaptive_refresh,
            alert_threshold,
            appearance,
            anchor_left,
            widget_visible,
            start_with_windows,
            displays,
            current_display,
            visibility,
        }
    }

    fn detached() -> Self {
        let (displays, current_display) = read_displays(LanguageId::English, None);
        Self {
            language: LanguageId::English,
            language_override: None,
            poll_interval: PollInterval::FifteenMinutes,
            adaptive_refresh: true,
            alert_threshold: AlertThreshold::Off,
            appearance: Appearance::default(),
            anchor_left: true,
            widget_visible: true,
            start_with_windows: is_startup_enabled(),
            displays,
            current_display,
            visibility: Visibility {
                claude: false,
                codex: true,
                cursor: true,
                claude_available: false,
                session: true,
                weekly: true,
            },
        }
    }

    pub(super) fn strings(&self) -> Strings {
        self.language.strings()
    }

    pub(super) fn claude_available(&self) -> bool {
        self.visibility.claude_available
    }

    pub(super) fn provider(&self, provider: FocusedProvider) -> Toggle {
        let checked = self.visibility.provider_shown(provider);
        Toggle {
            checked,
            enabled: self.visibility.can_set_provider(provider, !checked),
        }
    }

    pub(super) fn quota_window(&self, window: QuotaWindow) -> Toggle {
        let checked = self.visibility.window_shown(window);
        Toggle {
            checked,
            enabled: self.visibility.can_set_window(window, !checked),
        }
    }
}

fn read_displays(
    language: LanguageId,
    taskbar_hwnd: Option<HWND>,
) -> (Vec<(DisplayChoice, String)>, Option<usize>) {
    let mut displays = Vec::new();
    let mut current_display = None;
    for (index, taskbar) in native_interop::find_taskbars()
        .into_iter()
        .enumerate()
        .take(MAX_MONITOR_MENU_ITEMS)
    {
        if taskbar_hwnd == Some(taskbar.hwnd) {
            current_display = Some(displays.len());
        }
        let key = display_key(&taskbar, index);
        let label = if language == LanguageId::Korean {
            format!("디스플레이 {} ({key})", index + 1)
        } else {
            format!("Display {} ({key})", index + 1)
        };
        displays.push((
            DisplayChoice {
                index,
                device: native_interop::taskbar_monitor_device(&taskbar),
            },
            label,
        ));
    }
    (displays, current_display)
}

fn resolve_display_index(
    taskbars: &[native_interop::TaskbarWindow],
    choice: &DisplayChoice,
) -> usize {
    let want = choice
        .device
        .clone()
        .unwrap_or_else(|| (choice.index + 1).to_string());
    taskbars
        .iter()
        .enumerate()
        .position(|(index, taskbar)| display_key(taskbar, index) == want)
        .unwrap_or_else(|| {
            preferred_taskbar_index(taskbars, choice.device.as_deref(), choice.index)
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AppStateSnapshot {
    poll: PollInterval,
    adaptive: bool,
    claude: bool,
    codex: bool,
    cursor: bool,
    focus: FocusedProvider,
    session: bool,
    weekly: bool,
    alert: AlertThreshold,
    appearance: Appearance,
    language_override: Option<LanguageId>,
    anchor_left: bool,
    tray_offset: i32,
    widget_visible: bool,
    monitor_device: Option<String>,
}

impl AppStateSnapshot {
    fn from_state(state: &AppState) -> Self {
        Self {
            poll: PollInterval::from_stored(state.poll_interval_ms),
            adaptive: state.adaptive_refresh,
            claude: state.show_claude_code,
            codex: state.show_codex,
            cursor: state.show_cursor,
            focus: state.focused_provider,
            session: state.show_session_window,
            weekly: state.show_weekly_window,
            alert: AlertThreshold::from_stored(state.alert_threshold_percent),
            appearance: state.appearance,
            language_override: state.language_override,
            anchor_left: state.anchor_left,
            tray_offset: state.tray_offset,
            widget_visible: state.widget_visible,
            monitor_device: state.monitor_device.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EffectFacts {
    has_poll_result: bool,
    claude_available: bool,
}

enum Change {
    Pure(SettingsCommand),
    DisplayAttached { device: Option<String> },
    Done,
}

struct Effects {
    save: bool,
    retime: bool,
    retime_poll_ms: Option<u32>,
    show_widget: Option<bool>,
    reposition: bool,
    render: bool,
    sync_tray: bool,
    clear_notified: bool,
    notify: bool,
    force_poll: bool,
    clear_texts: bool,
    balloons: Vec<QuotaAlert>,
}

impl Effects {
    fn is_empty(&self) -> bool {
        !self.save
            && !self.retime
            && self.retime_poll_ms.is_none()
            && self.show_widget.is_none()
            && !self.reposition
            && !self.render
            && !self.sync_tray
            && !self.clear_notified
            && !self.notify
            && !self.force_poll
            && !self.clear_texts
            && self.balloons.is_empty()
    }
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            save: false,
            retime: false,
            retime_poll_ms: None,
            show_widget: None,
            reposition: false,
            render: false,
            sync_tray: false,
            clear_notified: false,
            notify: false,
            force_poll: false,
            clear_texts: false,
            balloons: Vec::new(),
        }
    }
}

struct Transition {
    after: AppStateSnapshot,
    effects: Effects,
}

fn unchanged(before: &AppStateSnapshot) -> Transition {
    Transition {
        after: before.clone(),
        effects: Effects::default(),
    }
}

/// Pure guard and effect selection. `mutate` only copies `after` into `AppState`.
fn effects_for(change: &Change, before: &AppStateSnapshot, facts: EffectFacts) -> Transition {
    match change {
        Change::Done => unchanged(before),
        Change::DisplayAttached { device } => {
            let mut after = before.clone();
            after.monitor_device = device.clone();
            after.tray_offset = 0;
            let mut effects = Effects::default();
            effects.save = true;
            effects.reposition = true;
            effects.render = true;
            Transition { after, effects }
        }
        Change::Pure(command) => effects_for_command(command, before, facts),
    }
}

fn effects_for_command(
    command: &SettingsCommand,
    before: &AppStateSnapshot,
    facts: EffectFacts,
) -> Transition {
    let visibility = Visibility {
        claude: before.claude,
        codex: before.codex,
        cursor: before.cursor,
        claude_available: facts.claude_available,
        session: before.session,
        weekly: before.weekly,
    };
    let mut after = before.clone();
    let mut effects = Effects::default();
    match command {
        SettingsCommand::SetPollInterval(interval) => {
            if before.poll == *interval {
                return unchanged(before);
            }
            after.poll = *interval;
            effects.save = true;
            effects.retime = facts.has_poll_result;
        }
        SettingsCommand::SetAdaptiveRefresh(enabled) => {
            if before.adaptive == *enabled {
                return unchanged(before);
            }
            after.adaptive = *enabled;
            effects.save = true;
            effects.render = true;
            effects.retime = facts.has_poll_result;
        }
        SettingsCommand::SetProviderShown(provider, shown) => {
            if visibility.provider_shown(*provider) == *shown
                || !visibility.can_set_provider(*provider, *shown)
            {
                return unchanged(before);
            }
            match provider {
                FocusedProvider::ClaudeCode => after.claude = *shown,
                FocusedProvider::Codex => after.codex = *shown,
                FocusedProvider::Cursor => after.cursor = *shown,
            }
            after.focus =
                normalize_focused_provider(after.claude, after.codex, after.cursor, before.focus);
            effects.save = true;
            effects.clear_texts = true;
            effects.force_poll = true;
            effects.reposition = true;
            effects.render = true;
            effects.sync_tray = true;
        }
        SettingsCommand::SetQuotaWindowShown(window, shown) => {
            if visibility.window_shown(*window) == *shown
                || !visibility.can_set_window(*window, *shown)
            {
                return unchanged(before);
            }
            match window {
                QuotaWindow::Session => after.session = *shown,
                QuotaWindow::Weekly => after.weekly = *shown,
            }
            effects.save = true;
            effects.render = true;
            effects.sync_tray = true;
        }
        SettingsCommand::SetAlertThreshold(threshold) => {
            if before.alert == *threshold {
                return unchanged(before);
            }
            after.alert = *threshold;
            effects.save = true;
            if *threshold == AlertThreshold::Off {
                effects.clear_notified = true;
            } else {
                effects.notify = true;
            }
        }
        SettingsCommand::SetAppearance(appearance) => {
            if before.appearance == *appearance {
                return unchanged(before);
            }
            after.appearance = *appearance;
            effects.save = true;
            effects.reposition = true;
            effects.render = true;
        }
        SettingsCommand::SetLanguage(language) => {
            if before.language_override == *language {
                return unchanged(before);
            }
            after.language_override = *language;
            effects.save = true;
            effects.render = true;
        }
        SettingsCommand::SetAnchorLeft(anchor_left) => {
            if before.anchor_left == *anchor_left {
                return unchanged(before);
            }
            after.anchor_left = *anchor_left;
            effects.save = true;
            effects.reposition = true;
        }
        SettingsCommand::ResetPosition => {
            if before.anchor_left && before.tray_offset == 0 {
                return unchanged(before);
            }
            after.anchor_left = true;
            after.tray_offset = 0;
            effects.save = true;
            effects.reposition = true;
        }
        SettingsCommand::SetDisplay(choice) => {
            if before.monitor_device == choice.device && before.tray_offset == 0 {
                return unchanged(before);
            }
            after.monitor_device = choice.device.clone();
            after.tray_offset = 0;
            effects.save = true;
            effects.reposition = true;
            effects.render = true;
        }
        SettingsCommand::SetWidgetVisible(visible) => {
            if before.widget_visible == *visible {
                return unchanged(before);
            }
            after.widget_visible = *visible;
            effects.save = true;
            effects.show_widget = Some(*visible);
        }
        SettingsCommand::SetStartWithWindows(_) => return unchanged(before),
    }
    Transition { after, effects }
}

/// Win32 and registry work that has to finish before `AppState` changes.
/// A failed display attach returns `None` and leaves the previous device in place.
fn prepare(widget: HWND, command: SettingsCommand) -> Option<Change> {
    match command {
        SettingsCommand::SetDisplay(choice) => {
            let taskbars = native_interop::find_taskbars();
            if taskbars.is_empty() {
                return None;
            }
            let index = resolve_display_index(&taskbars, &choice);
            let device = native_interop::taskbar_monitor_device(&taskbars[index]);
            if !attach_to_taskbar(widget, index) {
                return None;
            }
            Some(Change::DisplayAttached { device })
        }
        SettingsCommand::SetStartWithWindows(enable) => {
            if is_startup_enabled() == enable {
                return None;
            }
            set_startup_enabled(enable);
            Some(Change::Done)
        }
        other => Some(Change::Pure(other)),
    }
}

fn commit_snapshot(state: &mut AppState, before: &AppStateSnapshot, after: &AppStateSnapshot) {
    state.poll_interval_ms = after.poll.ms();
    state.adaptive_refresh = after.adaptive;
    state.show_claude_code = after.claude;
    state.show_codex = after.codex;
    state.show_cursor = after.cursor;
    state.focused_provider = after.focus;
    state.show_session_window = after.session;
    state.show_weekly_window = after.weekly;
    state.alert_threshold_percent = after.alert.percent();
    state.appearance = after.appearance;
    state.anchor_left = after.anchor_left;
    state.tray_offset = after.tray_offset;
    state.widget_visible = after.widget_visible;
    state.monitor_device = after.monitor_device.clone();
    if before.language_override != after.language_override {
        apply_language_to_state(state, after.language_override);
    }
}

fn clear_usage_texts(state: &mut AppState) {
    state.session_text = "...".to_string();
    state.weekly_text = "...".to_string();
    state.codex_session_text = "...".to_string();
    state.codex_weekly_text = "...".to_string();
    state.cursor_session_text = "...".to_string();
    state.cursor_weekly_text = "...".to_string();
}

fn mutate(state: &mut AppState, change: Change) -> Effects {
    let before = AppStateSnapshot::from_state(state);
    let facts = EffectFacts {
        has_poll_result: state.has_poll_result,
        claude_available: state.claude_code_available,
    };
    let transition = effects_for(&change, &before, facts);
    if transition.effects.is_empty() {
        return Effects::default();
    }
    commit_snapshot(state, &before, &transition.after);
    if transition.effects.clear_texts {
        clear_usage_texts(state);
    }
    if transition.effects.clear_notified {
        state.notified_quota_windows.clear();
    }
    let mut effects = transition.effects;
    if effects.notify {
        if let Some(data) = state.data.clone() {
            effects.balloons = collect_low_quota_alerts(state, &data);
        }
    }
    if effects.retime {
        state.monitor.reconfigure(
            state.poll_interval_ms,
            state.adaptive_refresh,
            Instant::now(),
            SystemTime::now(),
        );
        effects.retime_poll_ms = Some(effective_poll_interval(state));
    }
    effects
}

fn run_effects(widget: HWND, effects: Effects) {
    if effects.save {
        save_state_settings();
    }
    if let Some(delay_ms) = effects.retime_poll_ms {
        unsafe {
            SetTimer(widget, TIMER_POLL, delay_ms, None);
        }
    }
    if let Some(visible) = effects.show_widget {
        unsafe {
            if visible {
                position_at_taskbar();
                let _ = ShowWindow(widget, SW_SHOWNOACTIVATE);
                render_layered();
            } else {
                let _ = ShowWindow(widget, SW_HIDE);
            }
        }
    }
    if effects.reposition {
        position_at_taskbar();
    }
    if effects.render {
        render_layered();
    }
    if effects.sync_tray {
        sync_tray_icons(widget);
    }
    for alert in &effects.balloons {
        tray_icon::notify_balloon(widget, alert.kind, &alert.title, &alert.message);
    }
    if effects.force_poll {
        request_poll(SendHwnd::from_hwnd(widget), true);
    }
}

/// The one write path for user preferences.
/// The state lock is dropped before `save_state_settings` and before any Win32
/// call that can pump messages. `save_state_settings` takes the saves lock first.
pub(super) fn apply(widget: HWND, command: SettingsCommand) {
    let Some(change) = prepare(widget, command) else {
        return;
    };
    let effects = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        mutate(state, change)
    };
    run_effects(widget, effects);
}

#[cfg(test)]
fn sample_snapshot() -> AppStateSnapshot {
    AppStateSnapshot {
        poll: PollInterval::FifteenMinutes,
        adaptive: true,
        claude: false,
        codex: true,
        cursor: true,
        focus: FocusedProvider::Codex,
        session: true,
        weekly: true,
        alert: AlertThreshold::Ten,
        appearance: Appearance::default(),
        language_override: None,
        anchor_left: true,
        tray_offset: 0,
        widget_visible: true,
        monitor_device: None,
    }
}

#[cfg(test)]
fn facts(has_poll_result: bool, claude_available: bool) -> EffectFacts {
    EffectFacts {
        has_poll_result,
        claude_available,
    }
}

#[cfg(test)]
#[test]
fn stored_poll_and_alert_values_snap_to_presets() {
    assert_eq!(
        PollInterval::from_stored(900_000),
        PollInterval::FifteenMinutes
    );
    assert_eq!(
        PollInterval::from_stored(120_000),
        PollInterval::FifteenMinutes
    );
    assert_eq!(PollInterval::from_stored(60_000), PollInterval::OneMinute);
    assert_eq!(
        PollInterval::from_stored(300_000),
        PollInterval::FiveMinutes
    );
    assert_eq!(PollInterval::from_stored(3_600_000), PollInterval::OneHour);
    assert_eq!(PollInterval::from_stored(0), PollInterval::FifteenMinutes);
    for step in PollInterval::ALL {
        assert_eq!(PollInterval::from_stored(step.ms()), step);
    }

    assert_eq!(AlertThreshold::from_stored(0), AlertThreshold::Off);
    assert_eq!(AlertThreshold::from_stored(10), AlertThreshold::Ten);
    assert_eq!(AlertThreshold::from_stored(20), AlertThreshold::Twenty);
    assert_eq!(AlertThreshold::from_stored(30), AlertThreshold::Thirty);
    assert_eq!(AlertThreshold::from_stored(17), AlertThreshold::Off);
    for step in AlertThreshold::ALL {
        assert_eq!(AlertThreshold::from_stored(step.percent()), step);
    }
}

#[cfg(test)]
#[test]
fn visibility_refuses_the_last_provider_and_unavailable_claude() {
    let only_codex = Visibility {
        claude: false,
        codex: true,
        cursor: false,
        claude_available: true,
        session: true,
        weekly: false,
    };
    assert!(!only_codex.can_set_provider(FocusedProvider::Codex, false));
    assert!(only_codex.can_set_provider(FocusedProvider::Cursor, true));
    assert!(!only_codex.can_set_window(QuotaWindow::Session, false));
    assert!(only_codex.can_set_window(QuotaWindow::Weekly, true));

    let claude_unavailable = Visibility {
        claude: false,
        codex: true,
        cursor: true,
        claude_available: false,
        session: true,
        weekly: true,
    };
    assert!(!claude_unavailable.can_set_provider(FocusedProvider::ClaudeCode, true));
    assert!(claude_unavailable.can_set_provider(FocusedProvider::Codex, false));
}

#[cfg(test)]
#[test]
fn effects_refuse_the_last_provider_and_unavailable_claude() {
    let only_codex = AppStateSnapshot {
        claude: false,
        cursor: false,
        ..sample_snapshot()
    };
    let refused = effects_for(
        &Change::Pure(SettingsCommand::SetProviderShown(
            FocusedProvider::Codex,
            false,
        )),
        &only_codex,
        facts(false, true),
    );
    assert!(refused.after.codex);
    assert_eq!(refused.after, only_codex);
    assert!(refused.effects.is_empty());

    let before = sample_snapshot();
    let claude = effects_for(
        &Change::Pure(SettingsCommand::SetProviderShown(
            FocusedProvider::ClaudeCode,
            true,
        )),
        &before,
        facts(false, false),
    );
    assert!(!claude.after.claude);
    assert_eq!(claude.after, before);
    assert!(claude.effects.is_empty());
}

#[cfg(test)]
#[test]
fn disabling_the_focused_provider_uses_codex_cursor_claude_order() {
    let mut before = sample_snapshot();
    before.claude = true;
    before.focus = FocusedProvider::Codex;
    let disabled_codex = effects_for(
        &Change::Pure(SettingsCommand::SetProviderShown(
            FocusedProvider::Codex,
            false,
        )),
        &before,
        facts(false, true),
    );
    assert!(!disabled_codex.after.codex);
    assert_eq!(disabled_codex.after.focus, FocusedProvider::Cursor);
    assert!(disabled_codex.effects.force_poll);
    assert!(disabled_codex.effects.save);
    assert!(!disabled_codex.effects.retime);

    before.focus = FocusedProvider::Cursor;
    let disabled_cursor = effects_for(
        &Change::Pure(SettingsCommand::SetProviderShown(
            FocusedProvider::Cursor,
            false,
        )),
        &before,
        facts(false, true),
    );
    assert_eq!(disabled_cursor.after.focus, FocusedProvider::Codex);
}

#[cfg(test)]
#[test]
fn poll_retime_runs_only_after_a_poll_result() {
    let before = sample_snapshot();
    let command = Change::Pure(SettingsCommand::SetPollInterval(PollInterval::OneMinute));
    let with_result = effects_for(&command, &before, facts(true, true));
    assert!(with_result.effects.retime);
    assert!(with_result.effects.save);
    assert!(!with_result.effects.force_poll);
    assert!(!with_result.effects.render);
    assert_eq!(with_result.after.poll, PollInterval::OneMinute);

    let without_result = effects_for(&command, &before, facts(false, true));
    assert!(!without_result.effects.retime);
    assert!(without_result.effects.save);

    let same = effects_for(
        &Change::Pure(SettingsCommand::SetPollInterval(
            PollInterval::FifteenMinutes,
        )),
        &before,
        facts(true, true),
    );
    assert!(same.effects.is_empty());

    let adaptive = effects_for(
        &Change::Pure(SettingsCommand::SetAdaptiveRefresh(false)),
        &before,
        facts(true, true),
    );
    assert!(adaptive.effects.retime);
    assert!(adaptive.effects.render);
    let adaptive_before_poll = effects_for(
        &Change::Pure(SettingsCommand::SetAdaptiveRefresh(false)),
        &before,
        facts(false, true),
    );
    assert!(!adaptive_before_poll.effects.retime);
    assert!(adaptive_before_poll.effects.render);
    assert!(adaptive_before_poll.effects.save);
}

#[cfg(test)]
#[test]
fn alert_off_clears_notified_windows_and_a_new_threshold_notifies() {
    let before = sample_snapshot();
    let off = effects_for(
        &Change::Pure(SettingsCommand::SetAlertThreshold(AlertThreshold::Off)),
        &before,
        facts(true, true),
    );
    assert!(off.effects.clear_notified);
    assert!(!off.effects.notify);
    assert!(off.effects.save);
    assert_eq!(off.after.alert, AlertThreshold::Off);

    let raised = effects_for(
        &Change::Pure(SettingsCommand::SetAlertThreshold(AlertThreshold::Twenty)),
        &before,
        facts(false, true),
    );
    assert!(raised.effects.notify);
    assert!(!raised.effects.clear_notified);
    assert_eq!(raised.after.alert, AlertThreshold::Twenty);

    let same = effects_for(
        &Change::Pure(SettingsCommand::SetAlertThreshold(AlertThreshold::Ten)),
        &before,
        facts(true, true),
    );
    assert!(same.effects.is_empty());
}

#[cfg(test)]
#[test]
fn reset_position_sets_the_left_anchor_and_clears_the_offset() {
    let mut before = sample_snapshot();
    before.anchor_left = false;
    before.tray_offset = 40;
    let reset = effects_for(
        &Change::Pure(SettingsCommand::ResetPosition),
        &before,
        facts(false, true),
    );
    assert!(reset.after.anchor_left);
    assert_eq!(reset.after.tray_offset, 0);
    assert!(reset.effects.reposition);
    assert!(reset.effects.save);
    assert!(!reset.effects.render);

    let already = sample_snapshot();
    let again = effects_for(
        &Change::Pure(SettingsCommand::ResetPosition),
        &already,
        facts(false, true),
    );
    assert!(again.effects.is_empty());
    assert!(again.after.anchor_left);
    assert_eq!(again.after.tray_offset, 0);
}
