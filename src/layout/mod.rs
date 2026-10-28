//! Treemap layout. Pure geometry: no egui, no filesystem.

/// Axis-aligned rectangle in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }

    pub fn area(&self) -> f32 {
        self.w * self.h
    }

    pub fn min_side(&self) -> f32 {
        self.w.min(self.h)
    }

    /// Shrink by `left, top, right, bottom`. Never returns negative sizes.
    pub fn inset(&self, left: f32, top: f32, right: f32, bottom: f32) -> Rect {
        Rect {
            x: self.x + left,
            y: self.y + top,
            w: (self.w - left - right).max(0.0),
            h: (self.h - top - bottom).max(0.0),
        }
    }
}

/// Squarified treemap layout (Bruls, Huizing, van Wijk 2000).
///
/// `values` must be sorted in descending order. Zero or negative values get `Rect::ZERO`.
/// Returns one rect per value, in the same order, tiling `bounds` exactly.
pub fn squarify(values: &[f64], bounds: Rect) -> Vec<Rect> {
    let mut out = vec![Rect::ZERO; values.len()];
    let n = values.iter().take_while(|v| **v > 0.0).count();
    let total: f64 = values[..n].iter().sum();
    if n == 0 || total <= 0.0 || bounds.w <= 0.0 || bounds.h <= 0.0 {
        return out;
    }

    let scale = bounds.w as f64 * bounds.h as f64 / total;
    let areas: Vec<f64> = values[..n].iter().map(|v| v * scale).collect();

    // Free space as f64 to keep rounding error from accumulating across rows.
    let (mut fx, mut fy, mut fw, mut fh) = (
        bounds.x as f64,
        bounds.y as f64,
        bounds.w as f64,
        bounds.h as f64,
    );
    let mut i = 0;
    while i < n {
        let side = fw.min(fh);
        let mut j = i + 1;
        let mut sum = areas[i];
        let mut best = worst_ratio(areas[i], areas[i], sum, side);
        while j < n {
            let next_sum = sum + areas[j];
            let r = worst_ratio(areas[i], areas[j], next_sum, side);
            if r > best {
                break;
            }
            best = r;
            sum = next_sum;
            j += 1;
        }

        let last_row = j == n;
        if fw >= fh {
            // Wide free space: lay the row out as a column on the left.
            let col_w = if last_row { fw } else { sum / fh };
            let mut y = fy;
            for k in i..j {
                let h = if k + 1 == j {
                    fy + fh - y
                } else {
                    areas[k] / col_w
                };
                out[k] = Rect::new(fx as f32, y as f32, col_w as f32, h as f32);
                y += h;
            }
            fx += col_w;
            fw -= col_w;
        } else {
            // Tall free space: lay the row out along the top.
            let row_h = if last_row { fh } else { sum / fw };
            let mut x = fx;
            for k in i..j {
                let w = if k + 1 == j {
                    fx + fw - x
                } else {
                    areas[k] / row_h
                };
                out[k] = Rect::new(x as f32, fy as f32, w as f32, row_h as f32);
                x += w;
            }
            fy += row_h;
            fh -= row_h;
        }
        i = j;
    }
    out
}

/// Worst aspect ratio in a row, given its largest and smallest area, the row sum and the side
/// length it is laid along.
fn worst_ratio(max: f64, min: f64, sum: f64, side: f64) -> f64 {
    let s2 = side * side;
    let sum2 = sum * sum;
    (s2 * max / sum2).max(sum2 / (s2 * min))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlap(a: &Rect, b: &Rect) -> f32 {
        let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
        let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
        w.max(0.0) * h.max(0.0)
    }

    #[test]
    fn areas_are_proportional_and_tile_the_bounds() {
        let values = [600.0, 300.0, 200.0, 100.0, 50.0, 25.0, 5.0];
        let b = Rect::new(10.0, 20.0, 640.0, 400.0);
        let rects = squarify(&values, b);
        let total: f64 = values.iter().sum();
        let mut covered = 0.0;
        for (v, r) in values.iter().zip(&rects) {
            let want = (*v / total) as f32 * b.area();
            assert!(
                (r.area() - want).abs() / want < 1e-3,
                "{r:?} area {} want {want}",
                r.area()
            );
            assert!(r.x >= b.x - 1e-3 && r.y >= b.y - 1e-3);
            assert!(r.x + r.w <= b.x + b.w + 1e-3 && r.y + r.h <= b.y + b.h + 1e-3);
            covered += r.area();
        }
        assert!((covered - b.area()).abs() < 1.0);
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                assert!(overlap(&rects[i], &rects[j]) < 1e-2, "{i} and {j} overlap");
            }
        }
    }

    #[test]
    fn aspect_ratios_stay_reasonable() {
        let values: Vec<f64> = (1..=20).rev().map(|v| v as f64).collect();
        for r in squarify(&values, Rect::new(0.0, 0.0, 800.0, 500.0)) {
            let ratio = r.w.max(r.h) / r.w.min(r.h);
            assert!(ratio < 4.0, "{r:?} ratio {ratio}");
        }
    }

    #[test]
    fn degenerate_input() {
        assert!(squarify(&[], Rect::new(0.0, 0.0, 10.0, 10.0)).is_empty());
        assert_eq!(
            squarify(&[5.0, 0.0], Rect::new(0.0, 0.0, 10.0, 10.0))[1],
            Rect::ZERO
        );
        assert_eq!(
            squarify(&[5.0], Rect::new(0.0, 0.0, 0.0, 10.0))[0],
            Rect::ZERO
        );
        assert_eq!(
            squarify(&[5.0], Rect::new(1.0, 2.0, 3.0, 4.0))[0],
            Rect::new(1.0, 2.0, 3.0, 4.0)
        );
    }
}
