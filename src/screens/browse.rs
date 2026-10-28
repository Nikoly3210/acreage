//! S02 main treemap, S03 details, S04 zoom, S05 context menu, S06 trash confirm, S11 errors.

use std::collections::HashMap;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::{Align2, Id, Key, Modifiers, PointerButton, Rect, Sense, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::actions::{self, ActionResult};
use crate::filter;
use crate::layout::Rect as LRect;
use crate::model::{Kind, NodeId, Tag, Tree};
use crate::scan::mounts::MountTable;
use crate::scan::{ScanEvent, ScanOpts, Scanner};
use crate::ui::fonts;
use crate::ui::theme::{Palette, layout as lay, motion, sp, ty};
use crate::ui::treemap_paint::{BlockKind, BlockState, Category, paint_block};
use crate::ui::widgets::{self as w, Btn, ToastKind, Variant};
use crate::view::{Dir4, Item, LayoutParams, SizeMode, View};

const INGEST_BUDGET: Duration = Duration::from_millis(4);

#[derive(Clone, Copy, Debug, PartialEq)]
enum ScanState {
    Scanning,
    Done { secs: f32, cancelled: bool },
}

struct ZoomAnim {
    /// The zoomed block's rect in the layout it came from (zoom in) or goes to (zoom out).
    from: LRect,
    start: f64,
    zoom_in: bool,
}

struct Pending {
    items: Vec<NodeId>,
    working: bool,
}

struct Toast {
    kind: ToastKind,
    text: String,
    until: f64,
}

enum MenuAction {
    Open,
    OpenFolder,
    Terminal,
    CopyPath,
    Tag(Option<Tag>),
    ZoomIn,
    Trash,
}

pub enum BrowseOut {
    None,
    OpenPicker,
}

pub struct Browse {
    pub root_path: PathBuf,
    tree: Tree,
    scanner: Option<Scanner>,
    state: ScanState,
    dirs_done: u64,
    errors: Vec<(NodeId, io::ErrorKind)>,
    view: View,
    tags: HashMap<NodeId, Tag>,
    tags_gen: u64,
    fs_used: u64,
    fs_total: u64,
    mode: SizeMode,
    opts: ScanOpts,
    mounts: Arc<MountTable>,
    filter_bar: w::FilterBar,
    confirm: Option<Pending>,
    show_errors: bool,
    show_help: bool,
    show_more: bool,
    /// Frame number the overflow menu opened on; its opening click must not close it.
    more_opened_at: u64,
    hovered: Option<usize>,
    menu_target: Option<NodeId>,
    anim: Option<ZoomAnim>,
    toasts: Vec<Toast>,
    act_tx: Sender<ActionResult>,
    act_rx: Receiver<ActionResult>,
    ctx: egui::Context,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn params() -> LayoutParams {
    let t = &lay::tm::DIR_HEADER_H;
    LayoutParams {
        header_h: *t,
        pad: lay::tm::DIR_PAD,
        gap: lay::tm::GAP,
        min_block: lay::tm::MIN_BLOCK_PX,
        min_child_area: lay::tm::MIN_CHILD_AREA,
    }
}

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
    pub fn new(
        ctx: &egui::Context,
        root: PathBuf,
        opts: ScanOpts,
        mode: SizeMode,
        mounts: Arc<MountTable>,
        filter: Option<String>,
    ) -> Browse {
        let (act_tx, act_rx) = unbounded();
        let (fs_used, fs_total) = rustix::fs::statvfs(&root)
            .map(|v| {
                (
                    (v.f_blocks - v.f_bfree) * v.f_frsize,
                    v.f_blocks * v.f_frsize,
                )
            })
            .unwrap_or((0, 0));
        let mut b = Browse {
            tree: Tree::new(root.as_os_str().as_bytes()),
            root_path: root,
            scanner: None,
            state: ScanState::Scanning,
            dirs_done: 0,
            errors: vec![],
            view: View::new(0),
            tags: HashMap::new(),
            tags_gen: 0,
            fs_used,
            fs_total,
            mode,
            opts,
            mounts,
            filter_bar: w::FilterBar::default(),
            confirm: None,
            show_errors: false,
            show_help: false,
            show_more: false,
            more_opened_at: 0,
            hovered: None,
            menu_target: None,
            anim: None,
            toasts: vec![],
            act_tx,
            act_rx,
            ctx: ctx.clone(),
        };
        if let Some(f) = filter {
            b.filter_bar.text = f;
            b.apply_filter();
        }
        b.start_scan();
        b
    }

    fn start_scan(&mut self) {
        let ctx = self.ctx.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        self.tree = Tree::new(self.root_path.as_os_str().as_bytes());
        let filter = (self.view.filter_src.clone(), self.view.filter.take());
        self.view = View::new(0);
        self.view.set_filter(filter.0, filter.1);
        self.tags.clear();
        self.tags_gen += 1;
        self.errors.clear();
        self.dirs_done = 0;
        self.state = ScanState::Scanning;
        self.scanner = Some(Scanner::start(
            self.root_path.clone(),
            self.opts,
            self.mounts.clone(),
            wake,
        ));
    }

    fn scanning(&self) -> bool {
        self.state == ScanState::Scanning
    }

    fn paused(&self) -> bool {
        self.scanner.as_ref().is_some_and(|s| s.is_paused()) && self.scanning()
    }

    /// Splice scanner events into the tree, at most `INGEST_BUDGET` per frame.
    fn ingest(&mut self) {
        let Some(s) = &self.scanner else { return };
        let t0 = Instant::now();
        while let Ok(ev) = s.events.try_recv() {
            match ev {
                ScanEvent::Batch { parent, entries } => {
                    self.tree.insert_batch(parent, entries);
                }
                ScanEvent::DirDone { id } => {
                    self.tree.set_scanned(id);
                    self.dirs_done += 1;
                }
                ScanEvent::DirError { id, err } => {
                    self.tree.set_denied(id);
                    self.errors.push((id, err));
                }
                ScanEvent::Done { cancelled, elapsed } => {
                    self.state = ScanState::Done {
                        secs: elapsed.as_secs_f32(),
                        cancelled,
                    };
                    self.view.invalidate();
                }
            }
            if t0.elapsed() > INGEST_BUDGET {
                self.ctx.request_repaint();
                break;
            }
        }
    }

    fn handle_actions(&mut self) {
        while let Ok(r) = self.act_rx.try_recv() {
            match r {
                ActionResult::Trashed { ok, failed } => {
                    let freed: u64 = ok
                        .iter()
                        .filter(|id| self.tree.exists(**id))
                        .map(|id| self.tree.get(*id).size_disk)
                        .sum();
                    for id in &ok {
                        self.tree.remove(*id);
                        self.tags.remove(id);
                    }
                    self.tags_gen += 1;
                    self.view.fix_after_remove(&self.tree);
                    self.confirm = None;
                    if !ok.is_empty() {
                        let n = ok.len();
                        self.toast(
                            ToastKind::Success,
                            format!(
                                "Moved {n} item{} to Trash · {} freed",
                                if n == 1 { "" } else { "s" },
                                w::human_size(freed)
                            ),
                        );
                    }
                    if let Some((path, err)) = failed.first() {
                        let more = if failed.len() > 1 {
                            format!(" (and {} more)", failed.len() - 1)
                        } else {
                            String::new()
                        };
                        self.toast(
                            ToastKind::Error,
                            format!("Couldn't move {} to Trash: {err}{more}", path.display()),
                        );
                    }
                }
            }
        }
    }

    fn toast(&mut self, kind: ToastKind, text: String) {
        let now = self.ctx.input(|i| i.time);
        let life = if matches!(kind, ToastKind::Error | ToastKind::Warning) {
            10.0
        } else {
            4.0
        };
        self.toasts.push(Toast {
            kind,
            text,
            until: now + life,
        });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    fn apply_filter(&mut self) {
        match filter::parse(&self.filter_bar.text) {
            Ok(f) => {
                self.filter_bar.error = None;
                self.filter_bar.applied_terms = f.as_ref().map(|f| f.terms);
                self.view.set_filter(self.filter_bar.text.clone(), f);
            }
            Err(e) => self.filter_bar.error = Some(e.to_string()),
        }
    }

    fn zoom_in(&mut self, id: NodeId) {
        if !self.tree.exists(id) || self.tree.get(id).kind != Kind::Dir || id == self.view.zoom {
            return;
        }
        let from = self.view.index_of(id).map(|i| self.view.laid[i].rect);
        self.view.zoom_to(&self.tree, id);
        self.view.selected = None;
        if let Some(from) = from {
            self.anim = Some(ZoomAnim {
                from,
                start: self.ctx.input(|i| i.time),
                zoom_in: true,
            });
        }
    }

    fn zoom_out(&mut self, back: bool) {
        let from = self.view.zoom;
        let moved = if back {
            self.view.back(&self.tree)
        } else {
            self.view.up(&self.tree)
        };
        if moved {
            // Rect of the old zoom node in the new layout is only known after layout; mark it.
            self.anim = Some(ZoomAnim {
                from: LRect::ZERO,
                start: self.ctx.input(|i| i.time),
                zoom_in: false,
            });
            self.view.selected =
                Some(from).filter(|f| self.tree.is_ancestor_or_self(self.view.zoom, *f));
        }
    }

    fn ask_trash(&mut self, ids: Vec<NodeId>) {
        let ok: Vec<NodeId> = ids
            .into_iter()
            .filter(|id| {
                self.tree.exists(*id)
                    && *id != self.tree.root
                    && self.tree.get(*id).kind != Kind::MountPoint
            })
            .collect();
        if !ok.is_empty() {
            self.confirm = Some(Pending {
                items: ok,
                working: false,
            });
        }
    }

    fn do_trash(&mut self) {
        let Some(p) = &mut self.confirm else { return };
        if p.working {
            return;
        }
        p.working = true;
        let items = p
            .items
            .iter()
            .map(|id| (*id, self.tree.path(*id)))
            .collect();
        let ctx = self.ctx.clone();
        actions::trash(items, self.act_tx.clone(), move || ctx.request_repaint());
    }

    fn run_menu(&mut self, id: NodeId, a: MenuAction) {
        if !self.tree.exists(id) {
            return;
        }
        let path = self.tree.path(id);
        let res = match a {
            MenuAction::Open => actions::open(&path),
            MenuAction::OpenFolder => actions::open_containing(&path),
            MenuAction::Terminal => actions::terminal_here(&path),
            MenuAction::CopyPath => {
                self.ctx.copy_text(path.to_string_lossy().into_owned());
                self.toast(ToastKind::Info, format!("Copied {}", path.display()));
                Ok(())
            }
            MenuAction::Tag(t) => {
                match t {
                    Some(t) => self.tags.insert(id, t),
                    None => self.tags.remove(&id),
                };
                self.tags_gen += 1;
                Ok(())
            }
            MenuAction::ZoomIn => {
                self.zoom_in(id);
                Ok(())
            }
            MenuAction::Trash => {
                self.ask_trash(vec![id]);
                Ok(())
            }
        };
        if let Err(e) = res {
            self.toast(
                ToastKind::Error,
                format!("Couldn't open {}: {e}", path.display()),
            );
        }
    }

    fn node_summary(&self, id: NodeId) -> String {
        let n = self.tree.get(id);
        format!(
            "{} · {} · modified {}",
            self.tree.path(id).display(),
            w::human_size(self.view.size(&self.tree, id, self.mode)),
            w::ago(now_secs() - n.mtime)
        )
    }

    fn hovered_node(&self) -> Option<NodeId> {
        self.hovered
            .and_then(|i| self.view.laid.get(i))
            .and_then(|l| match l.item {
                Item::Node(n) => Some(n),
                Item::Agg { .. } => None,
            })
    }

    fn keyboard(&mut self, ui: &Ui) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let pressed = |k: Key| ui.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        let ctrl = |k: Key| ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, k));

        if self.confirm.is_some() {
            if pressed(Key::Enter) {
                self.do_trash();
            }
            return;
        }
        if pressed(Key::Slash) {
            // Drop the "/" text event too, or it lands in the freshly focused filter.
            ui.input_mut(|i| {
                i.events
                    .retain(|e| !matches!(e, egui::Event::Text(t) if t == "/"))
            });
            ui.memory_mut(|m| m.request_focus(Id::new("filter")));
        }
        if pressed(Key::Escape) {
            if self.show_help || self.show_errors {
                self.show_help = false;
                self.show_errors = false;
            } else {
                self.view.selected = None;
            }
        }
        let moves = [
            (Key::ArrowLeft, Dir4::Left),
            (Key::H, Dir4::Left),
            (Key::ArrowRight, Dir4::Right),
            (Key::L, Dir4::Right),
            (Key::ArrowUp, Dir4::Up),
            (Key::K, Dir4::Up),
            (Key::ArrowDown, Dir4::Down),
            (Key::J, Dir4::Down),
        ];
        for (k, d) in moves {
            if pressed(k) {
                self.view.selected = match self.view.selected {
                    Some(s) if self.view.index_of(s).is_some() => {
                        self.view.neighbor(s, d).or(Some(s))
                    }
                    _ => self.view.first_child(),
                };
            }
        }
        if pressed(Key::Enter)
            && let Some(s) = self.view.selected
        {
            self.zoom_in(s);
        }
        if pressed(Key::Backspace) || ui.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowUp))
        {
            self.zoom_out(false);
        }
        if ui.input(|i| i.pointer.button_pressed(PointerButton::Extra1))
            || ui.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowLeft))
        {
            self.zoom_out(true);
        }
        if pressed(Key::P)
            && let Some(s) = &self.scanner
        {
            s.set_paused(!s.is_paused());
        }
        if pressed(Key::F5) || ctrl(Key::R) {
            self.start_scan();
        }
        let target = self.view.selected.or_else(|| self.hovered_node());
        if let Some(t) = target {
            for (k, tag) in [
                (Key::Num1, Some(Tag::Red)),
                (Key::Num2, Some(Tag::Yellow)),
                (Key::Num3, Some(Tag::Green)),
                (Key::Num4, Some(Tag::Blue)),
                (Key::Num0, None),
            ] {
                if ctrl(k) {
                    self.run_menu(t, MenuAction::Tag(tag));
                }
            }
            if pressed(Key::Y) {
                self.run_menu(t, MenuAction::CopyPath);
            }
            if ctrl(Key::O) {
                self.run_menu(t, MenuAction::Open);
            }
            if ctrl(Key::T) {
                self.run_menu(t, MenuAction::Terminal);
            }
            if pressed(Key::Delete) {
                self.ask_trash(vec![t]);
            }
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, p: &Palette) -> BrowseOut {
        let mut out = BrowseOut::None;
        self.ingest();
        self.handle_actions();
        self.keyboard(ui);
        let now = ui.input(|i| i.time);
        self.toasts.retain(|t| t.until > now);

        // ---- top: toolbar, path, filter
        let tb = egui::Panel::top("toolbar")
            .frame(egui::Frame::NONE)
            .show_separator_line(false)
            .resizable(false)
            .show(ui, |ui| {
                w::toolbar(
                    ui,
                    p,
                    &w::ToolbarState {
                        paused: self.paused(),
                        scanning: self.scanning(),
                        can_back: !self.view.history.is_empty(),
                        can_up: self.view.zoom != self.tree.root,
                        multi_view: false,
                        export: false,
                        settings: false,
                    },
                )
            });
        match tb.inner {
            Some(w::ToolbarAction::Back) => self.zoom_out(true),
            Some(w::ToolbarAction::Up) => self.zoom_out(false),
            Some(w::ToolbarAction::Rescan) => self.start_scan(),
            Some(w::ToolbarAction::PauseResume) => {
                if let Some(s) = &self.scanner {
                    s.set_paused(!s.is_paused());
                }
            }
            Some(w::ToolbarAction::Open) => out = BrowseOut::OpenPicker,
            Some(w::ToolbarAction::More) => {
                self.show_more = !self.show_more;
                self.more_opened_at = ui.ctx().cumulative_pass_nr();
            }
            Some(w::ToolbarAction::Tags) => {
                self.filter_bar.text = ":all".into();
                self.apply_filter();
            }
            _ => {}
        }

        egui::Panel::top("nav")
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(egui::Margin::symmetric(sp(4) as i8, sp(2) as i8)),
            )
            .show_separator_line(false)
            .resizable(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = sp(2);
                let chain: Vec<NodeId> = self
                    .tree
                    .ancestors(self.view.zoom)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let names: Vec<String> = chain
                    .iter()
                    .map(|id| self.tree.name_lossy(*id).into_owned())
                    .collect();
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                if let Some(i) = w::breadcrumb(ui, p, &refs) {
                    let target = chain[i];
                    if target != self.view.zoom
                        && self.tree.is_ancestor_or_self(target, self.view.zoom)
                    {
                        self.view.zoom_to(&self.tree, target);
                        self.anim = Some(ZoomAnim {
                            from: LRect::ZERO,
                            start: now,
                            zoom_in: false,
                        });
                    }
                }
                match w::filter_bar(ui, p, &mut self.filter_bar, Id::new("filter")) {
                    w::FilterEvent::Apply | w::FilterEvent::Clear => self.apply_filter(),
                    w::FilterEvent::Help => self.show_help = !self.show_help,
                    _ => {}
                }
            });

        // ---- bottom: status, details, confirm
        let status = egui::Panel::bottom("status")
            .frame(egui::Frame::NONE)
            .show_separator_line(false)
            .resizable(false)
            .show(ui, |ui| {
                let scan = match self.state {
                    ScanState::Scanning if self.paused() => w::ScanStatus::Paused {
                        dirs: self.dirs_done,
                        bytes: self.tree.get(0).size_disk,
                    },
                    ScanState::Scanning => w::ScanStatus::Scanning {
                        dirs: self.dirs_done,
                        bytes: self.tree.get(0).size_disk,
                    },
                    ScanState::Done { secs, .. } => w::ScanStatus::Done {
                        secs,
                        errors: self.errors.len() as u32,
                    },
                };
                let sel = self
                    .hovered_node()
                    .or(self.view.selected)
                    .map(|id| self.node_summary(id));
                let mut counts = [0u32; 4];
                for t in self.tags.values() {
                    counts[t.index()] += 1;
                }
                w::status_bar(
                    ui,
                    p,
                    &w::StatusInfo {
                        scan,
                        selection: sel.as_deref(),
                        fs_used: self.fs_used,
                        fs_total: self.fs_total,
                        views: 1,
                        tag_counts: counts,
                    },
                )
            });
        if status.inner {
            self.show_errors = true;
        }

        egui::Panel::bottom("details").frame(egui::Frame::NONE).show_separator_line(false).resizable(false).show(ui, |ui| {
            match self.view.selected.or_else(|| self.hovered_node()).filter(|id| self.tree.exists(*id)) {
                Some(id) => {
                    let n = self.tree.get(id);
                    let name = self.tree.name_lossy(id).into_owned();
                    let path = self.tree.path(id).to_string_lossy().into_owned();
                    let is_dir = matches!(n.kind, Kind::Dir | Kind::Denied | Kind::MountPoint);
                    w::details_strip(
                        ui,
                        p,
                        &w::Details {
                            is_dir,
                            name: &name,
                            path: &path,
                            size_disk: n.size_disk,
                            size_apparent: n.size_apparent,
                            items: is_dir.then_some(n.items),
                            modified: &w::ago(now_secs() - n.mtime),
                            tag: self.tags.get(&id).copied(),
                            nlink: if n.kind == Kind::File { n.nlink } else { 1 },
                            apparent_mode: self.mode == SizeMode::Apparent,
                        },
                    );
                }
                None => {
                    egui::Frame::new().fill(p.surface).inner_margin(egui::Margin::symmetric(sp(4) as i8, 0)).show(ui, |ui| {
                        ui.set_height(lay::DETAILS_H);
                        ui.set_width(ui.available_width());
                        ui.horizontal_centered(|ui| {
                            ui.label(
                                egui::RichText::new("Click a block for details · double-click or Enter to zoom · right-click for actions")
                                    .font(fonts::font(ty::SM))
                                    .color(p.text_muted),
                            );
                        });
                    });
                }
            }
        });

        if self.confirm.is_some() {
            let pending = self.confirm.as_ref().unwrap();
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

        if self.show_errors {
            self.errors_panel(ui, p);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.canvas(ui, p));

        if self.show_help {
            self.help_window(ui, p);
        }
        if self.show_more && self.more_menu(ui, p) {
            out = BrowseOut::OpenPicker;
        }
        self.toasts_ui(ui, p);
        out
    }

    fn canvas(&mut self, ui: &mut Ui, p: &Palette) {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, p.tm_canvas);
        let bounds = to_l(rect.shrink(lay::tm::GAP));
        let throttle = Duration::from_secs_f32(motion::RELAYOUT_THROTTLE);
        let relaid = self.view.ensure_layout(
            &self.tree,
            &self.tags,
            self.tags_gen,
            bounds,
            &params(),
            self.mode,
            now_secs(),
            throttle,
        );
        if self.scanning() && !relaid {
            ui.ctx().request_repaint_after(throttle);
        }

        // Zoom animation: map layout coordinates through a rect-to-rect transform.
        let now = ui.input(|i| i.time);
        if let Some(a) = &mut self.anim
            && !a.zoom_in
            && a.from == LRect::ZERO
        {
            // Zoom out: the old zoom node's rect in the new layout is now known.
            let prev = self.view.history.last().copied();
            let old = self.view.selected.or(prev);
            a.from = old
                .and_then(|id| self.view.index_of(id))
                .map(|i| self.view.laid[i].rect)
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
            resp.hover_pos().and_then(|pos| self.view.hit(pos.x, pos.y))
        } else {
            None
        };
        let hovered_node = self.hovered_node();
        if resp.clicked() {
            self.view.selected = hovered_node;
        }
        if resp.double_clicked()
            && let Some(n) = hovered_node
        {
            if self.tree.get(n).kind == Kind::Dir {
                self.zoom_in(n);
            } else {
                self.view.selected = Some(n);
            }
        }
        if resp.secondary_clicked() {
            self.menu_target = hovered_node;
            if hovered_node.is_some() {
                self.view.selected = hovered_node;
            }
        }

        // Paint.
        let tree = &self.tree;
        for (i, l) in self.view.laid.iter().enumerate() {
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
                    let size = w::human_size(self.view.size(tree, id, self.mode));
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
                        selected: self.view.selected == Some(id),
                        tag: self.tags.get(&id).copied(),
                        blink: 0.0,
                        pending: n.kind == Kind::Dir && !tree.is_scanned(id),
                    };
                    paint_block(&painter, p, r, kind, &name, &size_label, &st);
                }
                Item::Agg { count, size, .. } => {
                    let label = format!("{} small items", w::group_digits(count as u64));
                    let st = BlockState {
                        hovered: self.hovered == Some(i),
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
                chosen = self.menu(ui, p, target).map(|a| (target, a));
            });
        }
        if let Some((id, a)) = chosen {
            self.run_menu(id, a);
        }
    }

    fn menu(&self, ui: &mut Ui, p: &Palette, id: NodeId) -> Option<MenuAction> {
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
        let mut item = |ui: &mut Ui, it: MenuItem, a: MenuAction| {
            if it.show(ui, p).clicked() {
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
        let zoom = self.view.zoom;
        let n = self.tree.get(zoom);
        let has_children = self.view.laid.len() > 1;
        let msg: Option<(String, Option<&str>)> = if n.kind == Kind::Denied {
            Some((
                format!("No permission to read {}.", self.tree.path(zoom).display()),
                Some("Pick another folder"),
            ))
        } else if has_children {
            None
        } else if self.view.filter.is_some()
            && self.view.size(&self.tree, zoom, self.mode) == 0
            && !self.tree.children(zoom).next().is_none()
        {
            Some((
                format!("Nothing here matches {}", self.view.filter_src),
                Some("Clear filter"),
            ))
        } else if self.scanning() && !self.tree.is_scanned(zoom) {
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
        let center = rect.center();
        let area = Rect::from_center_size(center, vec2(rect.width().min(520.0), 90.0));
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
                            self.filter_bar.text.clear();
                            self.apply_filter();
                        } else {
                            self.view.zoom = self.tree.root;
                            self.view.invalidate();
                        }
                    }
                }
            },
        );
    }

    /// Overflow menu for the compact toolbar. Returns true for "Open location".
    fn more_menu(&mut self, ui: &mut Ui, p: &Palette) -> bool {
        use crate::ui::widgets::MenuItem;
        let mut open_picker = false;
        let scanning = self.scanning();
        let paused = self.paused();
        let r = egui::Area::new(Id::new("more-menu"))
            .fixed_pos(pos2(sp(2), lay::TOOLBAR_H))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                w::menu_frame(ui, p, |ui| {
                    let label = if paused { "Resume scan" } else { "Pause scan" };
                    let ic = if paused { icon::PLAY } else { icon::PAUSE };
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
                    if MenuItem::new(icon::TAG, "Show tagged")
                        .show(ui, p)
                        .clicked()
                    {
                        self.filter_bar.text = ":all".into();
                        self.apply_filter();
                        self.show_more = false;
                    }
                });
            });
        if r.response.clicked_elsewhere() && ui.ctx().cumulative_pass_nr() != self.more_opened_at {
            self.show_more = false;
        }
        open_picker
    }

    fn errors_panel(&mut self, ui: &mut Ui, p: &Palette) {
        egui::Panel::right("errors")
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(egui::Margin::same(sp(6) as i8)),
            )
            .default_size(lay::DETAILS_SIDE_W + 120.0)
            .resizable(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Unreadable folders")
                            .font(fonts::font(ty::LG))
                            .color(p.text),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if Btn::icon(icon::X, "Close").show(ui, p).clicked() {
                            self.show_errors = false;
                        }
                    });
                });
                let n = self.errors.len();
                ui.label(
                    egui::RichText::new(format!(
                        "{n} folder{} couldn't be read. Their size isn't counted.",
                        if n == 1 { "" } else { "s" }
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

    fn help_window(&mut self, ui: &mut Ui, p: &Palette) {
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

    fn toasts_ui(&mut self, ui: &mut Ui, p: &Palette) {
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
