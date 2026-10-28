//! Design-system gallery: every primitive in isolation, in every state worth seeing.
//!
//!     cargo run --example gallery -- [--theme dark|light] [--page treemap|controls]
//!
//! Keys: T toggles theme, 1/2 switch page. The treemap page uses a fake home directory.

use std::collections::HashMap;

use canopy::layout::{Rect as LRect, squarify};
use canopy::ui::theme::{Palette, layout::tm as L, sp};
use canopy::ui::treemap_paint::{BlockKind, BlockState, Category, Tag, dir_body, paint_block};
use canopy::ui::widgets::{self as w, Btn, Size, Variant};
use canopy::ui::{fonts, style};
use egui::{Rect, Theme, pos2, vec2};
use egui_phosphor::regular as icon;

// ---------------------------------------------------------------- fake tree

enum Kind {
    Dir,
    File,
    Denied,
    Mount,
}

struct Node {
    name: String,
    size: u64,
    kind: Kind,
    children: Vec<Node>,
}

fn file(name: &str, size: u64) -> Node {
    Node {
        name: name.into(),
        size,
        kind: Kind::File,
        children: vec![],
    }
}

fn dir(name: &str, mut children: Vec<Node>) -> Node {
    children.sort_by_key(|c| std::cmp::Reverse(c.size));
    let size = children.iter().map(|c| c.size).sum();
    Node {
        name: name.into(),
        size,
        kind: Kind::Dir,
        children,
    }
}

/// Deterministic xorshift so the screenshot is stable.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo)
    }
}

const MB: u64 = 1 << 20;
const GB: u64 = 1 << 30;

fn many(rng: &mut Rng, prefix: &str, ext: &str, n: usize, lo: u64, hi: u64) -> Vec<Node> {
    (0..n)
        .map(|i| file(&format!("{prefix}{:03}.{ext}", i + 1), rng.range(lo, hi)))
        .collect()
}

fn fake_home() -> Node {
    let mut r = Rng(0x9e3779b97f4a7c15);
    let rng = &mut r;
    let mut videos = many(rng, "lecture_", "mkv", 6, 300 * MB, 1400 * MB);
    videos.push(file("holiday_2025.mp4", 9 * GB));
    videos.push(file("ubuntu-screencast.webm", 2 * GB + 300 * MB));
    let mut pictures = vec![];
    for year in 2021..=2026 {
        let n = 10 + (year - 2021) as usize * 6;
        pictures.push(dir(
            &year.to_string(),
            many(rng, "IMG_", "jpg", n, 3 * MB, 14 * MB),
        ));
    }
    pictures.push(dir("raw", many(rng, "DSC", "nef", 20, 24 * MB, 32 * MB)));
    let music = vec![
        dir("Jazz", many(rng, "track", "flac", 24, 20 * MB, 60 * MB)),
        dir("Podcasts", many(rng, "ep", "mp3", 40, 30 * MB, 90 * MB)),
        dir("Soundtracks", many(rng, "cue", "opus", 30, 4 * MB, 12 * MB)),
    ];
    let downloads = vec![
        file("archlinux-2026.10.01-x86_64.iso", 1300 * MB),
        file("win11_vm.qcow2", 21 * GB),
        file("dataset.tar.zst", 3 * GB + 600 * MB),
        file("thesis_final_v7.pdf", 38 * MB),
        file("game-patch.zip", 2 * GB),
    ]
    .into_iter()
    .chain(many(rng, "invoice_", "pdf", 40, 100 * 1024, 2 * MB))
    .collect();
    let mut target = many(rng, "libdep", "rlib", 30, 5 * MB, 80 * MB);
    target.push(file("canopy", 180 * MB));
    target.extend(many(rng, "obj", "o", 120, 64 * 1024, 4 * MB));
    let code = vec![
        dir(
            "canopy",
            vec![
                dir("target", target),
                dir("src", many(rng, "mod", "rs", 25, 8 * 1024, 120 * 1024)),
            ],
        ),
        dir("dotfiles", many(rng, "conf", "toml", 30, 1024, 40 * 1024)),
        dir(
            "site",
            vec![dir(
                "node_modules",
                many(rng, "pkg", "js", 200, 20 * 1024, 3 * MB),
            )],
        ),
    ];
    let cache = vec![
        dir("mozilla", many(rng, "cache2_", "bin", 60, 2 * MB, 40 * MB)),
        dir("pip", many(rng, "wheel", "whl", 30, MB, 60 * MB)),
        dir("yay", many(rng, "pkg", "zst", 15, 20 * MB, 400 * MB)),
    ];
    let mut root = dir(
        "/home/you",
        vec![
            dir("Videos", videos),
            dir("Pictures", pictures),
            dir("Music", music),
            dir("Downloads", downloads),
            dir("code", code),
            dir(".cache", cache),
            Node {
                name: ".local/share/containers".into(),
                size: 6 * GB,
                kind: Kind::Denied,
                children: vec![],
            },
            Node {
                name: "/home/you/nas".into(),
                size: 3 * GB,
                kind: Kind::Mount,
                children: vec![],
            },
            dir("Documents", many(rng, "doc", "odt", 60, 50 * 1024, 4 * MB)),
        ],
    );
    root.children.sort_by_key(|c| std::cmp::Reverse(c.size));
    root
}

// ---------------------------------------------------------------- treemap drawing

/// Path → state, to show every overlay in one screenshot.
struct Demo {
    tags: HashMap<String, Tag>,
    selected: String,
    blink: String,
    pending: String,
    hovered: Option<String>,
}

fn to_l(r: Rect) -> LRect {
    LRect::new(r.left(), r.top(), r.width(), r.height())
}
fn to_e(r: LRect) -> Rect {
    Rect::from_min_size(pos2(r.x, r.y), vec2(r.w, r.h))
}

#[allow(clippy::too_many_arguments)]
fn draw(
    painter: &egui::Painter,
    p: &Palette,
    n: &Node,
    path: &str,
    rect: Rect,
    depth: usize,
    demo: &Demo,
    hits: &mut Vec<(Rect, String)>,
) {
    let st = BlockState {
        hovered: demo.hovered.as_deref() == Some(path),
        selected: demo.selected == path,
        tag: demo.tags.get(path).copied(),
        blink: if demo.blink == path { 0.8 } else { 0.0 },
        pending: demo.pending == path,
    };
    let size = w::human_size(n.size);
    hits.push((rect, path.to_owned()));
    match n.kind {
        Kind::File => paint_block(
            painter,
            p,
            rect,
            BlockKind::File(Category::from_name(&n.name)),
            &n.name,
            &size,
            &st,
        ),
        Kind::Denied => paint_block(
            painter,
            p,
            rect,
            BlockKind::Denied,
            &n.name,
            "No access",
            &st,
        ),
        Kind::Mount => paint_block(
            painter,
            p,
            rect,
            BlockKind::MountPoint { depth },
            &n.name,
            "other filesystem",
            &st,
        ),
        Kind::Dir => {
            paint_block(
                painter,
                p,
                rect,
                BlockKind::Dir { depth },
                &n.name,
                &size,
                &st,
            );
            let Some(body) = dir_body(rect) else { return };
            // Fold children that would be smaller than ~12x12 px into one aggregate block.
            let scale = body.area() as f64 / n.size.max(1) as f64;
            let keep = n
                .children
                .iter()
                .take_while(|c| c.size as f64 * scale >= 144.0)
                .count();
            let rest: u64 = n.children[keep..].iter().map(|c| c.size).sum();
            let mut values: Vec<f64> = n.children[..keep].iter().map(|c| c.size as f64).collect();
            if rest > 0 {
                values.push(rest as f64);
                // squarify wants descending order; the aggregate may be bigger than the last kept child.
            }
            let mut order: Vec<usize> = (0..values.len()).collect();
            order.sort_by(|a, b| values[*b].total_cmp(&values[*a]));
            let sorted: Vec<f64> = order.iter().map(|i| values[*i]).collect();
            let rects = squarify(&sorted, to_l(body));
            for (k, &i) in order.iter().enumerate() {
                let r = to_e(rects[k]).shrink(L::GAP / 2.0);
                if r.width() < L::MIN_BLOCK_PX || r.height() < L::MIN_BLOCK_PX {
                    continue;
                }
                if i == keep {
                    let count = n.children.len() - keep;
                    let label = format!("{} small items", w::group_digits(count as u64));
                    paint_block(
                        painter,
                        p,
                        r,
                        BlockKind::SmallAgg,
                        &label,
                        &w::human_size(rest),
                        &BlockState::default(),
                    );
                } else {
                    let c = &n.children[i];
                    draw(
                        painter,
                        p,
                        c,
                        &format!("{path}/{}", c.name),
                        r,
                        depth + 1,
                        demo,
                        hits,
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------- app

#[derive(PartialEq, Clone, Copy)]
enum Page {
    Treemap,
    Controls,
}

struct Gallery {
    theme: Theme,
    page: Page,
    tree: Node,
    demo: Demo,
    filter: w::FilterBar,
    filter_err: w::FilterBar,
    filter_applied: w::FilterBar,
    toggles: [bool; 3],
}

impl Gallery {
    fn p(&self) -> &'static Palette {
        style::palette(self.theme)
    }

    fn page_treemap(&mut self, ui: &mut egui::Ui) {
        let p = self.p();
        w::toolbar(
            ui,
            p,
            &w::ToolbarState {
                paused: false,
                scanning: true,
                can_back: true,
                can_up: true,
                multi_view: true,
                export: true,
                settings: true,
            },
        );
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(sp(4) as i8, sp(2) as i8))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = sp(2);
                w::breadcrumb(ui, p, &["/home", "you", "Videos"]);
                w::filter_bar(
                    ui,
                    p,
                    &mut self.filter_applied,
                    egui::Id::new("filter-main"),
                );
            });
        let (_, closed) = w::tabs(ui, p, &["you", "Downloads  >1gb", ".cache  :red"], 0);
        let _ = closed;

        let bottom_h = 22.0 + 22.0;
        let avail = ui.available_rect_before_wrap();
        let canvas = Rect::from_min_max(avail.min, pos2(avail.right(), avail.bottom() - bottom_h));
        let resp = ui.allocate_rect(canvas, egui::Sense::click());
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 0, p.tm_canvas);
        let mut hits = vec![];
        let inner = canvas.shrink(L::GAP);
        draw(
            &painter,
            p,
            &self.tree,
            &self.tree.name,
            inner,
            0,
            &self.demo,
            &mut hits,
        );
        // Deepest hit under the pointer = hover (hits are pushed parent-first).
        self.demo.hovered = resp
            .hover_pos()
            .and_then(|pos| {
                hits.iter()
                    .rev()
                    .find(|(r, _)| r.contains(pos))
                    .map(|(_, path)| path.clone())
            })
            .or_else(|| Some(format!("{}/Pictures/2026", self.tree.name)));
        if resp.clicked()
            && let Some(h) = &self.demo.hovered
        {
            self.demo.selected = h.clone();
        }

        w::details_strip(
            ui,
            p,
            &w::Details {
                is_dir: false,
                name: "holiday_2025.mp4",
                path: "/home/you/Videos/holiday_2025.mp4",
                size_disk: 9 * GB,
                size_apparent: 9 * GB,
                items: None,
                modified: "4 months ago",
                tag: Some(Tag::Red),
                nlink: 1,
                apparent_mode: false,
            },
        );
        w::status_bar(
            ui,
            p,
            &w::StatusInfo {
                scan: w::ScanStatus::Scanning {
                    dirs: 41_213,
                    bytes: self.tree.size,
                },
                selection: None,
                fs_used: 812 * GB,
                fs_total: 1000 * GB,
                views: 3,
                tag_counts: [3, 1, 2, 0],
            },
        );
    }

    fn page_controls(&mut self, ui: &mut egui::Ui) {
        let p = self.p();
        egui::Frame::new()
            .fill(p.bg)
            .inner_margin(egui::Margin::same(sp(6) as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.columns(3, |cols| {
                    // ---- column 1
                    let ui = &mut cols[0];
                    w::section(ui, p, "Buttons");
                    for v in [
                        Variant::Primary,
                        Variant::Secondary,
                        Variant::Ghost,
                        Variant::Danger,
                    ] {
                        ui.horizontal(|ui| {
                            let name = format!("{v:?}");
                            Btn::new(&name).variant(v).show(ui, p);
                            Btn::new("Icon")
                                .with_icon(icon::TRASH)
                                .variant(v)
                                .size(Size::Sm)
                                .show(ui, p);
                            ui.add_enabled_ui(false, |ui| {
                                Btn::new("Disabled").variant(v).show(ui, p)
                            });
                        });
                    }
                    ui.horizontal(|ui| {
                        Btn::new("Moving…")
                            .variant(Variant::Danger)
                            .loading(true)
                            .show(ui, p);
                        Btn::icon(icon::PAUSE, "Pause").toggled(true).show(ui, p);
                        Btn::icon(icon::GEAR, "Settings").show(ui, p);
                        let r = Btn::new("Focused").show(ui, p);
                        r.request_focus();
                    });

                    w::section(ui, p, "Filter bar");
                    w::filter_bar(ui, p, &mut self.filter, egui::Id::new("f-empty"));
                    ui.add_space(sp(2));
                    w::filter_bar(ui, p, &mut self.filter_applied, egui::Id::new("f-applied"));
                    ui.add_space(sp(2));
                    w::filter_bar(ui, p, &mut self.filter_err, egui::Id::new("f-err"));

                    w::section(ui, p, "Breadcrumb");
                    w::breadcrumb(
                        ui,
                        p,
                        &["/", "home", "you", "code", "canopy", "target", "release"],
                    );
                    ui.scope(|ui| {
                        ui.set_max_width(260.0);
                        w::breadcrumb(
                            ui,
                            p,
                            &["/", "home", "you", "code", "canopy", "target", "release"],
                        );
                    });

                    w::section(ui, p, "Tags");
                    ui.horizontal_wrapped(|ui| {
                        for t in Tag::ALL {
                            w::tag_chip(ui, p, t);
                        }
                    });

                    // ---- column 2
                    let ui = &mut cols[1];
                    w::section(ui, p, "Context menu");
                    w::menu_frame(ui, p, |ui| {
                        w::menu_header(ui, p, "holiday_2025.mp4");
                        w::MenuItem::new(icon::ARROW_UP, "Open")
                            .shortcut("Ctrl+O")
                            .show(ui, p);
                        w::MenuItem::new(icon::FOLDER_OPEN, "Open containing folder")
                            .highlighted()
                            .show(ui, p);
                        w::MenuItem::new(icon::TERMINAL, "Open terminal here")
                            .shortcut("Ctrl+T")
                            .show(ui, p);
                        w::MenuItem::new(icon::COPY, "Copy path")
                            .shortcut("y")
                            .show(ui, p);
                        w::menu_separator(ui, p);
                        w::MenuItem::new(icon::TAG, "Tag").submenu().show(ui, p);
                        w::MenuItem::new(icon::ARROWS_CLOCKWISE, "Rescan this folder")
                            .enabled(false)
                            .show(ui, p);
                        w::menu_separator(ui, p);
                        w::MenuItem::new(icon::TRASH, "Move to Trash")
                            .shortcut("Del")
                            .danger()
                            .show(ui, p);
                        w::MenuItem::new(icon::X_CIRCLE, "Delete permanently")
                            .shortcut("Shift+Del")
                            .danger()
                            .show(ui, p);
                    });

                    w::section(ui, p, "Confirm panel");
                    w::confirm_panel(
                        ui,
                        p,
                        &w::Confirm {
                            permanent: false,
                            paths: &[
                                "~/Downloads/win11_vm.qcow2",
                                "~/Downloads/game-patch.zip",
                                "~/.cache/yay",
                            ],
                            total: "26.4 GiB",
                            working: false,
                            error: None,
                        },
                    );

                    w::section(ui, p, "Toasts");
                    w::toast(
                        ui,
                        p,
                        w::ToastKind::Success,
                        "Moved 3 items to Trash · 26.4 GiB freed",
                        Some("Undo"),
                    );
                    ui.add_space(sp(2));
                    w::toast(
                        ui,
                        p,
                        w::ToastKind::Warning,
                        "Live updates are limited to 65 536 folders.",
                        None,
                    );
                    ui.add_space(sp(2));
                    w::toast(
                        ui,
                        p,
                        w::ToastKind::Error,
                        "Couldn't write report.txt: Read-only file system",
                        None,
                    );

                    // ---- column 3
                    let ui = &mut cols[2];
                    w::section(ui, p, "Mount picker rows");
                    let mounts = [
                        w::Mount {
                            kind: w::MountKind::Home,
                            path: "/home",
                            device: "/dev/nvme0n1p3",
                            fstype: "btrfs",
                            used: 640 * GB,
                            total: 900 * GB,
                            unreadable: None,
                        },
                        w::Mount {
                            kind: w::MountKind::Local,
                            path: "/",
                            device: "/dev/nvme0n1p2",
                            fstype: "ext4",
                            used: 47 * GB,
                            total: 50 * GB,
                            unreadable: None,
                        },
                        w::Mount {
                            kind: w::MountKind::Removable,
                            path: "/run/media/you/STICK",
                            device: "/dev/sda1",
                            fstype: "exfat",
                            used: 12 * GB,
                            total: 64 * GB,
                            unreadable: None,
                        },
                        w::Mount {
                            kind: w::MountKind::Network,
                            path: "/home/you/nas",
                            device: "nas:/export",
                            fstype: "nfs4",
                            used: 3 * 1024 * GB,
                            total: 8 * 1024 * GB,
                            unreadable: None,
                        },
                        w::Mount {
                            kind: w::MountKind::Local,
                            path: "/boot",
                            device: "/dev/nvme0n1p1",
                            fstype: "vfat",
                            used: 0,
                            total: 0,
                            unreadable: Some("permission denied"),
                        },
                    ];
                    for (i, m) in mounts.iter().enumerate() {
                        w::mount_row(ui, p, m, i == 1);
                    }

                    w::section(ui, p, "Settings controls");
                    let [a, b, c] = &mut self.toggles;
                    w::toggle(ui, p, a, "Animations");
                    w::toggle(ui, p, b, "Show hidden files");
                    w::toggle(ui, p, c, "Stay on one filesystem");
                    ui.add_space(sp(2));
                    w::swatch(ui, p, canopy::ui::theme::tm::FILE_VIDEO, "Video");
                    w::swatch(
                        ui,
                        p,
                        egui::Color32::from_rgb(0x5a, 0x4a, 0x9a),
                        "Custom (too dark)",
                    );

                    w::section(ui, p, "Progress");
                    w::progress(ui, p, Some(0.62), 200.0);
                    w::progress(ui, p, None, 200.0);
                });
            });

        // Status bar variants pinned to the bottom.
        let p = self.p();
        for scan in [
            w::ScanStatus::Paused {
                dirs: 12_004,
                bytes: 88 * GB,
            },
            w::ScanStatus::Done {
                secs: 3.4,
                errors: 12,
            },
        ] {
            w::status_bar(
                ui,
                p,
                &w::StatusInfo {
                    scan,
                    selection: Some("~/Videos/holiday_2025.mp4 · 9.0 GiB · modified 4 months ago"),
                    fs_used: 940 * GB,
                    fs_total: 1000 * GB,
                    views: 1,
                    tag_counts: [0; 4],
                },
            );
        }
    }
}

impl eframe::App for Gallery {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let typing = ctx.egui_wants_keyboard_input();
        if !typing && ctx.input(|i| i.key_pressed(egui::Key::T)) {
            self.theme = if self.theme == Theme::Dark {
                Theme::Light
            } else {
                Theme::Dark
            };
            ctx.set_theme(self.theme);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Num1)) {
            self.page = Page::Treemap;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Num2)) {
            self.page = Page::Controls;
        }
        let p = self.p();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                match self.page {
                    Page::Treemap => self.page_treemap(ui),
                    Page::Controls => {
                        ui.spacing_mut().item_spacing.y = sp(2);
                        self.page_controls(ui)
                    }
                }
            });
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let theme = if arg("--theme").as_deref() == Some("light") {
        Theme::Light
    } else {
        Theme::Dark
    };
    let page = if arg("--page").as_deref() == Some("controls") {
        Page::Controls
    } else {
        Page::Treemap
    };

    let tree = fake_home();
    let root = tree.name.clone();
    let demo = Demo {
        tags: HashMap::from([
            (format!("{root}/Videos/holiday_2025.mp4"), Tag::Red),
            (format!("{root}/Downloads/win11_vm.qcow2"), Tag::Red),
            (format!("{root}/.cache/yay"), Tag::Red),
            (format!("{root}/Downloads/dataset.tar.zst"), Tag::Yellow),
            (format!("{root}/Music/Jazz"), Tag::Green),
            (format!("{root}/Pictures/raw"), Tag::Green),
        ]),
        selected: format!("{root}/Videos/holiday_2025.mp4"),
        blink: format!("{root}/Downloads/game-patch.zip"),
        pending: format!("{root}/code/site/node_modules"),
        hovered: None,
    };

    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("canopy — design gallery")
            .with_app_id("canopy")
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size(canopy::ui::theme::layout::MIN_WINDOW),
        ..Default::default()
    };
    eframe::run_native(
        "canopy-gallery",
        opts,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            style::apply(&cc.egui_ctx, theme);
            Ok(Box::new(Gallery {
                theme,
                page,
                tree,
                demo,
                filter: w::FilterBar::default(),
                filter_err: w::FilterBar {
                    text: "*.iso;>1gbb".into(),
                    error: Some("Unknown size unit 'gbb' at column 7".into()),
                    applied_terms: None,
                    presets: true,
                },
                filter_applied: w::FilterBar {
                    text: "*.mkv;*.mp4;>1gb".into(),
                    error: None,
                    applied_terms: Some(3),
                    presets: true,
                },
                toggles: [true, false, true],
            }))
        }),
    )
}
