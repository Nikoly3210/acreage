//! Mount table from /proc/self/mountinfo. Mount boundaries come from here, not from st_dev,
//! so btrfs subvolumes are included and bind mounts are not counted twice.

use std::collections::HashSet;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Mount {
    pub mount_point: PathBuf,
    pub fstype: String,
    pub source: String,
    /// Path inside the filesystem that is mounted here ("/" for a whole fs, "/@home" for a
    /// btrfs subvolume, "/some/dir" for a bind mount of a directory).
    pub root: String,
    /// btrfs `subvol=` from the super options, if any.
    pub subvol: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsClass {
    Local,
    Network,
    /// proc, sysfs, cgroup, tmpfs…: never entered, hidden in the picker by default.
    Pseudo,
}

impl Mount {
    pub fn class(&self) -> FsClass {
        classify(&self.fstype)
    }
}

pub fn classify(fstype: &str) -> FsClass {
    const PSEUDO: &[&str] = &[
        "proc",
        "sysfs",
        "devtmpfs",
        "devpts",
        "tmpfs",
        "ramfs",
        "cgroup",
        "cgroup2",
        "securityfs",
        "pstore",
        "efivarfs",
        "bpf",
        "debugfs",
        "tracefs",
        "configfs",
        "fusectl",
        "mqueue",
        "hugetlbfs",
        "autofs",
        "binfmt_misc",
        "overlay",
        "nsfs",
        "rpc_pipefs",
        "selinuxfs",
        "squashfs",
        "fuse.portal",
        "fuse.gvfsd-fuse",
        "nfsd",
        "zram",
    ];
    const NETWORK: &[&str] = &[
        "nfs",
        "nfs4",
        "cifs",
        "smb3",
        "smbfs",
        "sshfs",
        "fuse.sshfs",
        "9p",
        "ceph",
        "glusterfs",
        "davfs",
        "fuse.rclone",
    ];
    if PSEUDO.contains(&fstype) {
        FsClass::Pseudo
    } else if NETWORK.contains(&fstype) {
        FsClass::Network
    } else {
        FsClass::Local
    }
}

/// Undo mountinfo's octal escapes (\040 for space, \011 tab, \012 newline, \134 backslash).
fn unescape(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() {
            let oct = &b[i + 1..i + 4];
            if oct.iter().all(|c| (b'0'..=b'7').contains(c)) {
                out.push(
                    oct.iter()
                        .fold(0u8, |acc, c| acc.wrapping_mul(8).wrapping_add(c - b'0')),
                );
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

pub fn parse_mountinfo(text: &str) -> Vec<Mount> {
    let mut out = vec![];
    for line in text.lines() {
        // id parent maj:min root mount_point options [optional…] - fstype source super_options
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let lf: Vec<&str> = left.split(' ').collect();
        let rf: Vec<&str> = right.split(' ').collect();
        if lf.len() < 5 || rf.len() < 2 {
            continue;
        }
        let subvol = rf
            .get(2)
            .and_then(|o| o.split(',').find_map(|kv| kv.strip_prefix("subvol=")))
            .map(|v| String::from_utf8_lossy(&unescape(v)).into_owned());
        out.push(Mount {
            mount_point: PathBuf::from(OsString::from_vec(unescape(lf[4]))),
            fstype: rf[0].to_owned(),
            source: String::from_utf8_lossy(&unescape(rf[1])).into_owned(),
            root: String::from_utf8_lossy(&unescape(lf[3])).into_owned(),
            subvol,
        });
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct MountTable {
    pub mounts: Vec<Mount>,
    points: HashSet<PathBuf>,
    /// Mount points a scan goes into anyway: whole btrfs subvolumes of the filesystem being
    /// scanned (see `for_scan`).
    enter: HashSet<PathBuf>,
}

impl MountTable {
    pub fn read() -> MountTable {
        // ACREAGE_MOUNTINFO lets the e2e suite fake mounts without root.
        let src = std::env::var_os("ACREAGE_MOUNTINFO")
            .map(PathBuf::from)
            .unwrap_or_else(|| "/proc/self/mountinfo".into());
        let text = std::fs::read_to_string(src).unwrap_or_default();
        MountTable::from_mounts(parse_mountinfo(&text))
    }

    pub fn from_mounts(mounts: Vec<Mount>) -> MountTable {
        let points = mounts.iter().map(|m| m.mount_point.clone()).collect();
        MountTable {
            mounts,
            points,
            enter: HashSet::new(),
        }
    }

    /// The mount that contains `path` (longest mount point prefix; later mounts win ties).
    pub fn mount_of(&self, path: &Path) -> Option<&Mount> {
        self.mounts
            .iter()
            .enumerate()
            .filter(|(_, m)| path.starts_with(&m.mount_point))
            .max_by_key(|(i, m)| (m.mount_point.as_os_str().len(), *i))
            .map(|(_, m)| m)
    }

    /// Is `path` a mount point? (Cheap: one hash lookup.)
    pub fn is_mount_point(&self, path: &Path) -> bool {
        self.points.contains(path)
    }

    /// A mount point the scan must not enter (with "stay on one filesystem").
    pub fn is_boundary(&self, path: &Path) -> bool {
        self.points.contains(path) && !self.enter.contains(path)
    }

    /// This table, prepared for scanning `root`: btrfs subvolumes mounted below it that belong
    /// to the same btrfs filesystem count as the same filesystem and are entered. A mount is
    /// only entered when it mounts a whole subvolume (its root is the subvolume itself, not a
    /// directory inside one) that isn't already part of the scan, so bind mounts and the same
    /// subvolume mounted twice are never counted twice.
    pub fn for_scan(&self, root: &Path) -> MountTable {
        let mut t = self.clone();
        t.enter.clear();
        let Some(base) = self.mount_of(root).cloned() else {
            return t;
        };
        if base.fstype != "btrfs" {
            return t;
        }
        let whole_subvol =
            |m: &Mount| m.fstype == "btrfs" && m.subvol.as_deref() == Some(m.root.as_str());
        let mut seen: HashSet<(String, String)> =
            HashSet::from([(base.source.clone(), base.root.clone())]);
        // Shallow mount points first, so a parent subvolume is decided before its children.
        let mut below: Vec<&Mount> = self
            .mounts
            .iter()
            .filter(|m| m.mount_point.starts_with(root) && m.mount_point != base.mount_point)
            .collect();
        below.sort_by_key(|m| m.mount_point.components().count());
        for m in below {
            // Only reachable if every mount point between the root and it is entered too.
            let reachable = m
                .mount_point
                .ancestors()
                .skip(1)
                .take_while(|a| a.starts_with(root) && *a != root)
                .all(|a| !self.points.contains(a) || t.enter.contains(a));
            if reachable
                && m.source == base.source
                && whole_subvol(m)
                && seen.insert((m.source.clone(), m.root.clone()))
            {
                t.enter.insert(m.mount_point.clone());
            }
        }
        t
    }

    /// Class of the filesystem mounted exactly at `path`, if any.
    pub fn class_at(&self, path: &Path) -> Option<FsClass> {
        self.mounts
            .iter()
            .rev()
            .find(|m| m.mount_point == path)
            .map(|m| m.class())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
22 1 0:21 / /proc rw,nosuid shared:5 - proc proc rw
25 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
40 25 259:3 /@home /home rw,relatime shared:2 - btrfs /dev/nvme0n1p3 rw,subvol=/@home
41 40 0:50 / /home/you/my\\040nas rw - nfs4 nas:/export rw
42 25 0:30 / /tmp rw - tmpfs tmpfs rw";

    #[test]
    fn parses_and_classifies() {
        let t = MountTable::from_mounts(parse_mountinfo(SAMPLE));
        assert_eq!(t.mounts.len(), 5);
        assert!(t.is_mount_point(Path::new("/home/you/my nas")));
        assert_eq!(
            t.class_at(Path::new("/home/you/my nas")),
            Some(FsClass::Network)
        );
        assert_eq!(t.class_at(Path::new("/proc")), Some(FsClass::Pseudo));
        assert_eq!(
            t.mount_of(Path::new("/home/you/x")).unwrap().fstype,
            "btrfs"
        );
        assert_eq!(t.mount_of(Path::new("/etc")).unwrap().fstype, "ext4");
        assert_eq!(
            t.mount_of(Path::new("/home/you/my nas/a")).unwrap().source,
            "nas:/export"
        );
    }

    #[test]
    fn btrfs_subvolumes_of_the_same_fs_are_entered() {
        let info = "\
25 1 0:30 /@ / rw - btrfs /dev/sda2 rw,subvol=/@
26 25 0:31 /@home /home rw - btrfs /dev/sda2 rw,subvol=/@home
27 25 0:32 /@log /var/log rw - btrfs /dev/sda2 rw,subvol=/@log
28 25 0:33 /@home /srv/home-again rw - btrfs /dev/sda2 rw,subvol=/@home
29 25 0:34 /@/opt/x /mnt/bind rw - btrfs /dev/sda2 rw,subvol=/@
30 25 8:17 / /data rw - btrfs /dev/sdb1 rw,subvol=/
31 25 0:40 / /tmp rw - tmpfs tmpfs rw";
        let t = MountTable::from_mounts(parse_mountinfo(info)).for_scan(Path::new("/"));
        assert!(
            !t.is_boundary(Path::new("/home")),
            "subvolume of the same fs"
        );
        assert!(!t.is_boundary(Path::new("/var/log")));
        assert!(
            t.is_boundary(Path::new("/srv/home-again")),
            "same subvolume twice"
        );
        assert!(
            t.is_boundary(Path::new("/mnt/bind")),
            "bind mount of a directory"
        );
        assert!(
            t.is_boundary(Path::new("/data")),
            "another btrfs filesystem"
        );
        assert!(t.is_boundary(Path::new("/tmp")));
        // Scanning ext4 changes nothing.
        let e = MountTable::from_mounts(parse_mountinfo(
            "1 0 8:1 / / rw - ext4 /dev/sda1 rw\n2 1 8:2 / /home rw - ext4 /dev/sda2 rw",
        ));
        assert!(e.for_scan(Path::new("/")).is_boundary(Path::new("/home")));
    }
}
