//! Orbit capture-window colors. Copied from the C++ UI — do not invent replacements.
//!
//! * Function-call / CPU-scope 6-color: `ThreadColor.cpp` / `TimeGraph::GetColor(tid)`
//! * API scopes / tracks: `TimeGraph::GetColor(string_view)` → `palette[name_hash % 6]`
//! * Even-depth darken: `ThreadTrack.cpp` `kOddRowColorMultiplier` (210/255)
//!   when `(depth & 1) == 0`
//! * Async / GPU names use the same name-hash palette as API scopes
//! * Manual API: Material-500 `orbit_api_color` in `ApiInterface/Orbit.h` (`0xRRGGBBAA`)
//! * Thread states: `ThreadStateBar.cpp` `GetThreadStateColor`
//! * Chrome: Qt capture window (`#434343` canvas, `#323232` track, …)

use crate::{kind, thread_state};

pub mod mode {
    pub const AUTO_THREAD: u8 = 0;
    pub const AUTO_NAME: u8 = 1;
    pub const MANUAL_API: u8 = 2;
}

/// `#E74435 #2B91AF #B975B5 #57A64A #D7AB69 #F86516` as `0xAARRGGBB`.
pub const THREAD_PALETTE: [u32; 6] = [
    0xFFE7_4435,
    0xFF2B_91AF,
    0xFFB9_75B5,
    0xFF57_A64A,
    0xFFD7_AB69,
    0xFFF8_6516,
];

pub const SELECTION: u32 = 0xFF00_80FF;
pub const SAME_SCOPE_HIGHLIGHT: u32 = 0xFF64_B5F6;
pub const INACTIVE: u32 = 0xFF64_6464;
pub const BOX_BORDER: u32 = 0xFFFF_FFFF;
pub const SHADE_LEFT: f32 = 0.94;

pub const ORBIT_API_COLORS_RGBA: [u32; 19] = [
    0xF443_36FF, 0xE91E_63FF, 0x9C27_B0FF, 0x673A_B7FF, 0x3F51_B5FF, 0x2196_F3FF,
    0x03A9_F4FF, 0x00BC_D4FF, 0x0096_88FF, 0x4CAF_50FF, 0x8BC3_4AFF, 0xCDDC_39FF,
    0xFFEB_3BFF, 0xFFC1_07FF, 0xFF98_00FF, 0xFF57_22FF, 0x7955_48FF, 0x9E9E_9EFF,
    0x607D_8BFF,
];

pub const ORBIT_COLOR_AUTO: u32 = 0;
pub const ORBIT_COLOR_RED: u32 = 0xF443_36FF;

pub mod chrome {
    pub const CANVAS: u32 = 0xFF43_4343;
    pub const TRACK: u32 = 0xFF32_3232;
    pub const OTHER_PROCESS: u32 = 0xFF1E_1E28;
    pub const TIME_BAR: u32 = 0xFF21_2021;
    pub const QT_WINDOW: u32 = 0xFF35_3535;
    pub const INPUT_BASE: u32 = 0xFF19_1919;
    pub const TEXT: u32 = 0xFFFF_FFFF;
    pub const SELECTED_TAB: u32 = 0xFF64_B5F6;
    pub const TICK_MAJOR: u32 = 0xFFFF_FEFD;
    pub const TICK_MINOR: u32 = 0x40FF_FEFD;
    pub const PLAYHEAD: u32 = 0x80FF_FFFF;
}

pub fn scale_rgb(color: u32, num: u32, den: u32) -> u32 {
    let a = color & 0xFF00_0000;
    let r = ((color >> 16) & 0xFF) * num / den;
    let g = ((color >> 8) & 0xFF) * num / den;
    let b = (color & 0xFF) * num / den;
    a | (r << 16) | (g << 8) | b
}

pub fn thread_scope_color(tid: u32, depth: u8) -> u32 {
    apply_even_depth(THREAD_PALETTE[(tid as usize) % THREAD_PALETTE.len()], depth)
}

/// Same 6-color palette as [`thread_scope_color`], keyed by FNV-1a of the
/// interned scope name (or `name_id` bytes on an intern miss).
pub fn named_scope_color(name: &[u8], depth: u8) -> u32 {
    apply_even_depth(palette_index(name_hash(name)), depth)
}

fn apply_even_depth(mut c: u32, depth: u8) -> u32 {
    if depth & 1 == 0 {
        c = scale_rgb(c, 210, 255);
    }
    c
}

pub fn palette_index(id: u32) -> u32 {
    THREAD_PALETTE[(id as usize) % THREAD_PALETTE.len()]
}

pub fn name_hash(bytes: &[u8]) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(16_777_619);
    }
    h
}

pub fn async_scope_color(hash: u32) -> u32 {
    palette_index(hash)
}

pub fn rgba_word_to_argb(rgba: u32) -> u32 {
    let r = (rgba >> 24) & 0xFF;
    let g = (rgba >> 16) & 0xFF;
    let b = (rgba >> 8) & 0xFF;
    let a = rgba & 0xFF;
    let a = if a == 0 { 0xFF } else { a };
    (a << 24) | (r << 16) | (g << 8) | b
}

pub fn argb_to_css(argb: u32) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        (argb >> 16) & 0xFF,
        (argb >> 8) & 0xFF,
        argb & 0xFF
    )
}

pub fn material_index_to_argb(index_1based: u8) -> u32 {
    if index_1based == 0 {
        return thread_scope_color(0, 1);
    }
    let i = (index_1based as usize - 1).min(ORBIT_API_COLORS_RGBA.len() - 1);
    rgba_word_to_argb(ORBIT_API_COLORS_RGBA[i])
}

pub fn encode_manual_color(orbit_api_color: u32) -> (u8, u8) {
    if orbit_api_color == 0 || orbit_api_color == ORBIT_COLOR_AUTO {
        return (mode::AUTO_NAME, 0);
    }
    if let Some(i) = ORBIT_API_COLORS_RGBA.iter().position(|&c| c == orbit_api_color)
    {
        return (mode::MANUAL_API, (i + 1) as u8);
    }
    if let Some(i) = ORBIT_API_COLORS_RGBA
        .iter()
        .position(|&c| rgba_word_to_argb(c) == orbit_api_color)
    {
        return (mode::MANUAL_API, (i + 1) as u8);
    }
    (mode::AUTO_NAME, 0)
}

pub fn thread_state_color(state: u8) -> u32 {
    match state {
        thread_state::RUNNING => 0xFF4C_AF50,
        thread_state::RUNNABLE => 0xFF21_96F3,
        thread_state::INTERRUPTIBLE_SLEEP => 0xFF75_7575,
        thread_state::UNINTERRUPTIBLE_SLEEP => 0xFFFF_9800,
        thread_state::STOPPED => 0xFFF4_4336,
        thread_state::TRACED => 0xFF9C_27B0,
        thread_state::DEAD | thread_state::ZOMBIE => 0xFF00_0000,
        thread_state::PARKED | thread_state::IDLE => 0xFF79_5548,
        _ => INACTIVE,
    }
}

pub fn event_color(
    kind_id: u8,
    tid: u32,
    depth: u8,
    extra: u8,
    pad: u8,
    name_id: u32,
    name: Option<&[u8]>,
) -> u32 {
    if kind_id == kind::THREAD_STATE {
        return thread_state_color(extra);
    }
    if pad == mode::MANUAL_API {
        return material_index_to_argb(extra);
    }
    match kind_id {
        kind::API_SCOPE | kind::API_TRACK | kind::VALUE => {
            let id = name_id.to_le_bytes();
            named_scope_color(name.unwrap_or(&id), depth)
        }
        _ => thread_scope_color(tid, depth),
    }
}
