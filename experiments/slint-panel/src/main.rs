#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[path = "../../editor_pipe.rs"]
mod editor_pipe;
mod fonts;
mod options;

use editor_pipe::Engine;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{json, Value};
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, SharedString, VecModel};
use std::{env, ffi::c_void, sync::mpsc, thread, time::Duration};

slint::include_modules!();

enum Action {
    Select(usize, String),
    ListTargets(TargetMenu),
    Page(i32),
    Option(String, String, i32, String),
    SwitchTarget(usize, String),
    SetCanvasMode(String, String),
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
    fn DefWindowProcW(hwnd: *mut c_void, message: u32, wparam: usize, lparam: isize) -> isize;
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

#[repr(C)]
#[derive(Default)]
struct WindowRect { left: i32, top: i32, right: i32, bottom: i32 }

type WindowProcedure = unsafe extern "system" fn(*mut c_void, u32, usize, isize, usize, usize) -> isize;

#[link(name = "comctl32")]
extern "system" {
    fn SetWindowSubclass(hwnd: *mut c_void, procedure: WindowProcedure, id: usize, data: usize) -> i32;
    fn RemoveWindowSubclass(hwnd: *mut c_void, procedure: WindowProcedure, id: usize) -> i32;
    fn DefSubclassProc(hwnd: *mut c_void, message: u32, wparam: usize, lparam: isize) -> isize;
}

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
                _ if y < px(65) && (x < px(118) || (x >= px(408) && x < w - px(92))) => 2,
                _ => 1,                   // HTCLIENT
            };
            return hit;
        }
    }
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, panel_frame, id);
    }
    let result = DefSubclassProc(hwnd, message, wparam, lparam);
    if (message == 0x0005 && wparam != 1) || message == 0x02e0 {
        apply_panel_corners(hwnd);
    }
    result
}

unsafe fn apply_panel_corners(hwnd: *mut c_void) {
    let mut bounds = WindowRect::default();
    if GetWindowRect(hwnd, &mut bounds) == 0 { return; }
    let previous = CreateRectRgn(0, 0, 0, 0);
    if previous.is_null() { return; }
    let has_region = GetWindowRgn(hwnd, previous) != 0;
    if IsZoomed(hwnd) != 0 {
        DeleteObject(previous);
        if has_region { SetWindowRgn(hwnd, std::ptr::null_mut(), 1); }
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
    DwmSetWindowAttribute(hwnd, DWMWA_NCRENDERING_POLICY, &DWMNCRP_DISABLED, 4);
    let current = GetWindowLongW(hwnd, GWL_STYLE);
    let desired = (current & !WS_CAPTION) | WS_THICKFRAME;
    if current != desired {
        SetWindowLongW(hwnd, GWL_STYLE, desired);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, KEEP_PLACEMENT | SWP_FRAMECHANGED);
    }
    apply_panel_corners(hwnd);
}

fn install_panel_frame(hwnd: usize) {
    let hwnd = hwnd as *mut c_void;
    unsafe {
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

fn checked(reply: Value) -> Result<Value, String> {
    if reply["ok"] == true {
        Ok(reply)
    } else {
        Err(reply["error"]["code"].as_str().or(reply["error"].as_str())
            .unwrap_or("引擎拒绝了操作").to_owned())
    }
}

fn selected_index(state: &Value) -> Option<usize> {
    let regions = state["regions"].as_array()?;
    regions.iter().position(|r| r["id"] == state["selected_id"])
        .or_else(|| (!regions.is_empty()).then_some(0))
}

fn show_state(panel: &Panel, state: &Value) {
    let rows = state["regions"].as_array().cloned().unwrap_or_default();
    let selected = selected_index(state);
    let names = rows.iter().map(|row| {
        let title = row["text"].as_str().unwrap_or_default().lines().next().unwrap_or_default();
        SharedString::from(if title.is_empty() { "空白区域" } else { title })
    }).collect::<Vec<_>>();
    panel.set_regions(ModelRc::new(VecModel::from(names)));
    let scopes: Vec<_> = rows.iter().map(|row| {
        !row["window_profile"].as_str().unwrap_or_default().is_empty()
    }).collect();
    let details: Vec<_> = rows.iter().zip(&scopes).map(|(row, scoped)| {
        let mode = if row["position_mode"] == "visual" { "视觉" } else { "固定" };
        let scope = if *scoped { "跟随窗口" } else { "跟随应用" };
        SharedString::from(format!("{mode} · {scope}"))
    }).collect();
    panel.set_region_scoped(ModelRc::new(VecModel::from(scopes)));
    panel.set_region_details(ModelRc::new(VecModel::from(details)));
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
    panel.set_position_status(match selected.map(|index| &rows[index]["position_mode"]) {
        Some(mode) if mode == "visual" => "视觉定位 · 跟随参考区域",
        Some(_) => "窗口锚定 · 随所选窗口边缘或中心定位",
        None => "",
    }.into());
    let text: SharedString = selected.and_then(|i| rows[i]["text"].as_str()).unwrap_or_default().into();
    panel.set_loaded_text(text.clone());
    panel.set_draft(text);
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
    panel.set_settings(options::settings(state, selected.map_or(&Value::Null, |i| &rows[i]), panel.get_page()));
    panel.set_busy(false);
}

fn target_choices(reply: &Value, menu: TargetMenu) -> Vec<(TargetRow, Value)> {
    let active = reply["active_application"].as_str().unwrap_or_default();
    let mut choices = Vec::new();
    let mut parent = -1;
    let mut add = |title: &str, group: &str, kind: &str, current: bool, payload: Value| {
        if matches!(kind, "window" | "archive") { parent = choices.len() as i32; }
        choices.push((TargetRow {
            title: title.into(), group: group.into(), kind: kind.into(), current, parent,
        }, payload));
    };
    if menu != TargetMenu::Manage {
        for app in reply["applications"].as_array().into_iter().flatten() {
            let id = app["id"].as_str().unwrap_or_default();
            if menu == TargetMenu::Scope && id != active { continue; }
            let name = app["name"].as_str().unwrap_or("未命名应用");
            let windows = app["windows"].as_array().map(Vec::as_slice).unwrap_or_default();
            let profiles = app["profiles"].as_array().map(Vec::as_slice).unwrap_or_default();
            if menu == TargetMenu::Select {
                if id != active && windows.len() <= 1 && profiles.is_empty() {
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
                if menu == TargetMenu::Select {
                    if let Some(profile) = window["profile"].as_str().filter(|p| !p.is_empty() && *p != "legacy") {
                        add("删除窗口配置…", group, "window-action", false, json!({
                            "id": id, "index": window["index"], "delete_window_profile": profile, "name": title
                        }));
                    }
                }
            }
            if menu == TargetMenu::Scope { continue; }
            if windows.is_empty() {
                add("等待窗口", id, "waiting", false, Value::Null);
            }
            for profile in profiles {
                let title = profile["title"].as_str().filter(|title| !title.is_empty()).unwrap_or("无标题窗口");
                add(title, id, "archive", false, Value::Null);
                if profile["deleting"] != true {
                    let has_bound = id == active && windows.iter().any(|window| window["bound"] == true);
                    for window in windows.iter().filter(|w| !has_bound || w["bound"] == true) {
                        let destination = window["title"].as_str().unwrap_or("无标题窗口");
                        add(&format!("关联到「{destination}」"), id, "archive-action", false, json!({
                            "id": id, "index": window["index"], "window_profile": profile["id"]
                        }));
                    }
                }
                if profile["id"] != "legacy" {
                    add("删除窗口配置…", id, "archive-delete", false, json!({
                        "id": id, "delete_window_profile": profile["id"], "name": title
                    }));
                }
            }
        }
    }
    if menu == TargetMenu::Select {
        add("添加应用", "", "add", false, json!({"pick": true}));
        add("管理应用…", "", "manage", false, Value::Null);
    }
    if menu != TargetMenu::Scope {
        if !active.is_empty() {
            add("移除当前应用（保留配置）", "", "detach", false, json!({"remove_current": true}));
        }
        for app in reply["deletable_applications"].as_array().into_iter().flatten() {
            let name = app["name"].as_str().unwrap_or("未命名应用");
            add(&format!("删除 {name} 及配置…"), "", "delete", false,
                json!({"delete_id": app["id"], "name": name, "executable": app["executable"]}));
        }
    }
    choices
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
    let mut scope_target_list = false;
    let initial_state = state.clone();
    view.upgrade_in_event_loop(move |panel| {
        show_state(&panel, &initial_state);
        panel.set_status("".into());
    }).map_err(|e| e.to_string())?;

    loop {
        let action = match actions.recv_timeout(Duration::from_millis(350)) {
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
                if ["regions", "selected_id", "canvas_mode", "target_hwnd", "effective_positions", "tracking_states", "calibration_preview", "calibration_busy", "calibration_result", "calibration_retry_available", "visual_error", "pick_error", "cover_drag_relation", "visual_check_interval_seconds", "visible"].iter().any(|key| state[*key] != incoming[*key]) {
                    let visibility_changed = state["visible"] != incoming["visible"];
                    state = incoming.clone();
                    let latest = state.clone();
                    view.upgrade_in_event_loop(move |panel| {
                        if visibility_changed { sync_visibility(&panel, hwnd, &latest); }
                        if !panel.get_busy() && panel.get_draft() == panel.get_loaded_text() {
                            show_state(&panel, &latest);
                            panel.set_status(canvas_status(&latest).into());
                        }
                    }).map_err(|e| e.to_string())?;
                }
                continue;
            }
        };
        if let Action::ListTargets(menu) = action {
            scope_target_list = menu == TargetMenu::Scope;
            let result = engine.request("list_applications", json!({}), None).and_then(checked);
            let active_group: SharedString = result.as_ref().ok().and_then(|reply|
                reply["active_application"].as_str()).unwrap_or_default().into();
            let labels = match &result {
                Ok(reply) => {
                    targets = target_choices(reply, menu);
                    targets.iter().map(|(row, _)| row.clone()).collect()
                }
                Err(_) => Vec::new(),
            };
            view.upgrade_in_event_loop(move |panel| {
                panel.set_busy(false);
                match result {
                    Ok(_) => {
                        panel.set_targets(ModelRc::new(VecModel::from(labels)));
                        panel.set_managing_targets(menu == TargetMenu::Manage);
                        panel.set_expanded_group(if menu == TargetMenu::Select { active_group } else { "".into() });
                        panel.set_expanded_action(-1);
                        panel.set_expanded_management(menu == TargetMenu::Manage);
                        panel.set_pending_target(-1);
                        panel.set_choosing_target(true);
                    }
                    Err(error) => panel.set_status(error.into()),
                }
            }).map_err(|e| e.to_string())?;
            continue;
        }
        if let Action::Page(page) = action {
            let latest = state.clone();
            view.upgrade_in_event_loop(move |panel| {
                let region = selected_index(&latest).map_or(&Value::Null, |i| &latest["regions"][i]);
                panel.set_settings(options::settings(&latest, region, page));
                panel.set_page(page);
                panel.set_busy(false);
            }).map_err(|e| e.to_string())?;
            continue;
        }
        let finishing = matches!(action, Action::Finish(_));
        let switching = matches!(action, Action::SwitchTarget(..));
        let drawing = matches!(action, Action::SetCanvasMode(..));
        let configuring = matches!(action, Action::Option(..));
        let previous_visibility = state["visible"].clone();
        let result = (|| {
            match action {
                Action::Select(index, draft) => {
                    apply_text(&mut engine, &mut state, draft)?;
                    let id = state["regions"].get(index).ok_or("区域已经不存在")?["id"].clone();
                    state = checked(engine.request("select", json!({"id": id}), None)?)?["state"].clone();
                }
                Action::Apply(draft) => apply_text(&mut engine, &mut state, draft)?,
                Action::SwitchTarget(index, draft) => {
                    let payload = targets.get(index).ok_or("请重新打开应用列表")?.1.clone();
                    apply_text(&mut engine, &mut state, draft)?;
                    if scope_target_list {
                        let index = selected_index(&state).ok_or("Select a region")?;
                        let payload = json!({"id": state["regions"][index]["id"], "index": payload["index"]});
                        state = checked(engine.request("set_region_scope",payload,state["revision"].as_u64())?)?["state"].clone();
                    } else {
                        checked(engine.request("switch_application", payload, state["revision"].as_u64())?)?;
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
                    let region = selected_index(&state).map_or(&Value::Null, |index| &state["regions"][index]);
                    let (op, payload) = options::operation(&state, region, &key, &value, choice)?;
                    state = checked(engine.request(op, payload, state["revision"].as_u64())?)?["state"].clone();
                }
                Action::ListTargets(_) | Action::Page(_) => unreachable!(),
            }
            Ok::<(), String>(())
        })();
        if finishing && result.is_ok() {
            let _ = slint::quit_event_loop();
            return Ok(());
        }
        let latest = state.clone();
        let visibility_changed = previous_visibility != latest["visible"];
        view.upgrade_in_event_loop(move |panel| match result {
            Ok(()) => {
                show_state(&panel, &latest);
                if visibility_changed { sync_visibility(&panel, hwnd, &latest); }
                if switching {
                    panel.set_choosing_target(false);
                    panel.set_status("正在切换应用或窗口…".into());
                } else if drawing || configuring {
                    panel.set_status(canvas_status(&latest).into());
                } else {
                    panel.set_status("已应用。点「完成」保存并关闭。".into());
                }
            }
            Err(error) => {
                panel.set_busy(false);
                panel.set_status(error.into());
            }
        }).map_err(|e| e.to_string())?;
    }
}

fn canvas_status(state: &Value) -> &str {
    for key in ["pick_error", "visual_error"] {
        if let Some(error) = state[key].as_str().filter(|error| !error.is_empty()) { return error; }
    }
    if state["calibration_busy"] == true { return "正在校准…"; }
    if state["calibration_result"] == "saved" { return "校准已保存"; }
    match state["canvas_mode"].as_str() {
        Some("once") => "在目标窗口拖出一个遮盖区域。",
        Some("free") => "可连续框选区域。点击「结束」或按 Esc 退出。",
        _ => "",
    }
}

fn send(view: &slint::Weak<Panel>, channel: &mpsc::Sender<Action>, action: impl FnOnce(String) -> Action) {
    let Some(panel) = view.upgrade() else { return };
    if panel.get_busy() { return; }
    let engine_pid = panel.get_engine_pid();
    if engine_pid > 0 { unsafe { AllowSetForegroundWindow(engine_pid as u32); } }
    panel.set_busy(true);
    if channel.send(action(panel.get_draft().into())).is_err() {
        panel.set_busy(false);
        panel.set_status("引擎连接已断开".into());
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let panel = Panel::new()?;
    let Some((path, token)) = argument("--pipe").zip(argument("--session")) else {
        panel.set_status("请从 ItsOverlay 主程序打开面板。".into());
        panel.run()?;
        return Ok(());
    };
    let (tx, rx) = mpsc::channel();
    let (choose, apply, finish) = (panel.as_weak(), panel.as_weak(), panel.as_weak());
    let (select_tx, apply_tx) = (tx.clone(), tx.clone());
    let (open_tx, target_tx) = (tx.clone(), tx.clone());
    let (open_view, target_view) = (panel.as_weak(), panel.as_weak());
    let (canvas_view, canvas_tx) = (panel.as_weak(), tx.clone());
    panel.on_open_targets(move || send(&open_view, &open_tx, |_| Action::ListTargets(TargetMenu::Select)));
    let (manage_view, manage_tx) = (panel.as_weak(), tx.clone());
    panel.on_manage_targets(move || send(&manage_view, &manage_tx, |_| Action::ListTargets(TargetMenu::Manage)));
    panel.on_choose_target(move |i| send(&target_view, &target_tx, |draft| Action::SwitchTarget(i as usize, draft)));
    panel.on_set_canvas_mode(move |mode| send(&canvas_view, &canvas_tx, |draft| Action::SetCanvasMode(mode.into(), draft)));
    let (page_view, page_tx) = (panel.as_weak(), tx.clone());
    panel.on_set_page(move |page| send(&page_view, &page_tx, |_| Action::Page(page)));
    let (option_view, option_tx) = (panel.as_weak(), tx.clone());
    panel.on_set_option(move |key, value, choice| {
        send(&option_view, &option_tx, |draft| {
            if key == "scope_window" { Action::ListTargets(TargetMenu::Scope) }
            else { Action::Option(key.into(), value.into(), choice, draft) }
        });
    });
    panel.on_choose(move |i| send(&choose, &select_tx, |draft| Action::Select(i as usize, draft)));
    panel.on_apply(move || send(&apply, &apply_tx, Action::Apply));
    panel.on_finish(move || send(&finish, &tx, Action::Finish));
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
        install_panel_frame(hwnd);
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
