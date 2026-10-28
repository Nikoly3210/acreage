//! The app's widget primitives (components.md). Each takes the active `Palette` and paints
//! with token values only, never raw hex.

use egui::{
    Align, Color32, CornerRadius, Frame, Id, Layout, Margin, Rect, Response, Sense, Stroke,
    StrokeKind, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use egui_phosphor::regular as icon;

use super::fonts;
use super::style::{hover_tint, mix};
use super::theme::{Palette, layout as lay, radius, shadow, sp, tm, ty};
use super::treemap_paint::Tag;

// ---------------------------------------------------------------- helpers

/// 1024-based sizes with IEC units, as `du -h` users expect.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else if v < 10.0 {
        format!("{v:.1} {}", UNITS[u])
    } else {
        format!("{v:.0} {}", UNITS[u])
    }
}

/// Thousands separated with a thin space: 41 213.
pub fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('\u{2009}');
        }
        out.push(c);
    }
    out
}

/// WCAG 2 contrast ratio between two opaque colours.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    fn lum(c: Color32) -> f32 {
        let ch = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn focus_ring(ui: &Ui, p: &Palette, rect: Rect, r: u8) {
    ui.painter().rect_stroke(
        rect.expand(1.0),
        CornerRadius::same(r + 1),
        Stroke::new(2.0, p.focus),
        StrokeKind::Outside,
    );
}

fn text_galley(
    ui: &Ui,
    text: String,
    font: egui::FontId,
    color: Color32,
) -> std::sync::Arc<egui::Galley> {
    ui.painter().layout_no_wrap(text, font, color)
}

// ---------------------------------------------------------------- Button

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Sm,
    Md,
}

pub struct Btn<'a> {
    label: &'a str,
    icon: Option<&'a str>,
    variant: Variant,
    size: Size,
    toggled: bool,
    loading: bool,
    tooltip: Option<&'a str>,
}

impl<'a> Btn<'a> {
    pub fn new(label: &'a str) -> Self {
        Btn {
            label,
            icon: None,
            variant: Variant::Secondary,
            size: Size::Md,
            toggled: false,
            loading: false,
            tooltip: None,
        }
    }
    /// Icon-only button; `tooltip` doubles as the accessible label.
    pub fn icon(icon: &'a str, tooltip: &'a str) -> Self {
        Btn {
            icon: Some(icon),
            tooltip: Some(tooltip),
            variant: Variant::Ghost,
            ..Btn::new("")
        }
    }
    pub fn with_icon(mut self, icon: &'a str) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn variant(mut self, v: Variant) -> Self {
        self.variant = v;
        self
    }
    pub fn size(mut self, s: Size) -> Self {
        self.size = s;
        self
    }
    pub fn toggled(mut self, t: bool) -> Self {
        self.toggled = t;
        self
    }
    pub fn loading(mut self, l: bool) -> Self {
        self.loading = l;
        self
    }
    pub fn tooltip(mut self, t: &'a str) -> Self {
        self.tooltip = Some(t);
        self
    }

    pub fn show(self, ui: &mut Ui, p: &Palette) -> Response {
        let h = match self.size {
            Size::Sm => 24.0,
            Size::Md => 28.0,
        };
        let font = if self.variant == Variant::Ghost || self.label.is_empty() {
            fonts::font(ty::BASE)
        } else {
            fonts::bold(ty::BASE)
        };
        let icon_txt = if self.loading {
            Some(icon::CIRCLE_NOTCH)
        } else {
            self.icon
        };
        let text = match (icon_txt, self.label.is_empty()) {
            (Some(i), true) => i.to_owned(),
            (Some(i), false) => format!("{i}  {}", self.label),
            (None, _) => self.label.to_owned(),
        };
        let g = text_galley(ui, text, font, Color32::PLACEHOLDER);
        let w = if self.label.is_empty() {
            h
        } else {
            g.size().x + 2.0 * sp(4)
        };

        let sense = if self.loading {
            Sense::hover()
        } else {
            Sense::click()
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), sense);
        let enabled = ui.is_enabled() && !self.loading;
        let a11y = if self.label.is_empty() {
            self.tooltip.unwrap_or("")
        } else {
            self.label
        };
        resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, self.toggled, a11y));

        if ui.is_rect_visible(rect) {
            let hov = resp.hovered() && enabled;
            let down = resp.is_pointer_button_down_on() && enabled;
            let t = if down {
                0.12
            } else if hov {
                0.06
            } else {
                0.0
            };
            let (fill, stroke, fg) = match self.variant {
                Variant::Primary => (hover_tint(p, p.accent, t), Stroke::NONE, p.on_accent),
                Variant::Danger => (hover_tint(p, p.danger, t), Stroke::NONE, p.on_danger),
                Variant::Secondary => (
                    hover_tint(p, p.surface_2, t),
                    Stroke::new(1.0, if hov { p.border_input } else { p.border }),
                    p.text,
                ),
                Variant::Ghost => {
                    let base = if self.toggled || hov || down {
                        hover_tint(p, p.surface_2, t)
                    } else {
                        Color32::TRANSPARENT
                    };
                    (
                        base,
                        Stroke::NONE,
                        if self.toggled { p.accent } else { p.text },
                    )
                }
            };
            // Disabled: one neutral look for every variant, so it never reads as a faded call to action.
            let (fill, stroke, fg) = if ui.is_enabled() {
                (fill, stroke, fg)
            } else if self.variant == Variant::Ghost {
                (Color32::TRANSPARENT, Stroke::NONE, p.text_muted)
            } else {
                (p.surface_2, Stroke::new(1.0, p.border), p.text_muted)
            };
            let painter = ui.painter();
            painter.rect(
                rect,
                CornerRadius::same(radius::MD),
                fill,
                stroke,
                StrokeKind::Inside,
            );
            painter.galley(rect.center() - g.size() / 2.0, g, fg);
            if resp.has_focus() {
                focus_ring(ui, p, rect, radius::MD);
            }
        }
        match self.tooltip {
            Some(t) => resp.on_hover_text(t),
            None => resp,
        }
    }
}

// ---------------------------------------------------------------- Toolbar

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarAction {
    Open,
    Rescan,
    PauseResume,
    Back,
    Up,
    NewView,
    Tags,
    Export,
    Settings,
    More,
}

pub struct ToolbarState {
    pub paused: bool,
    pub scanning: bool,
    pub can_back: bool,
    pub can_up: bool,
    /// Features that may not be available yet; disabled buttons rather than dead ones.
    pub multi_view: bool,
    pub export: bool,
    pub settings: bool,
}

pub fn toolbar(ui: &mut Ui, p: &Palette, st: &ToolbarState) -> Option<ToolbarAction> {
    use ToolbarAction as A;
    let width = ui.available_width();
    let compact = width < lay::COMPACT_BELOW_W;
    let wide = width >= lay::WIDE_ABOVE_W;
    let mut out = None;

    let frame = Frame::new()
        .fill(p.surface)
        .inner_margin(Margin::symmetric(sp(2) as i8, sp(2) as i8));
    let r = frame.show(ui, |ui| {
        ui.set_height(lay::TOOLBAR_H - 2.0 * sp(2));
        ui.set_width(width - 2.0 * sp(2));
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = sp(1);
            let mut b = |ui: &mut Ui,
                         ic: &'static str,
                         label: &'static str,
                         act: A,
                         toggled: bool,
                         enabled: bool| {
                let btn = if wide {
                    Btn::new(label)
                        .with_icon(ic)
                        .variant(Variant::Ghost)
                        .tooltip(label)
                } else {
                    Btn::icon(ic, label)
                };
                let resp = ui
                    .add_enabled_ui(enabled, |ui| {
                        btn.size(Size::Sm).toggled(toggled).show(ui, p)
                    })
                    .inner;
                if resp.clicked() {
                    out = Some(act);
                }
            };
            b(ui, icon::ARROW_LEFT, "Back", A::Back, false, st.can_back);
            b(ui, icon::ARROW_UP, "Up", A::Up, false, st.can_up);
            b(ui, icon::ARROWS_CLOCKWISE, "Rescan", A::Rescan, false, true);
            if compact {
                b(ui, icon::DOTS_THREE, "More", A::More, false, true);
                return;
            }
            let (pi, pl) = if st.paused {
                (icon::PLAY, "Resume scan")
            } else {
                (icon::PAUSE, "Pause scan")
            };
            b(
                ui,
                pi,
                pl,
                A::PauseResume,
                st.paused,
                st.scanning || st.paused,
            );
            separator_v(ui, p);
            b(ui, icon::FOLDER_OPEN, "Open location", A::Open, false, true);
            b(ui, icon::PLUS, "New view", A::NewView, false, st.multi_view);
            b(ui, icon::TAG, "Show tagged", A::Tags, false, true);
            b(
                ui,
                icon::EXPORT,
                "Export report",
                A::Export,
                false,
                st.export,
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                b(ui, icon::GEAR, "Settings", A::Settings, false, st.settings);
            });
        });
    });
    let rr = r.response.rect;
    ui.painter().line_segment(
        [rr.left_bottom(), rr.right_bottom()],
        Stroke::new(1.0, p.border),
    );
    out
}

fn separator_v(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(sp(3), 16.0), Sense::hover());
    ui.painter().line_segment(
        [rect.center_top(), rect.center_bottom()],
        Stroke::new(1.0, p.border),
    );
}

// ---------------------------------------------------------------- Breadcrumb

/// Path bar. Returns the index of the clicked segment (0 = root chip).
pub fn breadcrumb(ui: &mut Ui, p: &Palette, segments: &[&str]) -> Option<usize> {
    let mut clicked = None;
    let font = fonts::font(ty::SM);
    let bold = fonts::bold(ty::SM);
    // Collapse the middle when it would not fit (rough estimate, 7px per char).
    let est: f32 = segments.iter().map(|s| s.len() as f32 * 7.0 + 18.0).sum();
    let collapse = segments.len() > 4 && est > ui.available_width();
    let shown: Vec<Option<usize>> = if collapse {
        let n = segments.len();
        vec![Some(0), None, Some(n - 2), Some(n - 1)]
    } else {
        (0..segments.len()).map(Some).collect()
    };

    ui.horizontal(|ui| {
        ui.set_height(lay::BREADCRUMB_H);
        ui.spacing_mut().item_spacing.x = sp(2);
        for (k, seg) in shown.iter().enumerate() {
            if k > 0 {
                ui.label(
                    egui::RichText::new("›")
                        .font(font.clone())
                        .color(p.text_muted),
                );
            }
            let Some(i) = *seg else {
                let r = Btn::icon(icon::DOTS_THREE, "Hidden folders")
                    .size(Size::Sm)
                    .show(ui, p);
                if r.clicked() {
                    clicked = Some(1);
                }
                continue;
            };
            let current = i == segments.len() - 1;
            let text = if i == 0 {
                format!("{} {}", icon::HARD_DRIVES, segments[0])
            } else {
                segments[i].to_owned()
            };
            let g = text_galley(
                ui,
                text,
                if current { bold.clone() } else { font.clone() },
                Color32::PLACEHOLDER,
            );
            let sense = if current {
                Sense::hover()
            } else {
                Sense::click()
            };
            let (rect, resp) = ui.allocate_exact_size(g.size() + vec2(4.0, 6.0), sense);
            resp.widget_info(|| WidgetInfo::labeled(WidgetType::Link, true, segments[i]));
            let color = if current || resp.hovered() {
                p.text
            } else {
                p.text_muted
            };
            let pos = rect.center() - g.size() / 2.0;
            let gw = g.size().x;
            ui.painter().galley(pos, g, color);
            if resp.hovered() && !current {
                let y = rect.bottom() - 2.0;
                ui.painter().line_segment(
                    [pos2(pos.x, y), pos2(pos.x + gw, y)],
                    Stroke::new(1.0, color),
                );
            }
            if resp.has_focus() {
                focus_ring(ui, p, rect, radius::SM);
            }
            if resp.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

// ---------------------------------------------------------------- Filter bar

#[derive(Default)]
pub struct FilterBar {
    pub text: String,
    /// Parse error shown under the bar; the previous filter stays applied.
    pub error: Option<String>,
    /// Number of terms in the applied filter, when one is applied.
    pub applied_terms: Option<usize>,
    /// Show the saved-filters button.
    pub presets: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterEvent {
    None,
    Apply,
    Clear,
    Presets,
    Help,
}

pub fn filter_bar(ui: &mut Ui, p: &Palette, fb: &mut FilterBar, id: Id) -> FilterEvent {
    let mut ev = FilterEvent::None;
    let focused = ui.memory(|m| m.has_focus(id));
    let (border, bw) = if fb.error.is_some() {
        (p.danger, 2.0)
    } else if focused {
        (p.focus, 2.0)
    } else if fb.applied_terms.is_some() {
        (p.accent, 1.0)
    } else {
        (p.border_input, 1.0)
    };

    let frame = Frame::new()
        .fill(p.bg)
        .stroke(Stroke::new(bw, border))
        .corner_radius(CornerRadius::same(radius::SM))
        .inner_margin(Margin::symmetric(sp(3) as i8, 0));
    frame.show(ui, |ui| {
        ui.set_height(lay::FILTER_H - 2.0);
        ui.horizontal_centered(|ui| {
            ui.label(egui::RichText::new(icon::FUNNEL).color(p.text_muted));
            let right_w = 3.0 * 26.0
                + if fb.applied_terms.is_some() {
                    64.0
                } else {
                    0.0
                };
            let te = egui::TextEdit::singleline(&mut fb.text)
                .id(id)
                .frame(Frame::NONE)
                .font(fonts::mono(ty::SM))
                .text_color(p.text)
                .hint_text(
                    egui::RichText::new("Filter: *.mkv;>1gb;<6months;|:green").color(p.text_muted),
                )
                .desired_width((ui.available_width() - right_w).max(40.0));
            let r = ui.add(te);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ev = FilterEvent::Apply;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if Btn::icon(icon::QUESTION, "Filter syntax")
                    .size(Size::Sm)
                    .show(ui, p)
                    .clicked()
                {
                    ev = FilterEvent::Help;
                }
                if fb.presets
                    && Btn::icon(icon::CARET_DOWN, "Saved filters")
                        .size(Size::Sm)
                        .show(ui, p)
                        .clicked()
                {
                    ev = FilterEvent::Presets;
                }
                if !fb.text.is_empty()
                    && Btn::icon(icon::X, "Clear filter")
                        .size(Size::Sm)
                        .show(ui, p)
                        .clicked()
                {
                    fb.text.clear();
                    ev = FilterEvent::Clear;
                }
                if let Some(n) = fb.applied_terms {
                    chip(
                        ui,
                        p,
                        &format!("{n} term{}", if n == 1 { "" } else { "s" }),
                        p.accent,
                    );
                }
            });
        });
    });
    if let Some(err) = &fb.error {
        ui.label(
            egui::RichText::new(format!("{} {err}", icon::WARNING))
                .font(fonts::font(ty::SM))
                .color(p.danger),
        );
    }
    ev
}

fn chip(ui: &mut Ui, p: &Palette, text: &str, color: Color32) {
    let g = text_galley(ui, text.to_owned(), fonts::font(ty::XS), color);
    let (rect, _) = ui.allocate_exact_size(g.size() + vec2(12.0, 4.0), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(radius::LG),
        mix(p.bg, color, 0.12),
        Stroke::new(1.0, color),
        StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - g.size() / 2.0, g, color);
}

// ---------------------------------------------------------------- Status bar

#[derive(Clone, Copy, Debug)]
pub enum ScanStatus {
    Scanning { dirs: u64, bytes: u64 },
    Paused { dirs: u64, bytes: u64 },
    Done { secs: f32, errors: u32 },
}

pub struct StatusInfo<'a> {
    pub scan: ScanStatus,
    pub selection: Option<&'a str>,
    pub fs_used: u64,
    pub fs_total: u64,
    pub views: usize,
    pub tag_counts: [u32; 4],
    /// Live updates: (folders watched, hit the watch limit). None when off.
    pub live: Option<(usize, bool)>,
}

/// Returns true when the "unreadable" link was clicked.
pub fn status_bar(ui: &mut Ui, p: &Palette, s: &StatusInfo) -> bool {
    let mut errors_clicked = false;
    let width = ui.available_width();
    let compact = width < lay::COMPACT_BELOW_W;
    let small = fonts::font(ty::SM);
    let rt = |t: String, c: Color32| egui::RichText::new(t).font(small.clone()).color(c);

    let r = Frame::new()
        .fill(p.surface)
        .inner_margin(Margin::symmetric(sp(4) as i8, 0))
        .show(ui, |ui| {
            ui.set_height(lay::STATUS_H);
            ui.set_width(width - 2.0 * sp(4));
            ui.horizontal_centered(|ui| {
                match s.scan {
                    ScanStatus::Scanning { dirs, bytes } => {
                        ui.add(egui::Spinner::new().size(12.0).color(p.accent));
                        ui.label(rt(
                            format!(
                                "Scanning · {} folders · {}",
                                group_digits(dirs),
                                human_size(bytes)
                            ),
                            p.text,
                        ));
                    }
                    ScanStatus::Paused { dirs, bytes } => {
                        ui.label(rt(icon::PAUSE.into(), p.warning));
                        ui.label(rt(
                            format!(
                                "Paused · {} folders · {}",
                                group_digits(dirs),
                                human_size(bytes)
                            ),
                            p.text,
                        ));
                    }
                    ScanStatus::Done { secs, errors } => {
                        ui.label(rt(icon::CHECK_CIRCLE.into(), p.success));
                        ui.label(rt(format!("Done in {secs:.1} s"), p.text));
                        if errors > 0 {
                            let link = ui.add(
                                egui::Label::new(rt(
                                    format!("{} {errors} unreadable", icon::WARNING),
                                    p.warning,
                                ))
                                .sense(Sense::click()),
                            );
                            if link.clicked() {
                                errors_clicked = true;
                            }
                        }
                    }
                }
                if let (Some(sel), false) = (s.selection, compact) {
                    ui.separator();
                    // Leave room for the right-hand group so the two never overlap.
                    let free = s.fs_total.saturating_sub(s.fs_used);
                    let right = format!(
                        "{} of {} used · {} free",
                        human_size(s.fs_used),
                        human_size(s.fs_total),
                        human_size(free)
                    );
                    let mut reserve = text_galley(ui, right, small.clone(), Color32::PLACEHOLDER)
                        .size()
                        .x
                        + 80.0
                        + 4.0 * sp(4);
                    if s.views > 1 {
                        reserve += 70.0;
                    }
                    if s.live.is_some() {
                        reserve += 60.0;
                    }
                    if s.tag_counts.iter().any(|c| *c > 0) {
                        reserve += 120.0;
                    }
                    let w = (ui.available_width() - reserve).max(0.0);
                    ui.scope(|ui| {
                        ui.set_max_width(w);
                        ui.add(egui::Label::new(rt(sel.to_owned(), p.text_muted)).truncate());
                    });
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let free = s.fs_total.saturating_sub(s.fs_used);
                    if compact {
                        ui.label(rt(format!("{} free", human_size(free)), p.text_muted));
                        return;
                    }
                    ui.label(rt(
                        format!(
                            "{} of {} used · {} free",
                            human_size(s.fs_used),
                            human_size(s.fs_total),
                            human_size(free)
                        ),
                        p.text_muted,
                    ));
                    let frac = if s.fs_total > 0 {
                        s.fs_used as f32 / s.fs_total as f32
                    } else {
                        0.0
                    };
                    progress(ui, p, Some(frac), 80.0);
                    if s.views > 1 {
                        ui.separator();
                        ui.label(rt(format!("{} views", s.views), p.text_muted));
                    }
                    if let Some((n, limited)) = s.live {
                        ui.separator();
                        let (txt, col) = if limited {
                            ("Live (limited)", p.warning)
                        } else {
                            ("Live", p.success)
                        };
                        ui.label(rt(format!("{} {txt}", icon::BROADCAST), col))
                            .on_hover_text(format!(
                                "Watching {} folders for changes",
                                group_digits(n as u64)
                            ));
                    }
                    if s.tag_counts.iter().any(|c| *c > 0) {
                        ui.separator();
                        for (tag, n) in Tag::ALL.iter().zip(s.tag_counts).rev() {
                            if n > 0 {
                                ui.label(rt(n.to_string(), p.text_muted));
                                dot(ui, tag.color(), 8.0);
                            }
                        }
                    }
                });
            });
        });
    let rr = r.response.rect;
    ui.painter()
        .line_segment([rr.left_top(), rr.right_top()], Stroke::new(1.0, p.border));
    errors_clicked
}

fn dot(ui: &mut Ui, c: Color32, d: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(d, d), Sense::hover());
    ui.painter()
        .circle(rect.center(), d / 2.0, c, Stroke::new(1.0, tm::LABEL_INK));
}

// ---------------------------------------------------------------- Progress, toggle, swatch

/// Determinate when `frac` is Some, otherwise a moving indeterminate segment.
pub fn progress(ui: &mut Ui, p: &Palette, frac: Option<f32>, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 6.0), Sense::hover());
    let bar = Rect::from_center_size(rect.center(), vec2(width, 4.0));
    let r = CornerRadius::same(2);
    ui.painter().rect_filled(bar, r, p.surface_2);
    match frac {
        Some(f) => {
            let mut fill = bar;
            fill.set_width(bar.width() * f.clamp(0.0, 1.0));
            ui.painter().rect_filled(fill, r, p.accent);
        }
        None => {
            let t = ui.input(|i| i.time) as f32;
            let seg = bar.width() * 0.3;
            let x = bar.left() + (t * 0.8).fract() * (bar.width() + seg) - seg;
            let fill = Rect::from_min_max(
                pos2(x.max(bar.left()), bar.top()),
                pos2((x + seg).min(bar.right()), bar.bottom()),
            );
            ui.painter().rect_filled(fill, r, p.accent);
            ui.ctx().request_repaint();
        }
    }
}

pub fn toggle(ui: &mut Ui, p: &Palette, on: &mut bool, label: &str) -> Response {
    ui.horizontal(|ui| {
        let (rect, mut resp) = ui.allocate_exact_size(vec2(28.0, 16.0), Sense::click());
        if resp.clicked() {
            *on = !*on;
            resp.mark_changed();
        }
        resp.widget_info(|| {
            WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, label)
        });
        let t = ui
            .ctx()
            .animate_bool_with_time(resp.id, *on, super::theme::motion::FAST);
        let r = CornerRadius::same(8);
        if *on {
            ui.painter().rect_filled(rect, r, p.accent);
        } else {
            ui.painter().rect(
                rect,
                r,
                p.bg,
                Stroke::new(1.0, p.border_input),
                StrokeKind::Inside,
            );
        }
        let x = egui::lerp(rect.left() + 8.0..=rect.right() - 8.0, t);
        let knob = if *on { p.on_accent } else { p.border_input };
        ui.painter()
            .circle_filled(pos2(x, rect.center().y), 5.0, knob);
        if resp.has_focus() {
            focus_ring(ui, p, rect, 8);
        }
        ui.label(egui::RichText::new(label).color(p.text));
        resp
    })
    .inner
}

/// Colour swatch with the live contrast of the treemap label ink on it.
pub fn swatch(ui: &mut Ui, p: &Palette, color: Color32, label: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
        ui.painter().rect(
            rect,
            CornerRadius::same(radius::SM),
            color,
            Stroke::new(1.0, p.border),
            StrokeKind::Inside,
        );
        let c = contrast(color, tm::LABEL_INK);
        ui.label(egui::RichText::new(label).color(p.text));
        let (txt, col) = if c >= 4.5 {
            (format!("{c:.1}:1"), p.text_muted)
        } else {
            (format!("{} {c:.1}:1", icon::WARNING), p.warning)
        };
        ui.label(
            egui::RichText::new(txt)
                .font(fonts::font(ty::XS))
                .color(col),
        );
    });
}

// ---------------------------------------------------------------- Tag chip

pub fn tag_chip(ui: &mut Ui, p: &Palette, tag: Tag) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = sp(2);
        dot(ui, tag.color(), 10.0);
        ui.label(
            egui::RichText::new(tag.label())
                .font(fonts::font(ty::SM))
                .color(p.text),
        );
    });
}

// ---------------------------------------------------------------- Menu

/// Menu container (context menus, dropdowns). egui's own popups use the same visuals.
pub fn menu_frame<R>(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(radius::MD))
        .shadow(shadow::MENU)
        .inner_margin(Margin::symmetric(0, sp(2) as i8))
        .show(ui, |ui| {
            ui.set_min_width(200.0);
            ui.set_max_width(260.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
}

pub struct MenuItem<'a> {
    pub icon: &'a str,
    pub label: &'a str,
    pub shortcut: &'a str,
    pub danger: bool,
    pub enabled: bool,
    pub submenu: bool,
    /// Force the highlight (keyboard cursor) for previews.
    pub highlighted: bool,
}

impl<'a> MenuItem<'a> {
    pub fn new(icon: &'a str, label: &'a str) -> Self {
        MenuItem {
            icon,
            label,
            shortcut: "",
            danger: false,
            enabled: true,
            submenu: false,
            highlighted: false,
        }
    }
    pub fn shortcut(mut self, s: &'a str) -> Self {
        self.shortcut = s;
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn submenu(mut self) -> Self {
        self.submenu = true;
        self
    }
    pub fn highlighted(mut self) -> Self {
        self.highlighted = true;
        self
    }

    pub fn show(self, ui: &mut Ui, p: &Palette) -> Response {
        let w = ui.available_width();
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(w, lay::MENU_ITEM_H), sense);
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, self.enabled, self.label));
        let painter = ui.painter();
        if self.enabled && (resp.hovered() || self.highlighted || resp.has_focus()) {
            painter.rect_filled(
                rect.shrink2(vec2(sp(2), 0.0)),
                CornerRadius::same(radius::SM),
                p.surface_2,
            );
        }
        let fg = match (self.enabled, self.danger) {
            (false, _) => p.text_muted.gamma_multiply(0.7),
            (true, true) => p.danger,
            (true, false) => p.text,
        };
        let font = fonts::font(ty::BASE);
        let x = rect.left() + sp(4);
        let ig = painter.layout_no_wrap(self.icon.to_owned(), font.clone(), fg);
        painter.galley(pos2(x, rect.center().y - ig.size().y / 2.0), ig, fg);
        let lg = painter.layout_no_wrap(self.label.to_owned(), font, fg);
        painter.galley(
            pos2(x + 16.0 + sp(3), rect.center().y - lg.size().y / 2.0),
            lg,
            fg,
        );
        let right = if self.submenu {
            icon::CARET_RIGHT.to_owned()
        } else {
            self.shortcut.to_owned()
        };
        if !right.is_empty() {
            let sg = painter.layout_no_wrap(right, fonts::font(ty::SM), p.text_muted);
            painter.galley(
                pos2(
                    rect.right() - sp(4) - sg.size().x,
                    rect.center().y - sg.size().y / 2.0,
                ),
                sg,
                p.text_muted,
            );
        }
        resp
    }
}

pub fn menu_separator(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), sp(3) + 1.0), Sense::hover());
    ui.painter().line_segment(
        [
            pos2(rect.left(), rect.center().y),
            pos2(rect.right(), rect.center().y),
        ],
        Stroke::new(1.0, p.border),
    );
}

pub fn menu_header(ui: &mut Ui, p: &Palette, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    let g = ui
        .painter()
        .layout_no_wrap(text.to_uppercase(), fonts::font(ty::XS), p.text_muted);
    ui.painter().galley(
        pos2(rect.left() + sp(4), rect.center().y - g.size().y / 2.0),
        g,
        p.text_muted,
    );
}

// ---------------------------------------------------------------- Mount row (S01)

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountKind {
    Local,
    Removable,
    Network,
    Home,
}

pub struct Mount<'a> {
    pub kind: MountKind,
    pub path: &'a str,
    pub device: &'a str,
    pub fstype: &'a str,
    pub used: u64,
    pub total: u64,
    /// Why it can't be scanned, if it can't.
    pub unreadable: Option<&'a str>,
}

pub fn mount_row(ui: &mut Ui, p: &Palette, m: &Mount, selected: bool) -> Response {
    let enabled = m.unreadable.is_none();
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(
        vec2(w, 44.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, selected, m.path));
    let painter = ui.painter();
    if enabled && (resp.hovered() || selected) {
        painter.rect_filled(rect, CornerRadius::same(radius::SM), p.surface_2);
    }
    if selected || resp.has_focus() {
        focus_ring(ui, p, rect.shrink(1.0), radius::SM);
    }
    let alpha = if enabled { 1.0 } else { 0.45 };
    let ic = match m.kind {
        MountKind::Local => icon::HARD_DRIVE,
        MountKind::Removable => icon::USB,
        MountKind::Network => icon::GLOBE,
        MountKind::Home => icon::HOUSE,
    };
    let x0 = rect.left() + sp(4);
    let ig = painter.layout_no_wrap(
        ic.to_owned(),
        egui::FontId::proportional(20.0),
        p.text_muted.gamma_multiply(alpha),
    );
    painter.galley(pos2(x0, rect.center().y - ig.size().y / 2.0), ig, p.text);

    let tx = x0 + 28.0;
    let tg = painter.layout_no_wrap(
        m.path.to_owned(),
        fonts::font(ty::MD),
        p.text.gamma_multiply(alpha),
    );
    painter.galley(pos2(tx, rect.top() + 5.0), tg, p.text);
    let sub = match m.unreadable {
        Some(why) => format!("{} · {}  —  {why}", m.device, m.fstype),
        None => format!("{} · {}", m.device, m.fstype),
    };
    let sg = painter.layout_no_wrap(sub, fonts::mono(ty::XS), p.text_muted.gamma_multiply(alpha));
    painter.galley(pos2(tx, rect.top() + 25.0), sg, p.text_muted);

    if m.total > 0 {
        let free = m.total.saturating_sub(m.used);
        let right = rect.right() - sp(4);
        let label = format!(
            "{} of {} · {} free",
            human_size(m.used),
            human_size(m.total),
            human_size(free)
        );
        let lg = painter.layout_no_wrap(
            label,
            fonts::font(ty::SM),
            p.text_muted.gamma_multiply(alpha),
        );
        let lw = lg.size().x;
        painter.galley(pos2(right - lw, rect.top() + 6.0), lg, p.text_muted);
        let bar = Rect::from_min_size(pos2(right - 120.0, rect.top() + 28.0), vec2(120.0, 4.0));
        painter.rect_filled(bar, CornerRadius::same(2), p.surface_2);
        let mut used = bar;
        used.set_width(bar.width() * (m.used as f32 / m.total as f32).clamp(0.0, 1.0));
        let frac = m.used as f32 / m.total as f32;
        let c = if frac > 0.9 {
            p.danger
        } else if frac > 0.75 {
            p.warning
        } else {
            p.accent
        };
        painter.rect_filled(used, CornerRadius::same(2), c.gamma_multiply(alpha));
    }
    if m.kind == MountKind::Network {
        let r = resp.on_hover_text("Network filesystem: scanning may be slow");
        return r;
    }
    resp
}

// ---------------------------------------------------------------- Confirm panel (S06)

pub struct Confirm<'a> {
    pub permanent: bool,
    pub paths: &'a [&'a str],
    pub total: &'a str,
    pub working: bool,
    pub error: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    None,
    Cancel,
    Confirm,
}

pub fn confirm_panel(ui: &mut Ui, p: &Palette, c: &Confirm) -> ConfirmAction {
    let mut act = ConfirmAction::None;
    let r = Frame::new()
        .fill(p.surface)
        .shadow(shadow::POP)
        .inner_margin(Margin::same(sp(6) as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = sp(3);
            let n = c.paths.len();
            let title = if c.permanent {
                format!(
                    "Delete {n} item{} permanently?",
                    if n == 1 { "" } else { "s" }
                )
            } else {
                format!("Move {n} item{} to Trash?", if n == 1 { "" } else { "s" })
            };
            ui.label(
                egui::RichText::new(title)
                    .font(fonts::font(ty::LG))
                    .color(p.text),
            );
            for path in c.paths.iter().take(5) {
                ui.label(
                    egui::RichText::new(*path)
                        .font(fonts::mono(ty::SM))
                        .color(p.text_muted),
                );
            }
            if n > 5 {
                ui.label(egui::RichText::new(format!("+{} more", n - 5)).color(p.text_muted));
            }
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Total").color(p.text_muted));
                ui.label(
                    egui::RichText::new(c.total)
                        .font(fonts::font(ty::MD))
                        .color(p.text),
                );
                if c.permanent {
                    ui.label(
                        egui::RichText::new(format!("{} This can't be undone.", icon::WARNING))
                            .color(p.danger),
                    );
                }
            });
            if let Some(err) = c.error {
                ui.label(egui::RichText::new(format!("{} {err}", icon::X_CIRCLE)).color(p.danger));
            }
            let row = vec2(ui.available_width(), 28.0);
            ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                let (label, ic) = if c.working {
                    (
                        if c.permanent {
                            "Deleting…"
                        } else {
                            "Moving…"
                        },
                        icon::CIRCLE_NOTCH,
                    )
                } else if c.permanent {
                    ("Delete permanently", icon::TRASH)
                } else {
                    ("Move to Trash", icon::TRASH)
                };
                let go = Btn::new(label)
                    .with_icon(ic)
                    .variant(Variant::Danger)
                    .loading(c.working)
                    .show(ui, p);
                if go.clicked() {
                    act = ConfirmAction::Confirm;
                }
                let cancel = ui
                    .add_enabled_ui(!c.working, |ui| Btn::new("Cancel").show(ui, p))
                    .inner;
                if cancel.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    act = ConfirmAction::Cancel;
                }
            });
        });
    let rr = r.response.rect;
    ui.painter()
        .line_segment([rr.left_top(), rr.right_top()], Stroke::new(2.0, p.danger));
    act
}

// ---------------------------------------------------------------- Toast

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

/// Returns true when the action button was clicked.
pub fn toast(ui: &mut Ui, p: &Palette, kind: ToastKind, text: &str, action: Option<&str>) -> bool {
    let (ic, c) = match kind {
        ToastKind::Info => (icon::INFO, p.accent),
        ToastKind::Success => (icon::CHECK_CIRCLE, p.success),
        ToastKind::Warning => (icon::WARNING, p.warning),
        ToastKind::Error => (icon::X_CIRCLE, p.danger),
    };
    let mut clicked = false;
    Frame::new()
        .fill(p.surface_2)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(radius::MD))
        .shadow(shadow::POP)
        .inner_margin(Margin::symmetric(sp(5) as i8, sp(4) as i8))
        .show(ui, |ui| {
            ui.set_max_width(360.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(ic).size(16.0).color(c));
                ui.add(egui::Label::new(egui::RichText::new(text).color(p.text)).wrap());
                if let Some(a) = action
                    && Btn::new(a)
                        .variant(Variant::Ghost)
                        .size(Size::Sm)
                        .show(ui, p)
                        .clicked()
                {
                    clicked = true;
                }
            });
        });
    clicked
}

// ---------------------------------------------------------------- Tabs (S07)

/// Returns (clicked tab, closed tab).
pub fn tabs(
    ui: &mut Ui,
    p: &Palette,
    labels: &[&str],
    active: usize,
) -> (Option<usize>, Option<usize>) {
    let (mut clicked, mut closed) = (None, None);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        for (i, label) in labels.iter().enumerate() {
            let is_active = i == active;
            let g = text_galley(
                ui,
                format!("{}  {label}", i + 1),
                fonts::font(ty::SM),
                Color32::PLACEHOLDER,
            );
            let w = g.size().x + 2.0 * sp(4) + 18.0;
            let (rect, resp) = ui.allocate_exact_size(vec2(w, lay::TABS_H), Sense::click());
            resp.widget_info(|| {
                WidgetInfo::selected(WidgetType::SelectableLabel, true, is_active, *label)
            });
            let fill = if is_active {
                p.bg
            } else if resp.hovered() {
                p.surface_2
            } else {
                p.surface
            };
            ui.painter().rect_filled(
                rect,
                CornerRadius {
                    nw: radius::SM,
                    ne: radius::SM,
                    sw: 0,
                    se: 0,
                },
                fill,
            );
            if is_active {
                let top = Rect::from_min_size(rect.min, vec2(rect.width(), 2.0));
                ui.painter().rect_filled(top, 0, p.accent);
            }
            let fg = if is_active { p.text } else { p.text_muted };
            ui.painter().galley(
                pos2(rect.left() + sp(4), rect.center().y - g.size().y / 2.0),
                g,
                fg,
            );
            let x_rect = Rect::from_center_size(
                pos2(rect.right() - 12.0, rect.center().y),
                vec2(16.0, 16.0),
            );
            let xr = ui.interact(x_rect, ui.id().with(("tab-x", i)), Sense::click());
            if xr.hovered() {
                ui.painter().rect_filled(
                    x_rect,
                    CornerRadius::same(radius::SM),
                    hover_tint(p, fill, 0.1),
                );
            }
            let xg = text_galley(ui, icon::X.into(), fonts::font(ty::XS), fg);
            ui.painter()
                .galley(x_rect.center() - xg.size() / 2.0, xg, fg);
            if xr.clicked() || resp.middle_clicked() {
                closed = Some(i);
            } else if resp.clicked() {
                clicked = Some(i);
            }
        }
    });
    (clicked, closed)
}

// ---------------------------------------------------------------- Details strip (S03)

pub struct Details<'a> {
    pub is_dir: bool,
    pub name: &'a str,
    pub path: &'a str,
    pub size_disk: u64,
    pub size_apparent: u64,
    pub items: Option<u64>,
    pub modified: &'a str,
    pub tag: Option<Tag>,
    pub nlink: u32,
    /// Lead with apparent size (the treemap is in apparent-size mode).
    pub apparent_mode: bool,
}

pub fn details_strip(ui: &mut Ui, p: &Palette, d: &Details) {
    let width = ui.available_width();
    Frame::new()
        .fill(p.surface)
        .inner_margin(Margin::symmetric(sp(4) as i8, 0))
        .show(ui, |ui| {
            ui.set_height(lay::DETAILS_H);
            ui.set_width(width - 2.0 * sp(4));
            ui.horizontal_centered(|ui| {
                let sm = fonts::font(ty::SM);
                let rt = |t: String, c: Color32| egui::RichText::new(t).font(sm.clone()).color(c);
                ui.label(rt(
                    (if d.is_dir { icon::FOLDER } else { icon::FILE }).into(),
                    p.text_muted,
                ));
                ui.label(
                    egui::RichText::new(d.name)
                        .font(fonts::bold(ty::SM))
                        .color(p.text),
                );
                let (main, other, other_name) = if d.apparent_mode {
                    (d.size_apparent, d.size_disk, "on disk")
                } else {
                    (d.size_disk, d.size_apparent, "apparent")
                };
                ui.label(rt(human_size(main), p.text));
                let a = d.size_apparent as f64;
                let dsk = d.size_disk as f64;
                if dsk > 0.0 && (a - dsk).abs() / dsk > 0.05 {
                    let hint = if a > dsk {
                        "sparse or compressed"
                    } else {
                        "block overhead"
                    };
                    ui.label(rt(
                        format!("({} {other_name}, {hint})", human_size(other)),
                        p.text_muted,
                    ));
                }
                if let Some(n) = d.items {
                    ui.label(rt(format!("{} items", group_digits(n)), p.text_muted));
                }
                ui.label(rt(format!("modified {}", d.modified), p.text_muted));
                if d.nlink > 1 {
                    ui.label(rt(format!("hardlinked ×{}", d.nlink), p.text_muted));
                }
                if let Some(t) = d.tag {
                    tag_chip(ui, p, t);
                }
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(d.path)
                            .font(fonts::mono(ty::XS))
                            .color(p.text_muted),
                    )
                    .truncate(),
                );
            });
        });
}

// ---------------------------------------------------------------- Section header

pub fn section(ui: &mut Ui, p: &Palette, title: &str) {
    ui.add_space(sp(5));
    ui.label(
        egui::RichText::new(title)
            .font(fonts::font(ty::MD))
            .color(p.text),
    );
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0, p.border);
    ui.add_space(sp(3));
}

// ---------------------------------------------------------------- Time

/// "just now", "5 minutes ago", "3 months ago", "2 years ago".
pub fn ago(secs: i64) -> String {
    let s = secs.max(0) as f64;
    let (n, unit) = if s < 60.0 {
        return "just now".into();
    } else if s < 3600.0 {
        (s / 60.0, "minute")
    } else if s < 86_400.0 {
        (s / 3600.0, "hour")
    } else if s < 86_400.0 * 30.44 {
        (s / 86_400.0, "day")
    } else if s < 86_400.0 * 365.25 {
        (s / (86_400.0 * 30.44), "month")
    } else {
        (s / (86_400.0 * 365.25), "year")
    };
    let n = n.floor() as u64;
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}
