//! The browse screen: S02 treemap, S03 details, S04 zoom, S05 context menu, S06 trash
//! confirm, S07 views, S08 export, S09 settings, S10 presets/help, S11 errors.
//!
//! Split by concern: `scanning` (scans, sub-scans, live updates, tags by path), `canvas`
//! (treemap, menu, empty states), `panels` (side panels, overlays, toasts).

mod canvas;
mod panels;
mod scanning;

use std::collections::HashMap;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::{SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::{Id, Key, Modifiers, PointerButton, Ui};

use crate::actions::{self, ActionResult};
use crate::config::{Config, Detail};
use crate::filter;
use crate::layout::Rect as LRect;
use crate::model::{Kind, NodeId, Tag, Tree};
use crate::report::Format;
use crate::scan::mounts::MountTable;
use crate::scan::{ScanOpts, Scanner};
use crate::ui::fonts;
use crate::ui::theme::{Palette, layout as lay, sp, ty};
use crate::ui::widgets::{self as w, ToastKind};
use crate::view::{Dir4, Item, LayoutParams, SizeMode, View};
use crate::watch::Watcher;

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
    Rescan,
    EnterMount,
    Trash,
}

/// A sub-scan of one folder: a refinement the user asked for, or a folder that appeared live.
struct Sub {
    root: NodeId,
    scanner: Scanner,
    user: bool,
}

/// One view of the scan (S07): its own zoom, history, selection and filter.
struct Tab {
    view: View,
    filter: w::FilterBar,
}

impl Tab {
    fn new(zoom: NodeId) -> Tab {
        Tab {
            view: View::new(zoom),
            filter: w::FilterBar {
                presets: true,
                ..Default::default()
            },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    None,
    Errors,
    Export,
    Settings,
}

struct ExportState {
    depth: usize,
    files: bool,
    format: Format,
    path: String,
    /// (inputs, rendered report): re-rendered only when something it depends on changes.
    cache: Option<(String, String)>,
}

pub enum BrowseOut {
    None,
    OpenPicker,
}

pub struct Browse {
    pub root_path: PathBuf,
    /// Settings as this screen uses them; the app saves and shares them.
    pub cfg: Config,
    tree: Tree,
    ids: Arc<AtomicU32>,
    scanner: Option<Scanner>,
    subs: Vec<Sub>,
    watcher: Option<Watcher>,
    state: ScanState,
    dirs_done: u64,
    errors: Vec<(NodeId, io::ErrorKind)>,
    tabs: Vec<Tab>,
    active: usize,
    /// Tags by path are the truth (they survive rescans); `tags` is their resolution to nodes.
    tag_paths: HashMap<PathBuf, Tag>,
    tags: HashMap<NodeId, Tag>,
    tags_gen: u64,
    /// When each node last changed on disk, for the blink.
    changed: HashMap<NodeId, f64>,
    /// Last time tags by path were matched against the tree (throttled while scanning).
    resolve_at: f64,
    limit_warned: bool,
    /// Keyboard-opened context menu (Shift+F10): target and where to show it.
    kb_menu: Option<(NodeId, egui::Pos2)>,
    kb_menu_focus: bool,
    show_keys: bool,
    /// "Other filesystem" folders the user chose to scan into; re-entered after a full rescan.
    entered_mounts: std::collections::HashSet<PathBuf>,
    fs_used: u64,
    fs_total: u64,
    mode: SizeMode,
    opts: ScanOpts,
    mounts: Arc<MountTable>,
    confirm: Option<Pending>,
    side: Side,
    export: ExportState,
    show_help: bool,
    show_more: bool,
    show_presets: bool,
    /// Frame a popup opened on: its opening click must not also close it.
    popup_opened_at: u64,
    hovered: Option<usize>,
    menu_target: Option<NodeId>,
    anim: Option<ZoomAnim>,
    toasts: Vec<Toast>,
    act_tx: Sender<ActionResult>,
    act_rx: Receiver<ActionResult>,
    /// Bumped by every full (re)scan. Node ids from an older scan must never touch the new tree.
    scan_gen: u64,
    ctx: egui::Context,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn params(detail: Detail) -> LayoutParams {
    LayoutParams {
        header_h: lay::tm::DIR_HEADER_H,
        pad: lay::tm::DIR_PAD,
        gap: lay::tm::GAP,
        min_block: lay::tm::MIN_BLOCK_PX,
        min_child_area: match detail {
            Detail::Coarse => lay::tm::MIN_CHILD_AREA_COARSE,
            Detail::Balanced => lay::tm::MIN_CHILD_AREA,
            Detail::Fine => lay::tm::MIN_CHILD_AREA_FINE,
        },
        show_mount_points: true,
        min_mount_side: lay::tm::MIN_MOUNT_SIDE,
    }
}

fn mode_of(cfg: &Config) -> SizeMode {
    if cfg.apparent() {
        SizeMode::Apparent
    } else {
        SizeMode::Disk
    }
}

fn default_report_path(fmt: Format) -> String {
    let stamp = crate::report::utc_stamp(now_secs())
        .replace([' ', ':'], "-")
        .replace("-UTC", "");
    format!("~/canopy-report-{stamp}.{}", fmt.ext())
}

impl Browse {
    pub fn new(
        ctx: &egui::Context,
        root: PathBuf,
        cfg: Config,
        mounts: Arc<MountTable>,
        filter: Option<String>,
        notice: Option<String>,
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
            mode: mode_of(&cfg),
            opts: ScanOpts {
                one_fs: cfg.one_filesystem,
            },
            cfg,
            ids: Arc::new(AtomicU32::new(1)),
            scanner: None,
            subs: vec![],
            watcher: None,
            state: ScanState::Scanning,
            dirs_done: 0,
            errors: vec![],
            tabs: vec![Tab::new(0)],
            active: 0,
            tag_paths: HashMap::new(),
            tags: HashMap::new(),
            tags_gen: 0,
            changed: HashMap::new(),
            resolve_at: 0.0,
            limit_warned: false,
            kb_menu: None,
            kb_menu_focus: false,
            show_keys: false,
            entered_mounts: Default::default(),
            fs_used,
            fs_total,
            mounts,
            confirm: None,
            side: Side::None,
            export: ExportState {
                depth: 3,
                files: false,
                format: Format::Text,
                path: default_report_path(Format::Text),
                cache: None,
            },
            show_help: false,
            show_more: false,
            show_presets: false,
            popup_opened_at: 0,
            hovered: None,
            menu_target: None,
            anim: None,
            toasts: vec![],
            act_tx,
            act_rx,
            scan_gen: 0,
            ctx: ctx.clone(),
        };
        let hide = b.cfg.hide_dotfiles;
        b.tabs[0].view.set_hide_dot(hide);
        if let Some(f) = filter {
            b.tabs[0].filter.text = f;
            b.apply_filter();
        }
        if let Some(n) = notice {
            b.toast(ToastKind::Warning, format!("Settings file ignored: {n}"));
        }
        b.start_scan();
        b
    }

    fn view(&self) -> &View {
        &self.tabs[self.active].view
    }

    fn scanning(&self) -> bool {
        self.state == ScanState::Scanning
    }

    fn paused(&self) -> bool {
        self.scanner.as_ref().is_some_and(|s| s.is_paused()) && self.scanning()
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
        let tab = &mut self.tabs[self.active];
        match filter::parse(&tab.filter.text) {
            Ok(f) => {
                tab.filter.error = None;
                tab.filter.applied_terms = f.as_ref().map(|f| f.terms);
                tab.view.set_filter(tab.filter.text.clone(), f);
            }
            Err(e) => tab.filter.error = Some(e.to_string()),
        }
    }

    fn set_filter_text(&mut self, text: &str) {
        self.tabs[self.active].filter.text = text.to_owned();
        self.apply_filter();
    }

    // ---- views (S07)

    fn new_tab(&mut self) {
        let cur = &self.tabs[self.active];
        let mut t = Tab::new(cur.view.zoom);
        t.filter.text = cur.filter.text.clone();
        t.view.set_hide_dot(self.cfg.hide_dotfiles);
        self.tabs.push(t);
        self.active = self.tabs.len() - 1;
        self.apply_filter();
        self.hovered = None;
    }

    fn close_tab(&mut self, i: usize) {
        if self.tabs.len() > 1 && i < self.tabs.len() {
            self.tabs.remove(i);
            if self.active >= self.tabs.len() || self.active > i {
                self.active = self.active.saturating_sub(1).min(self.tabs.len() - 1);
            }
            self.hovered = None;
        }
    }

    fn switch_tab(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        self.active = ((self.active as isize + delta).rem_euclid(n)) as usize;
        self.hovered = None;
        self.anim = None;
    }

    fn tab_label(&self, t: &Tab) -> String {
        let z = t.view.zoom;
        let name = if z == self.tree.root || !self.tree.exists(z) {
            self.root_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into())
        } else {
            self.tree.name_lossy(z).into_owned()
        };
        if t.view.filter_src.is_empty() {
            name
        } else {
            format!("{name} · {}", t.view.filter_src)
        }
    }

    // ---- navigation

    fn zoom_in(&mut self, id: NodeId) {
        let tab = &mut self.tabs[self.active];
        if !self.tree.exists(id) || self.tree.get(id).kind != Kind::Dir || id == tab.view.zoom {
            return;
        }
        let from = tab.view.index_of(id).map(|i| tab.view.laid[i].rect);
        tab.view.zoom_to(&self.tree, id);
        tab.view.selected = None;
        if let (Some(from), true) = (from, self.cfg.animations) {
            self.anim = Some(ZoomAnim {
                from,
                start: self.ctx.input(|i| i.time),
                zoom_in: true,
            });
        }
    }

    fn zoom_out(&mut self, back: bool) {
        let tab = &mut self.tabs[self.active];
        let from = tab.view.zoom;
        let moved = if back {
            tab.view.back(&self.tree)
        } else {
            tab.view.up(&self.tree)
        };
        if moved {
            tab.view.selected =
                Some(from).filter(|f| self.tree.is_ancestor_or_self(tab.view.zoom, *f));
            if self.cfg.animations {
                // The old zoom node's rect in the new layout is only known after layout.
                self.anim = Some(ZoomAnim {
                    from: LRect::ZERO,
                    start: self.ctx.input(|i| i.time),
                    zoom_in: false,
                });
            }
        }
    }

    // ---- actions

    fn set_tag(&mut self, id: NodeId, tag: Option<Tag>) {
        let path = self.tree.path(id);
        match tag {
            Some(t) => {
                self.tags.insert(id, t);
                self.tag_paths.insert(path, t);
            }
            None => {
                self.tags.remove(&id);
                self.tag_paths.remove(&path);
            }
        }
        self.tags_gen += 1;
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
        actions::trash(self.scan_gen, items, self.act_tx.clone(), move || {
            ctx.request_repaint()
        });
    }

    fn handle_actions(&mut self) {
        while let Ok(r) = self.act_rx.try_recv() {
            match r {
                ActionResult::Trashed { scan, ok, failed } if scan != self.scan_gen => {
                    // Finished after a rescan: the ids are stale, the new scan already shows the truth.
                    let n = ok.len();
                    if n > 0 {
                        let s = if n == 1 { "" } else { "s" };
                        self.toast(ToastKind::Success, format!("Moved {n} item{s} to Trash"));
                    }
                    if let Some((path, err)) = failed.first() {
                        self.toast(
                            ToastKind::Error,
                            format!("Couldn't move {} to Trash: {err}", path.display()),
                        );
                    }
                }
                ActionResult::Trashed { ok, failed, .. } => {
                    let freed: u64 = ok
                        .iter()
                        .filter(|id| self.tree.exists(**id))
                        .map(|id| self.tree.get(*id).size_disk)
                        .sum();
                    self.remove_nodes(&ok, true);
                    self.confirm = None;
                    if !ok.is_empty() {
                        let n = ok.len();
                        let s = if n == 1 { "" } else { "s" };
                        self.toast(
                            ToastKind::Success,
                            format!(
                                "Moved {n} item{s} to Trash · {} freed",
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

    /// Remove nodes from the tree and everything that points at them. `forget_tags` drops
    /// their tags by path too (the file is really gone, not just being rescanned).
    fn remove_nodes(&mut self, ids: &[NodeId], forget_tags: bool) {
        for id in ids {
            if !self.tree.exists(*id) {
                continue;
            }
            if forget_tags {
                let path = self.tree.path(*id);
                self.tag_paths.retain(|p, _| !p.starts_with(&path));
            }
            self.tree.remove(*id);
        }
        self.tags.retain(|id, _| self.tree.exists(*id));
        self.tags_gen += 1;
        for t in &mut self.tabs {
            t.view.fix_after_remove(&self.tree);
        }
        if self.menu_target.is_some_and(|m| !self.tree.exists(m)) {
            self.menu_target = None;
        }
        self.hovered = None;
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
                self.set_tag(id, t);
                Ok(())
            }
            MenuAction::ZoomIn => {
                self.zoom_in(id);
                Ok(())
            }
            MenuAction::Rescan => {
                self.rescan_folder(id);
                Ok(())
            }
            MenuAction::EnterMount => {
                self.enter_mount(id);
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
            w::human_size(self.view().size(&self.tree, id, self.mode)),
            w::ago(now_secs() - n.mtime)
        )
    }

    fn hovered_node(&self) -> Option<NodeId> {
        self.hovered
            .and_then(|i| self.view().laid.get(i))
            .and_then(|l| match l.item {
                Item::Node(n) => Some(n),
                Item::Agg { .. } => None,
            })
    }

    /// F5: rescan the zoomed folder (or everything at the top level).
    fn rescan_here(&mut self) {
        let z = self.view().zoom;
        if z == self.tree.root {
            self.start_scan()
        } else {
            self.rescan_folder(z)
        }
    }

    fn keyboard(&mut self, ui: &Ui) {
        // Tab shortcuts work even while typing in the filter.
        let shift_ctrl = Modifiers::COMMAND | Modifiers::SHIFT;
        if ui.input_mut(|i| {
            i.consume_key(shift_ctrl, Key::Tab) || i.consume_key(Modifiers::COMMAND, Key::PageUp)
        }) {
            self.switch_tab(-1);
        }
        if ui.input_mut(|i| {
            i.consume_key(Modifiers::COMMAND, Key::Tab)
                || i.consume_key(Modifiers::COMMAND, Key::PageDown)
        }) {
            self.switch_tab(1);
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::N)) {
            self.new_tab();
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::W)) {
            self.close_tab(self.active);
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Q)) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.kb_menu.is_some() {
            // The menu's items have focus; egui's own Tab/arrows/Enter drive them.
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.kb_menu = None;
            }
            return;
        }
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
            ui.memory_mut(|m| m.request_focus(Id::new(("filter", self.active))));
        }
        if pressed(Key::Escape) {
            if self.show_help
                || self.show_keys
                || self.side != Side::None
                || self.show_presets
                || self.show_more
            {
                self.show_help = false;
                self.show_keys = false;
                self.show_presets = false;
                self.show_more = false;
                self.side = Side::None;
            } else {
                self.tabs[self.active].view.selected = None;
            }
        }
        // Alt combos first: egui's consume_key ignores extra Alt, so the plain arrow
        // handlers below would otherwise swallow Alt+Left / Alt+Up.
        if ui.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowUp)) || pressed(Key::Backspace)
        {
            self.zoom_out(false);
        }
        if ui.input(|i| i.pointer.button_pressed(PointerButton::Extra1))
            || ui.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowLeft))
        {
            self.zoom_out(true);
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
                let v = &mut self.tabs[self.active].view;
                v.selected = match v.selected {
                    Some(s) if v.index_of(s).is_some() => v.neighbor(s, d).or(Some(s)),
                    _ => v.first_child(),
                };
            }
        }
        if pressed(Key::Enter)
            && let Some(s) = self.view().selected
        {
            if self.tree.exists(s) && self.tree.get(s).kind == Kind::MountPoint {
                self.enter_mount(s);
            } else {
                self.zoom_in(s);
            }
        }
        // Shift+F10 (the usual "context menu" key) opens the menu for the selected block.
        if ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::F10))
            && let Some(s) = self.view().selected.or_else(|| self.hovered_node())
            && let Some(i) = self.view().index_of(s)
        {
            let r = self.view().laid[i].rect;
            self.kb_menu = Some((s, egui::pos2(r.x + 8.0, r.y + lay::tm::DIR_HEADER_H)));
            self.kb_menu_focus = true;
            self.menu_target = Some(s);
        }
        // +/- change how much detail the treemap shows.
        let more = ui.input_mut(|i| {
            i.consume_key(Modifiers::NONE, Key::Plus) || i.consume_key(Modifiers::NONE, Key::Equals)
        });
        let less = pressed(Key::Minus);
        if more || less {
            self.step_detail(if more { 1 } else { -1 });
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Questionmark)) {
            self.show_keys = !self.show_keys;
        }
        if pressed(Key::P)
            && let Some(s) = &self.scanner
        {
            s.set_paused(!s.is_paused());
        }
        if ctrl(Key::R) {
            self.start_scan();
        } else if pressed(Key::F5) {
            self.rescan_here();
        }
        if self.side == Side::Export && ctrl(Key::S) {
            self.export_save();
        }
        if ctrl(Key::E) {
            self.side = if self.side == Side::Export {
                Side::None
            } else {
                Side::Export
            };
        }
        if ctrl(Key::Comma) {
            self.side = if self.side == Side::Settings {
                Side::None
            } else {
                Side::Settings
            };
        }
        let target = self.view().selected.or_else(|| self.hovered_node());
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
        self.live();
        self.handle_actions();
        self.keyboard(ui);
        let now = ui.input(|i| i.time);
        self.toasts.retain(|t| t.until > now);

        // ---- top: toolbar, path, filter, tabs
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
                        can_back: !self.view().history.is_empty(),
                        can_up: self.view().zoom != self.tree.root,
                        multi_view: true,
                        export: true,
                        settings: true,
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
                self.popup_opened_at = ui.ctx().cumulative_pass_nr();
            }
            Some(w::ToolbarAction::Tags) => self.set_filter_text(":all"),
            Some(w::ToolbarAction::NewView) => self.new_tab(),
            Some(w::ToolbarAction::Export) => {
                self.side = if self.side == Side::Export {
                    Side::None
                } else {
                    Side::Export
                };
            }
            Some(w::ToolbarAction::Settings) => {
                self.side = if self.side == Side::Settings {
                    Side::None
                } else {
                    Side::Settings
                };
            }
            None => {}
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
                let zoom = self.view().zoom;
                let chain: Vec<NodeId> = self
                    .tree
                    .ancestors(zoom)
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
                    if target != zoom && self.tree.is_ancestor_or_self(target, zoom) {
                        self.tabs[self.active].view.zoom_to(&self.tree, target);
                        if self.cfg.animations {
                            self.anim = Some(ZoomAnim {
                                from: LRect::ZERO,
                                start: now,
                                zoom_in: false,
                            });
                        }
                    }
                }
                let id = Id::new(("filter", self.active));
                match w::filter_bar(ui, p, &mut self.tabs[self.active].filter, id) {
                    w::FilterEvent::Apply | w::FilterEvent::Clear => self.apply_filter(),
                    w::FilterEvent::Help => self.show_help = !self.show_help,
                    w::FilterEvent::Presets => {
                        self.show_presets = !self.show_presets;
                        self.popup_opened_at = ui.ctx().cumulative_pass_nr();
                    }
                    w::FilterEvent::None => {}
                }
                if self.tabs.len() > 1 {
                    let labels: Vec<String> = self.tabs.iter().map(|t| self.tab_label(t)).collect();
                    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                    let (clicked, closed) = w::tabs(ui, p, &refs, self.active);
                    if let Some(c) = closed {
                        self.close_tab(c);
                    } else if let Some(c) = clicked {
                        self.active = c;
                        self.hovered = None;
                    }
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
                        of: self.expected_bytes(),
                    },
                    ScanState::Done { secs, .. } => w::ScanStatus::Done {
                        secs,
                        errors: self.errors.len() as u32,
                    },
                };
                let sel = self
                    .hovered_node()
                    .or(self.view().selected)
                    .map(|id| self.node_summary(id));
                let mut counts = [0u32; 4];
                for t in self.tags.values() {
                    counts[t.index()] += 1;
                }
                let live = self.watcher.as_ref().map(|w| (w.watched(), w.limited()));
                w::status_bar(
                    ui,
                    p,
                    &w::StatusInfo {
                        scan,
                        selection: sel.as_deref(),
                        fs_used: self.fs_used,
                        fs_total: self.fs_total,
                        views: self.tabs.len(),
                        tag_counts: counts,
                        live,
                    },
                )
            });
        if status.inner {
            self.side = Side::Errors;
        }

        egui::Panel::bottom("details")
            .frame(egui::Frame::NONE)
            .show_separator_line(false)
            .resizable(false)
            .show(ui, |ui| {
                self.details(ui, p);
            });

        if self.confirm.is_some() {
            self.confirm_panel(ui, p);
        }

        match self.side {
            Side::Errors => self.errors_panel(ui, p),
            Side::Export => self.export_panel(ui, p),
            Side::Settings => self.settings_panel(ui, p),
            Side::None => {}
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.canvas(ui, p));

        if self.show_help {
            self.help_window(ui, p);
        }
        if self.show_keys {
            self.keys_window(ui, p);
        }
        if self.kb_menu.is_some() {
            self.kb_menu_ui(ui, p);
        }
        if self.show_presets {
            self.presets_menu(ui, p);
        }
        if self.show_more && self.more_menu(ui, p) {
            out = BrowseOut::OpenPicker;
        }
        self.toasts_ui(ui, p);
        out
    }

    fn details(&self, ui: &mut Ui, p: &Palette) {
        match self
            .view()
            .selected
            .or_else(|| self.hovered_node())
            .filter(|id| self.tree.exists(*id))
        {
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
    }

    /// Everything the e2e suite asserts on (see `testhook`).
    pub fn test_state(&self, o: crate::testhook::Obj) -> crate::testhook::Obj {
        let path = |id: NodeId| self.tree.path(id).to_string_lossy().into_owned();
        let v = self.view();
        let scan = match self.state {
            ScanState::Scanning if self.paused() => "paused",
            ScanState::Scanning => "scanning",
            ScanState::Done {
                cancelled: true, ..
            } => "cancelled",
            ScanState::Done { .. } => "done",
        };
        let mut tags: Vec<String> = self
            .tags
            .iter()
            .filter(|(id, _)| self.tree.exists(**id))
            .map(|(id, t)| format!("{}={:?}", path(*id), t))
            .collect();
        tags.sort();
        let confirm: Vec<String> = self
            .confirm
            .as_ref()
            .map(|c| c.items.iter().map(|id| path(*id)).collect())
            .unwrap_or_default();
        let toasts: Vec<String> = self.toasts.iter().map(|t| t.text.clone()).collect();
        // Screen rects of the top two levels, so tests can click a specific block.
        let rects: Vec<String> = v
            .laid
            .iter()
            .filter(|l| l.depth <= 2)
            .filter_map(|l| match l.item {
                Item::Node(id) => Some(format!(
                    "{}|{}|{}|{}|{}",
                    path(id),
                    l.rect.x,
                    l.rect.y,
                    l.rect.w,
                    l.rect.h
                )),
                Item::Agg { .. } => None,
            })
            .collect();
        let tab_zooms: Vec<String> = self.tabs.iter().map(|t| path(t.view.zoom)).collect();
        let tab_filters: Vec<String> = self
            .tabs
            .iter()
            .map(|t| t.view.filter_src.clone())
            .collect();
        let presets: Vec<String> = self.cfg.presets.iter().map(|p| p.filter.clone()).collect();
        let side = match self.side {
            Side::None => "none",
            Side::Errors => "errors",
            Side::Export => "export",
            Side::Settings => "settings",
        };
        o.list("rects", &rects)
            .str("scan", scan)
            .str("root", &self.root_path.to_string_lossy())
            .str("zoom", &path(v.zoom))
            .opt(
                "selected",
                v.selected
                    .filter(|s| self.tree.exists(*s))
                    .map(path)
                    .as_deref(),
            )
            .opt("hovered", self.hovered_node().map(path).as_deref())
            .str("filter_text", &self.tabs[self.active].filter.text)
            .str("filter_applied", &v.filter_src)
            .opt(
                "filter_error",
                self.tabs[self.active].filter.error.as_deref(),
            )
            .num("zoom_size", v.size(&self.tree, v.zoom, self.mode))
            .num(
                "total_size",
                crate::view::node_size(&self.tree, self.tree.root, self.mode),
            )
            .num("items", self.tree.get(self.tree.root).items)
            .num("blocks", v.laid.len() as u64)
            .num("history", v.history.len() as u64)
            .num("errors", self.errors.len() as u64)
            .list("tags", &tags)
            .bool("confirm_open", self.confirm.is_some())
            .bool(
                "confirm_working",
                self.confirm.as_ref().is_some_and(|c| c.working),
            )
            .list("confirm_items", &confirm)
            .list("toasts", &toasts)
            .bool("errors_panel", self.side == Side::Errors)
            .str("side", side)
            .bool("help", self.show_help)
            .bool("more_menu", self.show_more)
            .bool("presets_menu", self.show_presets)
            .bool("animating", self.anim.is_some())
            .num("tabs", self.tabs.len() as u64)
            .num("active", self.active as u64)
            .list("tab_zooms", &tab_zooms)
            .list("tab_filters", &tab_filters)
            .num("subscans", self.subs.len() as u64)
            .opt(
                "live",
                self.watcher
                    .as_ref()
                    .map(|w| w.watched().to_string())
                    .as_deref(),
            )
            .num("blinking", self.changed.len() as u64)
            .str("theme", &self.cfg.theme)
            .str("size_mode", &self.cfg.size)
            .bool("hide_dotfiles", self.cfg.hide_dotfiles)
            .list("presets", &presets)
            .str("export_path", &self.export.path)
            .bool("kb_menu", self.kb_menu.is_some())
            .bool("keys_window", self.show_keys)
            .str("detail", &format!("{:?}", self.cfg.detail).to_lowercase())
            .list(
                "kinds",
                &self
                    .tree
                    .children(self.tree.root)
                    .map(|c| format!("{}={:?}", self.tree.name_lossy(c), self.tree.get(c).kind))
                    .collect::<Vec<_>>(),
            )
    }
}
