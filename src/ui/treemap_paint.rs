//! Painting single treemap blocks. Layout and hit-testing live elsewhere; this only knows
//! how one block of a given kind and state looks (components.md → Treemap canvas).

use std::sync::Arc;

use egui::{Color32, FontId, Galley, Painter, Rect, Shape, Stroke, StrokeKind, pos2, vec2};
use egui_phosphor::regular as icon;

use super::fonts;
use super::theme::{Palette, layout::tm as L, tm, ty};

pub use crate::model::Tag;

impl Tag {
    pub fn color(self) -> Color32 {
        match self {
            Tag::Red => tm::TAG_RED,
            Tag::Yellow => tm::TAG_YELLOW,
            Tag::Green => tm::TAG_GREEN,
            Tag::Blue => tm::TAG_BLUE,
        }
    }

    /// Default meaning; renameable in settings. Colour is never shown without this label.
    pub fn label(self) -> &'static str {
        match self {
            Tag::Red => "Remove",
            Tag::Yellow => "Check",
            Tag::Green => "Keep",
            Tag::Blue => "Someone else's",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Image,
    Video,
    Audio,
    Archive,
    Document,
    Code,
    Binary,
    Other,
}

impl Category {
    pub fn color(self) -> Color32 {
        match self {
            Category::Image => tm::FILE_IMAGE,
            Category::Video => tm::FILE_VIDEO,
            Category::Audio => tm::FILE_AUDIO,
            Category::Archive => tm::FILE_ARCHIVE,
            Category::Document => tm::FILE_DOCUMENT,
            Category::Code => tm::FILE_CODE,
            Category::Binary => tm::FILE_BINARY,
            Category::Other => tm::FILE_OTHER,
        }
    }

    /// Built-in extension table. Settings → type colours overrides it.
    pub fn from_name(name: &str) -> Category {
        let lower = name.to_ascii_lowercase();
        // Versioned shared libraries: libfoo.so.1.2.3
        if lower.contains(".so.") {
            return Category::Binary;
        }
        let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        match ext {
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "heic" | "svg" | "bmp" | "tif"
            | "tiff" | "raw" | "cr2" | "nef" | "dng" | "psd" | "xcf" => Category::Image,
            "mkv" | "mp4" | "webm" | "avi" | "mov" | "m4v" | "wmv" | "flv" | "ts" | "m2ts" => {
                Category::Video
            }
            "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" | "wv" | "ape" | "mid" => {
                Category::Audio
            }
            "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "xz" | "zst" | "bz2" | "lz4" | "deb"
            | "rpm" | "pkg" | "whl" | "jar" => Category::Archive,
            "pdf" | "epub" | "doc" | "docx" | "odt" | "xls" | "xlsx" | "ods" | "ppt" | "pptx"
            | "odp" | "txt" | "md" | "rtf" | "csv" => Category::Document,
            "rs" | "c" | "h" | "cpp" | "hpp" | "py" | "js" | "ts_" | "go" | "java" | "kt"
            | "rb" | "sh" | "lua" | "json" | "toml" | "yaml" | "yml" | "html" | "css" | "sql" => {
                Category::Code
            }
            "so" | "o" | "a" | "rlib" | "rmeta" | "bin" | "exe" | "dll" | "iso" | "img"
            | "qcow2" | "vdi" | "vmdk" | "appimage" | "pyc" | "class" | "wasm" => Category::Binary,
            _ => Category::Other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlockKind {
    Dir { depth: usize },
    File(Category),
    SmallAgg,
    Denied,
    MountPoint { depth: usize },
    Free,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BlockState {
    pub hovered: bool,
    pub selected: bool,
    pub tag: Option<Tag>,
    /// 1.0 right after a change, fading to 0.0 over `motion::BLINK`.
    pub blink: f32,
    /// Directory still being scanned.
    pub pending: bool,
}

pub fn dir_fill(p: &Palette, depth: usize) -> Color32 {
    [p.tm_dir_0, p.tm_dir_1, p.tm_dir_2, p.tm_dir_3][depth % 4]
}

/// Where a directory's children go: below the header, inside the padding.
/// Returns `None` when the block is too small to show children.
pub fn dir_body(rect: Rect) -> Option<Rect> {
    let body = Rect::from_min_max(
        pos2(rect.left() + L::DIR_PAD, rect.top() + L::DIR_HEADER_H),
        pos2(rect.right() - L::DIR_PAD, rect.bottom() - L::DIR_PAD),
    );
    (body.width() >= L::MIN_BLOCK_PX && body.height() >= L::MIN_BLOCK_PX).then_some(body)
}

/// Lay out `text`, shortening it with "…" until it fits `max_w`. `None` if not even "…" fits.
pub fn elide(
    painter: &Painter,
    text: &str,
    font: &FontId,
    color: Color32,
    max_w: f32,
) -> Option<Arc<Galley>> {
    let g = painter.layout_no_wrap(text.to_owned(), font.clone(), color);
    if g.size().x <= max_w {
        return Some(g);
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    let mut best = None;
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let s: String = chars[..mid].iter().collect::<String>() + "…";
        let g = painter.layout_no_wrap(s, font.clone(), color);
        if g.size().x <= max_w {
            best = Some(g);
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    best.or_else(|| {
        let g = painter.layout_no_wrap("…".into(), font.clone(), color);
        (g.size().x <= max_w).then_some(g)
    })
}

/// Paint one block: fill, label, then state overlays (tag, blink, hover, selection).
pub fn paint_block(
    painter: &Painter,
    p: &Palette,
    rect: Rect,
    kind: BlockKind,
    name: &str,
    size: &str,
    st: &BlockState,
) {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));

    let fill = match kind {
        BlockKind::Dir { depth } | BlockKind::MountPoint { depth } => dir_fill(p, depth),
        BlockKind::File(c) => c.color(),
        BlockKind::SmallAgg => p.tm_agg,
        BlockKind::Denied => p.tm_denied,
        BlockKind::Free => p.tm_free,
    };
    painter.rect_filled(rect, 0, fill);

    match kind {
        BlockKind::Denied => {
            let stroke = Stroke::new(2.0, p.tm_denied_stripe);
            let mut off = -rect.height();
            while off < rect.width() {
                let a = pos2(rect.left() + off, rect.bottom());
                painter.line_segment([a, a + vec2(rect.height(), -rect.height())], stroke);
                off += 6.0;
            }
        }
        BlockKind::MountPoint { .. } => {
            if let Some(body) = dir_body(rect) {
                painter.rect_filled(body, 0, p.tm_canvas);
                // Say how to go inside; hidden when there's no room for it.
                let font = fonts::font(ty::TM_FILE);
                if let Some(g) = elide(
                    &painter,
                    "Double-click or Enter to scan it",
                    &font,
                    p.text_muted,
                    body.width() - 8.0,
                ) && body.height() > g.size().y + 8.0
                    && g.size().x > 40.0
                {
                    painter.galley(body.center() - g.size() / 2.0, g, p.text_muted);
                }
            }
        }
        _ => {}
    }

    // Under the labels, so text keeps full contrast while the block flashes.
    if st.blink > 0.0 {
        // Always lighten: every fill carries dark label ink (or light ink on dark dir fills,
        // which stays readable because the overlay is capped at 35%).
        painter.rect_filled(
            rect,
            0,
            Color32::WHITE.gamma_multiply(0.35 * st.blink.clamp(0.0, 1.0)),
        );
    }

    paint_labels(&painter, p, rect, kind, name, size, st);

    // Overlays, in order: tag, hover, selection.
    if let Some(tag) = st.tag {
        let m = L::TAG_MARK;
        let tl = rect.left_top();
        if rect.size().min_elem() >= 24.0 {
            painter.rect_stroke(rect, 0, Stroke::new(2.0, tag.color()), StrokeKind::Inside);
        }
        if rect.size().min_elem() >= m {
            painter.add(Shape::convex_polygon(
                vec![tl, tl + vec2(m + 2.0, 0.0), tl + vec2(0.0, m + 2.0)],
                tag.color(),
                Stroke::new(1.0, tm::LABEL_INK),
            ));
        }
    }
    if st.hovered && !st.selected {
        // Accent outline plus a thin contrast line inside it, so the hover shows on any fill
        // (the accent is a green, like some file colours).
        painter.rect_stroke(
            rect,
            0,
            Stroke::new(L::HOVER_W, p.tm_hover),
            StrokeKind::Inside,
        );
        painter.rect_stroke(
            rect.shrink(L::HOVER_W),
            0,
            Stroke::new(1.0, p.tm_select_halo),
            StrokeKind::Inside,
        );
    }
    if st.selected {
        painter.rect_stroke(
            rect,
            0,
            Stroke::new(L::SELECT_W, p.tm_select),
            StrokeKind::Inside,
        );
        painter.rect_stroke(
            rect.shrink(L::SELECT_W),
            0,
            Stroke::new(1.0, p.tm_select_halo),
            StrokeKind::Inside,
        );
    }
}

fn paint_labels(
    painter: &Painter,
    p: &Palette,
    rect: Rect,
    kind: BlockKind,
    name: &str,
    size: &str,
    st: &BlockState,
) {
    if rect.width() < L::LABEL_MIN_W || rect.height() < L::LABEL_MIN_H {
        return;
    }
    let pad = 3.0;
    match kind {
        BlockKind::Dir { .. } | BlockKind::MountPoint { .. } => {
            let font = fonts::font(ty::TM_DIR);
            let color = p.tm_dir_label;
            let head = Rect::from_min_size(rect.min, vec2(rect.width(), L::DIR_HEADER_H));
            let mut avail = head.width() - 2.0 * pad;

            let size_g = painter.layout_no_wrap(size.to_owned(), fonts::font(ty::TM_FILE), color);
            let show_size = size_g.size().x + 40.0 < avail;
            if show_size {
                avail -= size_g.size().x + 6.0;
            }
            let prefix = match kind {
                BlockKind::MountPoint { .. } => format!("{} ", icon::HARD_DRIVES),
                _ => String::new(),
            };
            let suffix = if st.pending {
                format!(" {}", icon::CIRCLE_NOTCH)
            } else {
                String::new()
            };
            let text = format!("{prefix}{name}{suffix}");
            if let Some(g) = elide(painter, &text, &font, color, avail) {
                let y = head.center().y - g.size().y / 2.0;
                painter.galley(pos2(head.left() + pad, y), g, color);
            }
            if show_size {
                let y = head.center().y - size_g.size().y / 2.0;
                painter.galley(pos2(head.right() - pad - size_g.size().x, y), size_g, color);
            }
            if st.hovered {
                let y = head.bottom() - 2.0;
                painter.line_segment(
                    [pos2(head.left() + pad, y), pos2(head.right() - pad, y)],
                    Stroke::new(1.0, color.gamma_multiply(0.5)),
                );
            }
        }
        _ => {
            let (color, line1) = match kind {
                BlockKind::File(_) => (tm::LABEL_INK, name.to_owned()),
                BlockKind::SmallAgg => (p.tm_agg_label, name.to_owned()),
                BlockKind::Denied => (p.tm_denied_label, format!("{} {}", icon::LOCK, name)),
                BlockKind::Free => (p.tm_free_label, name.to_owned()),
                _ => unreachable!(),
            };
            let font = fonts::font(ty::TM_FILE);
            let avail = rect.width() - 2.0 * pad;
            let mut y = rect.top() + pad;
            if let Some(g) = elide(painter, &line1, &font, color, avail) {
                let h = g.size().y;
                painter.galley(pos2(rect.left() + pad, y), g, color);
                y += h;
            }
            if !size.is_empty()
                && y + ty::TM_FILE.line <= rect.bottom() - pad
                && let Some(g) = elide(painter, size, &font, color, avail)
            {
                painter.galley(pos2(rect.left() + pad, y), g, color);
            }
        }
    }
}
