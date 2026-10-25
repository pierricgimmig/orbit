//! WGSL for the hybrid GPU timeline.
//!
//! Pixel-column path: nearest-neighbor blit of the CPU raster.
//! Instanced path: SDF rounded rects + an analytical drop shadow.
//!
//! Sources live in `src/shaders/*.wgsl` so editors, `wgsl-analyzer`, and the
//! `naga` CLI can see them; `include_str!` bakes them in at compile time.

/// Full-viewport blit of the CPU raster.
pub const BLIT_WGSL: &str = include_str!("shaders/blit.wgsl");

/// Instanced scope rects: SDF rounded boxes + analytical drop shadow.
pub const INSTANCE_WGSL: &str = include_str!("shaders/instance.wgsl");

/// Textured dest-rect blit for the zoomed-out pixel-column LOD.
pub const BLIT_RECT_WGSL: &str = include_str!("shaders/blit_rect.wgsl");
