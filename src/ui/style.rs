//! Maps the token palette onto egui's built-in Style/Visuals, so stock egui widgets
//! (menus, scroll bars, text edits, tooltips) match the custom ones in `widgets`.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Theme, Visuals};

use super::theme::{DARK, LIGHT, Palette, radius, shadow, sp, ty};

/// Linear blend in sRGB space, good enough for hover tints.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

/// Lighter in dark mode, darker in light mode: the direction "hover" goes.
pub fn hover_tint(p: &Palette, c: Color32, t: f32) -> Color32 {
    mix(
        c,
        if p.dark {
            Color32::WHITE
        } else {
            Color32::BLACK
        },
        t,
    )
}

pub fn palette(theme: Theme) -> &'static Palette {
    match theme {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

fn visuals(p: &Palette) -> Visuals {
    let mut v = if p.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let md = CornerRadius::same(radius::MD);

    v.panel_fill = p.surface;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.bg;
    v.text_edit_bg_color = Some(p.bg);
    v.faint_bg_color = p.surface_2;
    v.code_bg_color = p.surface_2;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(radius::LG);
    v.menu_corner_radius = md;
    v.window_shadow = shadow::POP;
    v.popup_shadow = shadow::MENU;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;
    v.weak_text_color = Some(p.text_muted);
    v.selection.bg_fill = mix(p.surface, p.accent, 0.45);
    v.selection.stroke = Stroke::new(1.0, p.text);
    v.disabled_alpha = 0.45;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = md;

    w.inactive.bg_fill = p.surface_2;
    w.inactive.weak_bg_fill = p.surface_2;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = md;

    w.hovered.bg_fill = hover_tint(p, p.surface_2, 0.06);
    w.hovered.weak_bg_fill = hover_tint(p, p.surface_2, 0.06);
    w.hovered.bg_stroke = Stroke::new(1.0, p.border_input);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = md;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = hover_tint(p, p.surface_2, 0.12);
    w.active.weak_bg_fill = hover_tint(p, p.surface_2, 0.12);
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = md;
    w.active.expansion = 0.0;

    w.open = w.hovered;
    v
}

/// Install both themes and pick one. Call once at startup (after `fonts::install`).
pub fn apply(ctx: &egui::Context, theme: Theme) {
    ctx.set_visuals_of(Theme::Dark, visuals(&DARK));
    ctx.set_visuals_of(Theme::Light, visuals(&LIGHT));
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (
                TextStyle::Small,
                FontId::new(ty::XS.size, FontFamily::Proportional),
            ),
            (
                TextStyle::Body,
                FontId::new(ty::BASE.size, FontFamily::Proportional),
            ),
            (
                TextStyle::Button,
                FontId::new(ty::BASE.size, FontFamily::Proportional),
            ),
            (TextStyle::Heading, super::fonts::font(ty::LG)),
            (
                TextStyle::Monospace,
                FontId::new(ty::SM.size, FontFamily::Monospace),
            ),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(sp(3), sp(2));
        s.spacing.button_padding = egui::vec2(sp(4), sp(1) + 1.0);
        s.spacing.interact_size.y = 24.0;
        s.spacing.menu_margin = Margin::symmetric(0, sp(2) as i8);
        s.spacing.window_margin = Margin::same(sp(6) as i8);
    });
    ctx.set_theme(theme);
}
