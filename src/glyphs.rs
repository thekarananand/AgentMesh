//! Block elements, box drawing and Powerline separators, drawn as shapes instead of font
//! glyphs. Glyphs sit at fractional cell positions and their antialiased edges leave seams
//! between neighboring cells; shapes snapped to device pixels meet exactly.

use gpui::{Bounds, Hsla, PathBuilder, Pixels, Point, Window, fill, point, px};

/// One piece of a drawn character, in window coordinates.
pub enum Shape {
    /// Filled rectangle and its opacity.
    Rect(Bounds<Pixels>, f32),
    /// Closed, filled outline.
    Fill(Pen),
    /// Open outline stroked at a width.
    Stroke(Pen, Pixels),
}

pub struct Pen {
    start: Point<Pixels>,
    steps: Vec<Step>,
}

enum Step {
    Line(Point<Pixels>),
    Curve {
        to: Point<Pixels>,
        ctrl: Point<Pixels>,
    },
}

/// Whether `c` is drawn here instead of by the font.
pub fn covers(c: char) -> bool {
    matches!(c, '\u{2500}'..='\u{259F}' | '\u{E0B0}'..='\u{E0BF}')
}

/// Shapes for `c` filling `cell` and stretched right across `span` cells (a run of `─` or `█`
/// drawn as one shape), or `None` when the font should draw it. Line thickness always comes
/// from one cell: a run is wider, its lines are not thicker.
pub fn shapes(c: char, cell: Bounds<Pixels>, span: usize, scale: f32) -> Option<Vec<Shape>> {
    let cell = Cell::new(cell, span, scale);
    match c {
        '\u{2500}'..='\u{257F}' => cell.box_drawing(c),
        '\u{2580}'..='\u{259F}' => cell.block(c),
        '\u{E0B0}'..='\u{E0BF}' => Some(cell.powerline(c)),
        _ => None,
    }
}

pub fn paint(shape: Shape, color: Hsla, window: &mut Window) {
    match shape {
        Shape::Rect(bounds, alpha) => window.paint_quad(fill(bounds, color.opacity(alpha))),
        Shape::Fill(pen) => {
            if let Ok(path) = pen.build(PathBuilder::fill(), true) {
                window.paint_path(path, color);
            }
        }
        Shape::Stroke(pen, width) => {
            if let Ok(path) = pen.build(PathBuilder::stroke(width), false) {
                window.paint_path(path, color);
            }
        }
    }
}

impl Pen {
    fn new(start: Point<Pixels>) -> Self {
        Self {
            start,
            steps: Vec::new(),
        }
    }

    fn line(mut self, to: Point<Pixels>) -> Self {
        self.steps.push(Step::Line(to));
        self
    }

    fn curve(mut self, to: Point<Pixels>, ctrl: Point<Pixels>) -> Self {
        self.steps.push(Step::Curve { to, ctrl });
        self
    }

    fn build(
        self,
        mut builder: PathBuilder,
        close: bool,
    ) -> Result<gpui::Path<Pixels>, impl std::fmt::Debug> {
        builder.move_to(self.start);
        for step in self.steps {
            match step {
                Step::Line(to) => builder.line_to(to),
                Step::Curve { to, ctrl } => builder.curve_to(to, ctrl),
            }
        }
        if close {
            builder.close();
        }
        builder.build()
    }
}

/// Arm weights, in order up, right, down, left.
type Arms = [Weight; 4];

/// A line's weight, as Unicode names it. Ordered: the heavier arm of a pair sets the band.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Weight {
    None,
    Light,
    Heavy,
    Double,
}

// Short names for the arm logic (`Weight::None` reads badly next to `Option::None`).
const NONE: Weight = Weight::None;
const LIGHT: Weight = Weight::Light;
const HEAVY: Weight = Weight::Heavy;
const DOUBLE: Weight = Weight::Double;

/// A cell with edges snapped to device pixels, plus the line thickness.
struct Cell {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    /// Unsnapped center, so every cell of a row/column centers its lines the same way.
    cx: f32,
    cy: f32,
    /// Light line thickness: whole device pixels, at least one.
    thin: f32,
    scale: f32,
}

impl Cell {
    fn new(cell: Bounds<Pixels>, span: usize, scale: f32) -> Self {
        let snap = |v: Pixels| (f32::from(v) * scale).round() / scale;
        let width = f32::from(cell.size.width);
        let cell = Bounds::new(
            cell.origin,
            gpui::size(cell.size.width * span.max(1) as f32, cell.size.height),
        );
        Self {
            left: snap(cell.left()),
            top: snap(cell.top()),
            right: snap(cell.right()),
            bottom: snap(cell.bottom()),
            cx: f32::from(cell.center().x),
            cy: f32::from(cell.center().y),
            thin: (width * scale / 8.).round().max(1.) / scale,
            scale,
        }
    }

    fn snap(&self, v: f32) -> f32 {
        (v * self.scale).round() / self.scale
    }

    fn point(&self, x: f32, y: f32) -> Point<Pixels> {
        point(px(self.snap(x)), px(self.snap(y)))
    }

    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Shape {
        Shape::Rect(
            Bounds::from_corners(self.point(x0, y0), self.point(x1, y1)),
            1.,
        )
    }

    /// Cross-axis bands (lo, hi) of a line of `weight` centered on `center`, ascending:
    /// one for light/heavy, two for double.
    fn bands(&self, center: f32, weight: Weight) -> Vec<(f32, f32)> {
        let t = self.thin;
        let lo = self.snap(center - t / 2.);
        match weight {
            Weight::None => vec![],
            Weight::Light => vec![(lo, lo + t)],
            Weight::Heavy => {
                let lo = self.snap(center - t);
                vec![(lo, lo + 2. * t)]
            }
            Weight::Double => vec![(lo - t, lo), (lo + t, lo + 2. * t)],
        }
    }

    fn box_drawing(&self, c: char) -> Option<Vec<Shape>> {
        Some(match c {
            '┄' => self.dashes(3, LIGHT, true),
            '┅' => self.dashes(3, HEAVY, true),
            '┆' => self.dashes(3, LIGHT, false),
            '┇' => self.dashes(3, HEAVY, false),
            '┈' => self.dashes(4, LIGHT, true),
            '┉' => self.dashes(4, HEAVY, true),
            '┊' => self.dashes(4, LIGHT, false),
            '┋' => self.dashes(4, HEAVY, false),
            '╌' => self.dashes(2, LIGHT, true),
            '╍' => self.dashes(2, HEAVY, true),
            '╎' => self.dashes(2, LIGHT, false),
            '╏' => self.dashes(2, HEAVY, false),
            '╭' | '╮' | '╯' | '╰' => vec![self.rounded(c)],
            '╱' => vec![self.diagonal(true)],
            '╲' => vec![self.diagonal(false)],
            '╳' => vec![self.diagonal(true), self.diagonal(false)],
            _ => self.lines(arms(c)?),
        })
    }

    /// Straight arms from the center to the edges, joined like the font draws them.
    fn lines(&self, arms: Arms) -> Vec<Shape> {
        let [up, right, down, left] = arms;
        // Bands of the vertical arms across x, and of the horizontal arms across y.
        let x_bands = self.bands(self.cx, up.max(down));
        let y_bands = self.bands(self.cy, left.max(right));
        let mut out = Vec::new();
        for (weight, low, opposite) in [(left, true, right), (right, false, left)] {
            let lines = self.bands(self.cy, weight);
            for (index, &(y0, y1)) in lines.iter().enumerate() {
                let end = self.arm_end(weight, index, opposite, [up, down], low, self.cx, &x_bands);
                let (x0, x1) = if low {
                    (self.left, end)
                } else {
                    (end, self.right)
                };
                out.push(self.rect(x0, y0, x1, y1));
            }
        }
        for (weight, low, opposite) in [(up, true, down), (down, false, up)] {
            let lines = self.bands(self.cx, weight);
            for (index, &(x0, x1)) in lines.iter().enumerate() {
                let end = self.arm_end(
                    weight,
                    index,
                    opposite,
                    [left, right],
                    low,
                    self.cy,
                    &y_bands,
                );
                let (y0, y1) = if low {
                    (self.top, end)
                } else {
                    (end, self.bottom)
                };
                out.push(self.rect(x0, y0, x1, y1));
            }
        }
        out
    }

    /// Where line `index` of an arm stops near the center, along the arm's axis.
    /// `low`: the arm comes from the left/top edge. `sides`: the perpendicular arms on the
    /// low and high cross side; `perp`: their bands along this axis.
    #[allow(clippy::too_many_arguments)]
    fn arm_end(
        &self,
        weight: Weight,
        index: usize,
        opposite: Weight,
        sides: [Weight; 2],
        low: bool,
        center: f32,
        perp: &[(f32, f32)],
    ) -> f32 {
        let (Some(&first), Some(&last)) = (perp.first(), perp.last()) else {
            return center;
        };
        // Overlap the nearest perpendicular line, or reach past the farthest one.
        let (near, far) = if low {
            (first.1, last.1)
        } else {
            (last.0, first.0)
        };
        if weight == DOUBLE {
            // Each line of a double arm sits on one cross side; that side's arm decides.
            let (side, other) = if index == 0 {
                (sides[0], sides[1])
            } else {
                (sides[1], sides[0])
            };
            if side == DOUBLE {
                near
            } else if opposite != NONE {
                center
            } else if side != NONE || other != NONE {
                far
            } else {
                center
            }
        } else if opposite != NONE {
            center
        } else if sides[0] == DOUBLE && sides[1] == DOUBLE {
            near
        } else {
            far
        }
    }

    /// `count` dashes across the cell, horizontal or vertical.
    fn dashes(&self, count: usize, weight: Weight, horizontal: bool) -> Vec<Shape> {
        let (start, end) = if horizontal {
            (self.left, self.right)
        } else {
            (self.top, self.bottom)
        };
        let segment = (end - start) / count as f32;
        let center = if horizontal { self.cy } else { self.cx };
        let (lo, hi) = self.bands(center, weight)[0];
        (0..count)
            .map(|i| {
                let a = start + segment * (i as f32 + 0.2);
                let b = start + segment * (i as f32 + 0.8);
                if horizontal {
                    self.rect(a, lo, b, hi)
                } else {
                    self.rect(lo, a, hi, b)
                }
            })
            .collect()
    }

    /// Light arc corner: ╭ ╮ ╯ ╰.
    fn rounded(&self, c: char) -> Shape {
        let (x_lo, _) = self.bands(self.cx, LIGHT)[0];
        let (y_lo, _) = self.bands(self.cy, LIGHT)[0];
        let (cx, cy) = (x_lo + self.thin / 2., y_lo + self.thin / 2.);
        let radius = ((self.right - self.left) / 2.).min((self.bottom - self.top) / 2.);
        // Vertical end (edge it reaches) and horizontal end.
        let (y_edge, y_dir) = match c {
            '╭' | '╮' => (self.bottom, 1.),
            _ => (self.top, -1.),
        };
        let (x_edge, x_dir) = match c {
            '╭' | '╰' => (self.right, 1.),
            _ => (self.left, -1.),
        };
        let pt = |x: f32, y: f32| point(px(x), px(y));
        Shape::Stroke(
            Pen::new(pt(cx, y_edge))
                .line(pt(cx, cy + y_dir * radius))
                .curve(pt(cx + x_dir * radius, cy), pt(cx, cy))
                .line(pt(x_edge, cy)),
            px(self.thin),
        )
    }

    /// ╱ when `rising`, else ╲.
    fn diagonal(&self, rising: bool) -> Shape {
        let (y_start, y_end) = if rising {
            (self.bottom, self.top)
        } else {
            (self.top, self.bottom)
        };
        Shape::Stroke(
            Pen::new(self.point(self.left, y_start)).line(self.point(self.right, y_end)),
            px(self.thin),
        )
    }

    /// Block elements, U+2580..U+259F.
    fn block(&self, c: char) -> Option<Vec<Shape>> {
        // (x0, y0, x1, y1) in eighths of the cell.
        const FULL: &[(f32, f32, f32, f32)] = &[(0., 0., 8., 8.)];
        const UL: (f32, f32, f32, f32) = (0., 0., 4., 4.);
        const UR: (f32, f32, f32, f32) = (4., 0., 8., 4.);
        const LL: (f32, f32, f32, f32) = (0., 4., 4., 8.);
        const LR: (f32, f32, f32, f32) = (4., 4., 8., 8.);
        let (rects, alpha): (&[(f32, f32, f32, f32)], f32) = match c {
            '▀' => (&[(0., 0., 8., 4.)], 1.),
            '▁' => (&[(0., 7., 8., 8.)], 1.),
            '▂' => (&[(0., 6., 8., 8.)], 1.),
            '▃' => (&[(0., 5., 8., 8.)], 1.),
            '▄' => (&[(0., 4., 8., 8.)], 1.),
            '▅' => (&[(0., 3., 8., 8.)], 1.),
            '▆' => (&[(0., 2., 8., 8.)], 1.),
            '▇' => (&[(0., 1., 8., 8.)], 1.),
            '█' => (FULL, 1.),
            '▉' => (&[(0., 0., 7., 8.)], 1.),
            '▊' => (&[(0., 0., 6., 8.)], 1.),
            '▋' => (&[(0., 0., 5., 8.)], 1.),
            '▌' => (&[(0., 0., 4., 8.)], 1.),
            '▍' => (&[(0., 0., 3., 8.)], 1.),
            '▎' => (&[(0., 0., 2., 8.)], 1.),
            '▏' => (&[(0., 0., 1., 8.)], 1.),
            '▐' => (&[(4., 0., 8., 8.)], 1.),
            '░' => (FULL, 0.25),
            '▒' => (FULL, 0.5),
            '▓' => (FULL, 0.75),
            '▔' => (&[(0., 0., 8., 1.)], 1.),
            '▕' => (&[(7., 0., 8., 8.)], 1.),
            '▖' => (&[LL], 1.),
            '▗' => (&[LR], 1.),
            '▘' => (&[UL], 1.),
            '▙' => (&[UL, LL, LR], 1.),
            '▚' => (&[UL, LR], 1.),
            '▛' => (&[UL, UR, LL], 1.),
            '▜' => (&[UL, UR, LR], 1.),
            '▝' => (&[UR], 1.),
            '▞' => (&[UR, LL], 1.),
            '▟' => (&[UR, LL, LR], 1.),
            _ => return None,
        };
        let (w, h) = (self.right - self.left, self.bottom - self.top);
        Some(
            rects
                .iter()
                .map(|&(x0, y0, x1, y1)| {
                    let shape = self.rect(
                        self.left + w * x0 / 8.,
                        self.top + h * y0 / 8.,
                        self.left + w * x1 / 8.,
                        self.top + h * y1 / 8.,
                    );
                    match shape {
                        Shape::Rect(bounds, _) => Shape::Rect(bounds, alpha),
                        other => other,
                    }
                })
                .collect(),
        )
    }

    /// Powerline separators, U+E0B0..U+E0BF (Nerd Font code points).
    fn powerline(&self, c: char) -> Vec<Shape> {
        let (l, t, r, b) = (self.left, self.top, self.right, self.bottom);
        let cy = (t + b) / 2.;
        let pt = |x: f32, y: f32| point(px(x), px(y));
        let stroke = |pen: Pen| Shape::Stroke(pen, px(self.thin));
        // Triangle pointing right (solid) or its outline.
        let right_arrow = || Pen::new(pt(l, t)).line(pt(r, cy)).line(pt(l, b));
        let left_arrow = || Pen::new(pt(r, t)).line(pt(l, cy)).line(pt(r, b));
        let right_round = || {
            Pen::new(pt(l, t))
                .curve(pt(r, cy), pt(r, t))
                .curve(pt(l, b), pt(r, b))
        };
        let left_round = || {
            Pen::new(pt(r, t))
                .curve(pt(l, cy), pt(l, t))
                .curve(pt(r, b), pt(l, b))
        };
        let triangle = |a: (f32, f32), m: (f32, f32), z: (f32, f32)| {
            Shape::Fill(Pen::new(pt(a.0, a.1)).line(pt(m.0, m.1)).line(pt(z.0, z.1)))
        };
        let line = |a: (f32, f32), z: (f32, f32)| stroke(Pen::new(pt(a.0, a.1)).line(pt(z.0, z.1)));
        vec![match c {
            '\u{E0B0}' => Shape::Fill(right_arrow()),
            '\u{E0B1}' => stroke(right_arrow()),
            '\u{E0B2}' => Shape::Fill(left_arrow()),
            '\u{E0B3}' => stroke(left_arrow()),
            '\u{E0B4}' => Shape::Fill(right_round()),
            '\u{E0B5}' => stroke(right_round()),
            '\u{E0B6}' => Shape::Fill(left_round()),
            '\u{E0B7}' => stroke(left_round()),
            '\u{E0B8}' => triangle((l, t), (l, b), (r, b)),
            '\u{E0B9}' => line((l, t), (r, b)),
            '\u{E0BA}' => triangle((r, t), (r, b), (l, b)),
            '\u{E0BB}' => line((r, t), (l, b)),
            '\u{E0BC}' => triangle((l, t), (r, t), (l, b)),
            '\u{E0BD}' => line((r, t), (l, b)),
            '\u{E0BE}' => triangle((l, t), (r, t), (r, b)),
            _ => line((l, t), (r, b)),
        }]
    }
}

/// Arms of the straight box-drawing chars, from Unicode's names.
fn arms(c: char) -> Option<Arms> {
    // "URDL" weights for U+2500..U+257F; '-' marks chars handled elsewhere.
    const TABLE: [&str; 128] = [
        "0101", "0202", "1010", "2020", "-", "-", "-", "-", "-", "-", "-", "-", // 2500
        "0110", "0210", "0120", "0220", "0011", "0012", "0021", "0022", // 250C
        "1100", "1200", "2100", "2200", "1001", "1002", "2001", "2002", // 2514
        "1110", "1210", "2110", "1120", "2120", "2210", "1220", "2220", // 251C
        "1011", "1012", "2011", "1021", "2021", "2012", "1022", "2022", // 2524
        "0111", "0112", "0211", "0212", "0121", "0122", "0221", "0222", // 252C
        "1101", "1102", "1201", "1202", "2101", "2102", "2201", "2202", // 2534
        "1111", "1112", "1211", "1212", "2111", "1121", "2121", "2112", // 253C
        "2211", "1122", "1221", "2212", "1222", "2122", "2221", "2222", // 2544
        "-", "-", "-", "-", // 254C
        "0303", "3030", "0310", "0130", "0330", "0013", "0031", "0033", // 2550
        "1300", "3100", "3300", "1003", "3001", "3003", "1310", "3130", // 2558
        "3330", "1013", "3031", "3033", "0313", "0131", "0333", "1303", // 2560
        "3101", "3303", "1313", "3131", "3333", // 2568
        "-", "-", "-", "-", "-", "-", "-", // 256D
        "0001", "1000", "0100", "0010", "0002", "2000", "0200", "0020", // 2574
        "0201", "1020", "0102", "2010", // 257C
    ];
    let entry = TABLE.get((c as usize).checked_sub(0x2500)?)?;
    let mut arms = [NONE; 4];
    for (arm, digit) in arms.iter_mut().zip(entry.bytes()) {
        *arm = match digit {
            b'0' => NONE,
            b'1' => LIGHT,
            b'2' => HEAVY,
            b'3' => DOUBLE,
            _ => return None,
        };
    }
    (entry.len() == 4).then_some(arms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn cell() -> Bounds<Pixels> {
        Bounds::new(point(px(10.3), px(18.)), size(px(8.4), px(18.)))
    }

    #[test]
    fn every_covered_char_has_shapes() {
        let chars = ('\u{2500}'..='\u{259F}').chain('\u{E0B0}'..='\u{E0BF}');
        for c in chars {
            assert!(covers(c));
            let shapes = shapes(c, cell(), 1, 2.).unwrap_or_else(|| panic!("{c:?} not drawn"));
            assert!(!shapes.is_empty(), "{c:?} has no shapes");
        }
        assert!(!covers('a'));
        assert!(shapes('a', cell(), 1, 2.).is_none());
    }

    #[test]
    fn arms_follow_unicode_names() {
        assert_eq!(arms('─'), Some([NONE, LIGHT, NONE, LIGHT]));
        assert_eq!(arms('┏'), Some([NONE, HEAVY, HEAVY, NONE]));
        assert_eq!(arms('╋'), Some([HEAVY; 4]));
        assert_eq!(arms('╔'), Some([NONE, DOUBLE, DOUBLE, NONE]));
        assert_eq!(arms('╬'), Some([DOUBLE; 4]));
        assert_eq!(arms('╿'), Some([HEAVY, NONE, LIGHT, NONE]));
        assert_eq!(arms('┄'), None);
    }

    /// Edges land on device pixels, so neighbors share them exactly.
    #[test]
    fn rects_are_pixel_snapped() {
        let scale = 2.;
        for c in ['█', '▛', '─', '│', '┼', '╬', '┄'] {
            for shape in shapes(c, cell(), 1, scale).unwrap() {
                let Shape::Rect(bounds, _) = shape else {
                    panic!("{c:?} not rects");
                };
                for v in [bounds.left(), bounds.right(), bounds.top(), bounds.bottom()] {
                    let device = f32::from(v) * scale;
                    assert!((device - device.round()).abs() < 1e-3, "{c:?} edge {v:?}");
                }
            }
        }
    }

    /// A horizontal line spans the whole cell, so it joins its neighbors.
    #[test]
    fn horizontal_line_spans_cell() {
        let shapes = shapes('─', cell(), 1, 2.).unwrap();
        let left = shapes.iter().filter_map(|s| match s {
            Shape::Rect(b, _) => Some((f32::from(b.left()), f32::from(b.right()))),
            _ => None,
        });
        let (min, max) = left.fold((f32::MAX, f32::MIN), |(a, b), (l, r)| (a.min(l), b.max(r)));
        assert_eq!((min, max), (10.5, 18.5));
    }

    /// A run of 100 `─` is one long line exactly as thick as a single one (it once took its
    /// thickness from the run's width and painted a solid slab).
    #[test]
    fn runs_keep_one_cell_thickness() {
        let band = |span| {
            shapes('─', cell(), span, 2.)
                .unwrap()
                .iter()
                .filter_map(|shape| match shape {
                    Shape::Rect(bounds, _) => {
                        Some((f32::from(bounds.size.height), f32::from(bounds.right())))
                    }
                    _ => None,
                })
                .fold((0f32, 0f32), |(height, right), (h, r)| {
                    (height.max(h), right.max(r))
                })
        };
        let (one_height, _) = band(1);
        let (run_height, run_right) = band(100);
        assert_eq!(run_height, one_height);
        // 100 cells of 8.4px from x = 10.3, snapped to half pixels.
        assert_eq!(run_right, 850.5);
    }
}
