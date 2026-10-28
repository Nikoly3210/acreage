//! S01: choose a mounted filesystem or type a folder path.

use std::path::{Path, PathBuf};

use egui::{Key, Ui};
use egui_phosphor::regular as icon;

use crate::scan::mounts::{FsClass, MountTable};
use crate::ui::fonts;
use crate::ui::theme::{Palette, sp, ty};
use crate::ui::widgets::{self as w, Btn, MountKind, Variant};

struct Row {
    kind: MountKind,
    class: FsClass,
    path: PathBuf,
    label: String,
    device: String,
    fstype: String,
    used: u64,
    total: u64,
    unreadable: Option<String>,
}

pub struct Picker {
    rows: Vec<Row>,
    show_system: bool,
    path_text: String,
    pub error: Option<String>,
    selected: usize,
}

fn usage(path: &Path) -> Option<(u64, u64)> {
    let v = rustix::fs::statvfs(path).ok()?;
    let total = v.f_blocks * v.f_frsize;
    let used = (v.f_blocks - v.f_bfree) * v.f_frsize;
    Some((used, total))
}

fn readable(path: &Path) -> Result<(), String> {
    rustix::fs::access(
        path,
        rustix::fs::Access::READ_OK | rustix::fs::Access::EXEC_OK,
    )
    .map_err(|e| {
        if e == rustix::io::Errno::ACCESS {
            "permission denied".into()
        } else {
            e.to_string()
        }
    })
}

impl Picker {
    pub fn new(mounts: &MountTable) -> Picker {
        let mut rows: Vec<Row> = vec![];
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| "/".into());
        // Later entries for the same mount point shadow earlier ones.
        for m in mounts.mounts.iter().rev() {
            if rows.iter().any(|r| r.path == m.mount_point) {
                continue;
            }
            let class = m.class();
            let p = m.mount_point.to_string_lossy();
            let kind = if class == FsClass::Network {
                MountKind::Network
            } else if p.starts_with("/run/media/")
                || p.starts_with("/media/")
                || p.starts_with("/mnt/")
            {
                MountKind::Removable
            } else {
                MountKind::Local
            };
            // Never statvfs a network mount up front: a dead server would freeze the picker.
            let (used, total) = if class == FsClass::Local {
                usage(&m.mount_point).unwrap_or((0, 0))
            } else {
                (0, 0)
            };
            rows.push(Row {
                kind,
                class,
                path: m.mount_point.clone(),
                label: p.into_owned(),
                device: m.source.clone(),
                fstype: m.fstype.clone(),
                used,
                total,
                unreadable: if class == FsClass::Network {
                    None
                } else {
                    readable(&m.mount_point).err()
                },
            });
        }
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        let (used, total) = usage(&home).unwrap_or((0, 0));
        let home_dev = mounts
            .mount_of(&home)
            .map(|m| (m.source.clone(), m.fstype.clone()))
            .unwrap_or_default();
        rows.insert(
            0,
            Row {
                kind: MountKind::Home,
                class: FsClass::Local,
                label: home.to_string_lossy().into_owned(),
                path: home,
                device: home_dev.0,
                fstype: home_dev.1,
                used,
                total,
                unreadable: None,
            },
        );
        Picker {
            rows,
            show_system: false,
            path_text: String::new(),
            error: None,
            selected: 0,
        }
    }

    fn visible(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|i| self.show_system || self.rows[*i].class != FsClass::Pseudo)
            .collect()
    }

    /// Returns the folder to scan once the user picks one.
    pub fn ui(&mut self, ui: &mut Ui, p: &Palette) -> Option<PathBuf> {
        let mut pick = None;
        let vis = self.visible();
        let typing = ui.ctx().egui_wants_keyboard_input();
        if !typing && !vis.is_empty() {
            let pos = vis.iter().position(|i| *i == self.selected).unwrap_or(0);
            if ui.input(|i| i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::J)) {
                self.selected = vis[(pos + 1).min(vis.len() - 1)];
            }
            if ui.input(|i| i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::K)) {
                self.selected = vis[pos.saturating_sub(1)];
            }
            if ui.input(|i| i.key_pressed(Key::Enter))
                && self.rows[self.selected].unreadable.is_none()
            {
                pick = Some(self.rows[self.selected].path.clone());
            }
        }

        egui::Frame::new().fill(p.bg).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            let width = ui.available_width().min(720.0) - 2.0 * sp(6);
            let side = ((ui.available_width() - width) / 2.0).max(sp(6));
            ui.horizontal_top(|ui| {
                ui.add_space(side);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.add_space(sp(7));
                    ui.label(
                        egui::RichText::new("Choose a disk or folder")
                            .font(fonts::font(ty::XL))
                            .color(p.text),
                    );
                    ui.add_space(sp(2));
                    ui.label(
                        egui::RichText::new(
                            "canopy maps what's using the space, live, while it scans.",
                        )
                        .color(p.text_muted),
                    );
                    ui.add_space(sp(6));

                    ui.horizontal(|ui| {
                        let te = egui::TextEdit::singleline(&mut self.path_text)
                            .font(fonts::mono(ty::SM))
                            .hint_text("Type a folder path, e.g. ~/Downloads")
                            .desired_width(width - 90.0);
                        let r = ui.add(te);
                        let go = Btn::new("Scan")
                            .with_icon(icon::MAGNIFYING_GLASS_PLUS)
                            .variant(Variant::Primary)
                            .show(ui, p);
                        if go.clicked()
                            || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                        {
                            match resolve(&self.path_text) {
                                Ok(path) => pick = Some(path),
                                Err(e) => self.error = Some(e),
                            }
                        }
                    });
                    if let Some(e) = &self.error {
                        ui.label(
                            egui::RichText::new(format!("{} {e}", icon::WARNING)).color(p.danger),
                        );
                    }
                    ui.add_space(sp(6));

                    egui::ScrollArea::vertical()
                        .auto_shrink([false, true])
                        .max_height(ui.available_height() - 40.0)
                        .show(ui, |ui| {
                            for i in self.visible() {
                                let r = &self.rows[i];
                                let m = w::Mount {
                                    kind: r.kind,
                                    path: &r.label,
                                    device: &r.device,
                                    fstype: &r.fstype,
                                    used: r.used,
                                    total: r.total,
                                    unreadable: r.unreadable.as_deref(),
                                };
                                let resp = w::mount_row(ui, p, &m, i == self.selected);
                                if resp.clicked() {
                                    self.selected = i;
                                    pick = Some(r.path.clone());
                                }
                            }
                        });
                    ui.add_space(sp(4));
                    w::toggle(
                        ui,
                        p,
                        &mut self.show_system,
                        "Show system mounts (proc, tmpfs, cgroup…)",
                    );
                });
            });
        });
        pick
    }
}

/// Expand `~`, canonicalize, and require a readable directory.
pub fn resolve(text: &str) -> Result<PathBuf, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("Type a folder path first.".into());
    }
    let expanded = match t.strip_prefix('~') {
        Some(rest) => {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
            PathBuf::from(format!("{home}{rest}"))
        }
        None => PathBuf::from(t),
    };
    let path = expanded
        .canonicalize()
        .map_err(|e| format!("Can't open {}: {e}", expanded.display()))?;
    if !path.is_dir() {
        return Err(format!("{} is not a folder.", path.display()));
    }
    readable(&path).map_err(|e| format!("Can't read {}: {e}", path.display()))?;
    Ok(path)
}
