use eframe::egui::{FontData, FontDefinitions, FontFamily};

pub fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".into(),
        FontData::from_static(include_bytes!("../assets/fonts/Inter-Regular.ttf")).into(),
    );
    fonts.font_data.insert(
        "Inter Medium".into(),
        FontData::from_static(include_bytes!("../assets/fonts/Inter-Medium.ttf")).into(),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Inter".into());
    let mut medium = vec!["Inter Medium".into()];
    medium.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts
        .families
        .insert(FontFamily::Name("Medium".into()), medium);
    // Use locally installed fonts; do not download or redistribute Windows fonts.
    if let Some(windows) = std::env::var_os("WINDIR") {
        let directory = std::path::PathBuf::from(windows).join("Fonts");
        for name in [
            "segoeui.ttf",
            "YuGothR.ttc",
            "msjh.ttc",
            "malgun.ttf",
            "Nirmala.ttf",
            "LeelawUI.ttf",
        ] {
            if let Ok(bytes) = std::fs::read(directory.join(name)) {
                fonts
                    .font_data
                    .insert(name.into(), FontData::from_owned(bytes).into());
                for family in [FontFamily::Proportional, FontFamily::Monospace] {
                    fonts.families.entry(family).or_default().push(name.into());
                }
            }
        }
    }
    fonts
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(windows)]
    fn installed_fallbacks_render_multilingual_transcripts() {
        let ctx = eframe::egui::Context::default();
        ctx.set_fonts(super::definitions());
        let _ = ctx.run(Default::default(), |ctx| {
            ctx.fonts_mut(|fonts| {
                for c in "Hello café Привет 你好 日本語 한국어".chars() {
                    assert!(
                        fonts.has_glyph(&eframe::egui::FontId::proportional(15.0), c),
                        "Missing glyph U+{:04X}",
                        c as u32
                    );
                }
            });
        });
    }
}
