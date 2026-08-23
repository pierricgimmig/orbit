//! DAW chrome for the live viewer. Canonical Avery / Fusion hex values
//! stay in `orbit_live_event`; this module only remaps for display.

use eframe::egui::Color32;
use orbit_live_event::chrome;

pub const CANVAS: Color32 = Color32::from_rgb(0x0B, 0x0C, 0x0E);
pub const PANEL: Color32 = Color32::from_rgb(0x12, 0x14, 0x1A);
pub const RAIL: Color32 = Color32::from_rgb(0x10, 0x12, 0x16);
pub const TRACK: Color32 = Color32::from_rgb(0x16, 0x18, 0x1D);
pub const TRACK_ALT: Color32 = Color32::from_rgb(0x14, 0x16, 0x1B);
pub const INPUT: Color32 = Color32::from_rgb(0x0E, 0x10, 0x14);
pub const TEXT: Color32 = Color32::from_rgb(0xC4, 0xC7, 0xCC);
pub const MUTED: Color32 = Color32::from_rgb(0x6A, 0x6E, 0x76);
pub const ACCENT: Color32 = Color32::from_rgb(0x7A, 0xA4, 0xC2);
pub const HAIR: Color32 = Color32::from_rgba_premultiplied(18, 18, 18, 18);
pub const INSERT: Color32 = Color32::from_rgb(0xD0, 0xD8, 0xE0);
pub const PLAYHEAD: Color32 = Color32::from_rgb(0xE8, 0xEA, 0xEE);
pub const RADIUS: f32 = 4.0;
pub const TRACK_RADIUS: f32 = 2.0;

pub const DISPLAY_TRACK: u32 = 0xFF16_181D;

/// Mix Avery / Material toward graphite so clips sit on a dark lane.
pub fn display_argb(argb: u32) -> u32 {
    if argb == chrome::TRACK {
        return DISPLAY_TRACK;
    }
    let r = ((argb >> 16) & 0xFF) as i32;
    let g = ((argb >> 8) & 0xFF) as i32;
    let b = (argb & 0xFF) as i32;
    let a = argb & 0xFF00_0000;
    let mix = 62; // /255 toward #1A1C22
    let r = (r * (255 - mix) + 0x1A * mix) / 255;
    let g = (g * (255 - mix) + 0x1C * mix) / 255;
    let b = (b * (255 - mix) + 0x22 * mix) / 255;
    let r = (r * 214 / 255) as u32;
    let g = (g * 214 / 255) as u32;
    let b = (b * 214 / 255) as u32;
    a | (r << 16) | (g << 8) | b
}

pub fn remap_rgba8(bytes: &mut [u8]) {
    for px in bytes.chunks_exact_mut(4) {
        let argb = if px == [0x32, 0x32, 0x32, 0xFF] {
            chrome::TRACK
        } else {
            0xFF00_0000 | ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32
        };
        let out = display_argb(argb);
        px[0] = ((out >> 16) & 0xFF) as u8;
        px[1] = ((out >> 8) & 0xFF) as u8;
        px[2] = (out & 0xFF) as u8;
        px[3] = 0xFF;
    }
}

pub fn hairline() -> eframe::egui::Stroke {
    eframe::egui::Stroke::new(1.0, HAIR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::THREAD_PALETTE;

    #[test]
    fn display_keeps_hue_family_and_darkens() {
        let src = THREAD_PALETTE[0];
        let out = display_argb(src);
        assert_ne!(out, src);
        let sr = (src >> 16) & 0xFF;
        let or_ = (out >> 16) & 0xFF;
        assert!(or_ < sr);
        assert_eq!(display_argb(chrome::TRACK), DISPLAY_TRACK);
    }
}
