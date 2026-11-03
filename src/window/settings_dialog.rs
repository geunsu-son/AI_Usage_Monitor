use std::cell::Cell;
use std::sync::Once;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, GetMonitorInfoW, GetSysColorBrush, MonitorFromWindow,
    COLOR_BTNFACE, HFONT, LOGFONTW, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, BST_CHECKED, ICC_STANDARD_CLASSES, ICC_TAB_CLASSES, ICC_WIN95_CLASSES,
    INITCOMMONCONTROLSEX, NMHDR, TCIF_TEXT, TCITEMW, TCM_GETCURSEL, TCM_GETITEMCOUNT,
    TCM_INSERTITEMW, TCM_SETCURSEL, TCM_SETITEMW, TCN_SELCHANGE, WC_TABCONTROL,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::localization::LanguageId;
use crate::native_interop;

use super::settings::{
    AlertThreshold, DisplayChoice, PollInterval, SettingsCommand, SettingsView, Toggle, UsageRow,
};
use super::{
    claude_code_menu_label, load_embedded_app_icons, tr, Appearance, BarStyle, BarThickness,
    FocusedProvider, FontSize, Palette, WM_DPICHANGED_MSG,
};

const CLASS_NAME: &str = "AIUsageMonitorSettings";
const PROVIDER_CLASS_NAME: &str = "AIUsageMonitorProviderSettings";
const CONTROL_ID_BASE: u16 = 100;
const TAB_CONTROL_ID: u16 = 90;
const FIELD_COUNT: usize = 23;
const CLIENT_WIDTH: i32 = 460;
const PAD: i32 = 12;
const COMBO_DROP_HEIGHT: i32 = 200;
/// windows 0.58 has no name for this message. It sizes the drop list without
/// stretching the combo over the rows below it.
const CB_SETMINVISIBLE: u32 = 0x1701;

thread_local! {
    static DIALOG: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
    static PROVIDER_DIALOG: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
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

    ensure_common_controls();
    ensure_class();
    let instance = match unsafe { GetModuleHandleW(PCWSTR::null()) } {
        Ok(module) => HINSTANCE(module.0),
        Err(_) => return,
    };
    let state = Box::new(DialogState {
        widget,
        dpi: 96,
        font: HFONT::default(),
        tab: HWND::default(),
        active_tab: Tab::Models,
        controls: vec![HWND::default(); FIELD_COUNT],
        labels: vec![HWND::default(); FIELD_COUNT],
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
    let provider = PROVIDER_DIALOG.with(|cell| cell.get());
    if !provider.is_invalid() && unsafe { IsWindow(provider).as_bool() } {
        if unsafe { IsDialogMessageW(provider, msg).as_bool() } {
            return true;
        }
    }
    let hwnd = DIALOG.with(|cell| cell.get());
    if hwnd.is_invalid() || !unsafe { IsWindow(hwnd).as_bool() } {
        return false;
    }
    unsafe { IsDialogMessageW(hwnd, msg).as_bool() }
}

pub(super) fn sync() {
    let hwnd = DIALOG.with(|cell| cell.get());
    if !hwnd.is_invalid() && unsafe { IsWindow(hwnd).as_bool() } {
        sync_hwnd(hwnd);
    }
    let provider = PROVIDER_DIALOG.with(|cell| cell.get());
    if !provider.is_invalid() && unsafe { IsWindow(provider).as_bool() } {
        sync_provider_hwnd(provider);
    }
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

fn ensure_common_controls() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_TAB_CLASSES | ICC_STANDARD_CLASSES | ICC_WIN95_CLASSES,
        };
        let _ = unsafe { InitCommonControlsEx(&controls) };
    });
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
            let dpi = GetDpiForWindow(hwnd).max(96);
            state.dpi = dpi;
            state.font = message_font(dpi);
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, created.lpCreateParams as isize);
            create_children(hwnd, state, created.hInstance);
            LRESULT(0)
        }
        WM_COMMAND => on_command(hwnd, wparam, lparam),
        WM_NOTIFY => on_notify(hwnd, lparam),
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
            close_provider_dialog();
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

fn on_notify(hwnd: HWND, lparam: LPARAM) -> LRESULT {
    if lparam.0 == 0 {
        return LRESULT(0);
    }
    let header = unsafe { &*(lparam.0 as *const NMHDR) };
    if header.idFrom == TAB_CONTROL_ID as usize && header.code == TCN_SELCHANGE {
        let Some(state) = state_mut(hwnd) else {
            return LRESULT(0);
        };
        let selected = unsafe { SendMessageW(state.tab, TCM_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
        if selected >= 0 {
            if let Some(tab) = Tab::from_index(selected as usize) {
                state.active_tab = tab;
                let view = SettingsView::read();
                layout(hwnd, state, &view);
            }
        }
    }
    LRESULT(0)
}

fn on_command(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if SYNCING.with(|flag| flag.get()) {
        return LRESULT(0);
    }
    let id = (wparam.0 & 0xffff) as i32;
    let code = ((wparam.0 >> 16) & 0xffff) as u16;
    if id == IDOK.0 || id == IDCANCEL.0 {
        close_provider_dialog();
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
        close_provider_dialog();
        let _ = unsafe { DestroyWindow(hwnd) };
        return LRESULT(0);
    }
    let widget = state_mut(hwnd)
        .map(|state| state.widget)
        .unwrap_or_default();
    let control = HWND(lparam.0 as *mut _);
    let view = SettingsView::read();
    match field {
        Field::SettingsClaude => open_provider_dialog(hwnd, widget, FocusedProvider::ClaudeCode),
        Field::SettingsCodex => open_provider_dialog(hwnd, widget, FocusedProvider::Codex),
        Field::SettingsCursor => open_provider_dialog(hwnd, widget, FocusedProvider::Cursor),
        _ => match field.command(&view, control) {
            Some(command) => super::settings::apply(widget, command),
            None => sync_hwnd(hwnd),
        },
    }
    LRESULT(0)
}

struct DialogState {
    widget: HWND,
    dpi: u32,
    font: HFONT,
    tab: HWND,
    active_tab: Tab,
    controls: Vec<HWND>,
    labels: Vec<HWND>,
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
    state.tab = create_child(
        parent,
        WC_TABCONTROL,
        "",
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(WS_CLIPSIBLINGS.0),
        TAB_CONTROL_ID,
        instance,
    );
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
        let create_h = if field.kind() == ControlKind::Choice {
            px(state.dpi, COMBO_DROP_HEIGHT)
        } else {
            10
        };
        state.controls[index] = create_child_sized(
            parent,
            class,
            "",
            style,
            field.control_id(),
            instance,
            10,
            create_h,
        );
        state.labels[index] =
            create_child(parent, w!("STATIC"), "", WS_CHILD | WS_VISIBLE, 0, instance);
        if field.kind() == ControlKind::Choice {
            set_combo_min_visible(state.controls[index]);
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
    create_child_sized(parent, class, text, style, id, instance, 10, 10)
}

fn create_child_sized(
    parent: HWND,
    class: PCWSTR,
    text: &str,
    style: WINDOW_STYLE,
    id: u16,
    instance: HINSTANCE,
    width: i32,
    height: i32,
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
            width,
            height,
            parent,
            HMENU(id as usize as *mut _),
            instance,
            None,
        )
        .unwrap_or_default()
    }
}

fn set_combo_min_visible(control: HWND) {
    if control.is_invalid() {
        return;
    }
    unsafe {
        SendMessageW(control, CB_SETMINVISIBLE, WPARAM(8), LPARAM(0));
    }
}

fn apply_view(hwnd: HWND, state: &mut DialogState, view: &SettingsView) {
    set_text(hwnd, &view.strings().settings);
    fill_tabs(state, view);
    let strings = view.strings();
    let language = view.language;
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
                    set_combo_min_visible(control);
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
        // Model rows use the provider field's static label for the name column.
        if let Some(provider) = field.provider() {
            if matches!(
                field,
                Field::ProviderClaude | Field::ProviderCodex | Field::ProviderCursor
            ) {
                let name = match provider {
                    FocusedProvider::ClaudeCode => {
                        claude_code_menu_label(strings, language, view.claude_available())
                    }
                    FocusedProvider::Codex => strings.codex_model.to_string(),
                    FocusedProvider::Cursor => strings.cursor_model.to_string(),
                };
                set_text(state.labels[index], &name);
            }
        }
    }
    layout(hwnd, state, view);
}

fn fill_tabs(state: &mut DialogState, view: &SettingsView) {
    if state.tab.is_invalid() {
        return;
    }
    let count = unsafe { SendMessageW(state.tab, TCM_GETITEMCOUNT, WPARAM(0), LPARAM(0)).0 };
    let insert = count <= 0;
    for (index, tab) in Tab::ALL.iter().enumerate() {
        let title = tab.title(view);
        let mut wide = native_interop::wide_str(&title);
        let mut item = TCITEMW {
            mask: TCIF_TEXT,
            pszText: windows::core::PWSTR(wide.as_mut_ptr()),
            ..Default::default()
        };
        unsafe {
            if insert {
                SendMessageW(
                    state.tab,
                    TCM_INSERTITEMW,
                    WPARAM(index),
                    LPARAM(&mut item as *mut _ as isize),
                );
            } else {
                SendMessageW(
                    state.tab,
                    TCM_SETITEMW,
                    WPARAM(index),
                    LPARAM(&mut item as *mut _ as isize),
                );
            }
        }
    }
    unsafe {
        SendMessageW(
            state.tab,
            TCM_SETCURSEL,
            WPARAM(state.active_tab.index()),
            LPARAM(0),
        );
    }
}

fn layout(hwnd: HWND, state: &mut DialogState, view: &SettingsView) {
    let dpi = state.dpi;
    let pad = px(dpi, PAD);
    let width = px(dpi, CLIENT_WIDTH);
    // Keep the tab strip to header height only. A full-page SysTabControl32
    // paints over sibling controls after any click/sync, which looks blank.
    let tab_h = px(dpi, 30);
    place_z(state.tab, HWND_BOTTOM, pad, pad, width, tab_h, true);
    let content_left = pad;
    let content_width = width;
    let mut y = pad + tab_h + pad;

    // Hide every field first, then show the active tab (+ Close).
    // Do not resize while hiding: shrinking a combo to 0x0 (or to the closed
    // edit height) permanently collapses its drop-down list.
    for field in Field::ALL {
        let index = field.index();
        hide_control(state.controls[index]);
        hide_control(state.labels[index]);
    }

    if state.active_tab == Tab::Models {
        y = layout_model_rows(state, view, dpi, content_left, content_width, y);
    } else {
        for field in Field::ALL {
            if field.tab() != Some(state.active_tab) || !field.is_visible(view) {
                continue;
            }
            if matches!(
                field,
                Field::ProviderClaude
                    | Field::ProviderCodex
                    | Field::ProviderCursor
                    | Field::SettingsClaude
                    | Field::SettingsCodex
                    | Field::SettingsCursor
            ) {
                continue;
            }
            y = layout_field_row(state, view, field, dpi, content_left, content_width, y);
        }
    }

    y += pad;
    let close = Field::Close;
    place_z(
        state.controls[close.index()],
        HWND_TOP,
        content_left,
        y,
        content_width,
        px(dpi, 28),
        true,
    );
    y += px(dpi, 28) + pad;
    size_dialog(hwnd, state, pad * 2 + width, y + pad);
}

fn layout_model_rows(
    state: &DialogState,
    view: &SettingsView,
    dpi: u32,
    left: i32,
    width: i32,
    mut y: i32,
) -> i32 {
    let rows = [
        (
            Field::ProviderClaude,
            Field::SettingsClaude,
            FocusedProvider::ClaudeCode,
        ),
        (
            Field::ProviderCodex,
            Field::SettingsCodex,
            FocusedProvider::Codex,
        ),
        (
            Field::ProviderCursor,
            Field::SettingsCursor,
            FocusedProvider::Cursor,
        ),
    ];
    let name_w = px(dpi, 170);
    let check_w = px(dpi, 72);
    let gap = px(dpi, 8);
    let row_h = px(dpi, 28);
    let settings_w = (width - name_w - check_w - gap * 2).max(px(dpi, 80));
    for (provider_field, settings_field, provider) in rows {
        let name = state.labels[provider_field.index()];
        let check = state.controls[provider_field.index()];
        let settings = state.controls[settings_field.index()];
        place_z(name, HWND_TOP, left, y + px(dpi, 5), name_w, px(dpi, 18), true);
        place_z(
            check,
            HWND_TOP,
            left + name_w + gap,
            y,
            check_w,
            row_h,
            true,
        );
        let settings_enabled = provider != FocusedProvider::ClaudeCode || view.claude_available();
        let _ = unsafe { EnableWindow(settings, settings_enabled) };
        place_z(
            settings,
            HWND_TOP,
            left + name_w + gap + check_w + gap,
            y,
            settings_w,
            row_h,
            true,
        );
        y += row_h + px(dpi, 8);
    }
    y
}

fn layout_field_row(
    state: &DialogState,
    view: &SettingsView,
    field: Field,
    dpi: u32,
    left: i32,
    width: i32,
    y: i32,
) -> i32 {
    let index = field.index();
    let control = state.controls[index];
    let label = state.labels[index];
    match field.kind() {
        ControlKind::Check => {
            place_z(control, HWND_TOP, left, y, width, px(dpi, 22), true);
            place_z(label, HWND_TOP, 0, 0, 0, 0, false);
            y + px(dpi, 26)
        }
        ControlKind::Choice => {
            // Closed edit looks ~24px tall, but the HWND height must include the
            // drop-down list (CreateWindow height for CBS_DROPDOWNLIST).
            let drop_h = px(dpi, COMBO_DROP_HEIGHT);
            if field.side_label(view).is_some() {
                let label_w = px(dpi, 150);
                place_z(
                    label,
                    HWND_TOP,
                    left,
                    y + px(dpi, 3),
                    label_w,
                    px(dpi, 18),
                    true,
                );
                place_z(
                    control,
                    HWND_TOP,
                    left + label_w,
                    y,
                    width - label_w,
                    drop_h,
                    true,
                );
            } else {
                hide_control(label);
                place_z(control, HWND_TOP, left, y, width, drop_h, true);
            }
            set_combo_min_visible(control);
            y + px(dpi, 30)
        }
        ControlKind::Button => {
            place_z(control, HWND_TOP, left, y, width, px(dpi, 28), true);
            place_z(label, HWND_TOP, 0, 0, 0, 0, false);
            y + px(dpi, 34)
        }
    }
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
    place_z(hwnd, HWND::default(), x, y, width, height, show);
}

fn hide_control(hwnd: HWND) {
    if hwnd.is_invalid() {
        return;
    }
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

fn place_z(hwnd: HWND, insert_after: HWND, x: i32, y: i32, width: i32, height: i32, show: bool) {
    if hwnd.is_invalid() {
        return;
    }
    let flags = if insert_after == HWND::default() {
        SWP_NOZORDER | SWP_NOACTIVATE
    } else {
        SWP_NOACTIVATE
    };
    unsafe {
        let _ = SetWindowPos(hwnd, insert_after, x, y, width, height, flags);
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
        if !state.tab.is_invalid() {
            SendMessageW(state.tab, WM_SETFONT, font, LPARAM(1));
        }
        for hwnd in state.controls.iter().chain(state.labels.iter()) {
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
        set_combo_min_visible(control);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Models = 0,
    Appearance = 1,
    Refresh = 2,
    Alerts = 3,
    General = 4,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Self::Models,
        Self::Appearance,
        Self::Refresh,
        Self::Alerts,
        Self::General,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    fn title(self, view: &SettingsView) -> String {
        let language = view.language;
        let strings = view.strings();
        match self {
            Self::Models => strings.models.to_string(),
            Self::Appearance => tr(language, "모양", "Appearance").to_string(),
            Self::Refresh => strings.update_frequency.to_string(),
            Self::Alerts => tr(language, "알림", "Alerts").to_string(),
            Self::General => tr(language, "일반", "General").to_string(),
        }
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
    ProviderClaude,
    SettingsClaude,
    ProviderCodex,
    SettingsCodex,
    ProviderCursor,
    SettingsCursor,
    Palette,
    BarStyle,
    BarThickness,
    FontSize,
    AppearanceRecommended,
    AppearanceReset,
    PollInterval,
    AdaptiveRefresh,
    PreciseReset,
    AlertThreshold,
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
        Self::ProviderClaude,
        Self::SettingsClaude,
        Self::ProviderCodex,
        Self::SettingsCodex,
        Self::ProviderCursor,
        Self::SettingsCursor,
        Self::Palette,
        Self::BarStyle,
        Self::BarThickness,
        Self::FontSize,
        Self::AppearanceRecommended,
        Self::AppearanceReset,
        Self::PollInterval,
        Self::AdaptiveRefresh,
        Self::PreciseReset,
        Self::AlertThreshold,
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
            Self::ProviderClaude
            | Self::ProviderCodex
            | Self::ProviderCursor
            | Self::AdaptiveRefresh
            | Self::PreciseReset
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
            Self::SettingsClaude
            | Self::SettingsCodex
            | Self::SettingsCursor
            | Self::AppearanceRecommended
            | Self::AppearanceReset
            | Self::ResetPosition
            | Self::Close => ControlKind::Button,
        }
    }

    fn tab(self) -> Option<Tab> {
        match self {
            Self::ProviderClaude
            | Self::SettingsClaude
            | Self::ProviderCodex
            | Self::SettingsCodex
            | Self::ProviderCursor
            | Self::SettingsCursor => Some(Tab::Models),
            Self::Palette
            | Self::BarStyle
            | Self::BarThickness
            | Self::FontSize
            | Self::AppearanceRecommended
            | Self::AppearanceReset => Some(Tab::Appearance),
            Self::PollInterval | Self::AdaptiveRefresh | Self::PreciseReset => Some(Tab::Refresh),
            Self::AlertThreshold => Some(Tab::Alerts),
            Self::Language
            | Self::Display
            | Self::AnchorLeft
            | Self::ResetPosition
            | Self::WidgetVisible
            | Self::StartWithWindows => Some(Tab::General),
            Self::Close => None,
        }
    }

    fn provider(self) -> Option<FocusedProvider> {
        match self {
            Self::ProviderClaude | Self::SettingsClaude => Some(FocusedProvider::ClaudeCode),
            Self::ProviderCodex | Self::SettingsCodex => Some(FocusedProvider::Codex),
            Self::ProviderCursor | Self::SettingsCursor => Some(FocusedProvider::Cursor),
            _ => None,
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
            Self::PollInterval => tr(language, "업데이트 주기", "Update frequency"),
            Self::AlertThreshold => tr(language, "할당량 알림", "Quota alerts"),
            Self::Display => tr(language, "디스플레이", "Display"),
            Self::Language => view.strings().language,
            _ => return None,
        };
        Some(label.to_string())
    }

    fn present(self, view: &SettingsView) -> Presented {
        let strings = view.strings();
        let language = view.language;
        match self {
            Self::ProviderClaude | Self::ProviderCodex | Self::ProviderCursor => {
                let provider = self.provider().unwrap();
                let toggle = view.provider(provider);
                let enabled = if provider == FocusedProvider::ClaudeCode {
                    view.claude_available() && toggle.enabled
                } else {
                    toggle.enabled
                };
                Presented::Check {
                    text: tr(language, "표시", "Show").to_string(),
                    toggle: Toggle {
                        checked: toggle.checked,
                        enabled,
                    },
                }
            }
            Self::SettingsClaude | Self::SettingsCodex | Self::SettingsCursor => Presented::Button {
                text: tr(language, "설정", "Settings").to_string(),
                enabled: true,
            },
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
            Self::PreciseReset => check(
                tr(language, "정확한 갱신 시각/기간", "Precise reset time"),
                Toggle {
                    checked: view.precise_reset,
                    enabled: true,
                },
            ),
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
                    Self::ProviderClaude | Self::ProviderCodex | Self::ProviderCursor => self
                        .provider()
                        .map(|provider| SettingsCommand::SetProviderShown(provider, shown)),
                    Self::AdaptiveRefresh => Some(SettingsCommand::SetAdaptiveRefresh(shown)),
                    Self::PreciseReset => Some(SettingsCommand::SetPreciseReset(shown)),
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

fn choice(
    options: impl IntoIterator<Item = String>,
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

// --- Provider detail dialog -------------------------------------------------

const PROVIDER_ID_BASE: u16 = 300;
const PROVIDER_CLOSE_ID: u16 = 399;

struct ProviderDialogState {
    widget: HWND,
    owner: HWND,
    provider: FocusedProvider,
    dpi: u32,
    font: HFONT,
    controls: Vec<(u16, HWND)>,
}

fn close_provider_dialog() {
    let hwnd = PROVIDER_DIALOG.with(|cell| cell.get());
    if !hwnd.is_invalid() && unsafe { IsWindow(hwnd).as_bool() } {
        let _ = unsafe { DestroyWindow(hwnd) };
    }
}

fn open_provider_dialog(owner: HWND, widget: HWND, provider: FocusedProvider) {
    let existing = PROVIDER_DIALOG.with(|cell| cell.get());
    if unsafe { IsWindow(existing).as_bool() } {
        let same = provider_state_mut(existing).is_some_and(|s| s.provider == provider);
        if same {
            unsafe {
                let _ = ShowWindow(existing, SW_RESTORE);
                let _ = SetForegroundWindow(existing);
            }
            sync_provider_hwnd(existing);
            return;
        }
        let _ = unsafe { DestroyWindow(existing) };
    }

    ensure_provider_class();
    let instance = match unsafe { GetModuleHandleW(PCWSTR::null()) } {
        Ok(module) => HINSTANCE(module.0),
        Err(_) => return,
    };
    let dpi = unsafe { GetDpiForWindow(owner) }.max(96);
    let state = Box::new(ProviderDialogState {
        widget,
        owner,
        provider,
        dpi,
        font: message_font(dpi),
        controls: Vec::new(),
    });
    let raw = Box::into_raw(state);
    let title = provider_dialog_title(provider);
    let class_name = native_interop::wide_str(PROVIDER_CLASS_NAME);
    let title_wide = native_interop::wide_str(&title);
    let created = unsafe {
        CreateWindowExW(
            dialog_ex_style(),
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            dialog_style(),
            0,
            0,
            100,
            100,
            owner,
            None,
            instance,
            Some(raw as *const _),
        )
    };
    let hwnd = match created {
        Ok(hwnd) => hwnd,
        Err(error) => {
            drop(unsafe { Box::from_raw(raw) });
            crate::diagnose::log(format!("provider settings dialog was not created: {error}"));
            return;
        }
    };
    PROVIDER_DIALOG.with(|cell| cell.set(hwnd));
    sync_provider_hwnd(hwnd);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

fn provider_dialog_title(provider: FocusedProvider) -> String {
    let view = SettingsView::read();
    let strings = view.strings();
    match provider {
        FocusedProvider::ClaudeCode => {
            claude_code_menu_label(strings, view.language, view.claude_available())
        }
        FocusedProvider::Codex => strings.codex_model.to_string(),
        FocusedProvider::Cursor => strings.cursor_model.to_string(),
    }
}

fn ensure_provider_class() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let class_name = native_interop::wide_str(PROVIDER_CLASS_NAME);
        let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }.unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(provider_proc),
            hInstance: HINSTANCE(instance.0),
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
            hbrBackground: unsafe { GetSysColorBrush(COLOR_BTNFACE) },
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&class) } == 0 {
            crate::diagnose::log("provider settings dialog class was not registered");
        }
    });
}

unsafe extern "system" fn provider_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let created = &*(lparam.0 as *const CREATESTRUCTW);
            let state = &mut *(created.lpCreateParams as *mut ProviderDialogState);
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, created.lpCreateParams as isize);
            create_provider_children(hwnd, state, created.hInstance);
            LRESULT(0)
        }
        WM_COMMAND => on_provider_command(hwnd, wparam, lparam),
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            if PROVIDER_DIALOG.with(|cell| cell.get()) == hwnd {
                PROVIDER_DIALOG.with(|cell| cell.set(HWND::default()));
            }
            // Repaint the parent settings page; the tab strip must stay behind
            // the model rows after the owned popup closes.
            sync();
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let raw = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut ProviderDialogState;
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

fn provider_state_mut(hwnd: HWND) -> Option<&'static mut ProviderDialogState> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut ProviderDialogState;
    if raw.is_null() {
        None
    } else {
        Some(unsafe { &mut *raw })
    }
}

fn provider_row_ids(provider: FocusedProvider) -> Vec<ProviderControl> {
    let mut controls = Vec::new();
    match provider {
        FocusedProvider::ClaudeCode => {}
        FocusedProvider::Codex => {
            controls.push(ProviderControl::Usage(UsageRow::CodexSession));
            controls.push(ProviderControl::Usage(UsageRow::CodexWeekly));
        }
        FocusedProvider::Cursor => {
            controls.push(ProviderControl::Usage(UsageRow::CursorAuto));
            controls.push(ProviderControl::Usage(UsageRow::CursorApi));
        }
    }
    controls.push(ProviderControl::Close);
    controls
}

#[derive(Clone, Copy)]
enum ProviderControl {
    Usage(UsageRow),
    Close,
}

impl ProviderControl {
    fn id(self) -> u16 {
        match self {
            Self::Usage(row) => PROVIDER_ID_BASE + 1 + row as u16,
            Self::Close => PROVIDER_CLOSE_ID,
        }
    }
}

fn create_provider_children(parent: HWND, state: &mut ProviderDialogState, instance: HINSTANCE) {
    for control in provider_row_ids(state.provider) {
        let style = if matches!(control, ProviderControl::Close) {
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32)
        } else {
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32)
        };
        let hwnd = create_child(parent, w!("BUTTON"), "", style, control.id(), instance);
        if !state.font.is_invalid() {
            unsafe {
                SendMessageW(
                    hwnd,
                    WM_SETFONT,
                    WPARAM(state.font.0 as usize),
                    LPARAM(1),
                );
            }
        }
        state.controls.push((control.id(), hwnd));
    }
}

fn sync_provider_hwnd(hwnd: HWND) {
    let Some(_guard) = SyncGuard::enter() else {
        return;
    };
    let view = SettingsView::read();
    let Some(state) = provider_state_mut(hwnd) else {
        return;
    };
    set_text(hwnd, &provider_dialog_title(state.provider));
    let strings = view.strings();
    let dpi = state.dpi;
    let pad = px(dpi, PAD);
    let width = px(dpi, 360);
    let mut y = pad;
    let rows = provider_row_ids(state.provider);
    if rows.len() == 1 {
        // Claude: display toggle lives on the Models tab.
        let note = create_or_find_provider_note(hwnd, state);
        set_text(
            note,
            tr(
                view.language,
                "표시 여부는 모델 탭에서 변경합니다.",
                "Use the Models tab to show or hide this provider.",
            ),
        );
        place(note, pad, y, width, px(dpi, 40), true);
        y += px(dpi, 48);
    }
    for control in rows {
        let Some((_, ctrl)) = state
            .controls
            .iter()
            .find(|(id, _)| *id == control.id())
            .copied()
        else {
            continue;
        };
        match control {
            ProviderControl::Usage(row) => {
                let toggle = view.usage_row(row);
                let label = match row {
                    UsageRow::CodexSession | UsageRow::CodexWeekly => {
                        let full = row.label(strings);
                        full.split_once(' ')
                            .map(|(_, kind)| kind.to_string())
                            .unwrap_or(full)
                    }
                    UsageRow::CursorAuto => strings.cursor_auto_model.to_string(),
                    UsageRow::CursorApi => strings.cursor_api_model.to_string(),
                };
                set_text(ctrl, &label);
                set_checked(ctrl, toggle.checked);
                let _ = unsafe { EnableWindow(ctrl, toggle.enabled) };
                place(ctrl, pad, y, width, px(dpi, 22), true);
                y += px(dpi, 26);
            }
            ProviderControl::Close => {
                set_text(ctrl, "Close");
                place(ctrl, pad, y + pad / 2, width, px(dpi, 28), true);
                y += px(dpi, 28) + pad;
            }
        }
    }
    y += pad;
    size_provider_dialog(hwnd, state, pad * 2 + width, y);
}

fn create_or_find_provider_note(parent: HWND, state: &mut ProviderDialogState) -> HWND {
    const NOTE_ID: u16 = PROVIDER_ID_BASE + 50;
    if let Some((_, hwnd)) = state.controls.iter().find(|(id, _)| *id == NOTE_ID) {
        return *hwnd;
    }
    let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }
        .map(|module| HINSTANCE(module.0))
        .unwrap_or_default();
    let hwnd = create_child(parent, w!("STATIC"), "", WS_CHILD | WS_VISIBLE, NOTE_ID, instance);
    if !state.font.is_invalid() {
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETFONT,
                WPARAM(state.font.0 as usize),
                LPARAM(1),
            );
        }
    }
    state.controls.push((NOTE_ID, hwnd));
    hwnd
}

fn size_provider_dialog(hwnd: HWND, state: &ProviderDialogState, client_w: i32, client_h: i32) {
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
    let (x, y) = center_on_widget(state.owner, width, height);
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND_TOP,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

fn on_provider_command(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if SYNCING.with(|flag| flag.get()) {
        return LRESULT(0);
    }
    let id = (wparam.0 & 0xffff) as u16;
    let code = ((wparam.0 >> 16) & 0xffff) as u16;
    if code != BN_CLICKED as u16 {
        return LRESULT(0);
    }
    if id == PROVIDER_CLOSE_ID || id == IDCANCEL.0 as u16 || id == IDOK.0 as u16 {
        let _ = unsafe { DestroyWindow(hwnd) };
        return LRESULT(0);
    }
    let Some(state) = provider_state_mut(hwnd) else {
        return LRESULT(0);
    };
    let widget = state.widget;
    let control = HWND(lparam.0 as *mut _);
    let shown = is_checked(control);
    let command = UsageRow::ALL
        .into_iter()
        .find(|row| PROVIDER_ID_BASE + 1 + *row as u16 == id)
        .map(|row| SettingsCommand::SetUsageRowShown(row, shown));
    if let Some(command) = command {
        super::settings::apply(widget, command);
        sync();
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::MonitorFromWindow;
    use windows::Win32::UI::Controls::BST_CHECKED;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetActiveWindow, SetActiveWindow, SetFocus, VK_ESCAPE, VK_RETURN,
    };

    use super::super::settings::{AlertThreshold, PollInterval, SettingsCommand, SettingsView};
    use super::super::CURRENT_DPI;
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
            let owner = GetWindow(dialog, GW_OWNER).unwrap_or_default();
            assert!(
                owner == widget || owner == host,
                "owner={owner:?} widget={widget:?} host={host:?}"
            );

            let _ = SetActiveWindow(dialog);
            let _ = SetFocus(dialog);
            assert_eq!(GetActiveWindow(), dialog);

            for field in Field::ALL {
                let control = GetDlgItem(dialog, field.control_id() as i32).unwrap();
                assert!(GetDlgCtrlID(control) >= CONTROL_ID_BASE as i32);
            }
            let tab = GetDlgItem(dialog, TAB_CONTROL_ID as i32).unwrap();
            assert!(IsWindow(tab).as_bool());

            // Switch to Refresh tab so poll/adaptive are laid out and visible.
            SendMessageW(tab, TCM_SETCURSEL, WPARAM(Tab::Refresh.index()), LPARAM(0));
            if let Some(state) = state_mut(dialog) {
                state.active_tab = Tab::Refresh;
                layout(dialog, state, &SettingsView::read());
            }

            let poll = GetDlgItem(dialog, Field::PollInterval.control_id() as i32).unwrap();
            assert!(
                SendMessageW(poll, CB_GETCOUNT, WPARAM(0), LPARAM(0)).0
                    >= PollInterval::ALL.len() as isize
            );
            SendMessageW(poll, CB_SETCURSEL, WPARAM(0), LPARAM(0));
            let polled = Field::PollInterval.command(&SettingsView::read(), poll);
            assert_eq!(
                polled,
                Some(SettingsCommand::SetPollInterval(PollInterval::OneMinute))
            );
            notify(dialog, Field::PollInterval, poll, CBN_SELCHANGE);

            // Alerts tab: quota combo must list every threshold.
            SendMessageW(tab, TCM_SETCURSEL, WPARAM(Tab::Alerts.index()), LPARAM(0));
            if let Some(state) = state_mut(dialog) {
                state.active_tab = Tab::Alerts;
                layout(dialog, state, &SettingsView::read());
            }
            let alert = GetDlgItem(dialog, Field::AlertThreshold.control_id() as i32).unwrap();
            assert!(
                SendMessageW(alert, CB_GETCOUNT, WPARAM(0), LPARAM(0)).0
                    >= AlertThreshold::ALL.len() as isize
            );

            let dpi = GetDpiForWindow(dialog).max(96);
            let mut client = RECT::default();
            GetClientRect(dialog, &mut client).unwrap();
            assert_eq!(client.right, px(dpi, PAD) * 2 + px(dpi, CLIENT_WIDTH));
            assert_eq!(CURRENT_DPI.load(Ordering::Relaxed), dpi_before);

            let def_id = SendMessageW(dialog, DM_GETDEFID, WPARAM(0), LPARAM(0)).0 as u32;
            assert_eq!((def_id >> 16) as u16, DC_HASDEFID as u16);
            assert_eq!(def_id as u16, Field::Close.control_id());

            assert!(pre_translate(&key_message(dialog, VK_RETURN.0)));
            assert!(!IsWindow(dialog).as_bool());

            open(widget);
            let reopened = DIALOG.with(|cell| cell.get());
            assert!(IsWindow(reopened).as_bool());
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
                close_provider_dialog();
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
