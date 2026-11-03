use std::cell::Cell;
use std::sync::Once;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, GetMonitorInfoW, GetSysColorBrush, MonitorFromWindow,
    COLOR_BTNFACE, HFONT, LOGFONTW, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::BST_CHECKED;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::localization::LanguageId;
use crate::native_interop;

use super::settings::{
    AlertThreshold, DisplayChoice, PollInterval, QuotaWindow, SettingsCommand, SettingsView, Toggle,
};
use super::{
    claude_code_menu_label, load_embedded_app_icons, tr, Appearance, BarStyle, BarThickness,
    FocusedProvider, FontSize, Palette, WM_DPICHANGED_MSG,
};

const CLASS_NAME: &str = "AIUsageMonitorSettings";
const CONTROL_ID_BASE: u16 = 100;
const FIELD_COUNT: usize = 21;
const SECTION_COUNT: usize = 8;
const CLIENT_WIDTH: i32 = 440;
const PAD: i32 = 12;
/// windows 0.58 has no name for this message. It sizes the drop list without
/// stretching the combo over the rows below it.
const CB_SETMINVISIBLE: u32 = 0x1701;

thread_local! {
    static DIALOG: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
    // SendMessage while painting controls re-enters the wndproc. A bool on
    // DialogState would alias that borrow, so the guard lives beside it.
    static SYNCING: Cell<bool> = const { Cell::new(false) };
}

struct SyncGuard;

impl SyncGuard {
    fn enter() -> Option<Self> {
        if SYNCING.with(|flag| flag.get()) {
            return None;
        }
        SYNCING.with(|flag| flag.set(true));
        Some(Self)
    }
}

impl Drop for SyncGuard {
    fn drop(&mut self) {
        SYNCING.with(|flag| flag.set(false));
    }
}

/// Owner, not SetParent. SetParent would make the dialog a taskbar child, and a
/// child of the taskbar cannot be activated.
pub(super) fn open(widget: HWND) {
    let existing = DIALOG.with(|cell| cell.get());
    if unsafe { IsWindow(existing).as_bool() } {
        unsafe {
            let _ = ShowWindow(existing, SW_RESTORE);
            let _ = SetWindowPos(
                existing,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            let _ = SetForegroundWindow(existing);
        }
        sync();
        return;
    }

    ensure_class();
    let instance = match unsafe { GetModuleHandleW(PCWSTR::null()) } {
        Ok(module) => HINSTANCE(module.0),
        Err(_) => return,
    };
    let state = Box::new(DialogState {
        widget,
        dpi: 96,
        font: HFONT::default(),
        controls: vec![HWND::default(); FIELD_COUNT],
        labels: vec![HWND::default(); FIELD_COUNT],
        headers: vec![HWND::default(); SECTION_COUNT],
        combo_keys: vec![String::new(); FIELD_COUNT],
        positioned: false,
        anchor: None,
    });
    let raw = Box::into_raw(state);
    let title = native_interop::wide_str("Settings");
    let class_name = native_interop::wide_str(CLASS_NAME);
    let created = unsafe {
        CreateWindowExW(
            dialog_ex_style(),
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            dialog_style(),
            0,
            0,
            100,
            100,
            widget,
            None,
            instance,
            Some(raw as *const _),
        )
    };
    let hwnd = match created {
        Ok(hwnd) => hwnd,
        Err(error) => {
            drop(unsafe { Box::from_raw(raw) });
            crate::diagnose::log(format!("settings dialog was not created: {error}"));
            return;
        }
    };
    DIALOG.with(|cell| cell.set(hwnd));
    sync();
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

pub(super) fn pre_translate(msg: &MSG) -> bool {
    let hwnd = DIALOG.with(|cell| cell.get());
    if hwnd.is_invalid() || !unsafe { IsWindow(hwnd).as_bool() } {
        return false;
    }
    unsafe { IsDialogMessageW(hwnd, msg).as_bool() }
}

pub(super) fn sync() {
    let hwnd = DIALOG.with(|cell| cell.get());
    if hwnd.is_invalid() || !unsafe { IsWindow(hwnd).as_bool() } {
        return;
    }
    sync_hwnd(hwnd);
}

fn sync_hwnd(hwnd: HWND) {
    let Some(_guard) = SyncGuard::enter() else {
        return;
    };
    let view = SettingsView::read();
    let Some(state) = state_mut(hwnd) else {
        return;
    };
    apply_view(hwnd, state, &view);
}

fn dialog_style() -> WINDOW_STYLE {
    WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN
}

fn dialog_ex_style() -> WINDOW_EX_STYLE {
    WS_EX_CONTROLPARENT | WS_EX_DLGMODALFRAME
}

fn ensure_class() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let class_name = native_interop::wide_str(CLASS_NAME);
        let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }.unwrap_or_default();
        let (large_icon, small_icon) = load_embedded_app_icons();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(dialog_proc),
            hInstance: HINSTANCE(instance.0),
            hIcon: large_icon,
            hIconSm: small_icon,
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
            hbrBackground: unsafe { GetSysColorBrush(COLOR_BTNFACE) },
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&class) } == 0 {
            crate::diagnose::log("settings dialog class was not registered");
        }
    });
}

unsafe extern "system" fn dialog_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let created = &*(lparam.0 as *const CREATESTRUCTW);
            let state = &mut *(created.lpCreateParams as *mut DialogState);
            // The widget's CURRENT_DPI follows the taskbar. This window sits on
            // the monitor work area, so its scale comes from its own DPI.
            let dpi = GetDpiForWindow(hwnd).max(96);
            state.dpi = dpi;
            state.font = message_font(dpi);
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, created.lpCreateParams as isize);
            create_children(hwnd, state, created.hInstance);
            LRESULT(0)
        }
        WM_COMMAND => on_command(hwnd, wparam, lparam),
        msg if msg == DM_GETDEFID => {
            let packed = ((DC_HASDEFID as u32) << 16) | Field::Close.control_id() as u32;
            LRESULT(packed as isize)
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xffff) != WA_INACTIVE as usize {
                sync_hwnd(hwnd);
            }
            LRESULT(0)
        }
        msg if msg == WM_DPICHANGED_MSG => {
            let dpi = (wparam.0 & 0xffff) as u32;
            if dpi > 0 {
                let origin = if lparam.0 == 0 {
                    None
                } else {
                    let suggested = &*(lparam.0 as *const RECT);
                    Some((suggested.left, suggested.top))
                };
                if let Some(state) = state_mut(hwnd) {
                    install_font(state, dpi);
                    state.anchor = origin;
                }
                sync_hwnd(hwnd);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            if DIALOG.with(|cell| cell.get()) == hwnd {
                DIALOG.with(|cell| cell.set(HWND::default()));
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let raw = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut DialogState;
            if !raw.is_null() {
                let state = Box::from_raw(raw);
                if !state.font.is_invalid() {
                    let _ = DeleteObject(state.font);
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn on_command(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if SYNCING.with(|flag| flag.get()) {
        return LRESULT(0);
    }
    let id = (wparam.0 & 0xffff) as i32;
    let code = ((wparam.0 >> 16) & 0xffff) as u16;
    if id == IDOK.0 || id == IDCANCEL.0 {
        let _ = unsafe { DestroyWindow(hwnd) };
        return LRESULT(0);
    }
    let Some(field) = Field::from_control_id(id as u16) else {
        return LRESULT(0);
    };
    if !field.accepts(code) {
        return LRESULT(0);
    }
    if field == Field::Close {
        let _ = unsafe { DestroyWindow(hwnd) };
        return LRESULT(0);
    }
    let widget = state_mut(hwnd)
        .map(|state| state.widget)
        .unwrap_or_default();
    let control = HWND(lparam.0 as *mut _);
    let view = SettingsView::read();
    match field.command(&view, control) {
        Some(command) => super::settings::apply(widget, command),
        None => sync_hwnd(hwnd),
    }
    LRESULT(0)
}

struct DialogState {
    widget: HWND,
    dpi: u32,
    font: HFONT,
    controls: Vec<HWND>,
    labels: Vec<HWND>,
    headers: Vec<HWND>,
    combo_keys: Vec<String>,
    positioned: bool,
    anchor: Option<(i32, i32)>,
}

fn state_mut(hwnd: HWND) -> Option<&'static mut DialogState> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut DialogState;
    if raw.is_null() {
        None
    } else {
        Some(unsafe { &mut *raw })
    }
}

fn create_children(parent: HWND, state: &mut DialogState, instance: HINSTANCE) {
    for section in Section::ALL {
        state.headers[section.index()] =
            create_child(parent, w!("STATIC"), "", WS_CHILD | WS_VISIBLE, 0, instance);
    }
    for field in Field::ALL {
        let index = field.index();
        let (class, style) = match field.kind() {
            ControlKind::Check => (
                w!("BUTTON"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
            ),
            ControlKind::Choice => (
                w!("COMBOBOX"),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | WS_VSCROLL
                    | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_HASSTRINGS) as u32),
            ),
            ControlKind::Button => {
                let button = if field == Field::Close {
                    BS_DEFPUSHBUTTON
                } else {
                    BS_PUSHBUTTON
                };
                (
                    w!("BUTTON"),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(button as u32),
                )
            }
        };
        state.controls[index] =
            create_child(parent, class, "", style, field.control_id(), instance);
        state.labels[index] =
            create_child(parent, w!("STATIC"), "", WS_CHILD | WS_VISIBLE, 0, instance);
        if field.kind() == ControlKind::Choice {
            unsafe {
                SendMessageW(
                    state.controls[index],
                    CB_SETMINVISIBLE,
                    WPARAM(8),
                    LPARAM(0),
                );
            }
        }
    }
    paint_font(state);
}

fn create_child(
    parent: HWND,
    class: PCWSTR,
    text: &str,
    style: WINDOW_STYLE,
    id: u16,
    instance: HINSTANCE,
) -> HWND {
    let wide = native_interop::wide_str(text);
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR::from_raw(wide.as_ptr()),
            style,
            0,
            0,
            10,
            10,
            parent,
            HMENU(id as usize as *mut _),
            instance,
            None,
        )
        .unwrap_or_default()
    }
}

fn apply_view(hwnd: HWND, state: &mut DialogState, view: &SettingsView) {
    set_text(hwnd, &view.strings().settings);
    for section in Section::ALL {
        let header = state.headers[section.index()];
        match section_title(section, view) {
            Some(title) if section_visible(section, view) => set_text(header, &title),
            _ => {}
        }
    }
    for field in Field::ALL {
        let index = field.index();
        let control = state.controls[index];
        if control.is_invalid() {
            continue;
        }
        match field.present(view) {
            Presented::Check { text, toggle } => {
                set_text(control, &text);
                set_checked(control, toggle.checked);
                let _ = unsafe { EnableWindow(control, toggle.enabled) };
            }
            Presented::Choice {
                options,
                selected,
                enabled,
            } => {
                let key = options.join("\u{1}");
                if state.combo_keys[index] != key {
                    refill_combo(control, &options);
                    state.combo_keys[index] = key;
                }
                let selection = selected.unwrap_or(usize::MAX);
                unsafe {
                    SendMessageW(control, CB_SETCURSEL, WPARAM(selection), LPARAM(0));
                }
                let _ = unsafe { EnableWindow(control, enabled) };
            }
            Presented::Button { text, enabled } => {
                set_text(control, &text);
                let _ = unsafe { EnableWindow(control, enabled) };
            }
        }
        if let Some(label) = field.side_label(view) {
            set_text(state.labels[index], &label);
        }
    }
    layout(hwnd, state, view);
}

fn layout(hwnd: HWND, state: &mut DialogState, view: &SettingsView) {
    let dpi = state.dpi;
    let pad = px(dpi, PAD);
    let width = px(dpi, CLIENT_WIDTH);
    let mut y = pad;
    let mut last_section: Option<Section> = None;
    for field in Field::ALL {
        let index = field.index();
        let control = state.controls[index];
        let label = state.labels[index];
        if !field.is_visible(view) || control.is_invalid() {
            place(control, 0, 0, 0, 0, false);
            place(label, 0, 0, 0, 0, false);
            continue;
        }
        if last_section != Some(field.section()) {
            if let Some(previous) = last_section {
                if !section_visible(previous, view) {
                    place(state.headers[previous.index()], 0, 0, 0, 0, false);
                }
            }
            last_section = Some(field.section());
            let header = state.headers[field.section().index()];
            if section_title(field.section(), view).is_some() {
                place(header, pad, y, width, px(dpi, 18), true);
                y += px(dpi, 22);
            } else {
                place(header, 0, 0, 0, 0, false);
                y += px(dpi, 8);
            }
        }
        match field.kind() {
            ControlKind::Check => {
                place(control, pad, y, width, px(dpi, 22), true);
                place(label, 0, 0, 0, 0, false);
                y += px(dpi, 26);
            }
            ControlKind::Choice => {
                let row_h = px(dpi, 24);
                if field.side_label(view).is_some() {
                    let label_w = px(dpi, 150);
                    place(label, pad, y + px(dpi, 3), label_w, px(dpi, 18), true);
                    place(control, pad + label_w, y, width - label_w, row_h, true);
                } else {
                    place(label, 0, 0, 0, 0, false);
                    place(control, pad, y, width, row_h, true);
                }
                y += px(dpi, 30);
            }
            ControlKind::Button => {
                place(control, pad, y, width, px(dpi, 26), true);
                place(label, 0, 0, 0, 0, false);
                y += px(dpi, 32);
            }
        }
    }
    if let Some(previous) = last_section {
        if !section_visible(previous, view) {
            place(state.headers[previous.index()], 0, 0, 0, 0, false);
        }
    }
    for section in Section::ALL {
        if !section_visible(section, view) {
            place(state.headers[section.index()], 0, 0, 0, 0, false);
        }
    }
    y += pad;
    size_dialog(hwnd, state, pad * 2 + width, y);
}

fn size_dialog(hwnd: HWND, state: &mut DialogState, client_w: i32, client_h: i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: client_w,
        bottom: client_h,
    };
    let (style, ex_style) = unsafe {
        (
            WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32),
            WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32),
        )
    };
    unsafe {
        let _ = AdjustWindowRectExForDpi(&mut rect, style, false, ex_style, state.dpi);
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    unsafe {
        if let Some((x, y)) = state.anchor.take() {
            let _ = SetWindowPos(
                hwnd,
                HWND::default(),
                x,
                y,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            state.positioned = true;
        } else if state.positioned {
            let _ = SetWindowPos(
                hwnd,
                HWND::default(),
                0,
                0,
                width,
                height,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        } else {
            let (x, y) = center_on_widget(state.widget, width, height);
            let _ = SetWindowPos(
                hwnd,
                HWND::default(),
                x,
                y,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            state.positioned = true;
        }
    }
}

fn center_on_widget(widget: HWND, width: i32, height: i32) -> (i32, i32) {
    unsafe {
        let monitor = MonitorFromWindow(widget, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let work = info.rcWork;
            let x = work.left + (work.right - work.left - width).max(0) / 2;
            let y = work.top + (work.bottom - work.top - height).max(0) / 2;
            return (x, y);
        }
    }
    (80, 80)
}

fn place(hwnd: HWND, x: i32, y: i32, width: i32, height: i32, show: bool) {
    if hwnd.is_invalid() {
        return;
    }
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND::default(),
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd, if show { SW_SHOW } else { SW_HIDE });
    }
}

fn install_font(state: &mut DialogState, dpi: u32) {
    let font = message_font(dpi);
    if font.is_invalid() {
        state.dpi = dpi;
        return;
    }
    let old = state.font;
    state.font = font;
    state.dpi = dpi;
    paint_font(state);
    if !old.is_invalid() {
        unsafe {
            let _ = DeleteObject(old);
        }
    }
}

fn paint_font(state: &DialogState) {
    if state.font.is_invalid() {
        return;
    }
    let font = WPARAM(state.font.0 as usize);
    unsafe {
        for hwnd in state
            .controls
            .iter()
            .chain(state.labels.iter())
            .chain(state.headers.iter())
        {
            if !hwnd.is_invalid() {
                SendMessageW(*hwnd, WM_SETFONT, font, LPARAM(1));
            }
        }
    }
}

fn message_font(dpi: u32) -> HFONT {
    unsafe {
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        let _ = SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            Some(&mut metrics as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let mut font = metrics.lfMessageFont;
        let system = GetDpiForSystem().max(96);
        if font.lfFaceName[0] == 0 {
            font = LOGFONTW {
                lfHeight: -px(dpi, 12),
                ..Default::default()
            };
            let face = native_interop::wide_str("Segoe UI");
            for (index, unit) in face.iter().take(font.lfFaceName.len()).enumerate() {
                font.lfFaceName[index] = *unit;
            }
        } else {
            font.lfHeight = (font.lfHeight as i64 * dpi as i64 / system as i64) as i32;
        }
        CreateFontIndirectW(&font)
    }
}

fn px(dpi: u32, logical: i32) -> i32 {
    (logical as f64 * dpi.max(1) as f64 / 96.0).round() as i32
}

fn set_text(hwnd: HWND, text: &str) {
    if hwnd.is_invalid() {
        return;
    }
    let wide = native_interop::wide_str(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR::from_raw(wide.as_ptr()));
    }
}

fn set_checked(control: HWND, checked: bool) {
    let value = if checked { BST_CHECKED.0 as usize } else { 0 };
    unsafe {
        SendMessageW(control, BM_SETCHECK, WPARAM(value), LPARAM(0));
    }
}

fn is_checked(control: HWND) -> bool {
    unsafe { SendMessageW(control, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == BST_CHECKED.0 as isize }
}

fn selected_index(control: HWND) -> Option<usize> {
    let selected = unsafe { SendMessageW(control, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    if selected < 0 {
        None
    } else {
        Some(selected as usize)
    }
}

fn refill_combo(control: HWND, options: &[String]) {
    unsafe {
        SendMessageW(control, CB_RESETCONTENT, WPARAM(0), LPARAM(0));
        for option in options {
            let wide = native_interop::wide_str(option);
            SendMessageW(
                control,
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(wide.as_ptr() as isize),
            );
        }
    }
}

fn section_visible(section: Section, view: &SettingsView) -> bool {
    Field::ALL
        .iter()
        .any(|field| field.section() == section && field.is_visible(view))
}

fn section_title(section: Section, view: &SettingsView) -> Option<String> {
    let language = view.language;
    let strings = view.strings();
    let title: &str = match section {
        Section::Refresh => strings.update_frequency,
        Section::Providers => strings.models,
        Section::Usage => tr(language, "사용량 표시", "Usage display"),
        Section::Alerts => tr(language, "할당량 알림", "Quota alerts"),
        Section::Appearance => tr(language, "모양", "Appearance"),
        Section::Language => strings.language,
        Section::Position => "Position",
        Section::General => return None,
    };
    Some(title.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Refresh = 0,
    Providers = 1,
    Usage = 2,
    Alerts = 3,
    Appearance = 4,
    Language = 5,
    Position = 6,
    General = 7,
}

impl Section {
    const ALL: [Section; SECTION_COUNT] = [
        Self::Refresh,
        Self::Providers,
        Self::Usage,
        Self::Alerts,
        Self::Appearance,
        Self::Language,
        Self::Position,
        Self::General,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControlKind {
    Check,
    Choice,
    Button,
}

enum Presented {
    Check {
        text: String,
        toggle: Toggle,
    },
    Choice {
        options: Vec<String>,
        selected: Option<usize>,
        enabled: bool,
    },
    Button {
        text: String,
        enabled: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    PollInterval,
    AdaptiveRefresh,
    Provider(FocusedProvider),
    QuotaWindow(QuotaWindow),
    AlertThreshold,
    Palette,
    BarStyle,
    BarThickness,
    FontSize,
    AppearanceRecommended,
    AppearanceReset,
    Language,
    Display,
    AnchorLeft,
    ResetPosition,
    WidgetVisible,
    StartWithWindows,
    Close,
}

impl Field {
    const ALL: [Field; FIELD_COUNT] = [
        Self::PollInterval,
        Self::AdaptiveRefresh,
        Self::Provider(FocusedProvider::ClaudeCode),
        Self::Provider(FocusedProvider::Codex),
        Self::Provider(FocusedProvider::Cursor),
        Self::QuotaWindow(QuotaWindow::Session),
        Self::QuotaWindow(QuotaWindow::Weekly),
        Self::AlertThreshold,
        Self::Palette,
        Self::BarStyle,
        Self::BarThickness,
        Self::FontSize,
        Self::AppearanceRecommended,
        Self::AppearanceReset,
        Self::Language,
        Self::Display,
        Self::AnchorLeft,
        Self::ResetPosition,
        Self::WidgetVisible,
        Self::StartWithWindows,
        Self::Close,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|field| *field == self).unwrap()
    }

    fn control_id(self) -> u16 {
        CONTROL_ID_BASE + self.index() as u16
    }

    fn from_control_id(id: u16) -> Option<Self> {
        let index = id.checked_sub(CONTROL_ID_BASE)? as usize;
        Self::ALL.get(index).copied()
    }

    fn kind(self) -> ControlKind {
        match self {
            Self::AdaptiveRefresh
            | Self::Provider(_)
            | Self::QuotaWindow(_)
            | Self::AnchorLeft
            | Self::WidgetVisible
            | Self::StartWithWindows => ControlKind::Check,
            Self::PollInterval
            | Self::AlertThreshold
            | Self::Palette
            | Self::BarStyle
            | Self::BarThickness
            | Self::FontSize
            | Self::Language
            | Self::Display => ControlKind::Choice,
            Self::AppearanceRecommended
            | Self::AppearanceReset
            | Self::ResetPosition
            | Self::Close => ControlKind::Button,
        }
    }

    fn section(self) -> Section {
        match self {
            Self::PollInterval | Self::AdaptiveRefresh => Section::Refresh,
            Self::Provider(_) => Section::Providers,
            Self::QuotaWindow(_) => Section::Usage,
            Self::AlertThreshold => Section::Alerts,
            Self::Palette
            | Self::BarStyle
            | Self::BarThickness
            | Self::FontSize
            | Self::AppearanceRecommended
            | Self::AppearanceReset => Section::Appearance,
            Self::Language => Section::Language,
            Self::Display | Self::AnchorLeft | Self::ResetPosition => Section::Position,
            Self::WidgetVisible | Self::StartWithWindows | Self::Close => Section::General,
        }
    }

    fn is_visible(self, view: &SettingsView) -> bool {
        match self {
            Self::Display => view.displays.len() > 1,
            _ => true,
        }
    }

    fn accepts(self, code: u16) -> bool {
        match self.kind() {
            ControlKind::Check | ControlKind::Button => code == BN_CLICKED as u16,
            ControlKind::Choice => code == CBN_SELCHANGE as u16,
        }
    }

    fn side_label(self, view: &SettingsView) -> Option<String> {
        let language = view.language;
        let label = match self {
            Self::Palette => tr(language, "텍스트 및 색", "Text and colors"),
            Self::BarStyle => tr(language, "막대 스타일", "Bar style"),
            Self::BarThickness => tr(language, "막대 두께", "Bar thickness"),
            Self::FontSize => tr(language, "글자 크기", "Text size"),
            Self::Display => tr(language, "디스플레이", "Display"),
            _ => return None,
        };
        Some(label.to_string())
    }

    fn present(self, view: &SettingsView) -> Presented {
        let strings = view.strings();
        let language = view.language;
        match self {
            Self::PollInterval => choice(
                PollInterval::ALL.map(|interval| interval.label(strings).to_string()),
                PollInterval::ALL
                    .iter()
                    .position(|interval| *interval == view.poll_interval),
                true,
            ),
            Self::AdaptiveRefresh => check(
                tr(
                    language,
                    "남은 할당량 ≤20%일 때 1분마다 새로고침",
                    "Refresh every minute when remaining ≤20%",
                ),
                Toggle {
                    checked: view.adaptive_refresh,
                    enabled: true,
                },
            ),
            Self::Provider(provider) => {
                let toggle = view.provider(provider);
                let text = match provider {
                    FocusedProvider::ClaudeCode => {
                        claude_code_menu_label(strings, language, view.claude_available())
                    }
                    FocusedProvider::Codex => strings.codex_model.to_string(),
                    FocusedProvider::Cursor => strings.cursor_model.to_string(),
                };
                let toggle = if provider == FocusedProvider::ClaudeCode && !view.claude_available()
                {
                    Toggle {
                        enabled: false,
                        ..toggle
                    }
                } else {
                    toggle
                };
                Presented::Check { text, toggle }
            }
            Self::QuotaWindow(window) => {
                let text = match window {
                    QuotaWindow::Session => tr(language, "5시간 할당량", "5-hour quota"),
                    QuotaWindow::Weekly => tr(language, "주간 할당량", "Weekly quota"),
                };
                check(text, view.quota_window(window))
            }
            Self::AlertThreshold => choice(
                AlertThreshold::ALL.map(|threshold| threshold.label(language).to_string()),
                AlertThreshold::ALL
                    .iter()
                    .position(|threshold| *threshold == view.alert_threshold),
                true,
            ),
            Self::Palette => appearance_choice(
                view,
                |appearance| appearance.palette,
                Palette::ALL,
                |item| item.label(language),
            ),
            Self::BarStyle => appearance_choice(
                view,
                |appearance| appearance.bar_style,
                BarStyle::ALL,
                |item| item.label(language),
            ),
            Self::BarThickness => appearance_choice(
                view,
                |appearance| appearance.bar_thickness,
                BarThickness::ALL,
                |item| item.label(language),
            ),
            Self::FontSize => appearance_choice(
                view,
                |appearance| appearance.font_size,
                FontSize::ALL,
                |item| item.label(language),
            ),
            Self::AppearanceRecommended => button(tr(
                language,
                "권장: 반투명 어두운 작업 표시줄",
                "Recommended: translucent dark taskbar",
            )),
            Self::AppearanceReset => {
                button(tr(language, "기본 모양으로 재설정", "Reset appearance"))
            }
            Self::Language => {
                let mut options = vec![strings.system_default.to_string()];
                options.extend(LanguageId::ALL.map(|lang| lang.native_name().to_string()));
                let selected = match view.language_override {
                    None => Some(0),
                    Some(selected) => LanguageId::ALL
                        .iter()
                        .position(|lang| *lang == selected)
                        .map(|index| index + 1)
                        .or(Some(0)),
                };
                Presented::Choice {
                    options,
                    selected,
                    enabled: true,
                }
            }
            Self::Display => Presented::Choice {
                options: view
                    .displays
                    .iter()
                    .map(|(_, label)| label.clone())
                    .collect(),
                selected: view.current_display,
                enabled: true,
            },
            Self::AnchorLeft => check(
                tr(
                    language,
                    "작업 표시줄 왼쪽에 고정",
                    "Pin to the left edge of the taskbar",
                ),
                Toggle {
                    checked: view.anchor_left,
                    enabled: true,
                },
            ),
            Self::ResetPosition => button(strings.reset_position),
            Self::WidgetVisible => check(
                strings.show_widget,
                Toggle {
                    checked: view.widget_visible,
                    enabled: true,
                },
            ),
            Self::StartWithWindows => check(
                strings.start_with_windows,
                Toggle {
                    checked: view.start_with_windows,
                    enabled: true,
                },
            ),
            Self::Close => button("Close"),
        }
    }

    fn command(self, view: &SettingsView, control: HWND) -> Option<SettingsCommand> {
        match self.kind() {
            ControlKind::Check => {
                let shown = is_checked(control);
                match self {
                    Self::AdaptiveRefresh => Some(SettingsCommand::SetAdaptiveRefresh(shown)),
                    Self::Provider(provider) => {
                        Some(SettingsCommand::SetProviderShown(provider, shown))
                    }
                    Self::QuotaWindow(window) => {
                        Some(SettingsCommand::SetQuotaWindowShown(window, shown))
                    }
                    Self::AnchorLeft => Some(SettingsCommand::SetAnchorLeft(shown)),
                    Self::WidgetVisible => Some(SettingsCommand::SetWidgetVisible(shown)),
                    Self::StartWithWindows => Some(SettingsCommand::SetStartWithWindows(shown)),
                    _ => None,
                }
            }
            ControlKind::Choice => {
                let selected = selected_index(control)?;
                match self {
                    Self::PollInterval => PollInterval::ALL
                        .get(selected)
                        .copied()
                        .map(SettingsCommand::SetPollInterval),
                    Self::AlertThreshold => AlertThreshold::ALL
                        .get(selected)
                        .copied()
                        .map(SettingsCommand::SetAlertThreshold),
                    Self::Palette => {
                        appearance_command(view, selected, Palette::ALL, |appearance, palette| {
                            appearance.palette = palette;
                        })
                    }
                    Self::BarStyle => {
                        appearance_command(view, selected, BarStyle::ALL, |appearance, style| {
                            appearance.bar_style = style;
                        })
                    }
                    Self::BarThickness => appearance_command(
                        view,
                        selected,
                        BarThickness::ALL,
                        |appearance, thickness| {
                            appearance.bar_thickness = thickness;
                        },
                    ),
                    Self::FontSize => appearance_command(
                        view,
                        selected,
                        FontSize::ALL,
                        |appearance, font_size| {
                            appearance.font_size = font_size;
                        },
                    ),
                    Self::Language => {
                        if selected == 0 {
                            Some(SettingsCommand::SetLanguage(None))
                        } else {
                            LanguageId::ALL
                                .get(selected - 1)
                                .copied()
                                .map(|lang| SettingsCommand::SetLanguage(Some(lang)))
                        }
                    }
                    Self::Display => view.displays.get(selected).map(|(choice, _)| {
                        SettingsCommand::SetDisplay(DisplayChoice {
                            index: choice.index,
                            device: choice.device.clone(),
                        })
                    }),
                    _ => None,
                }
            }
            ControlKind::Button => match self {
                Self::AppearanceRecommended => Some(SettingsCommand::SetAppearance(
                    Appearance::translucent_dark_taskbar(),
                )),
                Self::AppearanceReset => {
                    Some(SettingsCommand::SetAppearance(Appearance::default()))
                }
                Self::ResetPosition => Some(SettingsCommand::ResetPosition),
                Self::Close => None,
                _ => None,
            },
        }
    }
}

fn check(text: &str, toggle: Toggle) -> Presented {
    Presented::Check {
        text: text.to_string(),
        toggle,
    }
}

fn button(text: &str) -> Presented {
    Presented::Button {
        text: text.to_string(),
        enabled: true,
    }
}

fn choice<const N: usize>(
    options: [String; N],
    selected: Option<usize>,
    enabled: bool,
) -> Presented {
    Presented::Choice {
        options: options.into_iter().collect(),
        selected,
        enabled,
    }
}

fn appearance_choice<T, const N: usize>(
    view: &SettingsView,
    current: impl Fn(Appearance) -> T,
    all: [T; N],
    label: impl Fn(T) -> &'static str,
) -> Presented
where
    T: Copy + PartialEq,
{
    let selected = current(view.appearance);
    choice(
        all.map(|item| label(item).to_string()),
        all.iter().position(|item| *item == selected),
        true,
    )
}

fn appearance_command<T, const N: usize>(
    view: &SettingsView,
    selected: usize,
    all: [T; N],
    write: impl FnOnce(&mut Appearance, T),
) -> Option<SettingsCommand>
where
    T: Copy,
{
    let value = *all.get(selected)?;
    let mut appearance = view.appearance;
    write(&mut appearance, value);
    Some(SettingsCommand::SetAppearance(appearance))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::MonitorFromWindow;
    use windows::Win32::UI::Controls::BST_CHECKED;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetActiveWindow, IsWindowEnabled, SetActiveWindow, SetFocus, VK_ESCAPE, VK_RETURN,
    };

    use super::super::settings::{PollInterval, SettingsCommand, SettingsView};
    use super::super::{FocusedProvider, CURRENT_DPI};
    use super::*;
    use crate::native_interop;

    #[test]
    fn control_ids_start_at_100_and_round_trip() {
        let mut seen = [false; FIELD_COUNT];
        for field in Field::ALL {
            let id = field.control_id();
            assert!(id >= CONTROL_ID_BASE);
            assert_eq!(Field::from_control_id(id), Some(field));
            assert!(!seen[field.index()]);
            seen[field.index()] = true;
        }
        assert!(Field::from_control_id(1).is_none());
        assert!(Field::from_control_id(2).is_none());
        assert_eq!(px(96, 12), 12);
        assert_eq!(px(192, 12), 24);
    }

    #[test]
    fn owned_dialog_applies_control_edits_and_reopens_the_same_window() {
        unsafe {
            let dpi_before = CURRENT_DPI.load(Ordering::Relaxed);
            let host = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("settings-host"),
                WS_POPUP | WS_VISIBLE,
                40,
                40,
                480,
                48,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let widget = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("settings-widget"),
                WS_POPUP,
                0,
                0,
                40,
                20,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            native_interop::embed_in_taskbar(widget, host);
            let _close = CloseWindows { host };

            open(widget);
            let dialog = DIALOG.with(|cell| cell.get());
            assert!(IsWindow(dialog).as_bool());
            open(widget);
            assert_eq!(DIALOG.with(|cell| cell.get()), dialog);

            let style = GetWindowLongW(dialog, GWL_STYLE) as u32;
            let ex_style = GetWindowLongW(dialog, GWL_EXSTYLE) as u32;
            assert_eq!(style & WS_CHILD.0, 0);
            assert_eq!(
                style & (WS_POPUP.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_CLIPCHILDREN.0),
                WS_POPUP.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_CLIPCHILDREN.0
            );
            assert_eq!(ex_style & WS_EX_CONTROLPARENT.0, WS_EX_CONTROLPARENT.0);
            assert_eq!(ex_style & WS_EX_DLGMODALFRAME.0, WS_EX_DLGMODALFRAME.0);
            assert_eq!(
                ex_style & (WS_EX_LAYERED.0 | WS_EX_NOACTIVATE.0 | WS_EX_APPWINDOW.0),
                0
            );
            let owner = GetWindow(dialog, GW_OWNER).unwrap_or_default();
            assert!(
                owner == widget || owner == host,
                "owner={owner:?} widget={widget:?} host={host:?}"
            );
            assert!(!MonitorFromWindow(dialog, MONITOR_DEFAULTTONEAREST).is_invalid());

            let _ = SetActiveWindow(dialog);
            let _ = SetFocus(dialog);
            assert_eq!(GetActiveWindow(), dialog);

            for field in Field::ALL {
                let control = GetDlgItem(dialog, field.control_id() as i32).unwrap();
                assert!(GetDlgCtrlID(control) >= CONTROL_ID_BASE as i32);
            }
            let view = SettingsView::read();
            let display = GetDlgItem(dialog, Field::Display.control_id() as i32).unwrap();
            assert_eq!(IsWindowVisible(display).as_bool(), view.displays.len() > 1);

            let claude = GetDlgItem(
                dialog,
                Field::Provider(FocusedProvider::ClaudeCode).control_id() as i32,
            )
            .unwrap();
            let codex = GetDlgItem(
                dialog,
                Field::Provider(FocusedProvider::Codex).control_id() as i32,
            )
            .unwrap();
            if !view.claude_available() {
                assert!(!IsWindowEnabled(claude).as_bool());
            }
            assert!(IsWindowEnabled(codex).as_bool());

            let poll = GetDlgItem(dialog, Field::PollInterval.control_id() as i32).unwrap();
            SendMessageW(poll, CB_SETCURSEL, WPARAM(0), LPARAM(0));
            let polled = Field::PollInterval.command(&view, poll);
            assert_eq!(
                polled,
                Some(SettingsCommand::SetPollInterval(PollInterval::OneMinute))
            );
            notify(dialog, Field::PollInterval, poll, CBN_SELCHANGE);
            let restored = PollInterval::ALL
                .iter()
                .position(|interval| *interval == SettingsView::read().poll_interval)
                .unwrap();
            assert_eq!(
                SendMessageW(poll, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0,
                restored as isize
            );

            let adaptive = GetDlgItem(dialog, Field::AdaptiveRefresh.control_id() as i32).unwrap();
            SendMessageW(adaptive, BM_SETCHECK, WPARAM(0), LPARAM(0));
            assert_eq!(
                Field::AdaptiveRefresh.command(&SettingsView::read(), adaptive),
                Some(SettingsCommand::SetAdaptiveRefresh(false))
            );
            notify(dialog, Field::AdaptiveRefresh, adaptive, BN_CLICKED);
            assert_eq!(
                SendMessageW(adaptive, BM_GETCHECK, WPARAM(0), LPARAM(0)).0,
                if SettingsView::read().adaptive_refresh {
                    BST_CHECKED.0 as isize
                } else {
                    0
                }
            );

            let dpi = GetDpiForWindow(dialog).max(96);
            let mut client = RECT::default();
            GetClientRect(dialog, &mut client).unwrap();
            assert_eq!(client.right, px(dpi, PAD) * 2 + px(dpi, CLIENT_WIDTH));
            let mut window_rect = RECT::default();
            GetWindowRect(dialog, &mut window_rect).unwrap();
            let monitor = MonitorFromWindow(dialog, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            assert!(GetMonitorInfoW(monitor, &mut info).as_bool());
            let dialog_height = window_rect.bottom - window_rect.top;
            let work_height = info.rcWork.bottom - info.rcWork.top;
            assert!(
                dialog_height <= work_height,
                "dialog height {dialog_height} exceeds work area {work_height}"
            );
            assert_eq!(CURRENT_DPI.load(Ordering::Relaxed), dpi_before);

            let def_id = SendMessageW(dialog, DM_GETDEFID, WPARAM(0), LPARAM(0)).0 as u32;
            assert_eq!((def_id >> 16) as u16, DC_HASDEFID as u16);
            assert_eq!(def_id as u16, Field::Close.control_id());

            assert!(pre_translate(&key_message(dialog, VK_RETURN.0)));
            assert!(!IsWindow(dialog).as_bool());

            open(widget);
            let reopened = DIALOG.with(|cell| cell.get());
            assert!(IsWindow(reopened).as_bool());
            assert_ne!(reopened, dialog);
            let adaptive =
                GetDlgItem(reopened, Field::AdaptiveRefresh.control_id() as i32).unwrap();
            let checked = SendMessageW(adaptive, BM_GETCHECK, WPARAM(0), LPARAM(0)).0
                == BST_CHECKED.0 as isize;
            assert_eq!(checked, SettingsView::read().adaptive_refresh);
            assert!(pre_translate(&key_message(reopened, VK_ESCAPE.0)));
            assert!(!IsWindow(reopened).as_bool());
        }
    }

    fn notify(dialog: HWND, field: Field, control: HWND, code: u32) {
        let wparam = WPARAM(field.control_id() as usize | ((code as usize) << 16));
        unsafe {
            SendMessageW(dialog, WM_COMMAND, wparam, LPARAM(control.0 as isize));
        }
    }

    fn key_message(dialog: HWND, key: u16) -> MSG {
        MSG {
            hwnd: dialog,
            message: WM_KEYDOWN,
            wParam: WPARAM(key as usize),
            lParam: LPARAM(0),
            time: 0,
            pt: POINT::default(),
        }
    }

    struct CloseWindows {
        host: HWND,
    }

    impl Drop for CloseWindows {
        fn drop(&mut self) {
            unsafe {
                let dialog = DIALOG.with(|cell| cell.get());
                if IsWindow(dialog).as_bool() {
                    let _ = DestroyWindow(dialog);
                }
                if IsWindow(self.host).as_bool() {
                    let _ = DestroyWindow(self.host);
                }
            }
        }
    }
}
