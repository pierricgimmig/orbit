//! Viewer chrome, read from the active colour scheme.
//!
//! Every value here comes from [`orbit_live_event::theme::active`]. The names
//! stay `UPPER_CASE` (they read like the constants they replaced) but are
//! functions, so a scheme change is reflected the next frame. The scope and
//! thread-state colours live in `orbit_live_event::color`, which reads the
//! same active theme.
#![allow(non_snake_case)]

use eframe::egui::Color32;
use orbit_live_event::chrome;
use orbit_live_event::theme::active;

/// `0xAARRGGBB` -> `Color32` (unmultiplied; opaque when `a == 0xFF`).
fn c(argb: u32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (argb >> 16) as u8,
        (argb >> 8) as u8,
        argb as u8,
        (argb >> 24) as u8,
    )
}

pub fn CANVAS() -> Color32 {
    c(active().canvas)
}
/// Opt-in timeline paper so Wallace drop shadows read against a light field.
pub fn PAPER() -> Color32 {
    c(active().paper)
}
pub fn PANEL() -> Color32 {
    c(active().panel)
}
/// Cool graphite stripes shared by the report grids and virtualized rows.
pub fn REPORT_ROW_ALT() -> Color32 {
    c(active().report_row_alt)
}
pub fn REPORT_ROW_HOVER() -> Color32 {
    c(active().report_row_hover)
}
pub fn RAIL() -> Color32 {
    c(active().rail)
}
pub fn TRACK() -> Color32 {
    c(active().track)
}
/// Neutral alt before per-process washes. Kept as the graphite baseline.
#[allow(dead_code)]
pub fn TRACK_ALT() -> Color32 {
    c(active().track_alt)
}
pub fn INPUT() -> Color32 {
    c(active().input)
}
pub fn TEXT() -> Color32 {
    c(active().text)
}
pub fn MUTED() -> Color32 {
    c(active().muted)
}
pub fn ACCENT() -> Color32 {
    c(active().accent)
}
pub fn HAIR() -> Color32 {
    let h = active().hair;
    Color32::from_rgba_premultiplied(
        (h >> 16) as u8,
        (h >> 8) as u8,
        h as u8,
        (h >> 24) as u8,
    )
}
pub fn INSERT() -> Color32 {
    c(active().insert)
}
pub fn PLAYHEAD() -> Color32 {
    c(active().playhead)
}
pub fn PAPER_PLAYHEAD() -> Color32 {
    c(active().paper_playhead)
}

pub fn timeline_canvas(light: bool) -> Color32 {
    if light {
        PAPER()
    } else {
        CANVAS()
    }
}

pub fn quiet_grid_line(paper: bool) -> Color32 {
    if paper || active().light {
        Color32::from_rgba_unmultiplied(20, 22, 26, 28)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 10)
    }
}

pub fn playhead_color(paper: bool) -> Color32 {
    if paper || active().light {
        PAPER_PLAYHEAD()
    } else {
        PLAYHEAD()
    }
}
pub const RADIUS: f32 = 4.0;
pub const TRACK_RADIUS: f32 = 2.0;

/// The scheme's track colour as `0xAARRGGBB`, what the timeline's
/// `chrome::TRACK` sentinel pixels are remapped to on display.
pub fn DISPLAY_TRACK() -> u32 {
    active().track
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WashRole {
    Process,
    Thread,
    ThreadAlt,
    Leaf,
}

/// A stable per-process slot in `0..8`. Reserved pids 1/2/3 (demo, viewer,
/// service) are pinned so they always differ; the rest hash. The virtual
/// count is fixed, so it is independent of how long a scheme's wash table or
/// scope palette is -- the caller takes it modulo the array it indexes.
pub fn process_wash_index(pid: u32) -> usize {
    match pid {
        1 => 0,
        2 => 1,
        3 => 2,
        _ => {
            let x = pid.wrapping_mul(0x9E37_79B9) ^ pid.rotate_right(16);
            3 + (x as usize % 5)
        }
    }
}

/// Thread-row base wash for `pid`. Same process ⇒ same family.
pub fn process_track_wash(pid: u32) -> Color32 {
    process_track_wash_role(pid, WashRole::Thread)
}

pub fn process_track_wash_role(pid: u32, role: WashRole) -> Color32 {
    let lift = match role {
        WashRole::Process => 10,
        WashRole::Thread => 0,
        WashRole::ThreadAlt => -3,
        WashRole::Leaf => -5,
    };
    let t = active();
    let idx = process_wash_index(pid);
    // Orbit ships an explicit near-black table; other schemes derive the wash
    // from their own canvas so a process band stays in the scheme's family.
    if !t.process_washes.is_empty() {
        let [r, g, b] = t.process_washes[idx % t.process_washes.len()];
        return Color32::from_rgb(chan(r, lift), chan(g, lift), chan(b, lift));
    }
    derive_wash(t, idx, lift)
}

/// A process band as a faint tint of the scheme's canvas toward one of its
/// scope accents -- distinct per process, but clearly the timeline
/// background rather than a foreign colour. The row lift is a small lightness
/// step (lighter than the canvas on a dark scheme, darker on a light one) so
/// the alternating thread rows still read.
fn derive_wash(t: &orbit_live_event::theme::Theme, idx: usize, lift: i16) -> Color32 {
    let accent = t.scope[idx % t.scope.len()];
    let mix = |base: u32, acc: u32| ((base * 92 + acc * 8) / 100) as i16;
    let chan_at = |shift: u32| {
        let base = (t.canvas >> shift) & 0xFF;
        let acc = (accent >> shift) & 0xFF;
        let dir = if t.light { -1 } else { 1 };
        (mix(base, acc) + lift * dir).clamp(0, 255) as u8
    };
    Color32::from_rgb(chan_at(16), chan_at(8), chan_at(0))
}

fn chan(v: u8, lift: i16) -> u8 {
    // The explicit-table (Orbit) path: near-black washes, clamped to the dark
    // end so a lift cannot push one into mid-grey.
    (i16::from(v) + lift).clamp(0x0B, 0x28) as u8
}

/// Scope / event colours are drawn as-is. Only the timeline's `chrome::TRACK`
/// sentinel is remapped onto the active scheme's track colour.
pub fn display_argb(argb: u32) -> u32 {
    if argb == chrome::TRACK {
        return DISPLAY_TRACK();
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
    eframe::egui::Stroke::new(1.0, HAIR())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::theme;
    use orbit_live_event::THREAD_PALETTE;

    #[test]
    fn display_keeps_raw_scope_palette() {
        let src = THREAD_PALETTE[0];
        assert_eq!(src, 0xFFE7_4435);
        assert_eq!(display_argb(src), src);
        assert_eq!(display_argb(THREAD_PALETTE[1]), THREAD_PALETTE[1]);
        assert_eq!(display_argb(chrome::TRACK), DISPLAY_TRACK());
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
    }

    #[test]
    fn paper_canvas_is_light_and_opt_in() {
        assert_eq!(timeline_canvas(false), CANVAS());
        assert_eq!(timeline_canvas(true), PAPER());
        assert!(PAPER().r() > 0xC0 && PAPER().g() > 0xC0 && PAPER().b() > 0xC0);
        assert_ne!(PAPER(), CANVAS());
        assert_ne!(playhead_color(true), playhead_color(false));
        assert!(quiet_grid_line(true).a() > quiet_grid_line(false).a());
    }

    #[test]
    fn derived_washes_stay_in_the_scheme_family() {
        // A non-default scheme's process band is a tint of its own canvas, not
        // the old near-black table -- that was the "clashes with the chrome"
        // report. Each channel should sit close to the canvas.
        theme::set_active_by_key("dracula");
        let canvas = theme::active().canvas;
        let (cr, cg, cb) = ((canvas >> 16) & 0xFF, (canvas >> 8) & 0xFF, canvas & 0xFF);
        let w = process_track_wash(1234);
        assert!((w.r() as i32 - cr as i32).abs() <= 40, "wash r near canvas");
        assert!((w.g() as i32 - cg as i32).abs() <= 40, "wash g near canvas");
        assert!((w.b() as i32 - cb as i32).abs() <= 40, "wash b near canvas");
        assert!(w.r() > 0x20 || w.g() > 0x20 || w.b() > 0x20, "not near-black");
        // Distinct processes get distinct bands.
        assert_ne!(process_track_wash(1234), process_track_wash(1237));
        theme::set_active(&theme::ORBIT);
    }

    #[test]
    fn a_scheme_switch_changes_the_chrome_then_restores() {
        let orbit_panel = PANEL();
        assert!(theme::set_active_by_key("dracula"));
        assert_ne!(PANEL(), orbit_panel, "the panel colour follows the scheme");
        theme::set_active(&theme::ORBIT);
        assert_eq!(PANEL(), orbit_panel);
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
