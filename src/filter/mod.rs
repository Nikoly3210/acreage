//! The one-line filter language.
//!
//! ```text
//! *.mkv;*.mp4;>1gb;<6months;|:green;|node_modules
//! ```
//!
//! Terms are separated by `;`. A leading `|` excludes instead of includes.
//! - glob: `*.jpg`, `IMG_*`. A plain word (`cache`) means `*cache*`. Case-insensitive.
//!   A glob that matches a folder name matches everything inside that folder.
//! - size: `>1gb`, `<500kb` (1024-based: b, k/kb/kib, m/mb/mib, g/gb/gib, t/tb/tib)
//! - age: `>2years` (older than), `<3months` (newer than); units h, d, w, mo, y and their
//!   long forms. Months are 30.44 days, years 365.25 days.
//! - tag: `:red`, `:yellow`, `:green`, `:blue`, `:all`. A tagged folder tags its contents.
//!
//! Combination: globs OR together, tags OR together, size and age conditions AND together
//! (so `>1mb;<10mb` is a range), and the groups AND with each other. Any exclude term that
//! matches removes the file. Filters select files; a folder shows if anything inside it does.

use std::fmt;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::model::Tag;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Gt,
    Lt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagSel {
    One(Tag),
    All,
}

impl TagSel {
    fn matches(self, tag: Option<Tag>) -> bool {
        match self {
            TagSel::All => tag.is_some(),
            TagSel::One(t) => tag == Some(t),
        }
    }
}

#[derive(Default, Debug)]
struct Group {
    globs: Option<GlobSet>,
    sizes: Vec<(Cmp, u64)>,
    /// (cmp, seconds): `>` = older than.
    ages: Vec<(Cmp, i64)>,
    tags: Vec<TagSel>,
}

impl Group {
    fn is_empty(&self) -> bool {
        self.globs.is_none()
            && self.sizes.is_empty()
            && self.ages.is_empty()
            && self.tags.is_empty()
    }
}

#[derive(Debug)]
pub struct Filter {
    include: Group,
    exclude: Group,
    pub terms: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based column of the offending term.
    pub col: usize,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (column {})", self.msg, self.col)
    }
}

/// What a folder passes down to everything inside it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Inherit {
    glob_hit: bool,
    excluded: bool,
    pub tag: Option<Tag>,
}

impl Inherit {
    /// Starting state for a view rooted below a tagged folder.
    pub fn with_tag(tag: Option<Tag>) -> Inherit {
        Inherit {
            tag,
            ..Default::default()
        }
    }
}

const KIB: f64 = 1024.0;
const DAY: f64 = 86_400.0;

fn size_unit(u: &str) -> Option<f64> {
    Some(match u {
        "b" => 1.0,
        "k" | "kb" | "kib" => KIB,
        "m" | "mb" | "mib" => KIB * KIB,
        "g" | "gb" | "gib" => KIB * KIB * KIB,
        "t" | "tb" | "tib" => KIB * KIB * KIB * KIB,
        _ => return None,
    })
}

fn age_unit(u: &str) -> Option<f64> {
    Some(match u {
        "h" | "hour" | "hours" => 3600.0,
        "d" | "day" | "days" => DAY,
        "w" | "week" | "weeks" => 7.0 * DAY,
        "mo" | "month" | "months" => 30.44 * DAY,
        "y" | "yr" | "year" | "years" => 365.25 * DAY,
        _ => return None,
    })
}

fn tag_sel(s: &str) -> Option<TagSel> {
    Some(match s {
        "red" => TagSel::One(Tag::Red),
        "yellow" => TagSel::One(Tag::Yellow),
        "green" => TagSel::One(Tag::Green),
        "blue" => TagSel::One(Tag::Blue),
        "all" | "any" => TagSel::All,
        _ => return None,
    })
}

pub fn parse(src: &str) -> Result<Option<Filter>, ParseError> {
    let mut inc_globs = GlobSetBuilder::new();
    let mut exc_globs = GlobSetBuilder::new();
    let (mut n_inc_globs, mut n_exc_globs) = (0, 0);
    let mut include = Group::default();
    let mut exclude = Group::default();
    let mut terms = 0;

    let mut col = 1;
    for raw in src.split(';') {
        let term_col = col + (raw.len() - raw.trim_start().len());
        col += raw.len() + 1;
        let mut t = raw.trim();
        if t.is_empty() {
            continue;
        }
        let err = |msg: String| ParseError { col: term_col, msg };
        terms += 1;
        let negate = t.starts_with('|');
        if negate {
            t = t[1..].trim_start();
            if t.is_empty() {
                return Err(err("Nothing after '|'".into()));
            }
        }
        let group = if negate { &mut exclude } else { &mut include };

        if let Some(name) = t.strip_prefix(':') {
            let sel = tag_sel(&name.to_ascii_lowercase()).ok_or_else(|| {
                err(format!(
                    "Unknown tag ':{name}'. Use :red, :yellow, :green, :blue or :all"
                ))
            })?;
            group.tags.push(sel);
        } else if t.starts_with('>') || t.starts_with('<') {
            let cmp = if t.starts_with('>') { Cmp::Gt } else { Cmp::Lt };
            let rest = t[1..].trim_start_matches('=').trim().to_ascii_lowercase();
            let split = rest
                .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                .unwrap_or(rest.len());
            let (num, unit) = rest.split_at(split);
            let unit = unit.trim();
            let num: f64 = num
                .parse()
                .map_err(|_| err(format!("Expected a number after '{}'", &t[..1])))?;
            if unit.is_empty() {
                return Err(err(format!(
                    "Add a unit, like {}{num}mb or {}{num}months",
                    &t[..1],
                    &t[..1]
                )));
            }
            if let Some(mult) = size_unit(unit) {
                group.sizes.push((cmp, (num * mult) as u64));
            } else if let Some(mult) = age_unit(unit) {
                group.ages.push((cmp, (num * mult) as i64));
            } else {
                return Err(err(format!("Unknown unit '{unit}'")));
            }
        } else {
            let pat = if t.contains(['*', '?', '[']) {
                t.to_owned()
            } else {
                format!("*{t}*")
            };
            let glob = GlobBuilder::new(&pat)
                .case_insensitive(true)
                .literal_separator(false)
                .build()
                .map_err(|e| err(format!("Bad pattern: {}", e.kind())))?;
            if negate {
                exc_globs.add(glob);
                n_exc_globs += 1;
            } else {
                inc_globs.add(glob);
                n_inc_globs += 1;
            }
        }
    }
    if terms == 0 {
        return Ok(None);
    }
    let build = |b: GlobSetBuilder| {
        b.build().map_err(|e| ParseError {
            col: 1,
            msg: e.to_string(),
        })
    };
    if n_inc_globs > 0 {
        include.globs = Some(build(inc_globs)?);
    }
    if n_exc_globs > 0 {
        exclude.globs = Some(build(exc_globs)?);
    }
    Ok(Some(Filter {
        include,
        exclude,
        terms,
    }))
}

fn cmp_ok(cmp: Cmp, value: i128, limit: i128) -> bool {
    match cmp {
        Cmp::Gt => value > limit,
        Cmp::Lt => value < limit,
    }
}

fn name_hit(globs: &Option<GlobSet>, name: &[u8]) -> bool {
    match globs {
        // Glob matching on raw bytes; invalid UTF-8 is matched lossily.
        Some(g) => g.is_match(String::from_utf8_lossy(name).as_ref()),
        None => false,
    }
}

impl Filter {
    /// State for the contents of a folder named `name` with tag `tag`, inside `parent`.
    pub fn enter_dir(&self, parent: &Inherit, name: &[u8], tag: Option<Tag>) -> Inherit {
        Inherit {
            glob_hit: parent.glob_hit || name_hit(&self.include.globs, name),
            excluded: parent.excluded || name_hit(&self.exclude.globs, name),
            tag: tag.or(parent.tag),
        }
    }

    /// Whether a file passes. `now` and `mtime` are Unix seconds.
    pub fn file_matches(
        &self,
        inh: &Inherit,
        name: &[u8],
        size: u64,
        mtime: i64,
        tag: Option<Tag>,
        now: i64,
    ) -> bool {
        if inh.excluded {
            return false;
        }
        let tag = tag.or(inh.tag);
        let age = (now - mtime).max(0) as i128;
        let size = size as i128;

        let inc = &self.include;
        if inc.globs.is_some() && !(inh.glob_hit || name_hit(&inc.globs, name)) {
            return false;
        }
        if !inc.tags.is_empty() && !inc.tags.iter().any(|s| s.matches(tag)) {
            return false;
        }
        if !inc.sizes.iter().all(|(c, v)| cmp_ok(*c, size, *v as i128)) {
            return false;
        }
        if !inc.ages.iter().all(|(c, v)| cmp_ok(*c, age, *v as i128)) {
            return false;
        }

        let exc = &self.exclude;
        if exc.is_empty() {
            return true;
        }
        let excluded = name_hit(&exc.globs, name)
            || exc.tags.iter().any(|s| s.matches(tag))
            || exc.sizes.iter().any(|(c, v)| cmp_ok(*c, size, *v as i128))
            || exc.ages.iter().any(|(c, v)| cmp_ok(*c, age, *v as i128));
        !excluded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const MB: u64 = 1 << 20;

    fn m(f: &str, name: &str, size: u64, age_days: i64, tag: Option<Tag>) -> bool {
        let f = parse(f).unwrap().unwrap();
        f.file_matches(
            &Inherit::default(),
            name.as_bytes(),
            size,
            NOW - age_days * 86_400,
            tag,
            NOW,
        )
    }

    #[test]
    fn empty_is_none() {
        assert!(parse("").unwrap().is_none());
        assert!(parse(" ; ;").unwrap().is_none());
    }

    #[test]
    fn globs_or_together_and_are_case_insensitive() {
        assert!(m("*.mkv;*.mp4", "a.MP4", 1, 0, None));
        assert!(!m("*.mkv;*.mp4", "a.avi", 1, 0, None));
        assert!(m("cache", "my-Cache-file", 1, 0, None));
    }

    #[test]
    fn sizes_and_ages_form_ranges() {
        assert!(m(">1mb;<10mb", "x", 5 * MB, 0, None));
        assert!(!m(">1mb;<10mb", "x", 20 * MB, 0, None));
        assert!(m(">2years", "x", 1, 800, None));
        assert!(!m(">2years", "x", 1, 700, None));
        assert!(m("<3months", "x", 1, 30, None));
        assert!(m("*.jpg;>1mb;<3months", "a.jpg", 2 * MB, 10, None));
        assert!(!m("*.jpg;>1mb;<3months", "a.png", 2 * MB, 10, None));
    }

    #[test]
    fn excludes_and_tags() {
        assert!(!m("|*.jpg", "a.jpg", 1, 0, None));
        assert!(m("|*.jpg", "a.png", 1, 0, None));
        assert!(m(":red", "a", 1, 0, Some(Tag::Red)));
        assert!(!m(":red", "a", 1, 0, Some(Tag::Blue)));
        assert!(m(":all", "a", 1, 0, Some(Tag::Blue)));
        assert!(!m("|:green", "a", 1, 0, Some(Tag::Green)));
        assert!(m("*.jpg;>1mb;<3months;|:yellow", "a.jpg", 2 * MB, 1, None));
        assert!(!m(
            "*.jpg;>1mb;<3months;|:yellow",
            "a.jpg",
            2 * MB,
            1,
            Some(Tag::Yellow)
        ));
    }

    #[test]
    fn folders_pass_globs_tags_and_exclusions_down() {
        let f = parse("node_modules").unwrap().unwrap();
        let inside = f.enter_dir(&Inherit::default(), b"node_modules", None);
        assert!(f.file_matches(&inside, b"index.js", 1, NOW, None, NOW));
        let f = parse("|node_modules").unwrap().unwrap();
        let inside = f.enter_dir(&Inherit::default(), b"node_modules", None);
        assert!(!f.file_matches(&inside, b"index.js", 1, NOW, None, NOW));
        let f = parse(":red").unwrap().unwrap();
        let inside = f.enter_dir(&Inherit::default(), b"old", Some(Tag::Red));
        assert!(f.file_matches(&inside, b"x", 1, NOW, None, NOW));
    }

    #[test]
    fn unicode_long_and_messy_input() {
        assert!(m("🎵", "Música 🎵.flac", 1, 0, None));
        assert!(m("*canción*", "Canción.mp3", 1, 0, None));
        let long: String = (0..300).map(|i| format!("*.e{i};")).collect();
        let f = parse(&long).unwrap().unwrap();
        assert_eq!(f.terms, 300);
        assert!(m("  *.iso ;  >1mb ; ", "a.iso", 2 * MB, 0, None));
        assert!(m(">=1mb", "x", 2 * MB, 0, None));
        assert!(parse(">").is_err());
        assert!(parse(">abc").is_err());
    }

    #[test]
    fn errors_point_at_the_term() {
        let e = parse("*.iso;>1gbb").unwrap_err();
        assert_eq!(e.col, 7);
        assert!(e.msg.contains("gbb"));
        assert!(parse(">5").unwrap_err().msg.contains("unit"));
        assert!(parse(":purple").unwrap_err().msg.contains(":purple"));
        assert!(parse("|").is_err());
    }
}
