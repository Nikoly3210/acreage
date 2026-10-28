//! Full scans, folder rescans, live updates and tags-by-path resolution.

use std::collections::{HashSet, VecDeque};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::{Browse, ScanState, Sub};
use crate::model::{Kind, NodeId, Tree};
use crate::scan::{ScanEvent, ScanOpts, Scanner};
use crate::ui::theme::motion;
use crate::ui::widgets::{ToastKind, group_digits};
use crate::view::View;
use crate::watch::{Listing, Watcher};

const INGEST_BUDGET: Duration = Duration::from_millis(4);
/// How often unresolved tags are matched against a growing tree.
const RESOLVE_EVERY: f64 = 0.25;

impl Browse {
    pub(super) fn wake(&self) -> Arc<dyn Fn() + Send + Sync> {
        let ctx = self.ctx.clone();
        Arc::new(move || ctx.request_repaint())
    }

    /// Throw the tree away and scan everything again. Tags survive: they are kept by path.
    pub(super) fn start_scan(&mut self) {
        self.scan_gen += 1;
        // Anything holding node ids of the old tree goes.
        self.confirm = None;
        self.menu_target = None;
        self.hovered = None;
        self.anim = None;
        self.subs.clear();
        self.watcher = None;
        self.changed.clear();
        self.opts = ScanOpts {
            one_fs: self.cfg.one_filesystem,
        };
        self.tree = Tree::new(self.root_path.as_os_str().as_bytes());
        for t in &mut self.tabs {
            let (src, f, hide) = (
                t.view.filter_src.clone(),
                t.view.filter.take(),
                t.view.hide_dot,
            );
            t.view = View::new(0);
            t.view.set_filter(src, f);
            t.view.set_hide_dot(hide);
        }
        self.tags.clear();
        self.tags_gen += 1;
        self.errors.clear();
        self.dirs_done = 0;
        self.state = ScanState::Scanning;
        let s = Scanner::start(
            self.root_path.clone(),
            self.opts,
            self.mounts.clone(),
            self.wake(),
        );
        self.ids = s.ids();
        self.scanner = Some(s);
    }

    /// Rescan one folder (S04 refinement), even while the main scan is still running.
    pub(super) fn rescan_folder(&mut self, id: NodeId) {
        if !self.tree.exists(id) || self.tree.get(id).kind != Kind::Dir {
            return;
        }
        if id == self.tree.root {
            self.start_scan();
            return;
        }
        if !self.tree.is_scanned(id) {
            self.toast(
                ToastKind::Info,
                "That folder is still being scanned.".into(),
            );
            return;
        }
        if self
            .subs
            .iter()
            .any(|s| self.tree.is_ancestor_or_self(s.root, id))
        {
            return; // already being rescanned (or one of its parents is)
        }
        // Sub-scans inside it are superseded by this one.
        let tree = &self.tree;
        self.subs.retain(|s| !tree.is_ancestor_or_self(id, s.root));
        self.tree.clear_children(id);
        self.after_tree_shrank();
        let path = self.tree.path(id);
        let s = Scanner::start_at(
            path,
            id,
            self.ids.clone(),
            self.opts,
            self.mounts.clone(),
            self.wake(),
        );
        self.subs.push(Sub {
            root: id,
            scanner: s,
            user: true,
        });
    }

    fn start_sub(&mut self, id: NodeId) {
        let path = self.tree.path(id);
        let s = Scanner::start_at(
            path,
            id,
            self.ids.clone(),
            self.opts,
            self.mounts.clone(),
            self.wake(),
        );
        self.subs.push(Sub {
            root: id,
            scanner: s,
            user: false,
        });
    }

    /// After nodes disappeared without being deleted on disk: drop resolved ids, keep tags by path.
    fn after_tree_shrank(&mut self) {
        let tree = &self.tree;
        self.tags.retain(|id, _| tree.exists(*id));
        self.changed.retain(|id, _| tree.exists(*id));
        self.tags_gen += 1;
        for t in &mut self.tabs {
            t.view.fix_after_remove(&self.tree);
        }
        self.hovered = None;
        if self.menu_target.is_some_and(|m| !self.tree.exists(m)) {
            self.menu_target = None;
        }
    }

    /// Splice scanner events (main scan and sub-scans) into the tree, at most
    /// `INGEST_BUDGET` per frame.
    pub(super) fn ingest(&mut self) {
        let t0 = Instant::now();
        let gen0 = self.tree.generation;
        let mut finished: Vec<NodeId> = vec![];
        loop {
            let mut any = false;
            if let Some(ev) = self.scanner.as_ref().and_then(|s| s.events.try_recv().ok()) {
                any = true;
                self.on_event(ev, None, &mut finished);
            }
            for i in 0..self.subs.len() {
                if let Some(ev) = self
                    .subs
                    .get(i)
                    .and_then(|s| s.scanner.events.try_recv().ok())
                {
                    any = true;
                    let root = self.subs[i].root;
                    self.on_event(ev, Some(root), &mut finished);
                }
            }
            if !any {
                break;
            }
            if t0.elapsed() > INGEST_BUDGET {
                self.ctx.request_repaint();
                break;
            }
        }
        for root in finished {
            let Some(i) = self.subs.iter().position(|s| s.root == root) else {
                continue;
            };
            let sub = self.subs.remove(i);
            if sub.user && self.tree.exists(root) {
                let msg = format!("Rescanned {}", self.tree.path(root).display());
                self.toast(ToastKind::Success, msg);
            }
            self.watch_subtree(&[root], !sub.user);
            self.resolve_tags(true);
        }
        if self.tree.generation != gen0 {
            self.resolve_tags(false);
        }
    }

    fn on_event(&mut self, ev: ScanEvent, sub: Option<NodeId>, finished: &mut Vec<NodeId>) {
        match ev {
            ScanEvent::Batch { parent, entries } => {
                self.tree.insert_batch(parent, entries);
            }
            ScanEvent::DirDone { id } => {
                self.tree.set_scanned(id);
                if sub.is_none() {
                    self.dirs_done += 1;
                }
            }
            ScanEvent::DirError { id, err } => {
                self.tree.set_denied(id);
                if !self.errors.iter().any(|(e, _)| *e == id) {
                    self.errors.push((id, err));
                }
            }
            ScanEvent::Done { cancelled, elapsed } => match sub {
                None => {
                    self.state = ScanState::Done {
                        secs: elapsed.as_secs_f32(),
                        cancelled,
                    };
                    for t in &mut self.tabs {
                        t.view.invalidate();
                    }
                    self.resolve_tags(true);
                    self.start_watcher();
                }
                Some(root) => finished.push(root),
            },
        }
    }

    /// Match tags kept by path to nodes. Throttled unless `now` (scan just finished).
    fn resolve_tags(&mut self, now: bool) {
        let t = self.ctx.input(|i| i.time);
        if self.tag_paths.is_empty() || (!now && t - self.resolve_at < RESOLVE_EVERY) {
            return;
        }
        self.resolve_at = t;
        let mut changed = false;
        for (p, tag) in &self.tag_paths {
            if let Some(id) = self.tree.find(p)
                && self.tags.get(&id) != Some(tag)
            {
                self.tags.insert(id, *tag);
                changed = true;
            }
        }
        if changed {
            self.tags_gen += 1;
        }
    }

    // ---- live updates (F06)

    pub(super) fn start_watcher(&mut self) {
        self.watcher = None;
        if !self.cfg.live_updates {
            return;
        }
        match Watcher::start(self.mounts.clone(), self.opts, self.wake()) {
            Ok(w) => {
                self.watcher = Some(w);
                // The folder being looked at first, then everything else, until the budget.
                let zoom = self.tabs[self.active].view.zoom;
                self.watch_subtree(&[zoom, self.tree.root], false);
            }
            Err(e) => self.toast(ToastKind::Warning, format!("Live updates are off: {e}")),
        }
    }

    pub(super) fn stop_watcher(&mut self) {
        self.watcher = None;
        self.changed.clear();
    }

    /// Watch every folder under `roots`, breadth first, within the watch budget.
    fn watch_subtree(&mut self, roots: &[NodeId], new: bool) {
        let Some(w) = &self.watcher else { return };
        let room = w.budget.saturating_sub(w.watched());
        let mut seen = HashSet::new();
        let mut queue: VecDeque<NodeId> = roots
            .iter()
            .copied()
            .filter(|r| self.tree.exists(*r))
            .collect();
        let mut out: Vec<PathBuf> = vec![];
        while let Some(id) = queue.pop_front() {
            if out.len() >= room {
                break;
            }
            if !seen.insert(id) || self.tree.get(id).kind != Kind::Dir {
                continue;
            }
            out.push(self.tree.path(id));
            queue.extend(
                self.tree
                    .children(id)
                    .filter(|c| self.tree.get(*c).kind == Kind::Dir),
            );
        }
        if new { w.watch_new(out) } else { w.watch(out) }
    }

    /// Apply folder listings from the watcher and age the blink.
    pub(super) fn live(&mut self) {
        let now = self.ctx.input(|i| i.time);
        let listings: Vec<Listing> = match &self.watcher {
            Some(w) => w.listings.try_iter().take(256).collect(),
            None => vec![],
        };
        for l in listings {
            self.apply_listing(l, now);
        }
        if let Some(w) = &self.watcher
            && w.limited()
            && !self.limit_warned
        {
            self.limit_warned = true;
            let msg = format!(
                "Live updates are limited to {} folders. Raise fs.inotify.max_user_watches for more.",
                group_digits(w.watched() as u64)
            );
            self.toast(ToastKind::Warning, msg);
        }
        self.changed.retain(|_, t| now - *t < motion::BLINK as f64);
        if !self.changed.is_empty() {
            self.ctx.request_repaint();
        }
    }

    fn apply_listing(&mut self, l: Listing, now: f64) {
        let Some(dir) = self.tree.find(&l.dir) else {
            return;
        };
        if self.tree.get(dir).kind != Kind::Dir || !self.tree.is_scanned(dir) {
            return;
        }
        if self
            .subs
            .iter()
            .any(|s| self.tree.is_ancestor_or_self(s.root, dir))
        {
            return; // a sub-scan is rewriting this part of the tree
        }
        // An error means the folder itself went away; its parent's listing removes it.
        let Ok(entries) = l.entries else { return };
        let ids = self.ids.clone();
        let mut alloc = || ids.fetch_add(1, Ordering::Relaxed);
        let diff = self.tree.apply_listing(dir, entries, &mut alloc);
        if !diff.removed.is_empty() {
            for id in &diff.removed {
                let p = self.tree.path(*id);
                self.tag_paths.retain(|tp, _| !tp.starts_with(&p));
            }
            self.after_tree_shrank();
        }
        if self.cfg.animations {
            for id in diff
                .changed
                .iter()
                .chain(&diff.added)
                .chain(std::iter::once(&dir))
            {
                self.changed.insert(*id, now);
            }
        }
        for d in diff.new_dirs {
            self.start_sub(d);
        }
        if !diff.added.is_empty() {
            self.resolve_tags(true);
        }
    }
}
