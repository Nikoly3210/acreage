use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub type NodeId = u32;
pub const NONE: NodeId = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Symlink,
    Other,
    /// A directory we could not read.
    Denied,
    /// Another filesystem mounted here; not entered.
    MountPoint,
}

const F_SCANNED: u8 = 1;
const F_HARDLINK_DUP: u8 = 2;
const F_REMOVED: u8 = 4;
const F_PLACEHOLDER: u8 = 8;

#[derive(Clone, Debug)]
pub struct Node {
    pub parent: NodeId,
    first_child: NodeId,
    next_sibling: NodeId,
    name_off: u32,
    name_len: u16,
    pub kind: Kind,
    flags: u8,
    /// Own size for files; subtree sum (including the dir's own blocks) for dirs.
    pub size_apparent: u64,
    pub size_disk: u64,
    /// Files in the subtree (a file counts itself).
    pub items: u64,
    /// Unix seconds; for dirs the newest mtime in the subtree.
    pub mtime: i64,
    pub nlink: u32,
}

impl Node {
    fn placeholder() -> Node {
        Node {
            parent: NONE,
            first_child: NONE,
            next_sibling: NONE,
            name_off: 0,
            name_len: 0,
            kind: Kind::Other,
            flags: F_PLACEHOLDER,
            size_apparent: 0,
            size_disk: 0,
            items: 0,
            mtime: 0,
            nlink: 1,
        }
    }
}

/// What `apply_listing` changed.
#[derive(Debug, Default)]
pub struct ListingDiff {
    pub removed: Vec<NodeId>,
    pub changed: Vec<NodeId>,
    pub added: Vec<NodeId>,
    /// Added entries that are folders and still need scanning.
    pub new_dirs: Vec<NodeId>,
}

/// Everything the scanner knows about one directory entry.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: NodeId,
    pub name: Vec<u8>,
    pub kind: Kind,
    pub size_apparent: u64,
    pub size_disk: u64,
    pub mtime: i64,
    pub nlink: u32,
    pub dev: u64,
    pub ino: u64,
}

/// Arena tree. Node ids are handed out by the scanner, so slots can arrive out of order;
/// unseen slots are placeholders.
pub struct Tree {
    nodes: Vec<Node>,
    names: Vec<u8>,
    pub root: NodeId,
    /// Bumped on every size or structure change. Views use it to invalidate layouts.
    pub generation: u64,
    seen_inodes: HashSet<(u64, u64)>,
}

impl Tree {
    /// A tree with only the root (id 0), named by its full path.
    pub fn new(root_path: &[u8]) -> Tree {
        let mut t = Tree {
            nodes: Vec::new(),
            names: Vec::new(),
            root: 0,
            generation: 0,
            seen_inodes: HashSet::new(),
        };
        let mut root = Node::placeholder();
        root.flags = 0;
        root.kind = Kind::Dir;
        (root.name_off, root.name_len) = t.push_name(root_path);
        t.nodes.push(root);
        t
    }

    fn push_name(&mut self, name: &[u8]) -> (u32, u16) {
        let name = &name[..name.len().min(u16::MAX as usize)];
        let off = self.names.len() as u32;
        self.names.extend_from_slice(name);
        (off, name.len() as u16)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn get(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn exists(&self, id: NodeId) -> bool {
        (id as usize) < self.nodes.len()
            && self.nodes[id as usize].flags & (F_REMOVED | F_PLACEHOLDER) == 0
    }

    pub fn name(&self, id: NodeId) -> &[u8] {
        let n = &self.nodes[id as usize];
        &self.names[n.name_off as usize..n.name_off as usize + n.name_len as usize]
    }

    /// Display form; invalid UTF-8 is replaced, never used for syscalls.
    pub fn name_lossy(&self, id: NodeId) -> Cow<'_, str> {
        String::from_utf8_lossy(self.name(id))
    }

    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut parts = vec![];
        let mut cur = id;
        while cur != NONE {
            parts.push(cur);
            cur = self.nodes[cur as usize].parent;
        }
        let mut p = PathBuf::new();
        for id in parts.into_iter().rev() {
            p.push(OsStr::from_bytes(self.name(id)));
        }
        p
    }

    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut cur = id;
        std::iter::from_fn(move || {
            if cur == NONE {
                return None;
            }
            let out = cur;
            cur = self.nodes[cur as usize].parent;
            Some(out)
        })
    }

    pub fn depth(&self, id: NodeId) -> usize {
        self.ancestors(id).count() - 1
    }

    /// True when `a` is `b` or one of its ancestors.
    pub fn is_ancestor_or_self(&self, a: NodeId, b: NodeId) -> bool {
        self.ancestors(b).any(|x| x == a)
    }

    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut cur = self.nodes[id as usize].first_child;
        std::iter::from_fn(move || {
            if cur == NONE {
                return None;
            }
            let out = cur;
            cur = self.nodes[cur as usize].next_sibling;
            Some(out)
        })
    }

    pub fn is_scanned(&self, id: NodeId) -> bool {
        self.nodes[id as usize].flags & F_SCANNED != 0
    }

    pub fn is_hardlink_dup(&self, id: NodeId) -> bool {
        self.nodes[id as usize].flags & F_HARDLINK_DUP != 0
    }

    pub fn set_scanned(&mut self, id: NodeId) {
        if self.exists(id) {
            self.nodes[id as usize].flags |= F_SCANNED;
            self.generation += 1;
        }
    }

    pub fn set_denied(&mut self, id: NodeId) {
        if self.exists(id) {
            let n = &mut self.nodes[id as usize];
            n.kind = Kind::Denied;
            n.flags |= F_SCANNED;
            self.generation += 1;
        }
    }

    /// Splice a batch of children under `parent` and propagate their sizes up.
    /// Returns false (and does nothing) if the parent is gone, e.g. trashed mid-scan.
    pub fn insert_batch(&mut self, parent: NodeId, entries: Vec<Entry>) -> bool {
        if !self.exists(parent) {
            return false;
        }
        let (mut da, mut dd, mut di, mut mt) = (0u64, 0u64, 0u64, i64::MIN);
        for e in entries {
            let idx = e.id as usize;
            if self.nodes.len() <= idx {
                self.nodes.resize(idx + 1, Node::placeholder());
            }
            let (name_off, name_len) = self.push_name(&e.name);
            let dup =
                e.kind == Kind::File && e.nlink > 1 && !self.seen_inodes.insert((e.dev, e.ino));
            let (a, d) = if dup {
                (0, 0)
            } else {
                (e.size_apparent, e.size_disk)
            };
            let is_file = !matches!(e.kind, Kind::Dir | Kind::Denied | Kind::MountPoint);
            let parent_first = self.nodes[parent as usize].first_child;
            self.nodes[idx] = Node {
                parent,
                first_child: NONE,
                next_sibling: parent_first,
                name_off,
                name_len,
                kind: e.kind,
                flags: if dup { F_HARDLINK_DUP } else { 0 }
                    | if is_file || e.kind == Kind::MountPoint {
                        F_SCANNED
                    } else {
                        0
                    },
                size_apparent: a,
                size_disk: d,
                items: is_file as u64,
                mtime: e.mtime,
                nlink: e.nlink,
            };
            self.nodes[parent as usize].first_child = e.id;
            da += a;
            dd += d;
            di += is_file as u64;
            mt = mt.max(e.mtime);
        }
        self.apply_delta(parent, da as i64, dd as i64, di as i64, mt);
        true
    }

    /// The single place where subtree sizes change: add deltas to `id` and every ancestor.
    pub fn apply_delta(&mut self, id: NodeId, da: i64, dd: i64, di: i64, mtime: i64) {
        let mut cur = id;
        while cur != NONE {
            let n = &mut self.nodes[cur as usize];
            n.size_apparent = n.size_apparent.saturating_add_signed(da);
            n.size_disk = n.size_disk.saturating_add_signed(dd);
            n.items = n.items.saturating_add_signed(di);
            n.mtime = n.mtime.max(mtime);
            cur = n.parent;
        }
        self.generation += 1;
    }

    /// Detach a subtree (after it was trashed) and subtract its sizes from the ancestors.
    pub fn remove(&mut self, id: NodeId) {
        if !self.exists(id) || id == self.root {
            return;
        }
        let n = self.nodes[id as usize].clone();
        // Unlink from the parent's child list.
        let p = n.parent;
        if self.nodes[p as usize].first_child == id {
            self.nodes[p as usize].first_child = n.next_sibling;
        } else {
            let mut c = self.nodes[p as usize].first_child;
            while c != NONE {
                if self.nodes[c as usize].next_sibling == id {
                    self.nodes[c as usize].next_sibling = n.next_sibling;
                    break;
                }
                c = self.nodes[c as usize].next_sibling;
            }
        }
        self.apply_delta(
            p,
            -(n.size_apparent as i64),
            -(n.size_disk as i64),
            -(n.items as i64),
            i64::MIN,
        );
        // Mark the whole subtree removed so late scanner batches for it are dropped.
        let mut stack = vec![id];
        while let Some(x) = stack.pop() {
            stack.extend(self.children(x));
            self.nodes[x as usize].flags |= F_REMOVED;
        }
    }

    pub fn child_by_name(&self, id: NodeId, name: &[u8]) -> Option<NodeId> {
        self.children(id).find(|c| self.name(*c) == name)
    }

    /// Node for an absolute path inside this tree, if it has been scanned.
    pub fn find(&self, path: &Path) -> Option<NodeId> {
        let root = Path::new(OsStr::from_bytes(self.name(self.root)));
        let rel = path.strip_prefix(root).ok()?;
        let mut cur = self.root;
        for comp in rel.components() {
            cur = self.child_by_name(cur, comp.as_os_str().as_bytes())?;
        }
        Some(cur)
    }

    /// Drop everything below `id` (before re-scanning it) and mark it pending again.
    pub fn clear_children(&mut self, id: NodeId) {
        if !self.exists(id) {
            return;
        }
        let kids: Vec<NodeId> = self.children(id).collect();
        for k in kids {
            self.remove(k);
        }
        self.nodes[id as usize].flags &= !F_SCANNED;
        self.generation += 1;
    }

    /// Bring one directory's direct children in line with a fresh listing of it.
    /// `alloc` hands out ids for new entries. New sub-folders come back in `new_dirs`
    /// so the caller can scan them.
    pub fn apply_listing(
        &mut self,
        dir: NodeId,
        listing: Vec<Entry>,
        alloc: &mut dyn FnMut() -> NodeId,
    ) -> ListingDiff {
        let mut diff = ListingDiff::default();
        if !self.exists(dir) {
            return diff;
        }
        let mut existing: HashMap<Vec<u8>, NodeId> = self
            .children(dir)
            .map(|c| (self.name(c).to_vec(), c))
            .collect();
        let mut added = vec![];
        for mut e in listing {
            match existing.remove(&e.name) {
                Some(id) if self.nodes[id as usize].kind == e.kind => {
                    let n = &self.nodes[id as usize];
                    let file_like = !matches!(n.kind, Kind::Dir | Kind::Denied | Kind::MountPoint);
                    let changed = n.mtime != e.mtime
                        || (file_like
                            && (n.size_apparent != e.size_apparent || n.size_disk != e.size_disk));
                    if changed && file_like {
                        let dup = self.is_hardlink_dup(id);
                        let (a, d) = if dup {
                            (0, 0)
                        } else {
                            (e.size_apparent, e.size_disk)
                        };
                        let (da, dd) = (
                            a as i64 - n.size_apparent as i64,
                            d as i64 - n.size_disk as i64,
                        );
                        let node = &mut self.nodes[id as usize];
                        node.size_apparent = a;
                        node.size_disk = d;
                        node.mtime = e.mtime;
                        node.nlink = e.nlink;
                        self.apply_delta(dir, da, dd, 0, e.mtime);
                        diff.changed.push(id);
                    } else if changed {
                        self.nodes[id as usize].mtime = self.nodes[id as usize].mtime.max(e.mtime);
                    }
                }
                Some(id) => {
                    // Same name, different kind (file replaced by a folder…): replace it.
                    self.remove(id);
                    diff.removed.push(id);
                    e.id = alloc();
                    added.push(e);
                }
                None => {
                    e.id = alloc();
                    added.push(e);
                }
            }
        }
        for (_, id) in existing {
            self.remove(id);
            diff.removed.push(id);
        }
        for e in &added {
            diff.added.push(e.id);
            if e.kind == Kind::Dir {
                diff.new_dirs.push(e.id);
            }
        }
        if !added.is_empty() {
            self.insert_batch(dir, added);
        }
        diff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: NodeId, name: &str, kind: Kind, size: u64) -> Entry {
        Entry {
            id,
            name: name.into(),
            kind,
            size_apparent: size,
            size_disk: size,
            mtime: id as i64,
            nlink: 1,
            dev: 1,
            ino: id as u64,
        }
    }

    #[test]
    fn sizes_propagate_and_remove_subtracts() {
        let mut t = Tree::new(b"/r");
        assert!(t.insert_batch(0, vec![e(1, "a", Kind::Dir, 4), e(2, "f", Kind::File, 10)]));
        assert!(t.insert_batch(
            1,
            vec![e(3, "g", Kind::File, 100), e(4, "h", Kind::File, 1)]
        ));
        assert_eq!(t.get(0).size_disk, 115);
        assert_eq!(t.get(1).size_disk, 105);
        assert_eq!(t.get(0).items, 3);
        assert_eq!(t.path(3), PathBuf::from("/r/a/g"));
        t.remove(1);
        assert_eq!(t.get(0).size_disk, 10);
        assert_eq!(t.get(0).items, 1);
        assert_eq!(t.children(0).collect::<Vec<_>>(), vec![2]);
        // A late batch for the removed dir is ignored.
        assert!(!t.insert_batch(1, vec![e(5, "late", Kind::File, 7)]));
        assert_eq!(t.get(0).size_disk, 10);
    }

    #[test]
    fn hardlinks_count_once() {
        let mut t = Tree::new(b"/r");
        let mut a = e(1, "a", Kind::File, 50);
        let mut b = e(2, "b", Kind::File, 50);
        a.nlink = 2;
        b.nlink = 2;
        b.ino = a.ino;
        t.insert_batch(0, vec![a, b]);
        assert_eq!(t.get(0).size_disk, 50);
        assert!(t.is_hardlink_dup(2) ^ t.is_hardlink_dup(1));
    }

    #[test]
    fn out_of_order_ids() {
        let mut t = Tree::new(b"/r");
        t.insert_batch(0, vec![e(7, "late-id", Kind::Dir, 1)]);
        assert!(t.exists(7));
        assert!(!t.exists(3));
        t.insert_batch(7, vec![e(3, "x", Kind::File, 5)]);
        assert_eq!(t.get(0).size_disk, 6);
    }

    #[test]
    fn find_clear_and_listing_diff() {
        let mut t = Tree::new(b"/r");
        t.insert_batch(0, vec![e(1, "a", Kind::Dir, 4), e(2, "f", Kind::File, 10)]);
        t.insert_batch(1, vec![e(3, "g", Kind::File, 100)]);
        assert_eq!(t.find(Path::new("/r/a/g")), Some(3));
        assert_eq!(t.find(Path::new("/r")), Some(0));
        assert_eq!(t.find(Path::new("/elsewhere")), None);

        // Listing of /r: f grew to 50, a stays, new file n and new dir d appear.
        let mut next = 10;
        let mut alloc = || {
            next += 1;
            next
        };
        let mut f2 = e(0, "f", Kind::File, 50);
        f2.mtime = 99;
        let listing = vec![
            e(0, "a", Kind::Dir, 4),
            f2,
            e(0, "n", Kind::File, 7),
            e(0, "d", Kind::Dir, 4),
        ];
        let diff = t.apply_listing(0, listing, &mut alloc);
        assert_eq!(diff.changed, vec![2]);
        assert_eq!(diff.added.len(), 2);
        assert_eq!(diff.new_dirs.len(), 1);
        assert_eq!(t.get(0).size_disk, 4 + 100 + 50 + 7 + 4);
        // f disappears.
        let diff = t.apply_listing(
            0,
            vec![
                e(0, "a", Kind::Dir, 4),
                e(0, "n", Kind::File, 7),
                e(0, "d", Kind::Dir, 4),
            ],
            &mut alloc,
        );
        assert_eq!(diff.removed, vec![2]);
        assert_eq!(t.get(0).size_disk, 4 + 100 + 7 + 4);

        t.clear_children(1);
        assert!(!t.is_scanned(1));
        assert_eq!(t.get(1).size_disk, 4);
        assert_eq!(t.get(0).size_disk, 4 + 7 + 4);
    }
}
