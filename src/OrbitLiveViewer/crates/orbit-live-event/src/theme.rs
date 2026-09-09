// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Colour schemes, in one place.
//!
//! Every colour the viewer draws -- the chrome (panels, text, accent), the
//! timeline canvas and track washes, the scope-box palette, the thread-state
//! bar, and the selection highlights -- comes from the active [`Theme`]. The
//! default is Orbit's original palette, reproduced value for value, so nothing
//! changes unless the reader picks another scheme.
//!
//! The active theme is a thread-local `&'static Theme` (the viewer is
//! single-threaded on wasm; the service never switches, so it keeps the
//! default and draws exactly as before). `color.rs` reads the scope and
//! thread-state colours from here; the viewer's `theme.rs` reads the chrome.
//!
//! The non-default schemes are built from well-known terminal palettes
//! (Dracula, Nord, Gruvbox, Solarized). A terminal palette is already a
//! curated, harmonious set of accent colours, which is exactly what the
//! scope boxes need to stay legible against the canvas.

use core::cell::Cell;

/// Colours are `0xAARRGGBB`. The viewer turns them into `egui::Color32`; the
/// rasteriser works in this word directly.
pub type Argb = u32;

/// The thread-state bar's colours, one per state the tracer reports.
#[derive(Clone, Copy, Debug)]
pub struct ThreadStates {
    pub running: Argb,
    pub runnable: Argb,
    pub interruptible: Argb,
    pub uninterruptible: Argb,
    pub stopped: Argb,
    pub traced: Argb,
    pub dead: Argb,
    pub parked: Argb,
}

/// A complete colour scheme. Every field is a colour the viewer draws; a
/// scheme is chosen once and read everywhere, so switching one reskins the
/// whole app.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    /// Shown in the picker.
    pub name: &'static str,
    /// Stable id for `?theme=` and `localStorage`.
    pub key: &'static str,

    // --- Timeline / scope colours (read by color.rs) ---
    /// Scope-box palette, indexed by a hash of the name or the thread id.
    /// Any length >= 1; the default keeps Orbit's six.
    pub scope: &'static [Argb],
    pub thread_states: ThreadStates,
    /// The sample-bar tick (kept the same for every sample by design).
    pub sample_tick: Argb,
    /// Marquee / range selection.
    pub selection: Argb,
    /// "Highlight every instance of this scope".
    pub same_scope: Argb,
    /// Greyed, non-matching search hit or inactive scope.
    pub inactive: Argb,

    // --- Chrome (read by the viewer's theme.rs) ---
    pub canvas: Argb,
    pub paper: Argb,
    pub panel: Argb,
    pub report_row_alt: Argb,
    pub report_row_hover: Argb,
    pub rail: Argb,
    pub track: Argb,
    pub track_alt: Argb,
    pub input: Argb,
    pub text: Argb,
    pub muted: Argb,
    pub accent: Argb,
    /// Faint hairline, premultiplied `0xAARRGGBB`.
    pub hair: Argb,
    pub insert: Argb,
    pub playhead: Argb,
    pub paper_playhead: Argb,
    /// Near-background per-process washes; index is a hash of the pid.
    pub process_washes: &'static [[u8; 3]],
}

// ---------------------------------------------------------------------------
// Orbit -- the original palette, reproduced exactly. Do not change these
// values: the default scheme must draw byte-for-byte as before.
// ---------------------------------------------------------------------------

/// The original six-colour scope palette: `#E74435 #2B91AF #B975B5 #57A64A
/// #D7AB69 #F86516`.
pub const ORBIT_SCOPE: [Argb; 6] = [
    0xFFE7_4435, 0xFF2B_91AF, 0xFFB9_75B5, 0xFF57_A64A, 0xFFD7_AB69, 0xFFF8_6516,
];

/// The original near-black process washes.
const ORBIT_WASHES: [[u8; 3]; 8] = [
    [0x1C, 0x16, 0x13],
    [0x13, 0x17, 0x1E],
    [0x13, 0x1A, 0x16],
    [0x1A, 0x15, 0x1C],
    [0x15, 0x18, 0x1B],
    [0x1B, 0x18, 0x13],
    [0x14, 0x16, 0x1A],
    [0x18, 0x15, 0x16],
];

pub static ORBIT: Theme = Theme {
    name: "Orbit",
    key: "orbit",
    scope: &ORBIT_SCOPE,
    thread_states: ThreadStates {
        running: 0xFF4C_AF50,
        runnable: 0xFF21_96F3,
        interruptible: 0xFF75_7575,
        uninterruptible: 0xFFFF_9800,
        stopped: 0xFFF4_4336,
        traced: 0xFF9C_27B0,
        dead: 0xFF00_0000,
        parked: 0xFF79_5548,
    },
    sample_tick: 0xFFEC_EFF1,
    selection: 0xFF00_80FF,
    same_scope: 0xFF64_B5F6,
    inactive: 0xFF64_6464,
    canvas: 0xFF0B_0C0E,
    paper: 0xFFE4_E6EA,
    panel: 0xFF12_141A,
    report_row_alt: 0xFF1C_2028,
    report_row_hover: 0xFF26_2D37,
    rail: 0xFF10_1216,
    track: 0xFF16_181D,
    track_alt: 0xFF14_161B,
    input: 0xFF0E_1014,
    text: 0xFFC4_C7CC,
    muted: 0xFF6A_6E76,
    accent: 0xFF7A_A4C2,
    hair: 0x1212_1212,
    insert: 0xFFD0_D8E0,
    playhead: 0xFFE8_EAEE,
    paper_playhead: 0xFF3A_3E46,
    process_washes: &ORBIT_WASHES,
};

// ---------------------------------------------------------------------------
// Terminal-derived dark schemes. Chrome maps the scheme's backgrounds to
// canvas/panel/rail/track and its foreground/comment to text/muted; the
// scope palette is the scheme's accent set, which is built to be harmonious.
// The washes stay the neutral originals -- they are a near-black tint under a
// process, not a scheme colour.
// ---------------------------------------------------------------------------

const DRACULA_SCOPE: [Argb; 7] = [
    0xFFFF_5555, 0xFFFF_B86C, 0xFFF1_FA8C, 0xFF50_FA7B, 0xFF8B_E9FD, 0xFFBD_93F9, 0xFFFF_79C6,
];
static DRACULA: Theme = Theme {
    name: "Dracula",
    key: "dracula",
    scope: &DRACULA_SCOPE,
    thread_states: ThreadStates {
        running: 0xFF50_FA7B,
        runnable: 0xFF8B_E9FD,
        interruptible: 0xFF62_72A4,
        uninterruptible: 0xFFFF_B86C,
        stopped: 0xFFFF_5555,
        traced: 0xFFBD_93F9,
        dead: 0xFF1A_1B23,
        parked: 0xFF62_72A4,
    },
    sample_tick: 0xFFF8_F8F2,
    selection: 0xFFBD_93F9,
    same_scope: 0xFF8B_E9FD,
    inactive: 0xFF62_72A4,
    canvas: 0xFF21_222C,
    paper: 0xFFF8_F8F2,
    panel: 0xFF28_2A36,
    report_row_alt: 0xFF2E_313F,
    report_row_hover: 0xFF3A_3D4D,
    rail: 0xFF1E_1F29,
    track: 0xFF2B_2E3B,
    track_alt: 0xFF24_2632,
    input: 0xFF1B_1C24,
    text: 0xFFF8_F8F2,
    muted: 0xFF6272A4,
    accent: 0xFFBD_93F9,
    hair: 0x14F8_F8F2,
    insert: 0xFFF8_F8F2,
    playhead: 0xFFF8_F8F2,
    paper_playhead: 0xFF3A_3E46,
    process_washes: &ORBIT_WASHES,
};

const NORD_SCOPE: [Argb; 8] = [
    0xFFBF_616A, 0xFFD0_8770, 0xFFEB_CB8B, 0xFFA3_BE8C, 0xFF8F_BCBB, 0xFF88_C0D0, 0xFF81_A1C1,
    0xFFB4_8EAD,
];
static NORD: Theme = Theme {
    name: "Nord",
    key: "nord",
    scope: &NORD_SCOPE,
    thread_states: ThreadStates {
        running: 0xFFA3_BE8C,
        runnable: 0xFF88_C0D0,
        interruptible: 0xFF4C_566A,
        uninterruptible: 0xFFD0_8770,
        stopped: 0xFFBF_616A,
        traced: 0xFFB4_8EAD,
        dead: 0xFF23_2831,
        parked: 0xFF5E_81AC,
    },
    sample_tick: 0xFFEC_EFF4,
    selection: 0xFF88_C0D0,
    same_scope: 0xFF81_A1C1,
    inactive: 0xFF4C_566A,
    canvas: 0xFF2E_3440,
    paper: 0xFFEC_EFF4,
    panel: 0xFF33_3B48,
    report_row_alt: 0xFF3B_4252,
    report_row_hover: 0xFF43_4C5E,
    rail: 0xFF2B_313C,
    track: 0xFF3B_4252,
    track_alt: 0xFF35_3D49,
    input: 0xFF27_2B35,
    text: 0xFFEC_EFF4,
    muted: 0xFF7B_88A1,
    accent: 0xFF88_C0D0,
    hair: 0x14EC_EFF4,
    insert: 0xFFE5_E9F0,
    playhead: 0xFFEC_EFF4,
    paper_playhead: 0xFF3A_3E46,
    process_washes: &ORBIT_WASHES,
};

const GRUVBOX_SCOPE: [Argb; 7] = [
    0xFFFB_4934, 0xFFFE_8019, 0xFFFA_BD2F, 0xFFB8_BB26, 0xFF8E_C07C, 0xFF83_A598, 0xFFD3_869B,
];
static GRUVBOX: Theme = Theme {
    name: "Gruvbox",
    key: "gruvbox",
    scope: &GRUVBOX_SCOPE,
    thread_states: ThreadStates {
        running: 0xFFB8_BB26,
        runnable: 0xFF83_A598,
        interruptible: 0xFF92_8374,
        uninterruptible: 0xFFFE_8019,
        stopped: 0xFFFB_4934,
        traced: 0xFFD3_869B,
        dead: 0xFF1D_2021,
        parked: 0xFF66_5C54,
    },
    sample_tick: 0xFFEB_DBB2,
    selection: 0xFF83_A598,
    same_scope: 0xFF8E_C07C,
    inactive: 0xFF66_5C54,
    canvas: 0xFF1D_2021,
    paper: 0xFFFB_F1C7,
    panel: 0xFF28_2828,
    report_row_alt: 0xFF3C_3836,
    report_row_hover: 0xFF50_4945,
    rail: 0xFF1D_2021,
    track: 0xFF32_302F,
    track_alt: 0xFF28_2828,
    input: 0xFF1D_2021,
    text: 0xFFEB_DBB2,
    muted: 0xFF92_8374,
    accent: 0xFF83_A598,
    hair: 0x14EB_DBB2,
    insert: 0xFFFB_F1C7,
    playhead: 0xFFEB_DBB2,
    paper_playhead: 0xFF3A_3E46,
    process_washes: &ORBIT_WASHES,
};

const SOLARIZED_SCOPE: [Argb; 8] = [
    0xFFDC_322F, 0xFFCB_4B16, 0xFFB5_8900, 0xFF85_9900, 0xFF2A_A198, 0xFF26_8BD2, 0xFF6C_71C4,
    0xFFD3_3682,
];
static SOLARIZED: Theme = Theme {
    name: "Solarized",
    key: "solarized",
    scope: &SOLARIZED_SCOPE,
    thread_states: ThreadStates {
        running: 0xFF85_9900,
        runnable: 0xFF26_8BD2,
        interruptible: 0xFF58_6E75,
        uninterruptible: 0xFFCB_4B16,
        stopped: 0xFFDC_322F,
        traced: 0xFF6C_71C4,
        dead: 0xFF00_1F27,
        parked: 0xFF58_6E75,
    },
    sample_tick: 0xFFEE_E8D5,
    selection: 0xFF26_8BD2,
    same_scope: 0xFF2A_A198,
    inactive: 0xFF58_6E75,
    canvas: 0xFF00_2B36,
    paper: 0xFFEE_E8D5,
    panel: 0xFF07_3642,
    report_row_alt: 0xFF0A_3A47,
    report_row_hover: 0xFF0E_4553,
    rail: 0xFF00_252E,
    track: 0xFF0B_3A46,
    track_alt: 0xFF06_3039,
    input: 0xFF00_212B,
    text: 0xFF93_A1A1,
    muted: 0xFF58_6E75,
    accent: 0xFF26_8BD2,
    hair: 0x14EE_E8D5,
    insert: 0xFFEE_E8D5,
    playhead: 0xFFEE_E8D5,
    paper_playhead: 0xFF3A_3E46,
    process_washes: &ORBIT_WASHES,
};

/// Every scheme, in the order the picker lists them. Orbit leads so the
/// default is the first thing offered.
pub static THEMES: &[&Theme] = &[&ORBIT, &DRACULA, &NORD, &GRUVBOX, &SOLARIZED];

thread_local! {
    static ACTIVE: Cell<&'static Theme> = const { Cell::new(&ORBIT) };
}

/// The active scheme. Every colour accessor reads through this.
pub fn active() -> &'static Theme {
    ACTIVE.with(|a| a.get())
}

/// Switch the active scheme. Idempotent; the viewer re-applies egui's visuals
/// after calling this.
pub fn set_active(theme: &'static Theme) {
    ACTIVE.with(|a| a.set(theme));
}

/// Look a scheme up by its [`Theme::key`].
pub fn by_key(key: &str) -> Option<&'static Theme> {
    THEMES.iter().copied().find(|t| t.key == key)
}

/// Switch by key; returns whether it matched a known scheme.
pub fn set_active_by_key(key: &str) -> bool {
    match by_key(key) {
        Some(t) => {
            set_active(t);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_matches_orbits_original_values() {
        // The whole promise: the default scheme is byte-for-byte the old
        // palette. These are the exact constants color.rs and theme.rs held.
        assert_eq!(active().key, "orbit");
        assert_eq!(active().scope, &[0xFFE7_4435, 0xFF2B_91AF, 0xFFB9_75B5, 0xFF57_A64A, 0xFFD7_AB69, 0xFFF8_6516]);
        assert_eq!(active().sample_tick, 0xFFEC_EFF1);
        assert_eq!(active().selection, 0xFF00_80FF);
        assert_eq!(active().same_scope, 0xFF64_B5F6);
        assert_eq!(active().canvas, 0xFF0B_0C0E);
        assert_eq!(active().panel, 0xFF12_141A);
        assert_eq!(active().text, 0xFFC4_C7CC);
        assert_eq!(active().accent, 0xFF7A_A4C2);
        assert_eq!(active().thread_states.running, 0xFF4C_AF50);
        assert_eq!(active().thread_states.stopped, 0xFFF4_4336);
    }

    #[test]
    fn keys_are_unique_and_resolvable() {
        let mut keys: Vec<&str> = THEMES.iter().map(|t| t.key).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "theme keys must be unique");
        assert_eq!(THEMES.len(), 5, "Orbit plus four terminal schemes");
        for t in THEMES {
            assert!(by_key(t.key).is_some());
            assert!(!t.scope.is_empty(), "every scheme needs a scope palette");
            assert_eq!(t.scope[0] >> 24, 0xFF, "scope colours are opaque ARGB");
        }
        assert!(by_key("nope").is_none());
    }

    #[test]
    fn switching_is_visible_then_restored() {
        assert!(set_active_by_key("dracula"));
        assert_eq!(active().key, "dracula");
        assert_ne!(active().scope, &ORBIT_SCOPE[..]);
        set_active(&ORBIT);
        assert_eq!(active().key, "orbit");
    }
}
