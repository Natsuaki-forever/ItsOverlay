#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[path = "../../editor_pipe.rs"]
mod editor_pipe;
mod fonts;
mod options;

use editor_pipe::Engine;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{json, Value};
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{
    cell::RefCell,
    env,
    ffi::c_void,
    rc::Rc,
    sync::{
        atomic::{AtomicI32, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

slint::include_modules!();

enum Action {
    Select(usize, String),
    ListTargets(TargetMenu),
    SelectManagedApp(usize),
    ManagerCommand(String, i32, usize, String),
    Page(i32),
    Option(String, String, i32, String),
    PreviewOption(String, String, String),
    SwitchTarget(usize, String),
    SetCanvasMode(String, String),
    CopyStyle,
    PasteStyle(String),
    ToggleHidden(usize, String),
    DuplicateRegion(String),
    Undo,
    Redo,
    SetHotkey(String),
    ToggleStartup,
    OpenConfigDir,
    Apply(String),
    Finish(String),
}

#[derive(Clone, Copy, PartialEq)]
enum TargetMenu { Select, Scope, Manage }

#[link(name = "user32")]
extern "system" {
    fn AllowSetForegroundWindow(process: u32) -> i32;
    fn GetWindowTextW(hwnd: *mut std::ffi::c_void, text: *mut u16, capacity: i32) -> i32;
    fn GetWindowRect(hwnd: *mut c_void, bounds: *mut WindowRect) -> i32;
    fn GetDpiForWindow(hwnd: *mut c_void) -> u32;
    fn GetWindowLongW(hwnd: *mut c_void, index: i32) -> i32;
    fn SetWindowLongW(hwnd: *mut c_void, index: i32, value: i32) -> i32;
    fn SetWindowPos(hwnd: *mut c_void, after: *mut c_void, x: i32, y: i32, width: i32, height: i32, flags: u32) -> i32;
    fn SetWindowRgn(hwnd: *mut c_void, region: *mut c_void, redraw: i32) -> i32;
    fn GetWindowRgn(hwnd: *mut c_void, region: *mut c_void) -> i32;
    fn IsZoomed(hwnd: *mut c_void) -> i32;
    fn IsIconic(hwnd: *mut c_void) -> i32;
    fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
    fn PeekMessageW(msg: *mut WinMsg, hwnd: *mut c_void, filter_min: u32, filter_max: u32, remove_msg: u32) -> i32;
    fn DefWindowProcW(hwnd: *mut c_void, message: u32, wparam: usize, lparam: isize) -> isize;
    fn MonitorFromRect(rect: *const WindowRect, flags: u32) -> *mut c_void;
    fn SendMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn LoadImageW(hinst: *mut c_void, name: *const u16, kind: u32, cx: i32, cy: i32, load: u32) -> *mut c_void;
    fn GetSystemMetrics(index: i32) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(module_name: *const u16) -> *mut c_void;
}

#[link(name = "advapi32")]
extern "system" {
    fn RegGetValueW(
        hkey: *mut c_void,
        sub_key: *const u16,
        value: *const u16,
        flags: u32,
        pdw_type: *mut u32,
        pv_data: *mut c_void,
        pcb_data: *mut u32,
    ) -> i32;
    fn RegSetKeyValueW(
        hkey: *mut c_void,
        sub_key: *const u16,
        value: *const u16,
        dw_type: u32,
        lp_data: *const c_void,
        cb_data: u32,
    ) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateRoundRectRgn(left: i32, top: i32, right: i32, bottom: i32, ellipse_w: i32, ellipse_h: i32) -> *mut c_void;
    fn CreateRectRgn(left: i32, top: i32, right: i32, bottom: i32) -> *mut c_void;
    fn EqualRgn(first: *mut c_void, second: *mut c_void) -> i32;
    fn DeleteObject(object: *mut c_void) -> i32;
}

#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(hwnd: *mut c_void, attribute: u32, value: *const u32, size: u32) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show_cmd: i32,
    ) -> isize;
}

#[repr(C)]
#[derive(Default)]
struct WindowRect { left: i32, top: i32, right: i32, bottom: i32 }

#[repr(C)]
struct WinMsg {
    hwnd: *mut c_void,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt_x: i32,
    pt_y: i32,
    l_private: u32,
}

type WindowProcedure = unsafe extern "system" fn(*mut c_void, u32, usize, isize, usize, usize) -> isize;

#[link(name = "comctl32")]
extern "system" {
    fn SetWindowSubclass(hwnd: *mut c_void, procedure: WindowProcedure, id: usize, data: usize) -> i32;
    fn RemoveWindowSubclass(hwnd: *mut c_void, procedure: WindowProcedure, id: usize) -> i32;
    fn DefSubclassProc(hwnd: *mut c_void, message: u32, wparam: usize, lparam: isize) -> isize;
}

static SELECTOR_RIGHT_DIP: AtomicI32 = AtomicI32::new(424);

unsafe extern "system" fn panel_frame(hwnd: *mut c_void, message: u32, wparam: usize, lparam: isize, id: usize, _: usize) -> isize {
    const WM_NCCALCSIZE: u32 = 0x83;
    const WM_NCHITTEST: u32 = 0x84;
    const WM_NCDESTROY: u32 = 0x82;
    if message == WM_NCCALCSIZE {
        // Let the Slint surface fill the resize frame.
        return 0;
    }
    if IsIconic(hwnd) == 0 {
        if message == 0x0086 { // WM_NCACTIVATE
            return DefWindowProcW(hwnd, message, wparam, -1);
        }
        if message == 0x0085 { return 0; } // WM_NCPAINT
    }
    if message == WM_NCHITTEST {
        let mut bounds = WindowRect::default();
        if GetWindowRect(hwnd, &mut bounds) != 0 {
            let x = lparam as i16 as i32 - bounds.left;
            let y = (lparam >> 16) as i16 as i32 - bounds.top;
            let w = bounds.right - bounds.left;
            let h = bounds.bottom - bounds.top;
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            let px = |dip| (dip * dpi + 48) / 96;
            let edge = px(7);
            let selector_right = SELECTOR_RIGHT_DIP.load(Ordering::Relaxed).max(291);
            let hit = match (x < edge, x >= w - edge, y < edge, y >= h - edge) {
                (true, _, true, _) => 13,  // HTTOPLEFT
                (_, true, true, _) => 14,  // HTTOPRIGHT
                (true, _, _, true) => 16,  // HTBOTTOMLEFT
                (_, true, _, true) => 17,  // HTBOTTOMRIGHT
                (_, _, true, _) => 12,     // HTTOP
                (_, _, _, true) => 15,     // HTBOTTOM
                (true, _, _, _) => 10,     // HTLEFT
                (_, true, _, _) => 11,     // HTRIGHT
                _ if y < px(65) && !(px(14)..px(58)).contains(&y) => 2,
                _ if y < px(65) && (x < px(150) || (x >= px(selector_right) && x < w - px(170))) => 2,
                _ => 1,                   // HTCLIENT
            };
            return hit;
        }
    }
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, panel_frame, id);
    }
    if message == 0x0100 && wparam == 0xe7 {
        // Wait briefly when VK_PACKET arrives ahead of its companion WM_CHAR/WM_KEYUP via PostMessageW.
        for _ in 0..30 {
            let mut char_msg: WinMsg = std::mem::zeroed();
            let mut up_msg: WinMsg = std::mem::zeroed();
            if PeekMessageW(&mut char_msg, hwnd, 0x0102, 0x0102, 0) != 0
                && PeekMessageW(&mut up_msg, hwnd, 0x0101, 0x0101, 0) != 0
            {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
    let result = DefSubclassProc(hwnd, message, wparam, lparam);
    if (message == 0x0005 && wparam != 1) || message == 0x02e0 {
        apply_panel_corners(hwnd);
    }
    result
}

unsafe fn try_enable_dwm_rounded_corners(hwnd: *mut c_void) -> bool {
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    if DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_ROUND, 4) == 0 {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &DWMWA_COLOR_NONE, 4);
        return true;
    }
    false
}

unsafe fn apply_panel_corners(hwnd: *mut c_void) {
    let previous = CreateRectRgn(0, 0, 0, 0);
    if previous.is_null() { return; }
    let has_region = GetWindowRgn(hwnd, previous) != 0;
    if try_enable_dwm_rounded_corners(hwnd) || IsZoomed(hwnd) != 0 {
        DeleteObject(previous);
        if has_region { SetWindowRgn(hwnd, std::ptr::null_mut(), 1); }
        return;
    }
    let mut bounds = WindowRect::default();
    if GetWindowRect(hwnd, &mut bounds) == 0 {
        DeleteObject(previous);
        return;
    }
    let diameter = (30 * GetDpiForWindow(hwnd).max(96) as i32 + 48) / 96;
    let region = CreateRoundRectRgn(0, 0, bounds.right - bounds.left, bounds.bottom - bounds.top, diameter, diameter);
    if !region.is_null() {
        if (has_region && EqualRgn(previous, region) != 0) || SetWindowRgn(hwnd, region, 1) == 0 {
            DeleteObject(region);
        }
    }
    DeleteObject(previous);
}

unsafe fn apply_panel_frame(hwnd: *mut c_void) {
    const GWL_STYLE: i32 = -16;
    const WS_CAPTION: i32 = 0x00c0_0000;
    const WS_THICKFRAME: i32 = 0x0004_0000;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const KEEP_PLACEMENT: u32 = 0x0017; // NOSIZE | NOMOVE | NOZORDER | NOACTIVATE
    const DWMWA_NCRENDERING_POLICY: u32 = 2;
    const DWMNCRP_DISABLED: u32 = 1;
    if !try_enable_dwm_rounded_corners(hwnd) {
        DwmSetWindowAttribute(hwnd, DWMWA_NCRENDERING_POLICY, &DWMNCRP_DISABLED, 4);
    }
    let current = GetWindowLongW(hwnd, GWL_STYLE);
    let desired = (current & !WS_CAPTION) | WS_THICKFRAME;
    if current != desired {
        SetWindowLongW(hwnd, GWL_STYLE, desired);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, KEEP_PLACEMENT | SWP_FRAMECHANGED);
    }
    apply_panel_corners(hwnd);
}

unsafe fn apply_panel_icon(hwnd: *mut c_void) {
    const IMAGE_ICON: u32 = 1;
    const WM_SETICON: u32 = 0x0080;
    const ICON_SMALL: usize = 0;
    const ICON_BIG: usize = 1;
    const SM_CXICON: i32 = 11;
    const SM_CYICON: i32 = 12;
    const SM_CXSMICON: i32 = 49;
    const SM_CYSMICON: i32 = 50;
    let instance = GetModuleHandleW(std::ptr::null());
    let res_id = 1usize as *const u16;
    let big = LoadImageW(instance, res_id, IMAGE_ICON, GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON), 0);
    if !big.is_null() {
        SendMessageW(hwnd, WM_SETICON, ICON_BIG, big as isize);
    }
    let small = LoadImageW(instance, res_id, IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), 0);
    if !small.is_null() {
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL, small as isize);
    }
}

fn install_panel_frame(hwnd: usize) {
    let hwnd = hwnd as *mut c_void;
    unsafe {
        apply_panel_icon(hwnd);
        if SetWindowSubclass(hwnd, panel_frame, 1, 0) != 0 {
            apply_panel_frame(hwnd);
        }
    }
}

fn sync_visibility(panel: &Panel, hwnd: usize, state: &Value) {
    if let Some(visible) = state["visible"].as_bool() {
        // Slint's show path invalidates the entire software surface after mapping.
        // A Win32-only show would leave the renderer's partial redraw cache intact.
        let result = if visible { panel.show() } else { panel.hide() };
        if let Err(error) = result {
            panel.set_status(format!("面板显示切换失败：{error}").into());
        } else if visible {
            // Winit reapplies native styles on show; keep decoration painting disabled.
            unsafe { apply_panel_frame(hwnd as *mut _); }
            panel.window().request_redraw();
        }
    }
}

fn argument(name: &str) -> Option<String> {
    let mut args = env::args().skip(1);
    while let Some(key) = args.next() {
        if key == name {
            return args.next();
        }
    }
    None
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn read_hkcu_dword(sub_key: &str, value_name: &str) -> Option<u32> {
    const HKEY_CURRENT_USER: usize = 0x8000_0001;
    const RRF_RT_REG_DWORD: u32 = 0x0000_0010;
    let key_w = to_wide(sub_key);
    let val_w = to_wide(value_name);
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER as *mut c_void,
            key_w.as_ptr(),
            val_w.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast::<c_void>(),
            &mut size,
        )
    };
    (status == 0).then_some(data)
}

fn write_hkcu_dword(sub_key: &str, value_name: &str, data: u32) {
    const HKEY_CURRENT_USER: usize = 0x8000_0001;
    const REG_DWORD: u32 = 4;
    let key_w = to_wide(sub_key);
    let val_w = to_wide(value_name);
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER as *mut c_void,
            key_w.as_ptr(),
            val_w.as_ptr(),
            REG_DWORD,
            (&data as *const u32).cast::<c_void>(),
            std::mem::size_of::<u32>() as u32,
        );
    }
}

fn read_hkcu_string(sub_key: &str, value_name: &str) -> Option<String> {
    const HKEY_CURRENT_USER: usize = 0x8000_0001;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    let key_w = to_wide(sub_key);
    let val_w = to_wide(value_name);
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * std::mem::size_of::<u16>()) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER as *mut c_void,
            key_w.as_ptr(),
            val_w.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast::<c_void>(),
            &mut size,
        )
    };
    if status != 0 || size < 2 {
        return None;
    }
    let units = (size as usize / std::mem::size_of::<u16>()).min(buf.len());
    let len = buf[..units].iter().position(|&c| c == 0).unwrap_or(units);
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn write_hkcu_string(sub_key: &str, value_name: &str, value: &str) {
    const HKEY_CURRENT_USER: usize = 0x8000_0001;
    const REG_SZ: u32 = 1;
    let key_w = to_wide(sub_key);
    let val_w = to_wide(value_name);
    let data_w = to_wide(value);
    let cb_data = (data_w.len() * std::mem::size_of::<u16>()) as u32;
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER as *mut c_void,
            key_w.as_ptr(),
            val_w.as_ptr(),
            REG_SZ,
            data_w.as_ptr().cast::<c_void>(),
            cb_data,
        );
    }
}

const PREFS_SUBKEY: &str = "Software\\ChatGPTOverlay\\NativeEditorLab";

fn parse_recent_colors_csv(csv: &str) -> Vec<String> {
    let mut colors = Vec::new();
    for part in csv.split(',') {
        let Some((r, g, b)) = parse_hex_rgb(part) else { continue };
        let hex = rgb_to_hex(r, g, b);
        if !colors.iter().any(|existing: &String| existing.eq_ignore_ascii_case(&hex)) {
            colors.push(hex);
            if colors.len() >= 8 {
                break;
            }
        }
    }
    colors
}

fn format_recent_colors_csv(colors: &[String]) -> String {
    colors.iter().take(8).cloned().collect::<Vec<_>>().join(",")
}

fn load_recent_colors() -> Option<Vec<String>> {
    if theme_no_preferences() {
        return None;
    }
    let raw = read_hkcu_string(PREFS_SUBKEY, "RecentColors")?;
    let parsed = parse_recent_colors_csv(&raw);
    (!parsed.is_empty()).then_some(parsed)
}

fn save_recent_colors(colors: &[String]) {
    if theme_no_preferences() {
        return;
    }
    write_hkcu_string(PREFS_SUBKEY, "RecentColors", &format_recent_colors_csv(colors));
}

fn parse_panel_bounds(raw: &str) -> Option<(i32, i32, i32, i32)> {
    let parts: Vec<i32> = raw
        .split(',')
        .filter_map(|s| s.trim().parse::<i32>().ok())
        .collect();
    if parts.len() != 4 {
        return None;
    }
    let (l, t, w, h) = (parts[0], parts[1], parts[2], parts[3]);
    if !(640..=3840).contains(&w) || !(525..=2160).contains(&h) {
        return None;
    }
    Some((l, t, w, h))
}

fn format_panel_bounds(l: i32, t: i32, w: i32, h: i32) -> String {
    format!("{l},{t},{w},{h}")
}

fn load_panel_bounds() -> Option<(i32, i32, i32, i32)> {
    if theme_no_preferences() {
        return None;
    }
    let raw = read_hkcu_string(PREFS_SUBKEY, "PanelBounds")?;
    parse_panel_bounds(&raw)
}

fn save_panel_bounds(hwnd: usize) {
    if theme_no_preferences() || hwnd == 0 {
        return;
    }
    let ptr = hwnd as *mut c_void;
    unsafe {
        if IsIconic(ptr) != 0 || IsZoomed(ptr) != 0 {
            return;
        }
        let mut rect = WindowRect::default();
        if GetWindowRect(ptr, &mut rect) != 0 {
            let w = rect.right - rect.left;
            let h = rect.bottom - rect.top;
            if (640..=3840).contains(&w) && (525..=2160).contains(&h) {
                write_hkcu_string(
                    PREFS_SUBKEY,
                    "PanelBounds",
                    &format_panel_bounds(rect.left, rect.top, w, h),
                );
            }
        }
    }
}

fn theme_no_preferences() -> bool {
    env::var_os("OVERLAY_NATIVE_EDITOR_LAB_NO_PREFERENCES")
        .is_some_and(|v| !v.is_empty() && v != "0")
}

fn system_prefers_dark() -> bool {
    if theme_no_preferences() {
        return false;
    }
    read_hkcu_dword(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
        "AppsUseLightTheme",
    ) == Some(0)
}

fn load_theme_mode() -> i32 {
    if theme_no_preferences() {
        return 0;
    }
    match read_hkcu_dword(PREFS_SUBKEY, "Theme") {
        Some(1) => 1,
        Some(2) => 2,
        _ => 0,
    }
}

fn save_theme_mode(mode: i32) {
    if theme_no_preferences() {
        return;
    }
    write_hkcu_dword(PREFS_SUBKEY, "Theme", mode.clamp(0, 2) as u32);
}

fn resolve_dark_for_mode(mode: i32, system_dark: bool) -> bool {
    match mode {
        1 => false,
        2 => true,
        _ => system_dark,
    }
}

fn friendly_error(code: &str) -> String {
    match code.trim() {
        "" => String::new(),
        "target_unavailable" => "目标窗口当前不可用或已最小化".to_owned(),
        "target_pixel_unavailable" => "无法读取目标窗口像素，请确认窗口未被最小化或受保护".to_owned(),
        "region_not_found" => "所选遮盖区域已不存在".to_owned(),
        "visual_worker_missing" => "视觉定位模块未就绪，请重试".to_owned(),
        "visual_position_not_ready" => "视觉定位尚未完成初始化，请稍候".to_owned(),
        "visual_mode_required" => "该操作仅在视觉定位模式下可用".to_owned(),
        "visual_result_outside_limits" => "视觉匹配超出搜索范围，请扩大找回距离或重选参考区".to_owned(),
        "visual_cover_outside_client" => "遮盖区域超出了窗口客户区范围".to_owned(),
        "calibration_unavailable" => "当前无法进行校准，请先关闭其他选区操作".to_owned(),
        "invalid_visual_check_interval" => "视觉检查间隔数值无效".to_owned(),
        "invalid_position_mode" => "定位模式无效".to_owned(),
        "render_failed" => "遮盖渲染失败，请检查字体或外观设置".to_owned(),
        "stale_revision" | "revision_mismatch" => "状态已更新，正在重新同步…".to_owned(),
        "Select a region" => "请先选择一个遮盖区域".to_owned(),
        "tracking_outside" => "遮盖区域超出目标窗口，请拖回后保存".to_owned(),
        "tracking_occluded" => "参考区域被遮挡，暂时保持上次位置".to_owned(),
        "tracking_ambiguous" => "找到多个相似区域，请缩小搜索范围或重选参考区".to_owned(),
        "tracking_not_found" => "未在目标窗口中找到参考内容，请检查或重新框选".to_owned(),
        "hotkey_in_use" => "该快捷键已被其他程序占用，请换一个组合键".to_owned(),
        "invalid_hotkey" => "快捷键格式无效，请使用 Ctrl / Alt / Shift + 字母或功能键".to_owned(),
        other => {
            if other.is_ascii() && other.contains('_') && !other.contains(' ') {
                format!("操作未能完成：{other}")
            } else {
                other.to_owned()
            }
        }
    }
}

fn checked(reply: Value) -> Result<Value, String> {
    if reply["ok"] == true {
        Ok(reply)
    } else {
        let raw = reply["error"]["code"]
            .as_str()
            .or(reply["error"].as_str())
            .unwrap_or("引擎拒绝了操作");
        Err(friendly_error(raw))
    }
}

fn selected_index(state: &Value) -> Option<usize> {
    let regions = state["regions"].as_array()?;
    regions.iter().position(|r| r["id"] == state["selected_id"])
        .or_else(|| (!regions.is_empty()).then_some(0))
}

enum DraftSync {
    Always,
    IfClean,
    IfCleanOrMatches(String),
}

fn parse_hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let trimmed = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
    if trimmed.len() == 6 {
        if let Ok(rgb) = u32::from_str_radix(trimmed, 16) {
            return Some(((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8));
        }
    }
    None
}

fn parse_hex_color(hex: &str, fallback: (u8, u8, u8)) -> slint::Color {
    let (r, g, b) = parse_hex_rgb(hex).unwrap_or(fallback);
    slint::Color::from_rgb_u8(r, g, b)
}

fn rgb_to_hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;
    let h = if delta <= 1e-5 {
        0.0
    } else if (max - rf).abs() <= 1e-5 {
        60.0 * (((gf - bf) / delta).rem_euclid(6.0))
    } else if (max - gf).abs() <= 1e-5 {
        60.0 * (((bf - rf) / delta) + 2.0)
    } else {
        60.0 * (((rf - gf) / delta) + 4.0)
    };
    let s = if max <= 1e-5 { 0.0 } else { (delta / max) * 100.0 };
    let v = max * 100.0;
    (h.clamp(0.0, 360.0), s.clamp(0.0, 100.0), v.clamp(0.0, 100.0))
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = h.clamp(0.0, 360.0) % 360.0;
    let s = (s.clamp(0.0, 100.0)) / 100.0;
    let v = (v.clamp(0.0, 100.0)) / 100.0;
    let c = v * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    (
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn apply_recent_colors(panel: &Panel, colors: &[String]) {
    let hexes: Vec<SharedString> = colors.iter().take(8).map(|s| s.as_str().into()).collect();
    let swatches: Vec<slint::Color> = colors
        .iter()
        .take(8)
        .map(|h| parse_hex_color(h.as_str(), (255, 255, 255)))
        .collect();
    panel.set_recent_color_hexes(ModelRc::new(VecModel::from(hexes)));
    panel.set_recent_color_swatches(ModelRc::new(VecModel::from(swatches)));
}

fn push_recent_color(panel: &Panel, hex: &str) {
    let Some((r, g, b)) = parse_hex_rgb(hex) else { return };
    let normalized = rgb_to_hex(r, g, b);
    let mut colors: Vec<String> = vec![normalized.clone()];
    for existing in panel.get_recent_color_hexes().iter() {
        if !existing.as_str().eq_ignore_ascii_case(&normalized) && colors.len() < 8 {
            colors.push(existing.as_str().to_owned());
        }
    }
    apply_recent_colors(panel, &colors);
    save_recent_colors(&colors);
}

const STYLE_KEYS: &[&str] = &[
    "font", "size", "bold", "align", "padding", "letter_spacing",
    "color", "opacity", "background",
    "scroll", "scroll_direction", "scroll_speed", "scroll_gap",
    "scroll_feather", "scroll_feather_width", "scroll_fill",
    "outline", "outline_width", "outline_color",
    "glow", "glow_color", "glow_radius", "glow_strength", "glow_pulse", "glow_period",
    "corner_radius", "border", "border_width", "border_color",
    "bg_gradient", "bg_gradient_color", "bg_gradient_angle",
    "shadow", "shadow_color", "shadow_offset_x", "shadow_offset_y",
    "shimmer", "shimmer_color", "shimmer_period", "shimmer_strength",
];

fn extract_style_values(region: &Value) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    let Some(obj) = region.as_object() else { return map };
    for &key in STYLE_KEYS {
        if let Some(val) = obj.get(key) {
            map.insert(key.to_owned(), val.clone());
        }
    }
    map
}

#[derive(Clone)]
struct UndoEntry {
    region_id: String,
    before: serde_json::Map<String, Value>,
    after: serde_json::Map<String, Value>,
}

fn diff_region_values(
    before_region: &Value,
    after_region: &Value,
    keys: &[&str],
) -> Option<(serde_json::Map<String, Value>, serde_json::Map<String, Value>)> {
    let mut before = serde_json::Map::new();
    let mut after = serde_json::Map::new();
    for &key in keys {
        let b = before_region.get(key).cloned().unwrap_or(Value::Null);
        let a = after_region.get(key).cloned().unwrap_or(Value::Null);
        if b != a {
            before.insert(key.to_owned(), b);
            after.insert(key.to_owned(), a);
        }
    }
    if before.is_empty() { None } else { Some((before, after)) }
}

fn push_undo(undo_stack: &mut Vec<UndoEntry>, redo_stack: &mut Vec<UndoEntry>, entry: UndoEntry) {
    redo_stack.clear();
    undo_stack.push(entry);
    if undo_stack.len() > 30 {
        undo_stack.remove(0);
    }
}

fn build_duplicate_region_payload(region: &Value, effective_rect: Option<&Value>) -> Value {
    let mut map = serde_json::Map::new();
    if let Some(obj) = region.as_object() {
        for (k, v) in obj {
            if matches!(k.as_str(), "id" | "position_mode" | "visual_binding" | "window_profile") {
                continue;
            }
            map.insert(k.clone(), v.clone());
        }
    }
    let rect_src = effective_rect.filter(|r| r.is_object()).unwrap_or(region);
    let x = rect_src["x"].as_f64().unwrap_or(20.0) + 20.0;
    let y = rect_src["y"].as_f64().unwrap_or(20.0) + 20.0;
    let width = rect_src["width"].as_f64().unwrap_or(160.0).max(8.0);
    let height = rect_src["height"].as_f64().unwrap_or(40.0).max(8.0);
    map.insert("x".into(), json!(x));
    map.insert("y".into(), json!(y));
    map.insert("width".into(), json!(width));
    map.insert("height".into(), json!(height));
    map.insert("position_mode".into(), json!("anchor"));
    map.insert("visual_binding".into(), Value::Null);
    map.insert("hidden".into(), json!(false));
    if region["position_mode"] == "visual" {
        map.insert("anchor_x".into(), json!("left"));
        map.insert("anchor_y".into(), json!("top"));
        map.insert("reference_width".into(), json!(0));
        map.insert("reference_height".into(), json!(0));
    }
    Value::Object(map)
}

fn region_tracking_summary(state: &Value, row: &Value, scoped: bool) -> (String, String) {
    if row["position_mode"] != "visual" {
        let label = if scoped { "固定 · 跟随窗口" } else { "固定 · 跟随应用" };
        return (label.to_owned(), "normal".to_owned());
    }
    let id = row["id"].as_str().unwrap_or_default();
    let ts = &state["tracking_states"][id];
    let ts_state = ts["state"].as_str().unwrap_or_default();
    let reason = ts["reason"].as_str().unwrap_or_default();
    if reason == "visual_cover_outside_client" || reason == "tracking_outside" {
        return ("视觉 · 超出窗口".to_owned(), "warn".to_owned());
    }
    if ts_state == "FOLLOWING" {
        return ("视觉 · 跟随中".to_owned(), "ok".to_owned());
    }
    if matches!(
        reason,
        "search_not_found" | "search_ambiguous" | "roi_occluded" | "reselect_required"
    ) || matches!(ts_state, "NEEDS_MANUAL" | "UNAVAILABLE")
    {
        return ("视觉 · 需关注".to_owned(), "warn".to_owned());
    }
    if matches!(ts_state, "HOLDING" | "VERIFYING" | "CALIBRATING")
        || reason.starts_with("searching")
        || reason.starts_with("confirming")
        || reason.starts_with("checking")
    {
        return ("视觉 · 定位中".to_owned(), "pending".to_owned());
    }
    let fallback = if scoped { "视觉 · 跟随窗口" } else { "视觉 · 跟随应用" };
    (fallback.to_owned(), "normal".to_owned())
}

fn selected_visual_detail(state: &Value, selected_row: Option<&Value>) -> String {
    if state["calibration_preview"].is_object() {
        return "校准预览中 · 请框选参考区域并确认保存".to_owned();
    }
    if state["canvas_mode"] == "cover_draft" {
        return "拖动遮盖调整相对位置，完成后点击「保存位置」".to_owned();
    }
    let Some(row) = selected_row else {
        return String::new();
    };
    if row["position_mode"] != "visual" {
        return "窗口锚定 · 随所选窗口边缘或中心定位".to_owned();
    }
    let id = row["id"].as_str().unwrap_or_default();
    let ts = &state["tracking_states"][id];
    let ts_state = ts["state"].as_str().unwrap_or_default();
    let reason = ts["reason"].as_str().unwrap_or_default();
    if reason == "visual_cover_outside_client" || reason == "tracking_outside" {
        return "已找到参考内容，但遮盖超出窗口范围，请拖回或调整偏移".to_owned();
    }
    if ts_state == "FOLLOWING" {
        return "视觉追踪运行中 · 已锁定参考特征并实时跟随".to_owned();
    }
    if reason.starts_with("searching") {
        return "正在目标窗口内搜索参考内容…".to_owned();
    }
    if reason == "confirming" {
        return "已发现候选位置，正在复核确认…".to_owned();
    }
    if reason == "search_not_found" {
        return "暂未找到参考内容，可重试、扩大找回距离或重选参考区".to_owned();
    }
    if reason == "search_ambiguous" {
        return "画面存在多处相似图案，建议缩小范围或换一处独特参考区".to_owned();
    }
    if reason == "roi_occluded" || reason == "transient_occlusion" {
        return "参考区域暂时被遮挡，当前保留上一次定位".to_owned();
    }
    "视觉追踪已启用 · 画面变化时自动定位参考区域".to_owned()
}

fn show_state(panel: &Panel, state: &Value, draft_sync: DraftSync) {
    let rows = state["regions"].as_array().cloned().unwrap_or_default();
    let selected = selected_index(state);
    let items: Vec<RegionItem> = rows
        .iter()
        .map(|row| {
            let title_line = row["text"]
                .as_str()
                .unwrap_or_default()
                .lines()
                .next()
                .unwrap_or_default();
            let scoped = !row["window_profile"].as_str().unwrap_or_default().is_empty();
            let hidden = row["hidden"] == true;
            let (raw_detail, raw_kind) = region_tracking_summary(state, row, scoped);
            let (detail, status_kind) = if hidden {
                (format!("已隐藏 · {raw_detail}"), "normal".to_owned())
            } else {
                (raw_detail, raw_kind)
            };
            RegionItem {
                title: SharedString::from(if title_line.is_empty() { "空白区域" } else { title_line }),
                detail: SharedString::from(detail),
                scoped,
                hidden,
                bg: parse_hex_color(row["background"].as_str().unwrap_or("#212121"), (33, 33, 33)),
                fg: parse_hex_color(row["color"].as_str().unwrap_or("#ececec"), (236, 236, 236)),
                status_kind: SharedString::from(status_kind),
            }
        })
        .collect();
    panel.set_region_items(ModelRc::new(VecModel::from(items)));
    panel.set_selected(selected.map_or(-1, |i| i as i32));
    panel.set_visual_position(selected.is_some_and(|index| rows[index]["position_mode"] == "visual"));
    panel.set_cover_outside(selected.is_some_and(|index| {
        let id = rows[index]["id"].as_str().unwrap_or_default();
        state["tracking_states"][id]["reason"] == "visual_cover_outside_client"
    }));
    panel.set_calibration_open(state["calibration_preview"].is_object());
    panel.set_calibration_has_reference(state["calibration_preview"]["reference"].is_array());
    let binding = selected.map_or(&Value::Null, |index| &rows[index]["visual_binding"]);
    panel.set_search_distance(if binding["tracking_kind"] == "features" {
        binding["search_limit_dip"].as_f64().map(|value| value.to_string()).unwrap_or_default().into()
    } else { "".into() });
    panel.set_retry_calibration(state["calibration_retry_available"] == true);
    panel.set_can_resume_calibration(binding.is_object());
    let selected_row = selected.map(|i| &rows[i]);
    panel.set_visual_status_detail(selected_visual_detail(state, selected_row).into());
    panel.set_position_status(match selected_row.map(|r| &r["position_mode"]) {
        Some(mode) if mode == "visual" => "视觉追踪 · 跟随参考区域",
        Some(_) => "窗口固定 · 随所选窗口边缘或中心定位",
        None => "",
    }.into());
    let text: SharedString = selected.and_then(|i| rows[i]["text"].as_str()).unwrap_or_default().into();
    let prev_loaded = panel.get_loaded_text();
    let cur_draft = panel.get_draft();
    panel.set_loaded_text(text.clone());
    let update_draft = match draft_sync {
        DraftSync::Always => true,
        DraftSync::IfClean => cur_draft == prev_loaded,
        DraftSync::IfCleanOrMatches(sent) => cur_draft == prev_loaded || cur_draft.as_str() == sent.as_str(),
    };
    if update_draft && cur_draft != text {
        panel.set_draft(text);
    }
    panel.set_can_select_target(state["canvas_modes"] == true);
    panel.set_engine_pid(state["engine_pid"].as_u64().unwrap_or(0) as i32);
    panel.set_can_draw(state["canvas_modes"] == true && state["target_hwnd"].as_u64().unwrap_or(0) != 0);
    panel.set_canvas_mode(state["canvas_mode"].as_str().unwrap_or("idle").into());
    let application = state["application_name"].as_str().filter(|name| !name.is_empty());
    let hwnd = state["target_hwnd"].as_u64().unwrap_or(0);
    let target = match (application, hwnd) {
        (Some(app), 0) => format!("{app} / 等待窗口"),
        (Some(app), hwnd) => {
            let mut title = [0u16; 1024];
            let length = unsafe { GetWindowTextW(hwnd as usize as *mut _, title.as_mut_ptr(), title.len() as i32) };
            let title = String::from_utf16_lossy(&title[..length.max(0) as usize]);
            if title.is_empty() { app.to_owned() } else { format!("{app} / {title}") }
        }
        (None, _) => "选择应用".to_owned(),
    };
    panel.set_target_name(target.into());
    panel.set_hotkey_text(state["hotkey"].as_str().unwrap_or("Ctrl+Alt+O").into());
    panel.set_hotkey_ok(state["hotkey_ok"].as_bool().unwrap_or(true));
    panel.set_startup_enabled(state["startup_enabled"].as_bool().unwrap_or(false));
    panel.set_app_version(state["app_version"].as_str().unwrap_or("dev").into());
    panel.set_settings(options::settings(state, selected.map_or(&Value::Null, |i| &rows[i]), panel.get_page()));
}

fn target_choices(reply: &Value, menu: TargetMenu) -> Vec<(TargetRow, Value)> {
    let active = reply["active_application"].as_str().unwrap_or_default();
    let mut choices = Vec::new();
    let mut add = |title: &str, group: &str, kind: &str, current: bool, payload: Value| {
        choices.push((TargetRow {
            title: title.into(), group: group.into(), kind: kind.into(), current, parent: -1,
        }, payload));
    };
    if menu != TargetMenu::Manage {
        for app in reply["applications"].as_array().into_iter().flatten() {
            let id = app["id"].as_str().unwrap_or_default();
            if menu == TargetMenu::Scope && id != active { continue; }
            let name = app["name"].as_str().unwrap_or("未命名应用");
            let windows = app["windows"].as_array().map(Vec::as_slice).unwrap_or_default();
            if menu == TargetMenu::Select {
                if id != active && windows.len() <= 1 {
                    let payload = windows.first().map_or(json!({"id": id}), |window|
                        json!({"id": id, "index": window["index"]}));
                    add(name, "", "app-direct", false, payload);
                    continue;
                }
                add(name, id, "app", id == active, Value::Null);
            }
            for window in windows {
                let title = window["title"].as_str().filter(|title| !title.is_empty()).unwrap_or("无标题窗口");
                let group = if menu == TargetMenu::Scope { "" } else { id };
                add(title, group, "window", window["bound"] == true, json!({"id": id, "index": window["index"]}));
            }
            if menu == TargetMenu::Scope { continue; }
            if windows.is_empty() {
                add("等待窗口", id, "waiting", false, Value::Null);
            }
        }
    }
    if menu == TargetMenu::Select {
        add("添加应用", "", "add", false, json!({"pick": true}));
        add("管理应用与窗口配置…", "", "manage", false, Value::Null);
    }
    choices
}

fn combined_applications(reply: &Value) -> Vec<Value> {
    let mut list: Vec<Value> = reply["applications"].as_array().cloned().unwrap_or_default();
    for del in reply["deletable_applications"].as_array().into_iter().flatten() {
        let Some(del_id) = del["id"].as_str().filter(|s| !s.is_empty()) else { continue };
        if !list.iter().any(|app| app["id"].as_str() == Some(del_id)) {
            list.push(json!({
                "id": del_id,
                "name": del["name"].as_str().unwrap_or("未命名应用"),
                "executable": del["executable"].as_str().unwrap_or_default(),
                "windows": [],
                "profiles": [],
            }));
        }
    }
    list
}

fn build_app_manager_models(
    reply: &Value,
    preferred_app_id: Option<&str>,
    preferred_index: Option<usize>,
) -> (Vec<ManagedAppItem>, usize, Vec<ManagedWindowItem>, Vec<ManagedProfileItem>) {
    let active = reply["active_application"].as_str().unwrap_or_default();
    let deletable = reply["deletable_applications"].as_array().map(Vec::as_slice).unwrap_or_default();
    let apps_raw = combined_applications(reply);
    let mut app_items = Vec::with_capacity(apps_raw.len());
    for app in &apps_raw {
        let id = app["id"].as_str().unwrap_or_default();
        let name = app["name"].as_str().filter(|s| !s.is_empty()).unwrap_or("未命名应用");
        let windows = app["windows"].as_array().map(Vec::as_slice).unwrap_or_default();
        let profiles = app["profiles"].as_array().map(Vec::as_slice).unwrap_or_default();
        let is_active = !active.is_empty() && id == active;
        let can_delete = deletable.iter().any(|d| d["id"].as_str() == Some(id));
        let subtitle = match (windows.len(), profiles.len()) {
            (w, p) if w > 0 && p > 0 => format!("{w} 个运行窗口 · {p} 个已存配置"),
            (w, _) if w > 0 => format!("{w} 个运行窗口"),
            (_, p) if p > 0 => format!("未运行 · {p} 个已存配置"),
            _ => "当前无运行窗口".to_owned(),
        };
        app_items.push(ManagedAppItem {
            id: id.into(),
            name: name.into(),
            subtitle: subtitle.into(),
            active: is_active,
            can_detach: is_active,
            can_delete,
        });
    }
    if app_items.is_empty() {
        return (app_items, 0, Vec::new(), Vec::new());
    }
    let selected_idx = preferred_app_id
        .and_then(|target_id| app_items.iter().position(|a| a.id.as_str() == target_id))
        .or_else(|| preferred_index.filter(|&idx| idx < app_items.len()))
        .or_else(|| app_items.iter().position(|a| a.active))
        .unwrap_or(0);
    let selected_app = &apps_raw[selected_idx];
    let windows = selected_app["windows"].as_array().map(Vec::as_slice).unwrap_or_default();
    let profiles = selected_app["profiles"].as_array().map(Vec::as_slice).unwrap_or_default();
    let window_items = windows.iter().map(|w| {
        let index = w["index"].as_i64().unwrap_or(0) as i32;
        let title = w["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("无标题窗口");
        let bound = w["bound"] == true;
        let profile = w["profile"].as_str().unwrap_or_default();
        let has_profile = !profile.is_empty() && profile != "legacy";
        let profile_label = match (bound, has_profile) {
            (true, true) => "当前窗口 · 独立配置",
            (true, false) => "当前绑定窗口",
            (false, true) => "使用独立配置",
            (false, false) => "共享默认配置",
        };
        ManagedWindowItem {
            index,
            title: title.into(),
            bound,
            has_profile,
            profile_label: profile_label.into(),
        }
    }).collect();
    let target_window = windows.iter().find(|w| w["bound"] == true).or_else(|| windows.first());
    let target_window_title = target_window
        .and_then(|w| w["title"].as_str())
        .filter(|t| !t.is_empty())
        .unwrap_or("当前窗口");
    let profile_items = profiles.iter().map(|p| {
        let id = p["id"].as_str().unwrap_or_default();
        let title = p["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("未命名窗口配置");
        let can_apply = p["deleting"] != true && target_window.is_some();
        let can_delete = !id.is_empty() && id != "legacy";
        ManagedProfileItem {
            id: id.into(),
            title: title.into(),
            can_apply,
            target_window_title: target_window_title.into(),
            can_delete,
        }
    }).collect();
    (app_items, selected_idx, window_items, profile_items)
}

fn populate_app_manager(
    panel: &Panel,
    reply: &Value,
    preferred_app_id: Option<&str>,
    preferred_index: Option<usize>,
) -> usize {
    let (apps, selected_idx, windows, profiles) = build_app_manager_models(reply, preferred_app_id, preferred_index);
    panel.set_managed_apps(ModelRc::new(VecModel::from(apps)));
    panel.set_managed_app_index(selected_idx as i32);
    panel.set_managed_windows(ModelRc::new(VecModel::from(windows)));
    panel.set_managed_profiles(ModelRc::new(VecModel::from(profiles)));
    selected_idx
}

fn manager_command_payload(
    reply: &Value,
    app_idx: usize,
    kind: &str,
    item_idx: i32,
) -> Result<(Value, Option<String>), String> {
    if kind == "add_app" {
        return Ok((json!({"pick": true}), None));
    }
    let apps = combined_applications(reply);
    let app = apps.get(app_idx).ok_or("请先选择一个应用")?;
    let id = app["id"].as_str().unwrap_or_default();
    let name = app["name"].as_str().filter(|s| !s.is_empty()).unwrap_or("未命名应用");
    let windows = app["windows"].as_array().map(Vec::as_slice).unwrap_or_default();
    let profiles = app["profiles"].as_array().map(Vec::as_slice).unwrap_or_default();
    match kind {
        "switch_app" => {
            let payload = windows.first().map_or(json!({"id": id}), |w| json!({"id": id, "index": w["index"]}));
            Ok((payload, Some(id.to_owned())))
        }
        "switch_window" => {
            let win = windows.get(item_idx.max(0) as usize).ok_or("目标窗口已关闭，请刷新")?;
            Ok((json!({"id": id, "index": win["index"]}), Some(id.to_owned())))
        }
        "delete_window_profile" => {
            let win = windows.get(item_idx.max(0) as usize).ok_or("目标窗口已关闭，请刷新")?;
            let profile = win["profile"].as_str().filter(|p| !p.is_empty() && *p != "legacy")
                .ok_or("该窗口没有独立配置")?;
            let title = win["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("无标题窗口");
            Ok((json!({
                "id": id,
                "index": win["index"],
                "delete_window_profile": profile,
                "name": title,
            }), Some(id.to_owned())))
        }
        "apply_profile" => {
            let prof = profiles.get(item_idx.max(0) as usize).ok_or("该窗口配置已不存在")?;
            let target_win = windows.iter().find(|w| w["bound"] == true)
                .or_else(|| windows.first())
                .ok_or("当前没有可应用配置的运行窗口")?;
            Ok((json!({
                "id": id,
                "index": target_win["index"],
                "window_profile": prof["id"],
            }), Some(id.to_owned())))
        }
        "delete_saved_profile" => {
            let prof = profiles.get(item_idx.max(0) as usize).ok_or("该窗口配置已不存在")?;
            let title = prof["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("未命名窗口配置");
            Ok((json!({
                "id": id,
                "delete_window_profile": prof["id"],
                "name": title,
            }), Some(id.to_owned())))
        }
        "detach_app" => Ok((json!({"remove_current": true}), Some(id.to_owned()))),
        "delete_app" => {
            let executable = reply["deletable_applications"].as_array().into_iter().flatten()
                .find(|d| d["id"].as_str() == Some(id))
                .and_then(|d| d["executable"].as_str())
                .or_else(|| app["executable"].as_str())
                .unwrap_or_default();
            Ok((json!({
                "delete_id": id,
                "name": name,
                "executable": executable,
            }), None))
        }
        _ => Err(format!("未知管理操作：{kind}")),
    }
}

fn apply_text(engine: &mut Engine, state: &mut Value, draft: String) -> Result<(), String> {
    let Some(index) = selected_index(state) else { return Ok(()) };
    let region = &state["regions"][index];
    if region["text"].as_str() != Some(draft.as_str()) {
        let reply = checked(engine.request("update",
            json!({"id": region["id"], "values": {"text": draft}}),
            state["revision"].as_u64())?)?;
        *state = reply["state"].clone();
    }
    Ok(())
}

fn work(hwnd: usize, path: String, token: String, actions: mpsc::Receiver<Action>, view: slint::Weak<Panel>) -> Result<(), String> {
    let mut engine = Engine::connect(&path, token)?;
    let initial = checked(engine.request("hello", json!({
        "hwnd": hwnd, "capabilities": ["visual_geometry_v1", "visual_tracking_v1", "visual_calibration_preview_v1"]
    }), None)?)?;
    let mut state = initial["state"].clone();
    let mut finish_sequence = state["finish_sequence"].as_u64();
    let mut targets = Vec::new();
    let mut apps_reply = Value::Null;
    let mut scope_target_list = false;
    let mut pending_action: Option<Action> = None;
    let mut copied_style: Option<serde_json::Map<String, Value>> = None;
    let mut undo_stack: Vec<UndoEntry> = Vec::new();
    let mut redo_stack: Vec<UndoEntry> = Vec::new();
    let mut preview_baseline: Option<(String, Value)> = None;
    let initial_state = state.clone();
    view.upgrade_in_event_loop(move |panel| {
        show_state(&panel, &initial_state, DraftSync::Always);
        panel.set_busy(false);
        panel.set_status("".into());
    }).map_err(|e| e.to_string())?;

    loop {
        let mut action = if let Some(next) = pending_action.take() {
            next
        } else {
            match actions.recv_timeout(Duration::from_millis(350)) {
                Ok(action) => action,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let reply = checked(engine.request("snapshot", json!({}), None)?)?;
                    let incoming = &reply["state"];
                    let next = incoming["finish_sequence"].as_u64();
                    if next != finish_sequence {
                        finish_sequence = next;
                        view.upgrade_in_event_loop(|panel| {
                            panel.set_busy(false);
                            panel.invoke_finish();
                        }).map_err(|e| e.to_string())?;
                    }
                    if ["regions", "selected_id", "canvas_mode", "target_hwnd", "effective_positions", "tracking_states", "calibration_preview", "calibration_busy", "calibration_result", "calibration_retry_available", "visual_error", "pick_error", "cover_drag_relation", "visual_check_interval_seconds", "visible", "application_id", "application_name", "binding_epoch", "hotkey", "hotkey_ok", "startup_enabled"].iter().any(|key| state[*key] != incoming[*key]) {
                        let visibility_changed = state["visible"] != incoming["visible"];
                        let selected_changed = state["selected_id"] != incoming["selected_id"]
                            || state["application_id"] != incoming["application_id"]
                            || state["binding_epoch"] != incoming["binding_epoch"];
                        state = incoming.clone();
                        let latest = state.clone();
                        view.upgrade_in_event_loop(move |panel| {
                            if visibility_changed { sync_visibility(&panel, hwnd, &latest); }
                            if !panel.get_busy() {
                                show_state(
                                    &panel,
                                    &latest,
                                    if selected_changed { DraftSync::Always } else { DraftSync::IfClean },
                                );
                                panel.set_status(canvas_status(&latest).into());
                            }
                        }).map_err(|e| e.to_string())?;
                    }
                    continue;
                }
            }
        };
        while let Ok(next) = actions.try_recv() {
            match (&action, &next) {
                (Action::Apply(_), Action::Apply(_)) => action = next,
                (Action::Option(k1, _, _, _), Action::Option(k2, _, _, _)) if k1 == k2 => action = next,
                (Action::PreviewOption(k1, _, _), Action::PreviewOption(k2, _, _)) if k1 == k2 => action = next,
                (Action::Page(_), Action::Page(_)) => action = next,
                (Action::SelectManagedApp(_), Action::SelectManagedApp(_)) => action = next,
                _ => {
                    pending_action = Some(next);
                    break;
                }
            }
        }
        if let Action::ListTargets(menu) = action {
            scope_target_list = menu == TargetMenu::Scope;
            let result = engine.request("list_applications", json!({}), None).and_then(checked);
            if let Ok(reply) = &result {
                apps_reply = reply.clone();
            }
            let active_group: SharedString = result.as_ref().ok().and_then(|reply|
                reply["active_application"].as_str()).unwrap_or_default().into();
            let labels: Vec<TargetRow> = match &result {
                Ok(reply) if menu != TargetMenu::Manage => {
                    targets = target_choices(reply, menu);
                    targets.iter().map(|(row, _)| row.clone()).collect()
                }
                _ => Vec::new(),
            };
            view.upgrade_in_event_loop(move |panel| {
                panel.set_busy(false);
                match result {
                    Ok(reply) => match menu {
                        TargetMenu::Manage => {
                            panel.set_choosing_target(false);
                            panel.set_scope_picker_open(false);
                            panel.set_manager_confirm_open(false);
                            populate_app_manager(&panel, &reply, None, None);
                            panel.set_app_manager_open(true);
                        }
                        TargetMenu::Scope => {
                            panel.set_choosing_target(false);
                            panel.set_app_manager_open(false);
                            panel.set_targets(ModelRc::new(VecModel::from(labels)));
                            panel.set_scope_picker_open(true);
                        }
                        TargetMenu::Select => {
                            panel.set_scope_picker_open(false);
                            panel.set_targets(ModelRc::new(VecModel::from(labels)));
                            panel.set_managing_targets(false);
                            panel.set_expanded_group(active_group);
                            panel.set_expanded_action(-1);
                            panel.set_expanded_management(false);
                            panel.set_pending_target(-1);
                            panel.set_choosing_target(true);
                        }
                    },
                    Err(error) => panel.set_status(error.into()),
                }
            }).map_err(|e| e.to_string())?;
            continue;
        }
        if let Action::SelectManagedApp(idx) = action {
            let reply = apps_reply.clone();
            view.upgrade_in_event_loop(move |panel| {
                panel.set_manager_confirm_open(false);
                populate_app_manager(&panel, &reply, None, Some(idx));
            }).map_err(|e| e.to_string())?;
            continue;
        }
        if let Action::CopyStyle = action {
            let copied = selected_index(&state).map(|idx| extract_style_values(&state["regions"][idx]));
            let ok = copied.as_ref().is_some_and(|m| !m.is_empty());
            if ok {
                copied_style = copied;
            }
            view.upgrade_in_event_loop(move |panel| {
                if ok {
                    panel.set_has_copied_style(true);
                    panel.set_status("已复制当前区域样式".into());
                } else {
                    panel.set_status("请先选择一个遮盖区域".into());
                }
            }).map_err(|e| e.to_string())?;
            continue;
        }
        if let Action::ManagerCommand(kind, item_idx, app_idx, draft) = action {
            let previous_visibility = state["visible"].clone();
            let is_add_app = kind == "add_app";
            let result = (|| {
                apply_text(&mut engine, &mut state, draft)?;
                if apps_reply.is_null() {
                    apps_reply = checked(engine.request("list_applications", json!({}), None)?)?;
                }
                let (payload, preferred_id) = manager_command_payload(&apps_reply, app_idx, &kind, item_idx)?;
                checked(engine.request("switch_application", payload, state["revision"].as_u64())?)?;
                state = checked(engine.request("snapshot", json!({}), None)?)?["state"].clone();
                if !is_add_app {
                    apps_reply = checked(engine.request("list_applications", json!({}), None)?)?;
                    undo_stack.clear();
                    redo_stack.clear();
                    preview_baseline = None;
                }
                Ok::<Option<String>, String>(preferred_id)
            })();
            let latest = state.clone();
            let latest_apps = apps_reply.clone();
            let visibility_changed = previous_visibility != latest["visible"];
            let can_undo = !undo_stack.is_empty();
            let can_redo = !redo_stack.is_empty();
            view.upgrade_in_event_loop(move |panel| {
                panel.set_busy(false);
                panel.set_can_undo(can_undo);
                panel.set_can_redo(can_redo);
                match result {
                    Ok(preferred_id) => {
                        if visibility_changed { sync_visibility(&panel, hwnd, &latest); }
                        show_state(&panel, &latest, DraftSync::Always);
                        if is_add_app {
                            panel.set_app_manager_open(false);
                            panel.set_status("请在屏幕上点选目标窗口…".into());
                        } else {
                            populate_app_manager(&panel, &latest_apps, preferred_id.as_deref(), Some(app_idx));
                            panel.set_status(canvas_status(&latest).into());
                        }
                    }
                    Err(error) => {
                        panel.set_status(error.into());
                    }
                }
            }).map_err(|e| e.to_string())?;
            continue;
        }
        if let Action::Page(raw_page) = action {
            let page = raw_page.clamp(0, 2);
            let latest = state.clone();
            view.upgrade_in_event_loop(move |panel| {
                let region = selected_index(&latest).map_or(&Value::Null, |i| &latest["regions"][i]);
                panel.set_settings(options::settings(&latest, region, page));
                panel.set_page(page);
            }).map_err(|e| e.to_string())?;
            continue;
        }
        let finishing = matches!(action, Action::Finish(_));
        let switching = matches!(action, Action::SwitchTarget(..));
        let mut switching_pick = false;
        let selecting = matches!(action, Action::Select(..));
        let drawing = matches!(action, Action::SetCanvasMode(..));
        let configuring = matches!(action, Action::Option(..));
        let previewing = matches!(action, Action::PreviewOption(..));
        let history_or_paste = matches!(action, Action::Undo | Action::Redo | Action::PasteStyle(_) | Action::DuplicateRegion(_));
        let mut status_override: Option<&'static str> = None;
        let sent_draft = match &action {
            Action::Apply(draft)
            | Action::SetCanvasMode(_, draft)
            | Action::Option(_, _, _, draft)
            | Action::PreviewOption(_, _, draft)
            | Action::ToggleHidden(_, draft)
            | Action::DuplicateRegion(draft)
            | Action::PasteStyle(draft) => Some(draft.clone()),
            _ => None,
        };
        let previous_visibility = state["visible"].clone();
        let result = (|| {
            match action {
                Action::Select(index, draft) => {
                    apply_text(&mut engine, &mut state, draft)?;
                    let id = state["regions"].get(index).ok_or("区域已经不存在")?["id"].clone();
                    state = checked(engine.request("select", json!({"id": id}), None)?)?["state"].clone();
                }
                Action::Apply(draft) => {
                    let before = selected_index(&state).map(|i| state["regions"][i].clone());
                    apply_text(&mut engine, &mut state, draft)?;
                    if let (Some(b), Some(after)) = (before, selected_index(&state).map(|i| &state["regions"][i])) {
                        if let Some((b_map, a_map)) = diff_region_values(&b, after, &["text"]) {
                            if let Some(region_id) = b["id"].as_str() {
                                push_undo(&mut undo_stack, &mut redo_stack, UndoEntry {
                                    region_id: region_id.to_owned(),
                                    before: b_map,
                                    after: a_map,
                                });
                            }
                        }
                    }
                }
                Action::PasteStyle(draft) => {
                    apply_text(&mut engine, &mut state, draft)?;
                    let style = copied_style.clone().ok_or("尚未复制任何区域样式")?;
                    let idx = selected_index(&state).ok_or("请先选择一个遮盖区域")?;
                    let before = state["regions"][idx].clone();
                    let region_id = before["id"].as_str().ok_or("区域已经不存在")?.to_owned();
                    state = checked(engine.request(
                        "update",
                        json!({"id": region_id, "values": style}),
                        state["revision"].as_u64(),
                    )?)?["state"].clone();
                    if let Some(after_idx) = state["regions"].as_array().and_then(|arr| arr.iter().position(|r| r["id"].as_str() == Some(&region_id))) {
                        if let Some((b_map, a_map)) = diff_region_values(&before, &state["regions"][after_idx], STYLE_KEYS) {
                            push_undo(&mut undo_stack, &mut redo_stack, UndoEntry {
                                region_id,
                                before: b_map,
                                after: a_map,
                            });
                        }
                    }
                    status_override = Some("已粘贴样式到当前区域");
                }
                Action::ToggleHidden(index, draft) => {
                    apply_text(&mut engine, &mut state, draft)?;
                    let region = state["regions"].get(index).ok_or("区域已经不存在")?.clone();
                    let region_id = region["id"].as_str().ok_or("区域已经不存在")?.to_owned();
                    let next_hidden = region["hidden"] != true;
                    state = checked(engine.request(
                        "update",
                        json!({"id": region_id, "values": {"hidden": next_hidden}}),
                        state["revision"].as_u64(),
                    )?)?["state"].clone();
                    if let Some(after) = state["regions"].as_array().and_then(|arr| arr.iter().find(|r| r["id"].as_str() == Some(&region_id))) {
                        if let Some((b_map, a_map)) = diff_region_values(&region, after, &["hidden"]) {
                            push_undo(&mut undo_stack, &mut redo_stack, UndoEntry {
                                region_id,
                                before: b_map,
                                after: a_map,
                            });
                        }
                    }
                    status_override = Some(if next_hidden { "已隐藏该区域" } else { "已显示该区域" });
                }
                Action::DuplicateRegion(draft) => {
                    apply_text(&mut engine, &mut state, draft)?;
                    let idx = selected_index(&state).ok_or("请先选择一个遮盖区域")?;
                    let region = &state["regions"][idx];
                    let id = region["id"].as_str().unwrap_or_default();
                    let effective_rect = state["effective_rects"].get(id);
                    let payload = build_duplicate_region_payload(region, effective_rect);
                    state = checked(engine.request("create", payload, state["revision"].as_u64())?)?["state"].clone();
                    status_override = Some("已复制区域");
                }
                Action::Undo => {
                    let entry = undo_stack.pop().ok_or("没有可撤销的更改")?;
                    state = checked(engine.request(
                        "update",
                        json!({"id": entry.region_id, "values": entry.before}),
                        state["revision"].as_u64(),
                    )?)?["state"].clone();
                    if state["selected_id"].as_str() != Some(&entry.region_id) {
                        if let Ok(sel_reply) = engine.request("select", json!({"id": entry.region_id}), None).and_then(checked) {
                            state = sel_reply["state"].clone();
                        }
                    }
                    redo_stack.push(entry);
                    preview_baseline = None;
                    status_override = Some("已撤销");
                }
                Action::Redo => {
                    let entry = redo_stack.pop().ok_or("没有可重做的更改")?;
                    state = checked(engine.request(
                        "update",
                        json!({"id": entry.region_id, "values": entry.after}),
                        state["revision"].as_u64(),
                    )?)?["state"].clone();
                    if state["selected_id"].as_str() != Some(&entry.region_id) {
                        if let Ok(sel_reply) = engine.request("select", json!({"id": entry.region_id}), None).and_then(checked) {
                            state = sel_reply["state"].clone();
                        }
                    }
                    undo_stack.push(entry);
                    preview_baseline = None;
                    status_override = Some("已重做");
                }
                Action::SetHotkey(spec) => {
                    state = checked(engine.request("set_hotkey", json!({"hotkey": spec}), None)?)?["state"].clone();
                    let hk = state["hotkey"].as_str().unwrap_or("Ctrl+Alt+O");
                    status_override = Some(if hk == "None" { "已关闭全局呼出快捷键" } else { "全局快捷键已更新" });
                }
                Action::ToggleStartup => {
                    state = checked(engine.request("toggle_startup", json!({}), None)?)?["state"].clone();
                    let on = state["startup_enabled"].as_bool().unwrap_or(false);
                    status_override = Some(if on { "已开启开机自启" } else { "已关闭开机自启" });
                }
                Action::OpenConfigDir => {
                    let _ = checked(engine.request("open_config_dir", json!({}), None)?)?;
                    status_override = Some("已打开配置目录");
                }
                Action::SwitchTarget(index, draft) => {
                    let payload = targets.get(index).ok_or("请重新打开应用列表")?.1.clone();
                    switching_pick = payload["pick"] == true;
                    apply_text(&mut engine, &mut state, draft)?;
                    if scope_target_list {
                        let index = selected_index(&state).ok_or_else(|| friendly_error("Select a region"))?;
                        let payload = json!({"id": state["regions"][index]["id"], "index": payload["index"]});
                        state = checked(engine.request("set_region_scope",payload,state["revision"].as_u64())?)?["state"].clone();
                    } else {
                        let reply = checked(engine.request("switch_application", payload, state["revision"].as_u64())?)?;
                        if !switching_pick && reply["state"].is_object() {
                            state = reply["state"].clone();
                        }
                        undo_stack.clear();
                        redo_stack.clear();
                        preview_baseline = None;
                    }
                }
                Action::SetCanvasMode(mode, draft) => {
                    apply_text(&mut engine, &mut state, draft)?;
                    let reply = checked(engine.request(
                        "set_canvas_mode", json!({"mode": mode}), state["revision"].as_u64(),
                    )?)?;
                    state = reply["state"].clone();
                }
                Action::Finish(draft) => {
                    apply_text(&mut engine, &mut state, draft)?;
                    checked(engine.request("commit_close", json!({}), state["revision"].as_u64())?)?;
                }
                Action::Option(key, value, choice, draft) => {
                    apply_text(&mut engine, &mut state, draft)?;
                    let current_region = selected_index(&state).map(|index| state["regions"][index].clone());
                    let region_ref = current_region.as_ref().unwrap_or(&Value::Null);
                    let (op, payload) = options::operation(&state, region_ref, &key, &value, choice)?;
                    let baseline = match (&preview_baseline, current_region.as_ref().and_then(|r| r["id"].as_str())) {
                        (Some((pid, pval)), Some(rid)) if pid == rid => Some(pval.clone()),
                        _ => current_region.clone(),
                    };
                    preview_baseline = None;
                    state = checked(engine.request(op, payload.clone(), state["revision"].as_u64())?)?["state"].clone();
                    if op == "update" {
                        if let (Some(b), Some(rid)) = (baseline, payload["id"].as_str()) {
                            if let Some(after) = state["regions"].as_array().and_then(|arr| arr.iter().find(|r| r["id"].as_str() == Some(rid))) {
                                if let Some(val_obj) = payload["values"].as_object() {
                                    let keys: Vec<&str> = val_obj.keys().map(|k| k.as_str()).collect();
                                    if let Some((b_map, a_map)) = diff_region_values(&b, after, &keys) {
                                        push_undo(&mut undo_stack, &mut redo_stack, UndoEntry {
                                            region_id: rid.to_owned(),
                                            before: b_map,
                                            after: a_map,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                Action::PreviewOption(key, value, draft) => {
                    apply_text(&mut engine, &mut state, draft)?;
                    if let Some(idx) = selected_index(&state) {
                        let region = &state["regions"][idx];
                        if let Some(rid) = region["id"].as_str() {
                            if preview_baseline.as_ref().is_none_or(|(id, _)| id != rid) {
                                preview_baseline = Some((rid.to_owned(), region.clone()));
                            }
                        }
                    }
                    let region = selected_index(&state).map_or(&Value::Null, |index| &state["regions"][index]);
                    let (op, payload) = options::operation(&state, region, &key, &value, -1)?;
                    state = checked(engine.request(op, payload, state["revision"].as_u64())?)?["state"].clone();
                }
                Action::ListTargets(_)
                | Action::Page(_)
                | Action::SelectManagedApp(_)
                | Action::CopyStyle
                | Action::ManagerCommand(..) => unreachable!(),
            }
            Ok::<(), String>(())
        })();
        if finishing && result.is_ok() {
            let _ = slint::quit_event_loop();
            return Ok(());
        }
        let latest = state.clone();
        let visibility_changed = previous_visibility != latest["visible"];
        let can_undo = !undo_stack.is_empty();
        let can_redo = !redo_stack.is_empty();
        view.upgrade_in_event_loop(move |panel| match result {
            Ok(()) => {
                if visibility_changed { sync_visibility(&panel, hwnd, &latest); }
                panel.set_can_undo(can_undo);
                panel.set_can_redo(can_redo);
                if previewing {
                    return;
                }
                let draft_sync = if selecting || switching || history_or_paste {
                    DraftSync::Always
                } else if let Some(sent) = sent_draft {
                    DraftSync::IfCleanOrMatches(sent)
                } else {
                    DraftSync::IfClean
                };
                show_state(&panel, &latest, draft_sync);
                if switching {
                    panel.set_busy(false);
                }
                if let Some(msg) = status_override {
                    panel.set_status(msg.into());
                } else if switching {
                    panel.set_choosing_target(false);
                    panel.set_scope_picker_open(false);
                    panel.set_status(if switching_pick {
                        "请在屏幕上点选目标窗口…".into()
                    } else {
                        canvas_status(&latest).into()
                    });
                } else if drawing || configuring {
                    panel.set_status(canvas_status(&latest).into());
                } else {
                    panel.set_status(canvas_status(&latest).into());
                }
            }
            Err(error) => {
                panel.set_busy(false);
                panel.set_status(error.into());
            }
        }).map_err(|e| e.to_string())?;
    }
}

fn canvas_status(state: &Value) -> String {
    for key in ["pick_error", "visual_error"] {
        if let Some(error) = state[key].as_str().filter(|error| !error.is_empty()) {
            return friendly_error(error);
        }
    }
    if state["calibration_busy"] == true { return "正在校准…".to_owned(); }
    if state["calibration_result"] == "saved" { return "校准已保存".to_owned(); }
    match state["canvas_mode"].as_str() {
        Some("once") => "在目标窗口拖出一个遮盖区域（Esc 取消）。".to_owned(),
        Some("free") => "可连续框选区域。点击「结束」或按 Esc 退出。".to_owned(),
        Some("cover_draft") => "在目标窗口拖动遮盖到合适位置后点击「保存遮盖位置」（Esc 取消）。".to_owned(),
        _ => String::new(),
    }
}

fn send_action(
    view: &slint::Weak<Panel>,
    channel: &mpsc::Sender<Action>,
    blocking: bool,
    action: impl FnOnce(String) -> Action,
) {
    let Some(panel) = view.upgrade() else { return };
    if panel.get_busy() { return; }
    let engine_pid = panel.get_engine_pid();
    if engine_pid > 0 { unsafe { AllowSetForegroundWindow(engine_pid as u32); } }
    if blocking {
        panel.set_busy(true);
    }
    if channel.send(action(panel.get_draft().into())).is_err() {
        if blocking {
            panel.set_busy(false);
        }
        panel.set_status("引擎连接已断开".into());
    }
}

fn sync_color_picker_ui(panel: &Panel, h: f32, s: f32, v: f32) -> String {
    let h = h.clamp(0.0, 360.0);
    let s = s.clamp(0.0, 100.0);
    let v = v.clamp(0.0, 100.0);
    let (r, g, b) = hsv_to_rgb(h, s, v);
    let (hr, hg, hb) = hsv_to_rgb(h, 100.0, 100.0);
    let hex = rgb_to_hex(r, g, b);
    panel.set_color_picker_h(h);
    panel.set_color_picker_s(s);
    panel.set_color_picker_v(v);
    panel.set_color_picker_hex(hex.clone().into());
    panel.set_color_picker_preview(slint::Color::from_rgb_u8(r, g, b));
    panel.set_color_picker_hue_color(slint::Color::from_rgb_u8(hr, hg, hb));
    hex
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let panel = Panel::new()?;
    let initial_mode = load_theme_mode();
    panel.set_theme_mode(initial_mode);
    panel.set_dark(resolve_dark_for_mode(initial_mode, system_prefers_dark()));
    if let Some(recent) = load_recent_colors() {
        apply_recent_colors(&panel, &recent);
    }
    let theme_view = panel.as_weak();
    panel.on_cycle_theme(move || {
        let Some(p) = theme_view.upgrade() else { return };
        let next_mode = (p.get_theme_mode() + 1) % 3;
        save_theme_mode(next_mode);
        let sys_dark = system_prefers_dark();
        let dark = resolve_dark_for_mode(next_mode, sys_dark);
        p.set_theme_mode(next_mode);
        p.set_dark(dark);
        let msg = match next_mode {
            0 => if sys_dark { "主题：跟随系统（当前深色）" } else { "主题：跟随系统（当前浅色）" },
            1 => "主题：浅色",
            _ => "主题：深色",
        };
        p.set_status(msg.into());
    });
    let theme_watch_timer = Rc::new(Timer::default());
    if !theme_no_preferences() {
        let watch_view = panel.as_weak();
        theme_watch_timer.start(TimerMode::Repeated, Duration::from_secs(2), move || {
            let Some(p) = watch_view.upgrade() else { return };
            if p.get_theme_mode() == 0 {
                let sys_dark = system_prefers_dark();
                if p.get_dark() != sys_dark {
                    p.set_dark(sys_dark);
                }
            }
        });
    }
    let Some((path, token)) = argument("--pipe").zip(argument("--session")) else {
        panel.set_busy(false);
        panel.set_status("请从 ItsOverlay 主程序打开面板。".into());
        panel.run()?;
        return Ok(());
    };
    let (tx, rx) = mpsc::channel();
    panel.on_selector_bounds_changed(|right_dip| {
        SELECTOR_RIGHT_DIP.store(right_dip, Ordering::Relaxed);
    });
    let panel_hwnd = std::sync::Arc::new(AtomicUsize::new(0));
    let minimize_hwnd = panel_hwnd.clone();
    panel.on_minimize(move || {
        let hwnd = minimize_hwnd.load(Ordering::Relaxed);
        if hwnd != 0 {
            unsafe { ShowWindow(hwnd as *mut c_void, 6); }
        }
    });
    let draft_timer = Rc::new(Timer::default());
    let preview_timer = Rc::new(Timer::default());
    let pending_preview: Rc<RefCell<Option<(String, String)>>> = Rc::new(RefCell::new(None));
    let preview_last = Rc::new(RefCell::new(std::time::Instant::now() - Duration::from_secs(1)));

    let (choose, apply, finish) = (panel.as_weak(), panel.as_weak(), panel.as_weak());
    let (select_tx, apply_tx) = (tx.clone(), tx.clone());
    let (open_tx, target_tx) = (tx.clone(), tx.clone());
    let (open_view, target_view) = (panel.as_weak(), panel.as_weak());
    let (canvas_view, canvas_tx) = (panel.as_weak(), tx.clone());
    panel.on_open_targets(move || send_action(&open_view, &open_tx, true, |_| Action::ListTargets(TargetMenu::Select)));
    let (manage_view, manage_tx) = (panel.as_weak(), tx.clone());
    panel.on_manage_targets(move || send_action(&manage_view, &manage_tx, true, |_| Action::ListTargets(TargetMenu::Manage)));
    panel.on_choose_target(move |i| send_action(&target_view, &target_tx, true, |draft| Action::SwitchTarget(i as usize, draft)));
    let (select_app_view, select_app_tx) = (panel.as_weak(), tx.clone());
    panel.on_select_managed_app(move |idx| {
        send_action(&select_app_view, &select_app_tx, false, |_| Action::SelectManagedApp(idx.max(0) as usize));
    });
    let (mgr_action_view, mgr_action_tx) = (panel.as_weak(), tx.clone());
    panel.on_manager_action(move |kind, item_idx| {
        let app_idx = mgr_action_view.upgrade().map_or(0, |p| p.get_managed_app_index().max(0) as usize);
        let kind: String = kind.into();
        send_action(&mgr_action_view, &mgr_action_tx, true, move |draft| {
            Action::ManagerCommand(kind, item_idx, app_idx, draft)
        });
    });
    let open_picker_view = panel.as_weak();
    panel.on_open_color_picker(move |key, title, hex, eyedropper_key| {
        let Some(p) = open_picker_view.upgrade() else { return };
        let (r, g, b) = parse_hex_rgb(hex.as_str()).unwrap_or((255, 255, 255));
        let (h, s, v) = rgb_to_hsv(r, g, b);
        p.set_color_picker_key(key);
        p.set_color_picker_title(title);
        p.set_color_picker_eyedropper_key(eyedropper_key);
        sync_color_picker_ui(&p, h, s, v);
        p.set_color_picker_open(true);
    });
    let preview_hsv_view = panel.as_weak();
    panel.on_preview_color_hsv(move |h, s, v| {
        let Some(p) = preview_hsv_view.upgrade() else { return };
        let hex = sync_color_picker_ui(&p, h, s, v);
        let key = p.get_color_picker_key();
        if !key.is_empty() {
            p.invoke_preview_option(key, hex.into());
        }
    });
    let commit_hsv_view = panel.as_weak();
    panel.on_commit_color_hsv(move |h, s, v| {
        let Some(p) = commit_hsv_view.upgrade() else { return };
        let hex = sync_color_picker_ui(&p, h, s, v);
        push_recent_color(&p, &hex);
        let key = p.get_color_picker_key();
        if !key.is_empty() {
            p.invoke_set_option(key, hex.into(), -1);
        }
    });
    let select_hex_view = panel.as_weak();
    panel.on_select_color_hex(move |raw_hex| {
        let Some(p) = select_hex_view.upgrade() else { return };
        let Some((r, g, b)) = parse_hex_rgb(raw_hex.as_str()) else {
            p.set_status("请输入有效的 6 位十六进制颜色（如 #2AB89D）".into());
            return;
        };
        let (h, s, v) = rgb_to_hsv(r, g, b);
        let hex = sync_color_picker_ui(&p, h, s, v);
        push_recent_color(&p, &hex);
        let key = p.get_color_picker_key();
        if !key.is_empty() {
            p.invoke_set_option(key, hex.into(), -1);
        }
    });
    panel.on_set_canvas_mode(move |mode| send_action(&canvas_view, &canvas_tx, false, |draft| Action::SetCanvasMode(mode.into(), draft)));
    let (page_view, page_tx) = (panel.as_weak(), tx.clone());
    panel.on_set_page(move |page| send_action(&page_view, &page_tx, false, |_| Action::Page(page)));
    let (option_view, option_tx) = (panel.as_weak(), tx.clone());
    let cancel_preview = preview_timer.clone();
    let cancel_preview_slot = pending_preview.clone();
    panel.on_set_option(move |key, value, choice| {
        cancel_preview.stop();
        *cancel_preview_slot.borrow_mut() = None;
        if let Some(p) = option_view.upgrade() {
            if matches!(key.as_str(), "color" | "background" | "outline_color" | "glow_color" | "border_color" | "bg_gradient_color" | "shadow_color" | "shimmer_color") {
                push_recent_color(&p, value.as_str());
                if p.get_color_picker_open() && p.get_color_picker_key() == key {
                    if let Some((r, g, b)) = parse_hex_rgb(value.as_str()) {
                        let (h, s, v) = rgb_to_hsv(r, g, b);
                        sync_color_picker_ui(&p, h, s, v);
                    }
                }
            }
        }
        let blocking = key == "scope_window";
        send_action(&option_view, &option_tx, blocking, |draft| {
            if key == "scope_window" { Action::ListTargets(TargetMenu::Scope) }
            else { Action::Option(key.into(), value.into(), choice, draft) }
        });
    });
    let preview_view = panel.as_weak();
    let preview_tx = tx.clone();
    let preview_timer_handle = preview_timer.clone();
    let preview_slot = pending_preview.clone();
    let preview_last_handle = preview_last.clone();
    panel.on_preview_option(move |key, value| {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(*preview_last_handle.borrow());
        if elapsed >= Duration::from_millis(60) {
            preview_timer_handle.stop();
            *preview_slot.borrow_mut() = None;
            *preview_last_handle.borrow_mut() = now;
            let key: String = key.into();
            let value: String = value.into();
            send_action(&preview_view, &preview_tx, false, |draft| {
                Action::PreviewOption(key, value, draft)
            });
            return;
        }
        *preview_slot.borrow_mut() = Some((key.into(), value.into()));
        let fire_view = preview_view.clone();
        let fire_tx = preview_tx.clone();
        let fire_slot = preview_slot.clone();
        let fire_last = preview_last_handle.clone();
        let remaining = Duration::from_millis(60).saturating_sub(elapsed);
        preview_timer_handle.start(TimerMode::SingleShot, remaining, move || {
            let Some((k, v)) = fire_slot.borrow_mut().take() else { return };
            *fire_last.borrow_mut() = std::time::Instant::now();
            send_action(&fire_view, &fire_tx, false, |draft| Action::PreviewOption(k, v, draft));
        });
    });
    let draft_view = panel.as_weak();
    let draft_tx = tx.clone();
    let draft_timer_handle = draft_timer.clone();
    panel.on_draft_edited(move |_value| {
        let fire_view = draft_view.clone();
        let fire_tx = draft_tx.clone();
        draft_timer_handle.start(TimerMode::SingleShot, Duration::from_millis(250), move || {
            let Some(p) = fire_view.upgrade() else { return };
            if p.get_busy() || p.get_draft() == p.get_loaded_text() { return; }
            send_action(&fire_view, &fire_tx, false, Action::Apply);
        });
    });
    let (copy_view, copy_tx) = (panel.as_weak(), tx.clone());
    panel.on_copy_style(move || send_action(&copy_view, &copy_tx, false, |_| Action::CopyStyle));
    let (paste_view, paste_tx) = (panel.as_weak(), tx.clone());
    panel.on_paste_style(move || send_action(&paste_view, &paste_tx, false, Action::PasteStyle));
    let (hide_view, hide_tx) = (panel.as_weak(), tx.clone());
    panel.on_toggle_region_hidden(move |idx| {
        send_action(&hide_view, &hide_tx, false, |draft| Action::ToggleHidden(idx.max(0) as usize, draft));
    });
    let (dup_view, dup_tx) = (panel.as_weak(), tx.clone());
    panel.on_duplicate_region(move || send_action(&dup_view, &dup_tx, false, Action::DuplicateRegion));
    let (undo_view, undo_tx) = (panel.as_weak(), tx.clone());
    panel.on_undo(move || send_action(&undo_view, &undo_tx, false, |_| Action::Undo));
    let (redo_view, redo_tx) = (panel.as_weak(), tx.clone());
    panel.on_redo(move || send_action(&redo_view, &redo_tx, false, |_| Action::Redo));
    let (hk_view, hk_tx) = (panel.as_weak(), tx.clone());
    panel.on_set_hotkey(move |spec| {
        let spec: String = spec.into();
        send_action(&hk_view, &hk_tx, false, move |_| Action::SetHotkey(spec));
    });
    let (st_view, st_tx) = (panel.as_weak(), tx.clone());
    panel.on_toggle_startup(move || send_action(&st_view, &st_tx, false, |_| Action::ToggleStartup));
    let (cfg_view, cfg_tx) = (panel.as_weak(), tx.clone());
    panel.on_open_config_dir(move || send_action(&cfg_view, &cfg_tx, false, |_| Action::OpenConfigDir));
    let theme_set_view = panel.as_weak();
    panel.on_set_theme_mode(move |mode| {
        let Some(p) = theme_set_view.upgrade() else { return };
        let next_mode = mode.clamp(0, 2);
        save_theme_mode(next_mode);
        let sys_dark = system_prefers_dark();
        p.set_theme_mode(next_mode);
        p.set_dark(resolve_dark_for_mode(next_mode, sys_dark));
    });
    panel.on_open_repo_url(move || {
        let op = to_wide("open");
        let url = to_wide("https://github.com/Natsuaki-forever/ItsOverlay");
        unsafe {
            ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), url.as_ptr(), std::ptr::null(), std::ptr::null(), 1);
        }
    });
    panel.on_choose(move |i| send_action(&choose, &select_tx, false, |draft| Action::Select(i as usize, draft)));
    let apply_timer = draft_timer.clone();
    panel.on_apply(move || {
        apply_timer.stop();
        send_action(&apply, &apply_tx, false, Action::Apply);
    });
    let finish_timer = draft_timer.clone();
    let finish_hwnd = panel_hwnd.clone();
    panel.on_finish(move || {
        finish_timer.stop();
        save_panel_bounds(finish_hwnd.load(Ordering::Relaxed));
        send_action(&finish, &tx, true, Action::Finish);
    });
    let closing = panel.as_weak();
    panel.window().on_close_requested(move || {
        if let Some(panel) = closing.upgrade() { panel.invoke_finish(); }
        CloseRequestResponse::KeepWindowShown
    });
    panel.show()?;
    let view = panel.as_weak();
    slint::Timer::single_shot(Duration::from_millis(25), move || {
        let Some(panel) = view.upgrade() else { return };
        let handle = panel.window().window_handle();
        let Ok(RawWindowHandle::Win32(handle)) = handle.window_handle().map(|h| h.as_raw()) else {
            panel.set_status("没有取得 Windows 原生窗口句柄。".into());
            panel.window().on_close_requested(|| {
                let _ = slint::quit_event_loop();
                CloseRequestResponse::HideWindow
            });
            return;
        };
        let hwnd = handle.hwnd.get() as usize;
        panel_hwnd.store(hwnd, Ordering::Relaxed);
        install_panel_frame(hwnd);
        if let Some((x, y, w, h)) = load_panel_bounds() {
            let rect = WindowRect { left: x, top: y, right: x + w, bottom: y + h };
            unsafe {
                if !MonitorFromRect(&rect, 0).is_null() {
                    SetWindowPos(hwnd as *mut c_void, std::ptr::null_mut(), x, y, w, h, 0x0014);
                    apply_panel_corners(hwnd as *mut c_void);
                }
            }
        }
        thread::spawn(move || {
            let error_view = view.clone();
            if let Err(error) = work(hwnd, path, token, rx, view) {
                let _ = error_view.upgrade_in_event_loop(move |panel| {
                    panel.set_busy(false);
                    panel.set_status(error.into());
                    if !panel.window().is_visible() {
                        let _ = slint::quit_event_loop();
                        return;
                    }
                    panel.on_finish(|| { let _ = slint::quit_event_loop(); });
                });
            }
        });
    });
    slint::run_event_loop_until_quit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_app_manager_models, build_duplicate_region_payload, diff_region_values,
        extract_style_values, format_panel_bounds, format_recent_colors_csv, friendly_error,
        hsv_to_rgb, parse_hex_rgb, parse_panel_bounds, parse_recent_colors_csv, push_undo,
        resolve_dark_for_mode, rgb_to_hex, rgb_to_hsv, target_choices, TargetMenu, UndoEntry,
    };
    use serde_json::json;

    #[test]
    fn maps_known_engine_error_codes() {
        assert_eq!(friendly_error("target_unavailable"), "目标窗口当前不可用或已最小化");
        assert_eq!(friendly_error("tracking_not_found"), "未在目标窗口中找到参考内容，请检查或重新框选");
        assert_eq!(friendly_error("stale_revision"), "状态已更新，正在重新同步…");
    }

    #[test]
    fn formats_unknown_snake_case_error_code_and_preserves_localized_text() {
        assert_eq!(friendly_error("custom_engine_fault"), "操作未能完成：custom_engine_fault");
        assert_eq!(friendly_error("区域已经不存在"), "区域已经不存在");
        assert_eq!(friendly_error(""), "");
    }

    #[test]
    fn round_trips_hex_and_hsv_colors() {
        for hex in ["#000000", "#ffffff", "#c9ee46", "#2ab89d", "#58a6ff", "#ff5c6c"] {
            let (r, g, b) = parse_hex_rgb(hex).expect("valid hex");
            let (h, s, v) = rgb_to_hsv(r, g, b);
            let (r2, g2, b2) = hsv_to_rgb(h, s, v);
            assert!((r as i16 - r2 as i16).abs() <= 1, "{hex}: {r} vs {r2}");
            assert!((g as i16 - g2 as i16).abs() <= 1, "{hex}: {g} vs {g2}");
            assert!((b as i16 - b2 as i16).abs() <= 1, "{hex}: {b} vs {b2}");
            assert_eq!(rgb_to_hex(r, g, b), hex);
        }
    }

    #[test]
    fn builds_clean_quick_switch_and_app_manager_models() {
        let reply = json!({
            "active_application": "app-1",
            "applications": [
                {
                    "id": "app-1",
                    "name": "ChatGPT",
                    "executable": "ChatGPT.exe",
                    "windows": [
                        {"index": 1, "title": "主会话", "bound": true, "profile": "prof-a"},
                        {"index": 2, "title": "副窗口", "bound": false, "profile": ""}
                    ],
                    "profiles": [
                        {"id": "prof-saved", "title": "旧窗口配置", "deleting": false}
                    ]
                },
                {
                    "id": "app-2",
                    "name": "Notepad",
                    "executable": "notepad.exe",
                    "windows": [
                        {"index": 1, "title": "笔记.txt", "bound": false, "profile": ""}
                    ],
                    "profiles": [
                        {"id": "prof-note", "title": "归档笔记", "deleting": false}
                    ]
                }
            ],
            "deletable_applications": [
                {"id": "app-1", "name": "ChatGPT", "executable": "ChatGPT.exe"},
                {"id": "app-2", "name": "Notepad", "executable": "notepad.exe"}
            ]
        });
        let quick = target_choices(&reply, TargetMenu::Select);
        let kinds: Vec<&str> = quick.iter().map(|(r, _)| r.kind.as_str()).collect();
        assert_eq!(kinds, vec!["app", "window", "window", "app-direct", "add", "manage"]);

        let scope = target_choices(&reply, TargetMenu::Scope);
        assert_eq!(scope.len(), 2);
        assert_eq!(scope[0].0.title.as_str(), "主会话");

        let (apps, selected_idx, windows, profiles) = build_app_manager_models(&reply, None, None);
        assert_eq!(apps.len(), 2);
        assert_eq!(selected_idx, 0);
        assert!(apps[0].active);
        assert!(apps[0].can_detach);
        assert!(apps[0].can_delete);
        assert_eq!(windows.len(), 2);
        assert!(windows[0].has_profile);
        assert!(!windows[1].has_profile);
        assert_eq!(profiles.len(), 1);
        assert!(profiles[0].can_apply);
        assert!(profiles[0].can_delete);
    }

    #[test]
    fn parses_and_formats_recent_colors_and_bounds() {
        let parsed = parse_recent_colors_csv(" #C9EE46 , invalid , #2ab89d , #c9ee46 , #112233 ");
        assert_eq!(parsed, vec!["#c9ee46", "#2ab89d", "#112233"]);
        assert_eq!(format_recent_colors_csv(&parsed), "#c9ee46,#2ab89d,#112233");

        assert_eq!(parse_panel_bounds("120, 80, 976, 577"), Some((120, 80, 976, 577)));
        assert_eq!(parse_panel_bounds("0,0,500,500"), None);
        assert_eq!(parse_panel_bounds("bad"), None);
        assert_eq!(format_panel_bounds(120, 80, 976, 577), "120,80,976,577");
    }

    #[test]
    fn extracts_style_and_tracks_undo_stack() {
        let before = json!({
            "id": "r1",
            "text": "old",
            "font": "Microsoft YaHei UI",
            "size": 14,
            "color": "#ffffff",
            "glow": false
        });
        let after = json!({
            "id": "r1",
            "text": "old",
            "font": "Microsoft YaHei UI",
            "size": 18,
            "color": "#c9ee46",
            "glow": true
        });
        let style = extract_style_values(&after);
        assert_eq!(style.get("size"), Some(&json!(18)));
        assert_eq!(style.get("color"), Some(&json!("#c9ee46")));
        assert!(!style.contains_key("text"));

        let (b_map, a_map) = diff_region_values(&before, &after, &["size", "color", "font"]).unwrap();
        assert_eq!(b_map.len(), 2);
        assert_eq!(a_map.get("color"), Some(&json!("#c9ee46")));

        let mut undo = Vec::new();
        let mut redo = vec![UndoEntry {
            region_id: "r1".into(),
            before: b_map.clone(),
            after: a_map.clone(),
        }];
        push_undo(&mut undo, &mut redo, UndoEntry {
            region_id: "r1".into(),
            before: b_map,
            after: a_map,
        });
        assert_eq!(undo.len(), 1);
        assert!(redo.is_empty());
    }

    #[test]
    fn builds_duplicate_region_payload_for_anchor_and_visual() {
        let anchor_region = json!({
            "id": "abc",
            "text": "遮盖A",
            "x": 100.0,
            "y": 50.0,
            "width": 160.0,
            "height": 40.0,
            "position_mode": "anchor",
            "hidden": true,
            "window_profile": "prof1",
            "color": "#ffffff"
        });
        let dup = build_duplicate_region_payload(&anchor_region, None);
        assert!(dup.get("id").is_none());
        assert!(dup.get("window_profile").is_none());
        assert_eq!(dup["x"], json!(120.0));
        assert_eq!(dup["y"], json!(70.0));
        assert_eq!(dup["hidden"], json!(false));
        assert_eq!(dup["position_mode"], json!("anchor"));
        assert_eq!(dup["color"], json!("#ffffff"));

        let visual_region = json!({
            "id": "vis",
            "text": "视觉B",
            "x": 10.0,
            "y": 10.0,
            "width": 100.0,
            "height": 30.0,
            "position_mode": "visual",
            "anchor_x": "right",
            "anchor_y": "bottom",
            "reference_width": 800,
            "reference_height": 600,
            "visual_binding": {"version": 1}
        });
        let eff = json!({"x": 300.0, "y": 240.0, "width": 180.0, "height": 44.0});
        let dup_vis = build_duplicate_region_payload(&visual_region, Some(&eff));
        assert_eq!(dup_vis["position_mode"], json!("anchor"));
        assert_eq!(dup_vis["visual_binding"], json!(null));
        assert_eq!(dup_vis["x"], json!(320.0));
        assert_eq!(dup_vis["y"], json!(260.0));
        assert_eq!(dup_vis["width"], json!(180.0));
        assert_eq!(dup_vis["height"], json!(44.0));
        assert_eq!(dup_vis["anchor_x"], json!("left"));
        assert_eq!(dup_vis["anchor_y"], json!("top"));
        assert_eq!(dup_vis["reference_width"], json!(0));
    }

    #[test]
    fn resolves_three_state_theme_mode() {
        assert!(!resolve_dark_for_mode(0, false));
        assert!(resolve_dark_for_mode(0, true));
        assert!(!resolve_dark_for_mode(1, true));
        assert!(resolve_dark_for_mode(2, false));
    }
}
