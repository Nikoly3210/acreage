//! A view of a scan: zoom, history, selection, filter and the cached treemap layout.
//! No egui here; the screen maps `Laid` rects to pixels.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::filter::{Filter, Inherit};
use crate::layout::{Rect, squarify};
use crate::model::{Kind, NONE, NodeId, Tag, Tree};

/// Pixel geometry the layout needs (from the design tokens).
#[derive(Clone, Copy, Debug)]
pub struct LayoutParams {
    pub header_h: f32,
    pub pad: f32,
    pub gap: f32,
    pub min_block: f32,
    /// Children smaller than this (px²) are folded into one "small items" block.
    pub min_child_area: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    Node(NodeId),
    /// Folded small children of `parent`.
    Agg {
        parent: NodeId,
        count: u32,
        size: u64,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Laid {
    pub rect: Rect,
    pub item: Item,
    /// Depth below the view's zoom node.
    pub depth: u16,
    /// Index of the enclosing block in the layout, `u32::MAX` for the zoom node.
    pub parent: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir4 {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeMode {
    Disk,
    Apparent,
}

pub fn node_size(tree: &Tree, id: NodeId, mode: SizeMode) -> u64 {
    let n = tree.get(id);
    match mode {
        SizeMode::Disk => n.size_disk,
        SizeMode::Apparent => n.size_apparent,
    }
}

/// Per-node size under a filter (0 = hidden). Iterative post-order over the subtree of `root`.
pub fn filtered_sizes(
    tree: &Tree,
    root: NodeId,
    f: &Filter,
    tags: &HashMap<NodeId, Tag>,
    mode: SizeMode,
    now: i64,
) -> Vec<u64> {
    let mut out = vec![0u64; tree.len()];
    // Tags on the zoom node's ancestors still apply inside it.
    let inherited_tag = tree.ancestors(root).find_map(|a| tags.get(&a).copied());
    let root_inh = Inherit::with_tag(inherited_tag);
    let mut stack = vec![(root, root_inh, false)];
    while let Some((id, inh, done)) = stack.pop() {
        if done {
            out[id as usize] = tree.children(id).map(|c| out[c as usize]).sum();
            continue;
        }
        stack.push((id, inh, true));
        for c in tree.children(id) {
            let n = tree.get(c);
            match n.kind {
                Kind::Dir => {
                    let child_inh = f.enter_dir(&inh, tree.name(c), tags.get(&c).copied());
                    stack.push((c, child_inh, false));
                }
                Kind::Denied | Kind::MountPoint => {}
                Kind::File | Kind::Symlink | Kind::Other => {
                    let size = node_size(tree, c, mode);
                    if f.file_matches(
                        &inh,
                        tree.name(c),
                        size,
                        n.mtime,
                        tags.get(&c).copied(),
                        now,
                    ) {
                        out[c as usize] = size;
                    }
                }
            }
        }
    }
    out
}

/// Lay out the subtree of `root` into `bounds`, parents before children.
pub fn layout(
    tree: &Tree,
    root: NodeId,
    bounds: Rect,
    size: &dyn Fn(NodeId) -> u64,
    p: &LayoutParams,
    out: &mut Vec<Laid>,
) {
    out.clear();
    if bounds.w < p.min_block || bounds.h < p.min_block {
        return;
    }
    place(tree, root, bounds, 0, u32::MAX, size, p, out);
}

#[allow(clippy::too_many_arguments)]
fn place(
    tree: &Tree,
    id: NodeId,
    rect: Rect,
    depth: u16,
    parent: u32,
    size: &dyn Fn(NodeId) -> u64,
    p: &LayoutParams,
    out: &mut Vec<Laid>,
) {
    let me = out.len() as u32;
    out.push(Laid {
        rect,
        item: Item::Node(id),
        depth,
        parent,
    });
    if tree.get(id).kind != Kind::Dir {
        return;
    }
    let body = rect.inset(p.pad, p.header_h, p.pad, p.pad);
    if body.w < p.min_block || body.h < p.min_block {
        return;
    }
    let mut kids: Vec<(u64, NodeId)> = tree
        .children(id)
        .map(|c| (size(c), c))
        .filter(|(s, _)| *s > 0)
        .collect();
    if kids.is_empty() {
        return;
    }
    // Largest first; ties by id so the order is stable between frames.
    kids.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let total: u64 = kids.iter().map(|k| k.0).sum();
    let scale = body.area() as f64 / total as f64;
    let keep = kids
        .iter()
        .take_while(|k| k.0 as f64 * scale >= p.min_child_area as f64)
        .count();
    let rest: u64 = kids[keep..].iter().map(|k| k.0).sum();

    let mut values: Vec<(f64, Option<NodeId>)> = kids[..keep]
        .iter()
        .map(|k| (k.0 as f64, Some(k.1)))
        .collect();
    if rest > 0 {
        values.push((rest as f64, None));
        values.sort_by(|a, b| b.0.total_cmp(&a.0));
    }
    let sizes: Vec<f64> = values.iter().map(|v| v.0).collect();
    let rects = squarify(&sizes, body);
    let half = p.gap / 2.0;
    for (r, (v, node)) in rects.iter().zip(values) {
        let r = r.inset(half, half, half, half);
        if r.w < p.min_block || r.h < p.min_block {
            continue;
        }
        match node {
            Some(c) => place(tree, c, r, depth + 1, me, size, p, out),
            None => out.push(Laid {
                rect: r,
                item: Item::Agg {
                    parent: id,
                    count: (kids.len() - keep) as u32,
                    size: v as u64,
                },
                depth: depth + 1,
                parent: me,
            }),
        }
    }
}

type LayoutKey = (NodeId, [i32; 4], u64, u64, u64, SizeMode);

pub struct View {
    pub zoom: NodeId,
    pub history: Vec<NodeId>,
    pub selected: Option<NodeId>,
    pub filter_src: String,
    pub filter: Option<Filter>,
    filter_gen: u64,
    pub laid: Vec<Laid>,
    sizes: Option<Vec<u64>>,
    key: Option<LayoutKey>,
    last_layout: Instant,
}

impl View {
    pub fn new(zoom: NodeId) -> View {
        View {
            zoom,
            history: vec![],
            selected: None,
            filter_src: String::new(),
            filter: None,
            filter_gen: 0,
            laid: vec![],
            sizes: None,
            key: None,
            last_layout: Instant::now() - Duration::from_secs(1),
        }
    }

    pub fn set_filter(&mut self, src: String, f: Option<Filter>) {
        self.filter_src = src;
        self.filter = f;
        self.filter_gen += 1;
    }

    /// Size of a node as this view sees it (filtered or not).
    pub fn size(&self, tree: &Tree, id: NodeId, mode: SizeMode) -> u64 {
        match &self.sizes {
            Some(s) => s.get(id as usize).copied().unwrap_or(0),
            None => node_size(tree, id, mode),
        }
    }

    pub fn zoom_to(&mut self, tree: &Tree, id: NodeId) {
        if id == self.zoom || !tree.exists(id) || tree.get(id).kind != Kind::Dir {
            return;
        }
        self.history.push(self.zoom);
        self.zoom = id;
        self.key = None;
    }

    pub fn back(&mut self, tree: &Tree) -> bool {
        while let Some(z) = self.history.pop() {
            if tree.exists(z) {
                self.selected = Some(self.zoom);
                self.zoom = z;
                self.key = None;
                return true;
            }
        }
        false
    }

    pub fn up(&mut self, tree: &Tree) -> bool {
        let parent = tree.get(self.zoom).parent;
        if parent == NONE {
            return false;
        }
        let from = self.zoom;
        self.zoom_to(tree, parent);
        self.selected = Some(from);
        true
    }

    /// After nodes were removed: move zoom/selection to the nearest surviving ancestor.
    pub fn fix_after_remove(&mut self, tree: &Tree) {
        if !tree.exists(self.zoom) {
            self.zoom = tree
                .ancestors(self.zoom)
                .find(|a| tree.exists(*a))
                .unwrap_or(tree.root);
        }
        if let Some(s) = self.selected
            && !tree.exists(s)
        {
            self.selected = None;
        }
        self.history.retain(|h| tree.exists(*h));
        self.key = None;
    }

    /// Recompute the layout if anything it depends on changed. While a scan is running,
    /// growth alone re-lays out at most every `throttle` so blocks don't jitter.
    #[allow(clippy::too_many_arguments)]
    pub fn ensure_layout(
        &mut self,
        tree: &Tree,
        tags: &HashMap<NodeId, Tag>,
        tags_gen: u64,
        bounds: Rect,
        p: &LayoutParams,
        mode: SizeMode,
        now: i64,
        throttle: Duration,
    ) -> bool {
        let bkey = [
            bounds.x as i32,
            bounds.y as i32,
            bounds.w as i32,
            bounds.h as i32,
        ];
        let key = (
            self.zoom,
            bkey,
            tree.generation,
            self.filter_gen,
            tags_gen,
            mode,
        );
        if self.key == Some(key) {
            return false;
        }
        if let Some(old) = self.key {
            let only_growth = old.0 == key.0
                && old.1 == key.1
                && old.3 == key.3
                && old.4 == key.4
                && old.5 == key.5;
            if only_growth && self.last_layout.elapsed() < throttle {
                return false;
            }
        }
        self.sizes = self
            .filter
            .as_ref()
            .map(|f| filtered_sizes(tree, self.zoom, f, tags, mode, now));
        let sizes = &self.sizes;
        let size_fn = |id: NodeId| match sizes {
            Some(s) => s[id as usize],
            None => node_size(tree, id, mode),
        };
        let mut laid = std::mem::take(&mut self.laid);
        layout(tree, self.zoom, bounds, &size_fn, p, &mut laid);
        self.laid = laid;
        self.key = Some(key);
        self.last_layout = Instant::now();
        true
    }

    /// Force a re-layout next frame.
    pub fn invalidate(&mut self) {
        self.key = None;
    }

    /// Deepest block under a point.
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        self.laid.iter().rposition(|l| {
            x >= l.rect.x && y >= l.rect.y && x < l.rect.x + l.rect.w && y < l.rect.y + l.rect.h
        })
    }

    pub fn index_of(&self, id: NodeId) -> Option<usize> {
        self.laid.iter().position(|l| l.item == Item::Node(id))
    }

    /// Spatial keyboard navigation among the selected block's siblings.
    pub fn neighbor(&self, from: NodeId, dir: Dir4) -> Option<NodeId> {
        let i = self.index_of(from)?;
        let me = self.laid[i];
        let (cx, cy) = (me.rect.x + me.rect.w / 2.0, me.rect.y + me.rect.h / 2.0);
        self.laid
            .iter()
            .filter(|l| {
                l.parent == me.parent && matches!(l.item, Item::Node(_)) && l.item != me.item
            })
            .filter_map(|l| {
                let (x, y) = (l.rect.x + l.rect.w / 2.0, l.rect.y + l.rect.h / 2.0);
                let (along, across) = match dir {
                    Dir4::Right => (x - cx, (y - cy).abs()),
                    Dir4::Left => (cx - x, (y - cy).abs()),
                    Dir4::Down => (y - cy, (x - cx).abs()),
                    Dir4::Up => (cy - y, (x - cx).abs()),
                };
                (along > 1.0).then_some((along + 2.0 * across, l.item))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .and_then(|(_, item)| match item {
                Item::Node(n) => Some(n),
                Item::Agg { .. } => None,
            })
    }

    /// The first (largest) child block of the zoom node, for selecting with the keyboard.
    pub fn first_child(&self) -> Option<NodeId> {
        self.laid
            .iter()
            .find(|l| l.parent == 0)
            .and_then(|l| match l.item {
                Item::Node(n) => Some(n),
                Item::Agg { .. } => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Entry;

    fn tree() -> Tree {
        let e = |id, name: &str, kind, size| Entry {
            id,
            name: name.into(),
            kind,
            size_apparent: size,
            size_disk: size,
            mtime: 0,
            nlink: 1,
            dev: 1,
            ino: id as u64,
        };
        let mut t = Tree::new(b"/r");
        t.insert_batch(
            0,
            vec![e(1, "big", Kind::Dir, 0), e(2, "a.iso", Kind::File, 6000)],
        );
        t.insert_batch(
            1,
            (3..40)
                .map(|i| {
                    e(
                        i,
                        &format!("f{i}.jpg"),
                        Kind::File,
                        if i == 3 { 3000 } else { 5 },
                    )
                })
                .collect(),
        );
        t
    }

    const P: LayoutParams = LayoutParams {
        header_h: 16.0,
        pad: 2.0,
        gap: 1.0,
        min_block: 3.0,
        min_child_area: 144.0,
    };

    #[test]
    fn layout_folds_tiny_children() {
        let t = tree();
        let mut out = vec![];
        layout(
            &t,
            0,
            Rect::new(0.0, 0.0, 400.0, 300.0),
            &|id| t.get(id).size_disk,
            &P,
            &mut out,
        );
        assert!(matches!(out[0].item, Item::Node(0)));
        let agg = out.iter().find_map(|l| match l.item {
            Item::Agg { count, .. } => Some(count),
            _ => None,
        });
        assert_eq!(agg, Some(36));
        assert!(out.iter().any(|l| l.item == Item::Node(3)));
    }

    #[test]
    fn filter_hides_and_resizes() {
        let t = tree();
        let f = crate::filter::parse("*.jpg").unwrap().unwrap();
        let s = filtered_sizes(&t, 0, &f, &HashMap::new(), SizeMode::Disk, 0);
        assert_eq!(s[2], 0);
        assert_eq!(s[1], 3180);
        assert_eq!(s[0], 3180);
    }

    #[test]
    fn back_and_up() {
        let t = tree();
        let mut v = View::new(0);
        v.zoom_to(&t, 1);
        assert_eq!(v.zoom, 1);
        assert!(v.up(&t));
        assert_eq!(v.zoom, 0);
        assert_eq!(v.selected, Some(1));
        assert!(v.back(&t));
        assert_eq!(v.zoom, 1);
    }
}
