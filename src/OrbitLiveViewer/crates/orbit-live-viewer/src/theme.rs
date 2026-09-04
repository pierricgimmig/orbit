//! DAW chrome for the live viewer. Canonical Avery / Fusion hex values
//! stay in `orbit_live_event`; this module only remaps for display.

use eframe::egui::Color32;
use orbit_live_event::chrome;

pub const CANVAS: Color32 = Color32::from_rgb(0x0B, 0x0C, 0x0E);
/// Opt-in timeline paper so Wallace drop shadows read against a light field.
pub const PAPER: Color32 = Color32::from_rgb(0xE4, 0xE6, 0xEA);
pub const PANEL: Color32 = Color32::from_rgb(0x12, 0x14, 0x1A);
pub const RAIL: Color32 = Color32::from_rgb(0x10, 0x12, 0x16);
pub const TRACK: Color32 = Color32::from_rgb(0x16, 0x18, 0x1D);
/// Neutral alt before per-process washes. Kept as the graphite baseline.
#[allow(dead_code)]
pub const TRACK_ALT: Color32 = Color32::from_rgb(0x14, 0x16, 0x1B);
pub const INPUT: Color32 = Color32::from_rgb(0x0E, 0x10, 0x14);
pub const TEXT: Color32 = Color32::from_rgb(0xC4, 0xC7, 0xCC);
pub const MUTED: Color32 = Color32::from_rgb(0x6A, 0x6E, 0x76);
pub const ACCENT: Color32 = Color32::from_rgb(0x7A, 0xA4, 0xC2);
pub const HAIR: Color32 = Color32::from_rgba_premultiplied(18, 18, 18, 18);
pub const INSERT: Color32 = Color32::from_rgb(0xD0, 0xD8, 0xE0);
pub const PLAYHEAD: Color32 = Color32::from_rgb(0xE8, 0xEA, 0xEE);
pub const PAPER_PLAYHEAD: Color32 = Color32::from_rgb(0x3A, 0x3E, 0x46);

pub fn timeline_canvas(light: bool) -> Color32 {
    if light {
        PAPER
    } else {
        CANVAS
    }
}

pub fn quiet_grid_line(light: bool) -> Color32 {
    if light {
        Color32::from_rgba_unmultiplied(20, 22, 26, 28)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 10)
    }
}

pub fn playhead_color(light: bool) -> Color32 {
    if light {
        PAPER_PLAYHEAD
    } else {
        PLAYHEAD
    }
}
pub const RADIUS: f32 = 4.0;
pub const TRACK_RADIUS: f32 = 2.0;

pub const DISPLAY_TRACK: u32 = 0xFF16_181D;

/// Near-black process washes: graphite with a hint of cool/warm. Low chroma.
/// Index is a stable hash of `pid` (reserved 1/2/3 are pinned so they differ).
const PROCESS_WASHES: [[u8; 3]; 8] = [
    [0x1C, 0x16, 0x13], // warm ember — demo pid 1
    [0x13, 0x17, 0x1E], // cool steel — viewer pid 2
    [0x13, 0x1A, 0x16], // pine — service pid 3
    [0x1A, 0x15, 0x1C], // plum
    [0x15, 0x18, 0x1B], // slate
    [0x1B, 0x18, 0x13], // ochre
    [0x14, 0x16, 0x1A], // ink
    [0x18, 0x15, 0x16], // rose-ash
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WashRole {
    Process,
    Thread,
    ThreadAlt,
    Leaf,
}

pub fn process_wash_index(pid: u32) -> usize {
    match pid {
        1 => 0,
        2 => 1,
        3 => 2,
        _ => {
            let x = pid.wrapping_mul(0x9E37_79B9) ^ pid.rotate_right(16);
            3 + (x as usize % (PROCESS_WASHES.len() - 3))
        }
    }
}

/// Thread-row base wash for `pid`. Same process ⇒ same family.
pub fn process_track_wash(pid: u32) -> Color32 {
    process_track_wash_role(pid, WashRole::Thread)
}

pub fn process_track_wash_role(pid: u32, role: WashRole) -> Color32 {
    let [r, g, b] = PROCESS_WASHES[process_wash_index(pid)];
    let lift = match role {
        WashRole::Process => 10,
        WashRole::Thread => 0,
        WashRole::ThreadAlt => -3,
        WashRole::Leaf => -5,
    };
    Color32::from_rgb(chan(r, lift), chan(g, lift), chan(b, lift))
}

fn chan(v: u8, lift: i16) -> u8 {
    (i16::from(v) + lift).clamp(0x0B, 0x28) as u8
}

/// Scope / event colors stay the raw Avery / Fusion hex. Only the Orbit
/// track-gray chrome value is remapped onto the DAW graphite lane.
pub fn display_argb(argb: u32) -> u32 {
    if argb == chrome::TRACK {
        return DISPLAY_TRACK;
    }
    argb
}

pub fn remap_rgba8(bytes: &mut [u8]) {
    for px in bytes.chunks_exact_mut(4) {
        if px[3] == 0 {
            continue;
        }
        let argb = if px == [0x32, 0x32, 0x32, 0xFF] {
            chrome::TRACK
        } else {
            0xFF00_0000 | ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32
        };
        if argb == chrome::TRACK {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            px[3] = 0;
            continue;
        }
        let out = display_argb(argb);
        px[0] = ((out >> 16) & 0xFF) as u8;
        px[1] = ((out >> 8) & 0xFF) as u8;
        px[2] = (out & 0xFF) as u8;
        px[3] = 0xFF;
    }
}

/// Desaturate + drop value for non-matching search hits. Empty cells stay put.
/// A flat grey of `level` (0..255) keeping the pixel's alpha: the C++
/// inactive (100) and same-process (140) shades.
pub fn grey_argb(argb: u32, level: u32) -> u32 {
    (argb & 0xFF00_0000) | (level << 16) | (level << 8) | level
}

pub fn dim_argb(argb: u32) -> u32 {
    let r = ((argb >> 16) & 0xFF) as u32;
    let g = ((argb >> 8) & 0xFF) as u32;
    let b = (argb & 0xFF) as u32;
    let a = argb & 0xFF00_0000;
    let luma = (r * 54 + g * 183 + b * 19) / 256;
    let mix = |c: u32| (c * 22 + luma * 78) / 100 * 38 / 100;
    a | (mix(r) << 16) | (mix(g) << 8) | mix(b)
}

pub fn hairline() -> eframe::egui::Stroke {
    eframe::egui::Stroke::new(1.0, HAIR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::THREAD_PALETTE;

    #[test]
    fn display_keeps_raw_scope_palette() {
        let src = THREAD_PALETTE[0];
        assert_eq!(src, 0xFFE7_4435);
        assert_eq!(display_argb(src), src);
        assert_eq!(display_argb(THREAD_PALETTE[1]), THREAD_PALETTE[1]);
        assert_eq!(display_argb(chrome::TRACK), DISPLAY_TRACK);
    }

    #[test]
    fn reserved_pids_get_distinct_dark_washes() {
        let a = process_track_wash(1);
        let b = process_track_wash(2);
        let c = process_track_wash(3);
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        for wash in [a, b, c] {
            assert!(wash.r() < 0x28 && wash.g() < 0x28 && wash.b() < 0x28);
        }
        assert_eq!(process_wash_index(1), 0);
        assert_eq!(process_wash_index(2), 1);
        assert_eq!(process_wash_index(3), 2);
        assert_eq!(process_track_wash(99), process_track_wash(99));
        assert_ne!(process_track_wash_role(1, WashRole::Process), a);
        assert_ne!(process_track_wash_role(1, WashRole::Leaf), a);
        assert_ne!(a, TRACK_ALT);
    }

    #[test]
    fn paper_canvas_is_light_and_opt_in() {
        assert_eq!(timeline_canvas(false), CANVAS);
        assert_eq!(timeline_canvas(true), PAPER);
        assert!(PAPER.r() > 0xC0 && PAPER.g() > 0xC0 && PAPER.b() > 0xC0);
        assert_ne!(PAPER, CANVAS);
        assert_ne!(playhead_color(true), playhead_color(false));
        assert!(quiet_grid_line(true).a() > quiet_grid_line(false).a());
    }

    #[test]
    fn dim_argb_lowers_chroma_and_value() {
        let src = 0xFFE7_4435;
        let out = dim_argb(src);
        assert_ne!(out, src);
        let sr = (src >> 16) & 0xFF;
        let or_ = (out >> 16) & 0xFF;
        assert!(or_ < sr);
    }
}
