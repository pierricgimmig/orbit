//! IBM Plex (OFL) for chrome and tabular status / time.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

pub const SANS: &[u8] = include_bytes!("../fonts/IBMPlexSans-Regular.ttf");
pub const SANS_MEDIUM: &[u8] = include_bytes!("../fonts/IBMPlexSans-Medium.ttf");
pub const MONO: &[u8] = include_bytes!("../fonts/IBMPlexMono-Regular.ttf");

pub fn medium() -> FontFamily {
    FontFamily::Name("ibm_medium".into())
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "ibm_plex_sans".into(),
        std::sync::Arc::new(FontData::from_static(SANS)),
    );
    fonts.font_data.insert(
        "ibm_plex_sans_md".into(),
        std::sync::Arc::new(FontData::from_static(SANS_MEDIUM)),
    );
    fonts.font_data.insert(
        "ibm_plex_mono".into(),
        std::sync::Arc::new(FontData::from_static(MONO)),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "ibm_plex_sans".into());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "ibm_plex_mono".into());
    fonts.families.insert(
        medium(),
        vec!["ibm_plex_sans_md".into(), "ibm_plex_sans".into()],
    );
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(11.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(13.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(12.5, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::new(16.0, medium()));
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(12.0, FontFamily::Monospace),
    );
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(11.0, 6.0);
    style.spacing.interact_size.y = 28.0;
    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plex_fonts_are_ttf() {
        assert!(SANS.starts_with(b"\0\x01\0\0") || SANS.starts_with(b"OTTO"));
        assert!(SANS_MEDIUM.len() > 1000);
        assert!(MONO.len() > 1000);
    }
}
