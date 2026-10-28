//! The treemap canvas (S02/S04), the context menu (S05) and the canvas empty states.

use std::collections::HashMap;
use std::time::Duration;

use egui::{Rect, Sense, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use super::{Browse, MenuAction, now_secs, params};
use crate::actions;
use crate::layout::Rect as LRect;
use crate::model::{Kind, NodeId, Tag};
use crate::ui::fonts;
use crate::ui::theme::{Palette, layout as lay, motion, sp, ty};
use crate::ui::treemap_paint::{BlockKind, BlockState, Category, paint_block};
use crate::ui::widgets::{self as w, Btn, Variant};
use crate::view::Item;

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn lerp_rect(a: LRect, b: LRect, t: f32) -> LRect {
    LRect::new(
        a.x + (b.x - a.x) * t,
        a.y + (b.y - a.y) * t,
        a.w + (b.w - a.w) * t,
        a.h + (b.h - a.h) * t,
    )
}

fn to_e(r: LRect) -> Rect {
    Rect::from_min_size(pos2(r.x, r.y), vec2(r.w, r.h))
}

fn to_l(r: Rect) -> LRect {
    LRect::new(r.left(), r.top(), r.width(), r.height())
}

impl Browse {
    pub(super) fn canvas(&mut self, ui: &mut Ui, p: &Palette) {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, p.tm_canvas);
        let bounds = to_l(rect.shrink(lay::tm::GAP));
        let throttle = Duration::from_secs_f32(motion::RELAYOUT_THROTTLE);
        let busy = self.scanning() || !self.subs.is_empty();
        let relaid = self.tabs[self.active].view.ensure_layout(
            &self.tree,
            &self.tags,
            self.tags_gen,
            bounds,
            &params(self.cfg.detail),
            self.mode,
            now_secs(),
            throttle,
        );
        if busy && !relaid {
            ui.ctx().request_repaint_after(throttle);
        }

        // Zoom animation: map layout coordinates through a rect-to-rect transform.
        let now = ui.input(|i| i.time);
        let view = &self.tabs[self.active].view;
        if let Some(a) = &mut self.anim
            && !a.zoom_in
            && a.from == LRect::ZERO
        {
            // Zoom out: the old zoom node's rect in the new layout is now known.
            let old = view.selected.or(view.history.last().copied());
            a.from = old
                .and_then(|id| view.index_of(id))
                .map(|i| view.laid[i].rect)
                .unwrap_or(bounds);
        }
        let t = self
            .anim
            .as_ref()
            .map(|a| ((now - a.start) as f32 / motion::ZOOM).clamp(0.0, 1.0));
        let (src, dst) = match (&self.anim, t) {
            (Some(a), Some(t)) if t < 1.0 => {
                ui.ctx().request_repaint();
                let e = ease_out(t);
                if a.zoom_in {
                    (bounds, lerp_rect(a.from, bounds, e))
                } else {
                    (lerp_rect(a.from, bounds, e), bounds)
                }
            }
            _ => {
                self.anim = None;
                (bounds, bounds)
            }
        };
        let map = |r: LRect| -> Rect {
            let sx = dst.w / src.w.max(1.0);
            let sy = dst.h / src.h.max(1.0);
            to_e(LRect::new(
                dst.x + (r.x - src.x) * sx,
                dst.y + (r.y - src.y) * sy,
                r.w * sx,
                r.h * sy,
            ))
        };

        // Hover, clicks.
        self.hovered = if self.anim.is_none() {
            resp.hover_pos()
                .and_then(|pos| self.tabs[self.active].view.hit(pos.x, pos.y))
        } else {
            None
        };
        let hovered_node = self.hovered_node();
        if resp.clicked() {
            self.tabs[self.active].view.selected = hovered_node;
        }
        if resp.double_clicked()
            && let Some(n) = hovered_node
        {
            if self.tree.get(n).kind == Kind::Dir {
                self.zoom_in(n);
            } else if self.tree.get(n).kind == Kind::MountPoint {
                self.enter_mount(n);
            } else {
                self.tabs[self.active].view.selected = Some(n);
            }
        }
        if resp.secondary_clicked() {
            self.menu_target = hovered_node;
            if hovered_node.is_some() {
                self.tabs[self.active].view.selected = hovered_node;
            }
        }

        let blink = self.blink_levels(now);

        // Paint.
        let tree = &self.tree;
        let view = &self.tabs[self.active].view;
        for (i, l) in view.laid.iter().enumerate() {
            let r = map(l.rect);
            if !r.intersects(rect) {
                continue;
            }
            match l.item {
                Item::Node(id) => {
                    let n = tree.get(id);
                    let depth = l.depth as usize;
                    let name = if id == tree.root {
                        self.root_path.to_string_lossy()
                    } else {
                        tree.name_lossy(id)
                    };
                    let size = w::human_size(view.size(tree, id, self.mode));
                    let (kind, size_label) = match n.kind {
                        Kind::Dir => (BlockKind::Dir { depth }, size),
                        Kind::File => (BlockKind::File(Category::from_name(&name)), size),
                        Kind::Symlink | Kind::Other => (BlockKind::File(Category::Other), size),
                        Kind::Denied => (BlockKind::Denied, "No access".to_owned()),
                        Kind::MountPoint => (
                            BlockKind::MountPoint { depth },
                            "other filesystem".to_owned(),
                        ),
                    };
                    let st = BlockState {
                        hovered: self.hovered == Some(i),
                        selected: view.selected == Some(id),
                        tag: self.tags.get(&id).copied(),
                        blink: blink.get(&i).copied().unwrap_or(0.0),
                        pending: n.kind == Kind::Dir && !tree.is_scanned(id),
                    };
                    paint_block(&painter, p, r, kind, &name, &size_label, &st);
                }
                Item::Agg { count, size, .. } => {
                    let label = format!("{} small items", w::group_digits(count as u64));
                    let st = BlockState {
                        hovered: self.hovered == Some(i),
                        blink: blink.get(&i).copied().unwrap_or(0.0),
                        ..Default::default()
                    };
                    paint_block(
                        &painter,
                        p,
                        r,
                        BlockKind::SmallAgg,
                        &label,
                        &w::human_size(size),
                        &st,
                    );
                }
            }
        }

        self.empty_states(ui, p, rect);

        // Context menu (S05).
        let mut chosen = None;
        if let Some(target) = self.menu_target.filter(|t| self.tree.exists(*t)) {
            resp.context_menu(|ui| {
                chosen = self.menu(ui, p, target, false).map(|a| (target, a));
            });
        }
        if let Some((id, a)) = chosen {
            self.run_menu(id, a);
        }
    }

    /// Blink strength per laid block: a changed node lights up itself, or the nearest
    /// enclosing block that is visible when it's too small to draw.
    fn blink_levels(&self, now: f64) -> HashMap<usize, f32> {
        let mut out = HashMap::new();
        if self.changed.is_empty() {
            return out;
        }
        let view = &self.tabs[self.active].view;
        let index: HashMap<NodeId, usize> = view
            .laid
            .iter()
            .enumerate()
            .filter_map(|(i, l)| match l.item {
                Item::Node(n) => Some((n, i)),
                Item::Agg { .. } => None,
            })
            .collect();
        for (id, t) in &self.changed {
            let level = (1.0 - (now - t) as f32 / motion::BLINK).clamp(0.0, 1.0);
            if level <= 0.0 || !self.tree.exists(*id) {
                continue;
            }
            // Don't light up the whole view for a change deep inside it.
            if let Some(i) = self
                .tree
                .ancestors(*id)
                .find_map(|a| index.get(&a).copied())
                .filter(|i| *i != 0)
            {
                let e = out.entry(i).or_insert(0.0f32);
                *e = e.max(level);
            }
        }
        out
    }

    /// The block menu (S05). `focus_first` puts keyboard focus on the first item (Shift+F10).
    pub(super) fn menu(
        &self,
        ui: &mut Ui,
        p: &Palette,
        id: NodeId,
        focus_first: bool,
    ) -> Option<MenuAction> {
        use crate::ui::widgets::{MenuItem, menu_header, menu_separator};
        let n = self.tree.get(id);
        let is_dir = n.kind == Kind::Dir;
        let can_trash = id != self.tree.root && n.kind != Kind::MountPoint;
        let has_term = actions::terminal_command().is_some();
        let mut out = None;
        ui.set_min_width(240.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        let name = self.tree.name_lossy(id);
        menu_header(ui, p, &name);
        let mut first = focus_first;
        let mut item = |ui: &mut Ui, it: MenuItem, a: MenuAction| {
            let r = it.show(ui, p);
            if std::mem::take(&mut first) {
                r.request_focus();
            }
            if r.clicked() {
                out = Some(a);
                ui.close();
            }
        };
        item(
            ui,
            MenuItem::new(icon::ARROW_SQUARE_OUT, "Open").shortcut("Ctrl+O"),
            MenuAction::Open,
        );
        item(
            ui,
            MenuItem::new(icon::FOLDER_OPEN, "Open containing folder"),
            MenuAction::OpenFolder,
        );
        item(
            ui,
            MenuItem::new(icon::TERMINAL, "Open terminal here")
                .shortcut("Ctrl+T")
                .enabled(has_term),
            MenuAction::Terminal,
        );
        item(
            ui,
            MenuItem::new(icon::COPY, "Copy path").shortcut("y"),
            MenuAction::CopyPath,
        );
        if is_dir {
            item(
                ui,
                MenuItem::new(icon::MAGNIFYING_GLASS_PLUS, "Zoom in").shortcut("Enter"),
                MenuAction::ZoomIn,
            );
            item(
                ui,
                MenuItem::new(icon::ARROWS_CLOCKWISE, "Rescan this folder")
                    .enabled(self.tree.is_scanned(id)),
                MenuAction::Rescan,
            );
        }
        if n.kind == Kind::MountPoint {
            let pseudo = self.mounts.class_at(&self.tree.path(id))
                == Some(crate::scan::mounts::FsClass::Pseudo);
            item(
                ui,
                MenuItem::new(icon::HARD_DRIVES, "Scan this filesystem too")
                    .shortcut("Enter")
                    .enabled(!pseudo),
                MenuAction::EnterMount,
            );
        }
        menu_separator(ui, p);
        menu_header(ui, p, "Tag");
        for (i, t) in Tag::ALL.iter().enumerate() {
            let label = format!("{} · {}", ["Red", "Yellow", "Green", "Blue"][i], t.label());
            let shortcut = format!("Ctrl+{}", i + 1);
            item(
                ui,
                MenuItem::new(icon::TAG, &label).shortcut(&shortcut),
                MenuAction::Tag(Some(*t)),
            );
        }
        if self.tags.contains_key(&id) {
            item(
                ui,
                MenuItem::new(icon::X, "Remove tag").shortcut("Ctrl+0"),
                MenuAction::Tag(None),
            );
        }
        menu_separator(ui, p);
        item(
            ui,
            MenuItem::new(icon::TRASH, "Move to Trash")
                .shortcut("Del")
                .danger()
                .enabled(can_trash),
            MenuAction::Trash,
        );
        out
    }

    fn empty_states(&mut self, ui: &mut Ui, p: &Palette, rect: Rect) {
        let view = &self.tabs[self.active].view;
        let zoom = view.zoom;
        let n = self.tree.get(zoom);
        let has_children = view.laid.len() > 1;
        let msg: Option<(String, Option<&str>)> = if n.kind == Kind::Denied {
            Some((
                format!("No permission to read {}.", self.tree.path(zoom).display()),
                Some("Pick another folder"),
            ))
        } else if has_children {
            None
        } else if (view.filter.is_some() || view.hide_dot)
            && view.size(&self.tree, zoom, self.mode) == 0
            && self.tree.children(zoom).next().is_some()
        {
            let what = if view.filter_src.is_empty() {
                "the current settings".to_owned()
            } else {
                view.filter_src.clone()
            };
            Some((
                format!("Nothing here matches {what}"),
                view.filter.is_some().then_some("Clear filter"),
            ))
        } else if !self.tree.is_scanned(zoom) {
            Some((
                format!("Reading {} …", self.tree.path(zoom).display()),
                None,
            ))
        } else if self.tree.children(zoom).next().is_none() {
            Some(("This folder is empty.".into(), None))
        } else {
            None
        };
        let Some((text, button)) = msg else { return };
        let area = Rect::from_center_size(rect.center(), vec2(rect.width().min(520.0), 90.0));
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::top_down(egui::Align::Center)),
            |ui| {
                ui.label(
                    egui::RichText::new(text)
                        .font(fonts::font(ty::MD))
                        .color(p.text_muted),
                );
                if let Some(b) = button {
                    ui.add_space(sp(3));
                    if Btn::new(b)
                        .variant(Variant::Secondary)
                        .show(ui, p)
                        .clicked()
                    {
                        if b == "Clear filter" {
                            self.set_filter_text("");
                        } else {
                            let root = self.tree.root;
                            let v = &mut self.tabs[self.active].view;
                            v.zoom = root;
                            v.invalidate();
                        }
                    }
                }
            },
        );
    }
}
