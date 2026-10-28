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
        out.push(Mount {
            mount_point: PathBuf::from(OsString::from_vec(unescape(lf[4]))),
            fstype: rf[0].to_owned(),
            source: String::from_utf8_lossy(&unescape(rf[1])).into_owned(),
        });
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct MountTable {
    pub mounts: Vec<Mount>,
    points: HashSet<PathBuf>,
}

impl MountTable {
    pub fn read() -> MountTable {
        let text = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        MountTable::from_mounts(parse_mountinfo(&text))
    }

    pub fn from_mounts(mounts: Vec<Mount>) -> MountTable {
        let points = mounts.iter().map(|m| m.mount_point.clone()).collect();
        MountTable { mounts, points }
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
}
