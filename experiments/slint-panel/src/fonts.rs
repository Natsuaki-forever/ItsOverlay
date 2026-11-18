use std::{ffi::c_void, ptr::null_mut, sync::OnceLock};

#[repr(C)]
struct LogFont {
    metrics: [i32; 5],
    italic: u8,
    underline: u8,
    strike_out: u8,
    charset: u8,
    precision: [u8; 4],
    face: [u16; 32],
}

#[link(name = "user32")]
extern "system" {
    fn GetDC(window: *mut c_void) -> *mut c_void;
    fn ReleaseDC(window: *mut c_void, dc: *mut c_void) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn EnumFontFamiliesExW(
        dc: *mut c_void,
        query: *mut LogFont,
        callback: unsafe extern "system" fn(*const LogFont, *const c_void, u32, isize) -> i32,
        data: isize,
        flags: u32,
    ) -> i32;
}

unsafe extern "system" fn collect(font: *const LogFont, _: *const c_void, _: u32, data: isize) -> i32 {
    let face = &(*font).face;
    let end = face.iter().position(|&unit| unit == 0).unwrap_or(face.len());
    if end > 0 && face[0] != u16::from(b'@') {
        let families = &mut *(data as *mut Vec<String>);
        families.push(String::from_utf16_lossy(&face[..end]));
    }
    1
}

pub fn families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        let mut list = Vec::new();
        let mut query = LogFont {
            metrics: [0; 5], italic: 0, underline: 0, strike_out: 0,
            charset: 1, // DEFAULT_CHARSET enumerates installed font families.
            precision: [0; 4], face: [0; 32],
        };
        unsafe {
            let dc = GetDC(null_mut());
            if !dc.is_null() {
                EnumFontFamiliesExW(dc, &mut query, collect, &mut list as *mut Vec<String> as isize, 0);
                ReleaseDC(null_mut(), dc);
            }
        }
        list.sort_by_cached_key(|name: &String| name.to_lowercase());
        list.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        list
    })
}
