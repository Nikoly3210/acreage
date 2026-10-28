//! Side panels (S08 export, S09 settings, S11 errors), the trash confirm (S06),
//! popups (S10 presets and help, overflow menu) and toasts.

use std::path::PathBuf;
use std::time::Duration;

use egui::{Align2, Id, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use super::{Browse, Side, mode_of, now_secs};
use crate::config::{Detail, Preset};
use crate::report::{self, Format, Header, ReportOpts};
use crate::ui::fonts;
use crate::ui::theme::{Palette, layout as lay, sp, ty};
use crate::ui::widgets::{self as w, Btn, MenuItem, Size, ToastKind, Variant};
use crate::view::SizeMode;

fn expand_home(p: &str) -> PathBuf {
    match p.trim().strip_prefix('~') {
        Some(rest) => PathBuf::from(format!(
            "{}{rest}",
            std::env::var("HOME").unwrap_or_default()
        )),
        None => PathBuf::from(p.trim()),
    }
}

/// Panel title row with a close button. Returns true when closed.
fn title(ui: &mut Ui, p: &Palette, text: &str) -> bool {
    let mut closed = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(text)
                .font(fonts::font(ty::LG))
                .color(p.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if Btn::icon(icon::X, "Close (Esc)").show(ui, p).clicked() {
                closed = true;
            }
        });
    });
    ui.add_space(sp(3));
    closed
}

fn side_panel(id: &'static str, p: &Palette) -> egui::Panel {
    egui::Panel::right(id)
        .frame(
            egui::Frame::new()
                .fill(p.surface)
                .inner_margin(egui::Margin::same(sp(6) as i8)),
        )
        .default_size(lay::SIDE_PANEL_W)
        .resizable(true)
}

fn label(ui: &mut Ui, p: &Palette, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(fonts::font(ty::SM))
            .color(p.text_muted),
    );
}

impl Browse {
    pub(super) fn confirm_panel(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(pending) = self.confirm.as_ref() else {
            return;
        };
        let paths: Vec<String> = pending
            .items
            .iter()
            .map(|id| self.tree.path(*id).to_string_lossy().into_owned())
            .collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let total: u64 = pending
            .items
            .iter()
            .map(|id| crate::view::node_size(&self.tree, *id, self.mode))
            .sum();
        let total_s = w::human_size(total);
        let working = pending.working;
        let act = egui::Panel::bottom("confirm")
            .frame(egui::Frame::NONE)
            .show_separator_line(false)
            .resizable(false)
            .show(ui, |ui| {
                w::confirm_panel(
                    ui,
                    p,
                    &w::Confirm {
                        permanent: false,
                        paths: &refs,
                        total: &total_s,
                        working,
                        error: None,
                    },
                )
            });
        match act.inner {
            w::ConfirmAction::Confirm => self.do_trash(),
            w::ConfirmAction::Cancel if !working => self.confirm = None,
            _ => {}
        }
    }

    pub(super) fn errors_panel(&mut self, ui: &mut Ui, p: &Palette) {
        side_panel("errors", p).show(ui, |ui| {
            if title(ui, p, "Unreadable folders") {
                self.side = Side::None;
            }
            let n = self.errors.len();
            let s = if n == 1 { "" } else { "s" };
            ui.label(
                egui::RichText::new(format!(
                    "{n} folder{s} couldn't be read. Their size isn't counted."
                ))
                .color(p.text_muted),
            );
            ui.add_space(sp(4));
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (id, err) in &self.errors {
                    if !self.tree.exists(*id) {
                        continue;
                    }
                    ui.label(
                        egui::RichText::new(self.tree.path(*id).to_string_lossy())
                            .font(fonts::mono(ty::SM))
                            .color(p.text),
                    );
                    ui.label(
                        egui::RichText::new(err.to_string())
                            .font(fonts::font(ty::XS))
                            .color(p.text_muted),
                    );
                    ui.add_space(sp(2));
                }
            });
        });
    }

    // ---- S08 export

    fn report_text(&mut self) -> String {
        let view = &self.tabs[self.active].view;
        let key = format!(
            "{}|{}|{}|{:?}|{}|{}|{}|{:?}|{}",
            view.zoom,
            self.export.depth,
            self.export.files,
            self.export.format,
            self.tree.generation,
            view.filter_src,
            self.tags_gen,
            self.mode,
            view.hide_dot
        );
        if let Some((k, text)) = &self.export.cache
            && *k == key
        {
            return text.clone();
        }
        let opts = ReportOpts {
            depth: self.export.depth,
            files: self.export.files,
            top: 50,
            format: self.export.format,
        };
        let size_label = if self.mode == SizeMode::Apparent {
            "apparent size"
        } else {
            "disk usage"
        };
        let filter = (!view.filter_src.is_empty()).then_some(view.filter_src.as_str());
        let header = Header {
            when: report::utc_stamp(now_secs()),
            size_label,
            filter,
        };
        let size = |id| view.size(&self.tree, id, self.mode);
        let text = report::render(&self.tree, view.zoom, &size, &opts, &header);
        self.export.cache = Some((key, text.clone()));
        text
    }

    pub(super) fn export_panel(&mut self, ui: &mut Ui, p: &Palette) {
        let text = self.report_text();
        let zoom_path = self.tree.path(self.tabs[self.active].view.zoom);
        let filter = self.tabs[self.active].view.filter_src.clone();
        side_panel("export", p).show(ui, |ui| {
            if title(ui, p, "Export report") {
                self.side = Side::None;
            }
            label(ui, p, "Folder (the one this view is zoomed into)");
            ui.label(
                egui::RichText::new(zoom_path.to_string_lossy())
                    .font(fonts::mono(ty::SM))
                    .color(p.text),
            );
            if !filter.is_empty() {
                label(
                    ui,
                    p,
                    &format!("Only what matches this view's filter: {filter}"),
                );
            }
            ui.add_space(sp(3));
            ui.horizontal(|ui| {
                label(ui, p, "Levels");
                if Btn::icon(icon::MINUS, "Fewer levels")
                    .size(Size::Sm)
                    .show(ui, p)
                    .clicked()
                    && self.export.depth > 1
                {
                    self.export.depth -= 1;
                }
                ui.label(
                    egui::RichText::new(self.export.depth.to_string())
                        .font(fonts::bold(ty::BASE))
                        .color(p.text),
                );
                if Btn::icon(icon::PLUS, "More levels")
                    .size(Size::Sm)
                    .show(ui, p)
                    .clicked()
                    && self.export.depth < 12
                {
                    self.export.depth += 1;
                }
            });
            w::toggle(
                ui,
                p,
                &mut self.export.files,
                "List files, not just folder totals",
            );
            ui.horizontal(|ui| {
                label(ui, p, "Format");
                for (f, name) in [
                    (Format::Text, "Text"),
                    (Format::Csv, "CSV"),
                    (Format::Json, "JSON"),
                ] {
                    let on = self.export.format == f;
                    if Btn::new(name)
                        .variant(Variant::Ghost)
                        .size(Size::Sm)
                        .toggled(on)
                        .show(ui, p)
                        .clicked()
                        && !on
                    {
                        let old = format!(".{}", self.export.format.ext());
                        if let Some(stem) = self.export.path.strip_suffix(&old) {
                            self.export.path = format!("{stem}.{}", f.ext());
                        }
                        self.export.format = f;
                    }
                }
            });
            ui.add_space(sp(3));
            label(ui, p, "Preview");
            let preview: String = text.lines().take(80).collect::<Vec<_>>().join("\n");
            egui::Frame::new()
                .fill(p.bg)
                .corner_radius(egui::CornerRadius::same(crate::ui::theme::radius::SM))
                .inner_margin(egui::Margin::same(sp(4) as i8))
                .show(ui, |ui| {
                    egui::ScrollArea::both()
                        .max_height((ui.available_height() - 110.0).max(80.0))
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(preview)
                                        .font(fonts::mono(ty::XS))
                                        .color(p.text),
                                )
                                .extend(),
                            );
                        });
                });
            ui.add_space(sp(3));
            label(ui, p, "Save to");
            ui.add(
                egui::TextEdit::singleline(&mut self.export.path)
                    .font(fonts::mono(ty::SM))
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if Btn::new("Save")
                    .with_icon(icon::FLOPPY_DISK)
                    .variant(Variant::Primary)
                    .show(ui, p)
                    .clicked()
                {
                    self.export_save();
                }
                if Btn::new("Copy").with_icon(icon::COPY).show(ui, p).clicked() {
                    self.ctx.copy_text(text.clone());
                    let n = text.lines().count();
                    self.toast(ToastKind::Info, format!("Copied the report ({n} lines)"));
                }
            });
        });
    }

    /// Write the current report to the path in the export panel (also Ctrl+S).
    pub(super) fn export_save(&mut self) {
        let text = self.report_text();
        let target = expand_home(&self.export.path);
        match std::fs::write(&target, &text) {
            Ok(()) => self.toast(
                ToastKind::Success,
                format!("Saved report to {}", target.display()),
            ),
            Err(e) => self.toast(
                ToastKind::Error,
                format!("Couldn't write {}: {e}", target.display()),
            ),
        }
    }

    // ---- S09 settings

    fn save_cfg(&mut self) {
        if let Err(e) = self.cfg.save() {
            let path = crate::config::path();
            self.toast(
                ToastKind::Error,
                format!("Couldn't save settings to {}: {e}", path.display()),
            );
        }
    }

    pub(super) fn settings_panel(&mut self, ui: &mut Ui, p: &Palette) {
        let before = self.cfg.clone();
        side_panel("settings", p).show(ui, |ui| {
            if title(ui, p, "Settings") {
                self.side = Side::None;
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = sp(3);
                w::section(ui, p, "Look");
                ui.horizontal(|ui| {
                    label(ui, p, "Theme");
                    for (v, name) in [("dark", "Dark"), ("light", "Light")] {
                        let on = self.cfg.theme == v;
                        if Btn::new(name)
                            .variant(Variant::Ghost)
                            .size(Size::Sm)
                            .toggled(on)
                            .show(ui, p)
                            .clicked()
                        {
                            self.cfg.theme = v.into();
                        }
                    }
                });
                w::toggle(
                    ui,
                    p,
                    &mut self.cfg.animations,
                    "Animations (zoom, blink on change)",
                );
                ui.horizontal(|ui| {
                    label(ui, p, "Detail");
                    for (v, name) in [
                        (Detail::Coarse, "Fewer blocks"),
                        (Detail::Balanced, "Balanced"),
                        (Detail::Fine, "Every file that fits"),
                    ] {
                        let on = self.cfg.detail == v;
                        if Btn::new(name)
                            .variant(Variant::Ghost)
                            .size(Size::Sm)
                            .toggled(on)
                            .show(ui, p)
                            .clicked()
                        {
                            self.cfg.detail = v;
                        }
                    }
                });

                w::section(ui, p, "Sizes");
                ui.horizontal(|ui| {
                    for (v, name) in [
                        ("disk", "Space on disk (like du)"),
                        ("apparent", "File length"),
                    ] {
                        let on = self.cfg.size == v;
                        if Btn::new(name)
                            .variant(Variant::Ghost)
                            .size(Size::Sm)
                            .toggled(on)
                            .show(ui, p)
                            .clicked()
                        {
                            self.cfg.size = v.into();
                        }
                    }
                });
                w::toggle(
                    ui,
                    p,
                    &mut self.cfg.hide_dotfiles,
                    "Hide dotfiles and dot-folders",
                );

                w::section(ui, p, "Scanning");
                w::toggle(
                    ui,
                    p,
                    &mut self.cfg.one_filesystem,
                    "Stay on one filesystem (applies to the next scan)",
                );
                w::toggle(
                    ui,
                    p,
                    &mut self.cfg.live_updates,
                    "Live updates while files change",
                );

                w::section(ui, p, "Saved filters");
                let mut remove = None;
                if self.cfg.presets.is_empty() {
                    label(ui, p, "None yet. Use the ▾ in the filter bar to save one.");
                }
                for (i, pr) in self.cfg.presets.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&pr.name).color(p.text));
                        ui.label(
                            egui::RichText::new(&pr.filter)
                                .font(fonts::mono(ty::XS))
                                .color(p.text_muted),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if Btn::icon(icon::X, "Remove this filter")
                                .size(Size::Sm)
                                .show(ui, p)
                                .clicked()
                            {
                                remove = Some(i);
                            }
                        });
                    });
                }
                if let Some(i) = remove {
                    self.cfg.presets.remove(i);
                }
                ui.add_space(sp(5));
                label(
                    ui,
                    p,
                    &format!("Saved in {}", crate::config::path().display()),
                );
            });
        });
        if self.cfg != before {
            self.settings_changed(&before);
            self.save_cfg();
        }
    }

    fn settings_changed(&mut self, before: &crate::config::Config) {
        let cfg = self.cfg.clone();
        if cfg.size != before.size {
            self.mode = mode_of(&cfg);
        }
        if cfg.hide_dotfiles != before.hide_dotfiles {
            for t in &mut self.tabs {
                t.view.set_hide_dot(cfg.hide_dotfiles);
            }
        }
        if cfg.detail != before.detail {
            for t in &mut self.tabs {
                t.view.invalidate();
            }
        }
        if cfg.live_updates != before.live_updates {
            if cfg.live_updates {
                if !self.scanning() {
                    self.start_watcher();
                }
            } else {
                self.stop_watcher();
            }
        }
        if !cfg.animations {
            self.changed.clear();
            self.anim = None;
        }
        if cfg.one_filesystem != before.one_filesystem {
            self.toast(
                ToastKind::Info,
                "Applies to the next full rescan (Ctrl+R).".into(),
            );
        }
    }

    // ---- S10 presets and help

    pub(super) fn presets_menu(&mut self, ui: &mut Ui, p: &Palette) {
        let applied = self.tabs[self.active].view.filter_src.clone();
        let can_save =
            !applied.is_empty() && !self.cfg.presets.iter().any(|pr| pr.filter == applied);
        let x = ui.max_rect().right() - 300.0 - sp(4);
        let y = lay::TOOLBAR_H + lay::BREADCRUMB_H + lay::FILTER_H + sp(3);
        let mut pick = None;
        let mut save = false;
        let r = egui::Area::new(Id::new("presets-menu"))
            .fixed_pos(pos2(x, y))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                w::menu_frame(ui, p, |ui| {
                    ui.set_min_width(300.0);
                    w::menu_header(ui, p, "Saved filters");
                    for pr in &self.cfg.presets {
                        if MenuItem::new(icon::FUNNEL, &pr.name)
                            .shortcut(&pr.filter)
                            .show(ui, p)
                            .clicked()
                        {
                            pick = Some(pr.filter.clone());
                        }
                    }
                    w::menu_separator(ui, p);
                    if MenuItem::new(icon::PLUS, "Save current filter")
                        .enabled(can_save)
                        .show(ui, p)
                        .clicked()
                    {
                        save = true;
                    }
                });
            });
        if let Some(f) = pick {
            self.set_filter_text(&f);
            self.show_presets = false;
        }
        if save {
            self.cfg.presets.push(Preset {
                name: applied.clone(),
                filter: applied,
            });
            self.save_cfg();
            self.show_presets = false;
        }
        if r.response.clicked_elsewhere() && ui.ctx().cumulative_pass_nr() != self.popup_opened_at {
            self.show_presets = false;
        }
    }

    pub(super) fn help_window(&mut self, ui: &mut Ui, p: &Palette) {
        let mut open = self.show_help;
        egui::Window::new("Filter syntax")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::RIGHT_TOP, vec2(-sp(6), 110.0))
            .show(ui.ctx(), |ui| {
                let rows = [
                    ("*.mkv;*.mp4", "either extension"),
                    (
                        "cache",
                        "names containing “cache” (folders include their contents)",
                    ),
                    (">1gb  <500mb", "size; together they make a range"),
                    (">2years  <3months", "older than / newer than (h d w mo y)"),
                    (":red  :all", "tagged red / tagged with anything"),
                    ("|*.log  |:green", "leave these out"),
                ];
                egui::Grid::new("syntax")
                    .num_columns(2)
                    .spacing(vec2(sp(6), sp(3)))
                    .show(ui, |ui| {
                        for (k, v) in rows {
                            ui.label(
                                egui::RichText::new(k)
                                    .font(fonts::mono(ty::SM))
                                    .color(p.accent),
                            );
                            ui.label(egui::RichText::new(v).color(p.text));
                            ui.end_row();
                        }
                    });
                ui.add_space(sp(3));
                ui.label(
                    egui::RichText::new("Separate terms with ; and press Enter.")
                        .color(p.text_muted),
                );
            });
        self.show_help = open;
    }

    /// The block menu opened from the keyboard (Shift+F10), at the selected block.
    pub(super) fn kb_menu_ui(&mut self, ui: &mut Ui, p: &Palette) {
        let Some((id, pos)) = self.kb_menu else {
            return;
        };
        if !self.tree.exists(id) {
            self.kb_menu = None;
            return;
        }
        let focus = std::mem::take(&mut self.kb_menu_focus);
        let mut chosen = None;
        let r = egui::Area::new(Id::new("kb-menu"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                w::menu_frame(ui, p, |ui| {
                    chosen = self.menu(ui, p, id, focus);
                });
            });
        if let Some(a) = chosen {
            self.kb_menu = None;
            self.run_menu(id, a);
        } else if r.response.clicked_elsewhere() {
            self.kb_menu = None;
        }
    }

    /// `?`: every keyboard shortcut in one place.
    pub(super) fn keys_window(&mut self, ui: &mut Ui, p: &Palette) {
        let mut open = self.show_keys;
        egui::Window::new("Keyboard shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                let groups: [(&str, &[(&str, &str)]); 4] = [
                    (
                        "Move",
                        &[
                            ("h j k l / arrows", "select the next block"),
                            ("Enter", "zoom in (or scan into another filesystem)"),
                            ("Backspace, Alt+Up", "up one folder"),
                            ("Alt+Left, mouse back", "back"),
                            ("+ / -", "more or less detail"),
                        ],
                    ),
                    (
                        "Act on the selected block",
                        &[
                            ("Shift+F10", "open its menu"),
                            ("Ctrl+1…4, Ctrl+0", "tag red/yellow/green/blue, untag"),
                            ("y", "copy its path"),
                            ("Ctrl+O, Ctrl+T", "open it, terminal there"),
                            ("Delete", "move to Trash (asks first)"),
                        ],
                    ),
                    (
                        "Scan and views",
                        &[
                            ("F5, Ctrl+R", "rescan this folder, rescan everything"),
                            ("p", "pause or resume the scan"),
                            ("/", "filter"),
                            (
                                "Ctrl+N, Ctrl+W, Ctrl+Tab",
                                "new view, close view, next view",
                            ),
                        ],
                    ),
                    (
                        "Panels",
                        &[
                            ("Ctrl+E (then Ctrl+S)", "export report (save it)"),
                            ("Ctrl+,", "settings"),
                            ("Ctrl+Shift+T", "light or dark theme"),
                            ("?, Esc, Ctrl+Q", "this list, close what's open, quit"),
                        ],
                    ),
                ];
                // One grid for every group, so the columns line up.
                egui::Grid::new("keys")
                    .num_columns(2)
                    .spacing(vec2(sp(6), sp(2)))
                    .show(ui, |ui| {
                        for (title, rows) in groups {
                            ui.label(
                                egui::RichText::new(title)
                                    .font(fonts::font(ty::MD))
                                    .color(p.text),
                            );
                            ui.end_row();
                            for (k, v) in rows {
                                ui.label(
                                    egui::RichText::new(*k)
                                        .font(fonts::mono(ty::SM))
                                        .color(p.accent),
                                );
                                ui.label(egui::RichText::new(*v).color(p.text));
                                ui.end_row();
                            }
                        }
                    });
            });
        self.show_keys = open;
    }

    /// Overflow menu for the compact toolbar. Returns true for "Open location".
    pub(super) fn more_menu(&mut self, ui: &mut Ui, p: &Palette) -> bool {
        let mut open_picker = false;
        let scanning = self.scanning();
        let paused = self.paused();
        let r = egui::Area::new(Id::new("more-menu"))
            .fixed_pos(pos2(sp(2), lay::TOOLBAR_H))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                w::menu_frame(ui, p, |ui| {
                    let (ic, label) = if paused {
                        (icon::PLAY, "Resume scan")
                    } else {
                        (icon::PAUSE, "Pause scan")
                    };
                    if MenuItem::new(ic, label)
                        .shortcut("p")
                        .enabled(scanning)
                        .show(ui, p)
                        .clicked()
                    {
                        if let Some(s) = &self.scanner {
                            s.set_paused(!s.is_paused());
                        }
                        self.show_more = false;
                    }
                    if MenuItem::new(icon::FOLDER_OPEN, "Open location")
                        .show(ui, p)
                        .clicked()
                    {
                        open_picker = true;
                    }
                    if MenuItem::new(icon::PLUS, "New view")
                        .shortcut("Ctrl+N")
                        .show(ui, p)
                        .clicked()
                    {
                        self.new_tab();
                        self.show_more = false;
                    }
                    if MenuItem::new(icon::TAG, "Show tagged")
                        .show(ui, p)
                        .clicked()
                    {
                        self.set_filter_text(":all");
                        self.show_more = false;
                    }
                    if MenuItem::new(icon::EXPORT, "Export report")
                        .shortcut("Ctrl+E")
                        .show(ui, p)
                        .clicked()
                    {
                        self.side = Side::Export;
                        self.show_more = false;
                    }
                    if MenuItem::new(icon::GEAR, "Settings")
                        .shortcut("Ctrl+,")
                        .show(ui, p)
                        .clicked()
                    {
                        self.side = Side::Settings;
                        self.show_more = false;
                    }
                });
            });
        if r.response.clicked_elsewhere() && ui.ctx().cumulative_pass_nr() != self.popup_opened_at {
            self.show_more = false;
        }
        open_picker
    }

    pub(super) fn toasts_ui(&mut self, ui: &mut Ui, p: &Palette) {
        if self.toasts.is_empty() {
            return;
        }
        let ctx = ui.ctx().clone();
        let offset = vec2(-sp(6), -(lay::STATUS_H + lay::DETAILS_H + sp(6)));
        let mut dismiss = None;
        egui::Area::new(Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, offset)
            .order(egui::Order::Foreground)
            .show(&ctx, |ui| {
                ui.spacing_mut().item_spacing.y = sp(2);
                for (i, t) in self.toasts.iter().enumerate() {
                    if w::toast(ui, p, t.kind, &t.text, Some("Dismiss")) {
                        dismiss = Some(i);
                    }
                }
            });
        if let Some(i) = dismiss {
            self.toasts.remove(i);
        }
        let next = self
            .toasts
            .iter()
            .map(|t| t.until)
            .fold(f64::INFINITY, f64::min);
        ctx.request_repaint_after(Duration::from_secs_f64(
            (next - ui.input(|i| i.time)).max(0.05),
        ));
    }
}
