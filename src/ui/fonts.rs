//! Fonts: the user's own fontconfig choices (sans, sans bold, monospace) plus Phosphor icons.
//! Nothing is bundled except the icon font, so the app looks native on any setup.

use std::process::Command;
use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily, FontId};

use super::theme::TypeStep;

const BOLD: &str = "acreage-bold";

fn fc_match(pattern: &str) -> Option<Vec<u8>> {
    let out = Command::new("fc-match")
        .args(["-f", "%{file}", pattern])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?;
    // .ttc collections would need a face index; fall back to egui's fonts for those.
    if path.ends_with(".ttc") {
        return None;
    }
    std::fs::read(path.trim()).ok()
}

/// Load fonts into the context. Falls back to egui's built-ins for anything fontconfig can't give.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let add_front = |fonts: &mut FontDefinitions, key: &str, pattern: &str, family: FontFamily| {
        if let Some(bytes) = fc_match(pattern) {
            fonts
                .font_data
                .insert(key.into(), Arc::new(FontData::from_owned(bytes)));
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, key.into());
            true
        } else {
            false
        }
    };
    add_front(
        &mut fonts,
        "sys-sans",
        "sans-serif",
        FontFamily::Proportional,
    );
    add_front(&mut fonts, "sys-mono", "monospace", FontFamily::Monospace);

    // Icons go into Proportional before the bold family copies it, so bold text gets icons too.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    let mut bold = fonts.families[&FontFamily::Proportional].clone();
    if let Some(bytes) = fc_match("sans-serif:bold") {
        fonts.font_data.insert(
            "sys-sans-bold".into(),
            Arc::new(FontData::from_owned(bytes)),
        );
        bold.insert(0, "sys-sans-bold".into());
    }
    fonts.families.insert(FontFamily::Name(BOLD.into()), bold);

    ctx.set_fonts(fonts);
}

/// Font for a type step. Weights of 600 and up use the bold face.
pub fn font(step: TypeStep) -> FontId {
    let family = if step.weight >= 600 {
        FontFamily::Name(BOLD.into())
    } else {
        FontFamily::Proportional
    };
    FontId::new(step.size, family)
}

pub fn mono(step: TypeStep) -> FontId {
    FontId::monospace(step.size)
}

/// Bold face at a type step's size, regardless of the step's weight.
pub fn bold(step: TypeStep) -> FontId {
    FontId::new(step.size, FontFamily::Name(BOLD.into()))
}
