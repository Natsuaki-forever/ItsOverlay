use serde_json::{json, Value};
use slint::{Color, ModelRc, SharedString, VecModel};
use crate::Setting;

const ON_OFF: &[(&str, &str)] = &[("关闭", "false"), ("开启", "true")];
const ALIGN: &[(&str, &str)] = &[("左对齐", "left"), ("居中", "center"), ("右对齐", "right")];
const ANCHOR_X: &[(&str, &str)] = &[("跟随左侧", "left"), ("跟随中心", "center"), ("跟随右侧", "right")];
const ANCHOR_Y: &[(&str, &str)] = &[("跟随顶部", "top"), ("跟随中心", "center"), ("跟随底部", "bottom")];
const DIRECTION: &[(&str, &str)] = &[("向左", "left"), ("向右", "right")];
const INTERVAL: &[(&str, &str)] = &[("只随窗口变化", "0"), ("每 15 秒", "15"), ("每 30 秒", "30"), ("每 60 秒", "60")];
const RELATION_MODE: &[(&str, &str)] = &[("固定大小", "position_only"), ("跟随参考框边界", "follow_bounds")];

struct Field {
    page: i32,
    key: &'static str,
    label: &'static str,
    options: &'static [(&'static str, &'static str)],
}

macro_rules! field {
    ($page:expr, $key:expr, $label:expr) => { field!($page, $key, $label, &[]) };
    ($page:expr, $key:expr, $label:expr, $choices:expr) => { Field {page: $page, key: $key, label: $label, options: $choices} };
}

const FIELDS: &[Field] = &[
    field!(0,"font","字体"), field!(0,"size","字号"), field!(0,"bold","粗体",ON_OFF),
    field!(0,"align","对齐",ALIGN), field!(0,"padding","文字边距"), field!(0,"letter_spacing","字距"),
    field!(0,"color","文字颜色"), field!(0,"opacity","不透明度"), field!(0,"background","背景颜色"),
    field!(0,"corner_radius","圆角半径"),
    field!(0,"border","卡片边框",ON_OFF), field!(0,"border_width","边框粗细"), field!(0,"border_color","边框颜色"),
    field!(1,"x","X"), field!(1,"y","Y"), field!(1,"width","宽度"), field!(1,"height","高度"),
    field!(1,"anchor_x","横向定位",ANCHOR_X), field!(1,"anchor_y","纵向定位",ANCHOR_Y),
    field!(2,"scroll","循环滚动",ON_OFF), field!(2,"scroll_direction","滚动方向",DIRECTION),
    field!(2,"scroll_speed","滚动速度"), field!(2,"scroll_gap","循环间距"),
    field!(2,"scroll_feather","两侧渐隐",ON_OFF), field!(2,"scroll_feather_width","羽化宽度"),
    field!(2,"outline","文字描边",ON_OFF), field!(2,"outline_width","描边宽度"), field!(2,"outline_color","描边颜色"),
    field!(2,"glow","霓虹发光",ON_OFF), field!(2,"glow_color","发光颜色"), field!(2,"glow_radius","发光半径"),
    field!(2,"glow_strength","发光强度"), field!(2,"glow_pulse","呼吸效果",ON_OFF), field!(2,"glow_period","呼吸周期"),
    field!(2,"scroll_fill","无缝铺满",ON_OFF),
    field!(2,"bg_gradient","背景渐变",ON_OFF), field!(2,"bg_gradient_color","渐变副色"), field!(2,"bg_gradient_angle","渐变角度"),
    field!(2,"shadow","文字投影",ON_OFF), field!(2,"shadow_color","投影颜色"),
    field!(2,"shadow_offset_x","水平偏移"), field!(2,"shadow_offset_y","垂直偏移"),
    field!(2,"shimmer","流光扫光",ON_OFF), field!(2,"shimmer_color","流光颜色"),
    field!(2,"shimmer_period","扫光周期"), field!(2,"shimmer_strength","流光强度"),
];

fn row(key: &str, label: &str, value: &Value, options: &[(&str, &str)]) -> Setting {
    let value = match value {
        Value::String(string) => string.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let font = key == "font";
    let (choices, choice) = if font {
        let mut names: Vec<SharedString> = crate::fonts::families().iter().map(|name| name.as_str().into()).collect();
        let selected = names.iter().position(|name| name.as_str() == value).unwrap_or_else(|| {
            names.insert(0, value.as_str().into());
            0
        });
        (names, selected as i32)
    } else {
        (options.iter().map(|(label, _)| SharedString::from(*label)).collect(),
            options.iter().position(|(_, item)| *item == value).map_or(-1, |i| i as i32))
    };
    let swatch = matches!(key, "background" | "color" | "outline_color" | "glow_color" | "border_color" | "bg_gradient_color" | "shadow_color" | "shimmer_color")
        .then(|| value.strip_prefix('#'))
        .flatten()
        .filter(|hex| hex.len() == 6)
        .and_then(|hex| u32::from_str_radix(hex, 16).ok());
    let (minimum, maximum, step) = match key {
        "opacity" => (0., 255., 1.),
        "scroll_feather_width" => (1., 200., 1.),
        "scroll_speed" => (5., 300., 1.),
        "scroll_gap" => (8., 300., 1.),
        "outline_width" => (0.5, 12., 0.1),
        "glow_radius" => (1., 24., 1.),
        "glow_strength" => (0., 100., 1.),
        "glow_period" => (0.5, 10., 0.1),
        "corner_radius" => (0., 64., 1.),
        "border_width" => (0.5, 12., 0.5),
        "bg_gradient_angle" => (0., 360., 5.),
        "shadow_offset_x" => (-12., 12., 1.),
        "shadow_offset_y" => (-12., 12., 1.),
        "shimmer_period" => (0.5, 10., 0.1),
        "shimmer_strength" => (0., 100., 1.),
        _ => (0., 0., 1.),
    };
    let number = value.parse().unwrap_or(minimum);
    Setting {
        key: key.into(),
        label: label.into(),
        choice,
        value: value.into(),
        choices: ModelRc::new(VecModel::from(choices)),
        action: false,
        swatch: swatch.map_or(Color::default(), |rgb| {
            Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
        }),
        has_swatch: swatch.is_some(),
        toggle: options == ON_OFF,
        number,
        minimum,
        maximum,
        step,
        geometry_pair: -1,
        geometry_tail: false,
        section: if key.starts_with("reference.") { 2 }
            else if key.starts_with("relation.") || matches!(key, "begin_cover_drag" | "commit_cover_drag") { 1 }
            else { 0 },
    }
}

fn coordinate_fields(rows: &mut Vec<Setting>, key: &str, labels: &[&str], values: &Value) {
    if !values.is_array() { return; }
    let start = rows.len();
    for (i, label) in labels.iter().enumerate() {
        let mut entry = row(&format!("{key}.{i}"), label, &values[i], &[]);
        if let Some(number) = values[i].as_f64() {
            entry.value = format!("{number:.2}").trim_end_matches('0').trim_end_matches('.').into();
        }
        if i % 2 == 0 && i + 1 < labels.len() {
            entry.geometry_pair = (start + i + 1) as i32;
        } else if i % 2 == 1 {
            entry.geometry_tail = true;
        }
        rows.push(entry);
    }
}

fn geometry_value(current: &Value, text: &str, index: i32, minimum: Option<f64>) -> Result<Value, String> {
    let value = match index {
        -2 | -3 => current.as_f64().ok_or("无法读取当前坐标")? + if index == -2 { -1. } else { 1. },
        _ => text.trim().parse::<f64>().map_err(|_| "请输入有效数字")?,
    };
    if !value.is_finite() { return Err("请输入有限数值".into()); }
    Ok(json!(minimum.map_or(value, |min| value.max(min))))
}

pub fn settings(state: &Value, region: &Value, page: i32) -> ModelRc<Setting> {
    let page = page.clamp(0, 2);
    let mut rows = Vec::new();
    let id = region["id"].as_str().unwrap_or("");
    let visual = region["position_mode"] == "visual";
    let preview = &state["calibration_preview"];
    if !id.is_empty() {
        for field in FIELDS.iter().filter(|field| {
            field.page == page && (page != 1 || (!visual && !preview.is_object()))
        }) {
            let value = state["effective_positions"][id].get(field.key).unwrap_or(&region[field.key]);
            rows.push(row(field.key, field.label, value, field.options));
        }
        if page == 1 && (visual || preview.is_object()) {
            let binding = &region["visual_binding"];
            rows.push(row("interval", "兜底检查", &state["visual_check_interval_seconds"], INTERVAL));
            let relation = if preview.is_object() { &preview["relation"] } else { &binding["relation"] };
            if relation.is_object() && (visual || preview["reference"].is_array()) {
                let kind = if preview.is_object() { &preview["tracking_kind"] } else { &binding["tracking_kind"] };
                if kind != "features" { rows.push(row("relation.mode", "遮盖跟随", &relation["mode"], RELATION_MODE)); }
                if relation["mode"] == "position_only" {
                    rows.push(row("relation.anchor_x", "水平基准", &relation["anchor_x"], ANCHOR_X));
                    rows.push(row("relation.anchor_y", "垂直基准", &relation["anchor_y"], ANCHOR_Y));
                    coordinate_fields(&mut rows, "relation.offset_dip", &["X 偏移","Y 偏移"], &relation["offset_dip"]);
                    coordinate_fields(&mut rows, "relation.fixed_size_dip", &["遮盖宽","遮盖高"], &relation["fixed_size_dip"]);
                } else {
                    coordinate_fields(&mut rows, "relation.margins_dip", &["左边距","上边距","右边距","下边距"], &relation["margins_dip"]);
                }
                rows.push(row("relation.padding_physical", "防漏边像素", &relation["padding_physical"], &[]));
            }
            let reference = if preview.is_object() { &preview["reference"] } else { &binding["reference_rect_dip"] };
            if reference.is_array() {
                coordinate_fields(&mut rows, "reference", &["参考 X","参考 Y","参考宽","参考高"], reference);
            }
        }
    }
    ModelRc::new(VecModel::from(rows))
}

fn scalar(text: &str, old: &Value) -> Result<Value, String> {
    if old.is_string() { return Ok(json!(text)); }
    let value: Value = serde_json::from_str(text.trim()).map_err(|_| "请输入有效数字")?;
    if value.is_number() || value.is_boolean() { Ok(value) } else { Err("请输入有效数值".into()) }
}

fn selected_option<'a>(options: &'a [(&str, &str)], index: i32) -> Result<&'a str, String> {
    options.get(index as usize).map(|(_, value)| *value).ok_or("选项无效".into())
}

pub fn operation(state: &Value, region: &Value, key: &str, text: &str, index: i32) -> Result<(&'static str, Value), String> {
    let id = &region["id"];
    let preview = &state["calibration_preview"];
    let binding = &region["visual_binding"];
    let token = &preview["token"];
    if let Some(field) = FIELDS.iter().find(|field| field.key == key) {
        let value = if matches!(key, "x" | "y" | "width" | "height") && matches!(index, -2 | -3) {
            let current = state["effective_positions"][id.as_str().unwrap_or_default()][key]
                .as_f64().or_else(|| region[key].as_f64()).ok_or("无法读取当前坐标")?;
            let adjusted = current + if index == -2 { -1.0 } else { 1.0 };
            json!(if matches!(key, "width" | "height") { adjusted.max(1.0) } else { adjusted })
        } else {
            scalar(if field.options.is_empty() {text} else {selected_option(field.options,index)?}, &region[key])?
        };
        return Ok(("update", json!({"id": id, "coordinate_space": "client", "values": { (key): value }})));
    }
    if let Some(part) = key.strip_prefix("reference.") {
        let position: usize = part.parse().map_err(|_|"参考框字段无效")?;
        let mut rect = if preview.is_object() { preview["reference"].clone() } else { binding["reference_rect_dip"].clone() };
        let dimension = rect.get_mut(position).ok_or("参考框字段无效")?;
        *dimension = geometry_value(dimension, text, index, (position >= 2).then_some(1.))?;
        return Ok(if preview.is_object() { ("set_calibration_reference",json!({"token":token,"rect":rect})) } else { ("recalibrate_reference",json!({"id":id,"rect":rect})) });
    }
    if let Some(part) = key.strip_prefix("relation.") {
        let mut relation = if preview.is_object() { preview["relation"].clone() } else { binding["relation"].clone() };
        if let Some((field, position)) = part.split_once('.') {
            let position: usize = position.parse().map_err(|_|"遮盖关系无效")?;
            let dimension = relation[field].get_mut(position).ok_or("遮盖关系无效")?;
            *dimension = geometry_value(dimension, text, index, (field == "fixed_size_dip").then_some(1.))?;
        } else {
            let value = match part {
                "mode" => json!(selected_option(RELATION_MODE,index)?),
                "anchor_x" => json!(selected_option(ANCHOR_X,index)?),
                "anchor_y" => json!(selected_option(ANCHOR_Y,index)?),
                _ => scalar(text,&json!(0))?,
            };
            relation[part] = value;
        }
        return Ok(if preview.is_object() { ("update_calibration_preview",json!({"token":token,"relation":relation})) } else { ("update_cover_relation",json!({"id":id,"relation":relation})) });
    }
    Ok(match key {
        "clear" => ("clear_regions", json!({})),
        "delete" => ("delete", json!({"id":id})),
        "pick_text" => ("pick_color",json!({"id":id,"field":"color"})),
        "pick_background" => ("pick_color",json!({"id":id,"field":"background"})),
        "scope_app" => ("set_region_scope",json!({"id":id})),
        "begin_features" => ("begin_calibration",json!({"id":id,"tracking_kind":"features","search_limit_dip":binding["search_limit_dip"].as_f64().unwrap_or(500.)})),
        "begin_boundary" => ("begin_calibration",json!({"id":id,"tracking_kind":"boundary"})),
        "confirm_calibration" => ("confirm_calibration",json!({"token":token})),
        "pick_calibration_reference" => ("pick_calibration_reference",json!({"token":token})),
        "cancel_calibration" => ("cancel_calibration",json!({"token":token})),
        "retry_calibration" => ("retry_calibration",json!({})),
        "refresh_visual" => ("refresh_visual",json!({"id":id})),
        "resume" => ("set_position_mode",json!({"id":id,"mode":"visual"})),
        "anchor" => ("set_position_mode",json!({"id":id,"mode":"anchor","use_saved_position":true})),
        "begin_cover_drag" => ("begin_cover_drag",json!({"id":id})),
        "commit_cover_drag" => ("update_cover_relation",json!({"id":id,"relation":state["cover_drag_relation"]})),
        "search" => ("set_visual_search_limit",json!({"id":id,"limit":scalar(text,&json!(0))?})),
        "interval" => ("set_visual_check_interval",json!({"seconds":scalar(selected_option(INTERVAL,index)?,&json!(0))?})),
        _ => return Err("未知设置".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::settings;
    use serde_json::json;
    use slint::Model;

    fn sample_region(mode: &str) -> serde_json::Value {
        let mut region: serde_json::Value = serde_json::from_str(r##"{
            "id": "r1",
            "text": "hello",
            "font": "Segoe UI",
            "size": 14,
            "bold": false,
            "align": "left",
            "padding": 8,
            "letter_spacing": 0.0,
            "color": "#ececec",
            "opacity": 255,
            "background": "#212121",
            "position_mode": "anchor",
            "x": 10, "y": 20, "width": 100, "height": 40,
            "anchor_x": "left", "anchor_y": "top",
            "scroll": false, "scroll_direction": "left", "scroll_speed": 40, "scroll_gap": 40,
            "scroll_feather": false, "scroll_feather_width": 24,
            "outline": false, "outline_width": 1.5, "outline_color": "#000000",
            "glow": false, "glow_color": "#2ab89d", "glow_radius": 10,
            "glow_strength": 75, "glow_pulse": true, "glow_period": 2.0,
            "scroll_fill": false, "corner_radius": 0,
            "border": false, "border_width": 1.5, "border_color": "#c9ee46",
            "bg_gradient": false, "bg_gradient_color": "#3d2f66", "bg_gradient_angle": 135,
            "shadow": false, "shadow_color": "#000000", "shadow_offset_x": 2.0, "shadow_offset_y": 2.0,
            "shimmer": false, "shimmer_color": "#ffffff", "shimmer_period": 2.5, "shimmer_strength": 45,
            "visual_binding": {
                "tracking_kind": "features",
                "search_limit_dip": 500,
                "reference_rect_dip": [10, 20, 120, 40],
                "relation": {
                    "mode": "position_only",
                    "anchor_x": "left", "anchor_y": "top",
                    "offset_dip": [0, 0], "fixed_size_dip": [100, 40],
                    "padding_physical": 2
                }
            }
        }"##).unwrap();
        region["position_mode"] = serde_json::Value::String(mode.to_owned());
        region
    }

    #[test]
    fn builds_three_balanced_pages() {
        let state = json!({"visual_check_interval_seconds": 0});
        let anchor_region = sample_region("anchor");

        let p0 = settings(&state, &anchor_region, 0);
        assert_eq!(p0.row_count(), 13);
        assert_eq!(p0.row_data(0).unwrap().key, "font");
        assert_eq!(p0.row_data(7).unwrap().key, "opacity");
        assert_eq!(p0.row_data(8).unwrap().key, "background");
        assert_eq!(p0.row_data(9).unwrap().key, "corner_radius");

        let p1_anchor = settings(&state, &anchor_region, 1);
        assert_eq!(p1_anchor.row_count(), 6);
        assert_eq!(p1_anchor.row_data(0).unwrap().key, "x");

        let visual_region = sample_region("visual");
        let p1_visual = settings(&state, &visual_region, 1);
        assert!(p1_visual.iter().any(|s| s.key == "interval" && s.section == 0));
        assert!(p1_visual.iter().any(|s| s.key.starts_with("relation.") && s.section == 1));
        assert!(p1_visual.iter().any(|s| s.key.starts_with("reference.") && s.section == 2));

        let p2 = settings(&state, &anchor_region, 2);
        assert_eq!(p2.row_count(), 27);
        assert_eq!(p2.row_data(0).unwrap().key, "scroll");
        assert_eq!(p2.row_data(6).unwrap().key, "outline");
        assert_eq!(p2.row_data(9).unwrap().key, "glow");
        assert_eq!(p2.row_data(14).unwrap().key, "glow_period");
        assert_eq!(p2.row_data(15).unwrap().key, "scroll_fill");
        assert_eq!(p2.row_data(16).unwrap().key, "bg_gradient");
        assert_eq!(p2.row_data(26).unwrap().key, "shimmer_strength");
    }
}
