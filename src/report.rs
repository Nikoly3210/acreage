//! Text, CSV and JSON reports of a (sub)tree. Used by the export panel and `--report`.

use std::fmt::Write;

use crate::model::{Kind, NodeId, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    Csv,
    Json,
}

impl Format {
    pub fn parse(s: &str) -> Option<Format> {
        match s {
            "text" | "txt" => Some(Format::Text),
            "csv" => Some(Format::Csv),
            "json" => Some(Format::Json),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Format::Text => "txt",
            Format::Csv => "csv",
            Format::Json => "json",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReportOpts {
    /// Levels below the root to list (1 = the root's children).
    pub depth: usize,
    /// List files too, not just folder totals.
    pub files: bool,
    /// At most this many entries per folder; the rest are summed into one line.
    pub top: usize,
    pub format: Format,
}

impl Default for ReportOpts {
    fn default() -> Self {
        ReportOpts {
            depth: 3,
            files: false,
            top: 50,
            format: Format::Text,
        }
    }
}

pub struct Header<'a> {
    pub when: String,
    pub size_label: &'a str,
    pub filter: Option<&'a str>,
}

struct Row {
    depth: usize,
    /// For text: "├─ " / "└─ " prefixes per level.
    prefix: String,
    name: String,
    path: String,
    size: u64,
    kind: &'static str,
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Dir => "dir",
        Kind::File => "file",
        Kind::Symlink => "symlink",
        Kind::Other => "other",
        Kind::Denied => "denied",
        Kind::MountPoint => "mount",
    }
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn collect(tree: &Tree, root: NodeId, size: &dyn Fn(NodeId) -> u64, o: &ReportOpts) -> Vec<Row> {
    let mut rows = vec![Row {
        depth: 0,
        prefix: String::new(),
        name: tree.path(root).to_string_lossy().into_owned(),
        path: tree.path(root).to_string_lossy().into_owned(),
        size: size(root),
        kind: kind_name(tree.get(root).kind),
    }];
    walk(tree, root, size, o, 1, "", &mut rows);
    rows
}

fn walk(
    tree: &Tree,
    id: NodeId,
    size: &dyn Fn(NodeId) -> u64,
    o: &ReportOpts,
    depth: usize,
    indent: &str,
    rows: &mut Vec<Row>,
) {
    if depth > o.depth {
        return;
    }
    let mut kids: Vec<(u64, NodeId)> = tree
        .children(id)
        .filter(|c| {
            o.files
                || matches!(
                    tree.get(*c).kind,
                    Kind::Dir | Kind::Denied | Kind::MountPoint
                )
        })
        .map(|c| (size(c), c))
        .filter(|(s, _)| *s > 0 || tree.get(id).kind == Kind::Denied)
        .collect();
    kids.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let rest: Vec<(u64, NodeId)> = if kids.len() > o.top {
        kids.split_off(o.top)
    } else {
        vec![]
    };
    let n = kids.len() + (!rest.is_empty()) as usize;
    for (i, (s, c)) in kids.iter().enumerate() {
        let last = i + 1 == n;
        rows.push(Row {
            depth,
            prefix: format!("{indent}{}", if last { "└─ " } else { "├─ " }),
            name: tree.name_lossy(*c).into_owned(),
            path: tree.path(*c).to_string_lossy().into_owned(),
            size: *s,
            kind: kind_name(tree.get(*c).kind),
        });
        if tree.get(*c).kind == Kind::Dir {
            let child_indent = format!("{indent}{}", if last { "   " } else { "│  " });
            walk(tree, *c, size, o, depth + 1, &child_indent, rows);
        }
    }
    if !rest.is_empty() {
        let total: u64 = rest.iter().map(|r| r.0).sum();
        rows.push(Row {
            depth,
            prefix: format!("{indent}└─ "),
            name: format!("… {} more", rest.len()),
            path: String::new(),
            size: total,
            kind: "rest",
        });
    }
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn render(
    tree: &Tree,
    root: NodeId,
    size: &dyn Fn(NodeId) -> u64,
    o: &ReportOpts,
    h: &Header,
) -> String {
    let rows = collect(tree, root, size, o);
    let total = rows[0].size.max(1) as f64;
    let mut out = String::new();
    match o.format {
        Format::Text => {
            let _ = writeln!(
                out,
                "canopy report · {} · {} · {}",
                rows[0].path, h.when, h.size_label
            );
            if let Some(f) = h.filter {
                let _ = writeln!(out, "filter: {f}");
            }
            out.push('\n');
            for r in &rows {
                let pct = r.size as f64 * 100.0 / total;
                let _ = writeln!(
                    out,
                    "{:>10}  {:>5.1}%  {}{}",
                    human(r.size),
                    pct,
                    r.prefix,
                    r.name
                );
            }
        }
        Format::Csv => {
            out.push_str("size_bytes,percent,depth,kind,path\n");
            for r in &rows {
                let pct = r.size as f64 * 100.0 / total;
                let path = if r.kind == "rest" {
                    r.name.clone()
                } else {
                    r.path.clone()
                };
                let _ = writeln!(
                    out,
                    "{},{:.2},{},{},{}",
                    r.size,
                    pct,
                    r.depth,
                    r.kind,
                    csv_field(&path)
                );
            }
        }
        Format::Json => {
            out.push_str("{\n");
            let _ = writeln!(out, "  \"root\": {},", json_str(&rows[0].path));
            let _ = writeln!(out, "  \"generated\": {},", json_str(&h.when));
            let _ = writeln!(out, "  \"size\": {},", json_str(h.size_label));
            let _ = writeln!(
                out,
                "  \"filter\": {},",
                h.filter.map(json_str).unwrap_or_else(|| "null".into())
            );
            out.push_str("  \"entries\": [\n");
            for (i, r) in rows.iter().enumerate() {
                let path = if r.kind == "rest" { &r.name } else { &r.path };
                let _ = write!(
                    out,
                    "    {{\"path\": {}, \"kind\": \"{}\", \"depth\": {}, \"bytes\": {}}}",
                    json_str(path),
                    r.kind,
                    r.depth,
                    r.size
                );
                out.push_str(if i + 1 < rows.len() { ",\n" } else { "\n" });
            }
            out.push_str("  ]\n}\n");
        }
    }
    out
}

/// "2026-10-28 14:05 UTC" from Unix seconds (no date library needed for one format).
pub fn utc_stamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60
    )
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
            vec![
                e(1, "Videos", Kind::Dir, 0),
                e(2, "a,b.txt", Kind::File, 100),
                e(3, "Music", Kind::Dir, 0),
            ],
        );
        t.insert_batch(
            1,
            vec![
                e(4, "x.mkv", Kind::File, 600),
                e(5, "y.mkv", Kind::File, 200),
            ],
        );
        t.insert_batch(3, vec![e(6, "s.mp3", Kind::File, 100)]);
        t
    }

    #[test]
    fn text_folders_only_and_with_files() {
        let t = tree();
        let size = |id| t.get(id).size_disk;
        let h = Header {
            when: "now".into(),
            size_label: "disk usage",
            filter: None,
        };
        let txt = render(
            &t,
            0,
            &size,
            &ReportOpts {
                depth: 1,
                ..Default::default()
            },
            &h,
        );
        assert!(txt.contains("1000 B  100.0%  /r"), "{txt}");
        assert!(txt.contains(" 80.0%  ├─ Videos"), "{txt}");
        assert!(txt.contains(" 10.0%  └─ Music"), "{txt}");
        assert!(!txt.contains("x.mkv"));
        let txt = render(
            &t,
            0,
            &size,
            &ReportOpts {
                depth: 2,
                files: true,
                top: 1,
                ..Default::default()
            },
            &h,
        );
        assert!(txt.contains("│  └─ … 1 more"), "{txt}");
        assert!(txt.contains("x.mkv"));
    }

    #[test]
    fn csv_and_json_escape() {
        let t = tree();
        let size = |id| t.get(id).size_disk;
        let h = Header {
            when: "now".into(),
            size_label: "disk usage",
            filter: Some("*.mkv \"x\""),
        };
        let o = ReportOpts {
            depth: 1,
            files: true,
            format: Format::Csv,
            ..Default::default()
        };
        let csv = render(&t, 0, &size, &o, &h);
        assert!(csv.contains("100,10.00,1,file,\"/r/a,b.txt\""), "{csv}");
        let json = render(
            &t,
            0,
            &size,
            &ReportOpts {
                format: Format::Json,
                ..o
            },
            &h,
        );
        assert!(json.contains("\"filter\": \"*.mkv \\\"x\\\"\""), "{json}");
        assert!(
            json.contains(
                "{\"path\": \"/r/Videos\", \"kind\": \"dir\", \"depth\": 1, \"bytes\": 800}"
            ),
            "{json}"
        );
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc_stamp(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_stamp(1_793_189_100), "2026-10-28 12:05 UTC");
        assert_eq!(utc_stamp(951_782_400), "2000-02-29 00:00 UTC");
    }
}
