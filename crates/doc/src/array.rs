//! Associative arrays: the parameters an array keeps, and where they place its items.
//!
//! An associative array is a block reference to an anonymous block (`*U…`, the *array block*)
//! that holds one block reference per item. Each item refers to the array's *source block*
//! (another anonymous block holding the source objects) or, for items replaced with ARRAYEDIT,
//! to another block. Other programs see an ordinary block reference; CADCraft keeps the
//! parameters below on the array block ([`Block::array`](crate::Block)) so the items can be
//! regenerated when the array is edited.
//!
//! All positions are in the array block's coordinates: its origin is the array's base point,
//! which is where the array's block reference is inserted.

use cadcraft_geom::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::{Common, Drawing, Entity, EntityKind, Insert, Lineweight};

/// Most items one array may have (items × rows × levels), whatever MAXARRAY says.
pub const MAX_ARRAY_ITEMS: u64 = 10_000_000;

fn one() -> u32 {
    1
}

/// The parameters of an associative array.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArrayParams {
    /// The source block the items refer to.
    pub source: String,
    pub kind: ArrayKind,
    /// Levels: copies of the whole array along Z.
    #[serde(default = "one")]
    pub levels: u32,
    #[serde(default)]
    pub level_spacing: f64,
    /// Items that refer to another block than the source (ARRAYEDIT REPlace).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced: Vec<ReplacedItem>,
    /// The source objects are out for editing (ARRAYEDIT Source, until ARRAYCLOSE). Not saved:
    /// a drawing saved meanwhile keeps the objects as separate objects.
    #[serde(skip)]
    pub editing: Option<SourceEdit>,
}

/// ARRAYEDIT Source in progress: the array's source objects placed in the drawing for editing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceEdit {
    /// The array (block reference) being edited.
    pub array: crate::Handle,
    /// The item the objects were placed at: (i, j, level).
    pub item: (u32, u32, u32),
    /// The objects placed for editing.
    pub objects: Vec<crate::Handle>,
    /// Objects with this handle or above were drawn while editing; they join the source too.
    pub first_new: u64,
}

/// Rectangular or polar placement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ArrayKind {
    Rect(RectArray),
    Polar(PolarArray),
    Path(PathArray),
}

/// The most vertices a path array keeps of its path.
pub const MAX_PATH_POINTS: usize = 20_000;

/// A path array: items along a curve. The curve is kept as the polyline it was flattened to
/// (arcs and splines in short chords), starting at the array's base point, so the items stay
/// where they were put when the original curve is edited or erased.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathArray {
    /// The path from its start, relative to the first vertex (so the first is the origin).
    pub points: Vec<Vec2>,
    /// The path returns to its start (a segment joins the last vertex to the first).
    #[serde(default)]
    pub closed: bool,
    /// Items along the path, the first at its start.
    pub count: u32,
    /// Items divide the path evenly; otherwise they are `spacing` apart along it.
    pub divide: bool,
    #[serde(default)]
    pub spacing: f64,
    /// Items turn with the path (else they keep their orientation).
    pub align: bool,
}

impl PathArray {
    /// The path's segments with the length up to the end of each, and the total length;
    /// `None` when a vertex is not finite or the path has no length.
    fn segments(&self) -> Option<(Vec<(Vec2, Vec2, f64)>, f64)> {
        let closing = if self.closed { self.points.last().copied().zip(self.points.first().copied()) } else { None };
        let pairs = self.points.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?))).chain(closing);
        let mut out = Vec::new();
        let mut total = 0.0;
        for (a, b) in pairs {
            let l = a.dist(b);
            if l > 1e-12 {
                total += l;
                out.push((a, b, total));
            }
        }
        (total.is_finite() && total > 1e-12).then_some((out, total))
    }

    /// Whether the path can carry items: 2 to [`MAX_PATH_POINTS`] finite vertices with a
    /// length, a finite spacing (above zero unless the items divide the path).
    pub fn is_valid(&self) -> bool {
        (2..=MAX_PATH_POINTS).contains(&self.points.len())
            && self.count >= 1
            && self.points.iter().all(|p| p.x.is_finite() && p.y.is_finite())
            && self.spacing.is_finite()
            && (self.divide || self.spacing > 0.0)
            && self.segments().is_some()
    }

    /// Each item's offset from the first and its turn (radians), in order.
    fn placed(&self) -> Option<Vec<(Vec2, f64)>> {
        let (segs, total) = self.segments()?;
        let step = if self.divide {
            let gaps = if self.closed { self.count } else { self.count.max(2) - 1 };
            total / f64::from(gaps)
        } else {
            self.spacing
        };
        let at = |d: f64| {
            let d = if self.closed { d.rem_euclid(total) } else { d.clamp(0.0, total) };
            let k = segs.partition_point(|s| s.2 < d).min(segs.len().saturating_sub(1));
            let (a, b, end) = *segs.get(k)?;
            let l = a.dist(b);
            let t = if l > 0.0 { (1.0 - (end - d) / l).clamp(0.0, 1.0) } else { 0.0 };
            Some((a + (b - a) * t, (b - a).angle()))
        };
        let (p0, a0) = at(0.0)?;
        (0..self.count)
            .map(|i| {
                let (p, a) = at(step * f64::from(i))?;
                Some((p - p0, if self.align { a - a0 } else { 0.0 }))
            })
            .collect()
    }
}

/// A rectangular array: columns along the axis angle, rows across it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RectArray {
    pub cols: u32,
    pub rows: u32,
    pub col_spacing: f64,
    pub row_spacing: f64,
    /// Direction of the rows (the column axis), radians.
    #[serde(default)]
    pub axis_angle: f64,
    /// Elevation added per row.
    #[serde(default)]
    pub row_elevation: f64,
}

/// A polar array: items around a centre, rows as rings further out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolarArray {
    pub center: Vec2,
    pub items: u32,
    /// Angle to fill, degrees (positive = counterclockwise, ±360 = a full circle).
    pub fill: f64,
    /// Items are rotated as they go around (else they keep their orientation).
    pub rotate: bool,
    #[serde(default = "one")]
    pub rows: u32,
    /// Distance between rings, outwards from the centre.
    #[serde(default)]
    pub row_spacing: f64,
    #[serde(default)]
    pub row_elevation: f64,
}

/// An item that refers to another block. `i` is the column (rectangular) or the item around the
/// circle (polar), `j` the row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplacedItem {
    pub i: u32,
    pub j: u32,
    #[serde(default)]
    pub level: u32,
    pub block: String,
}

/// Where one item goes: its block reference's insertion point (the image of the array's base
/// point) and rotation (radians).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrayItem {
    pub i: u32,
    pub j: u32,
    pub level: u32,
    pub insert: Vec3,
    pub rotation: f64,
}

impl ArrayParams {
    /// Items along the first direction (columns or items around) and rows.
    pub fn grid(&self) -> (u32, u32) {
        match &self.kind {
            ArrayKind::Rect(r) => (r.cols, r.rows),
            ArrayKind::Polar(p) => (p.items, p.rows),
            ArrayKind::Path(p) => (p.count, 1),
        }
    }

    /// The number of items, saturating.
    pub fn item_count(&self) -> u64 {
        let (a, b) = self.grid();
        u64::from(a).saturating_mul(u64::from(b)).saturating_mul(u64::from(self.levels))
    }

    /// Whether the parameters are usable: at least one item in every direction, at most
    /// [`MAX_ARRAY_ITEMS`] in all, and finite numbers. Parameters read from files are checked
    /// with this before they are kept.
    pub fn is_valid(&self) -> bool {
        let (a, b) = self.grid();
        let nums = match &self.kind {
            ArrayKind::Rect(r) => [r.col_spacing, r.row_spacing, r.axis_angle, r.row_elevation, 0.0, 0.0],
            ArrayKind::Polar(p) => [p.center.x, p.center.y, p.fill, p.row_spacing, p.row_elevation, 0.0],
            ArrayKind::Path(_) => [0.0; 6],
        };
        if matches!(&self.kind, ArrayKind::Path(p) if !p.is_valid()) {
            return false;
        }
        a >= 1
            && b >= 1
            && self.levels >= 1
            && self.item_count() <= MAX_ARRAY_ITEMS
            && !self.source.trim().is_empty()
            && nums.iter().chain(std::iter::once(&self.level_spacing)).all(|v| v.is_finite())
            && self.replaced.len() as u64 <= MAX_ARRAY_ITEMS
    }

    /// Where every item goes, row by row and level by level; `None` when the parameters are not
    /// [valid](Self::is_valid).
    pub fn items(&self) -> Option<Vec<ArrayItem>> {
        if !self.is_valid() {
            return None;
        }
        let n = usize::try_from(self.item_count()).ok()?;
        let mut out = Vec::with_capacity(n);
        // A path's placement does not depend on the level: build it once, not once per level.
        let path_placed = match &self.kind {
            ArrayKind::Path(p) => Some(p.placed()?),
            _ => None,
        };
        for level in 0..self.levels {
            let lz = self.level_spacing * f64::from(level);
            match &self.kind {
                ArrayKind::Rect(r) => {
                    let (ux, uy) = (Vec2::from_angle(r.axis_angle), Vec2::from_angle(r.axis_angle).perp());
                    for j in 0..r.rows {
                        for i in 0..r.cols {
                            let p = ux * (r.col_spacing * f64::from(i)) + uy * (r.row_spacing * f64::from(j));
                            out.push(ArrayItem { i, j, level, insert: p.to3(lz + r.row_elevation * f64::from(j)), rotation: 0.0 });
                        }
                    }
                }
                ArrayKind::Path(_) => {
                    for (i, (at, rotation)) in path_placed.iter().flatten().enumerate() {
                        out.push(ArrayItem { i: u32::try_from(i).ok()?, j: 0, level, insert: at.to3(lz), rotation: *rotation });
                    }
                }
                ArrayKind::Polar(p) => {
                    let total = p.fill.to_radians();
                    let full = (total.abs() - std::f64::consts::TAU).abs() < 1e-9;
                    let step = if full { total / f64::from(p.items.max(1)) } else { total / f64::from(p.items.saturating_sub(1).max(1)) };
                    // Rows move the items outwards: along the line from the centre to the base point.
                    let out_dir = {
                        let d = (Vec2::ZERO - p.center).normalized();
                        if d == Vec2::ZERO { Vec2::X } else { d }
                    };
                    for j in 0..p.rows {
                        let o = out_dir * (p.row_spacing * f64::from(j));
                        for i in 0..p.items {
                            let a = step * f64::from(i);
                            let at = o.rotate_about(p.center, a);
                            let rotation = if p.rotate { a } else { 0.0 };
                            out.push(ArrayItem { i, j, level, insert: at.to3(lz + p.row_elevation * f64::from(j)), rotation });
                        }
                    }
                }
            }
        }
        Some(out)
    }

    /// The block an item refers to: a replacement, else the source.
    pub fn block_for(&self, i: u32, j: u32, level: u32) -> &str {
        self.replaced.iter().rev().find(|r| (r.i, r.j, r.level) == (i, j, level)).map_or(self.source.as_str(), |r| r.block.as_str())
    }

    /// The block references that make up the array block: one per item, on layer 0 with ByBlock
    /// properties so they take the array's. Handles are allocated from `d`. `None` when the
    /// parameters are not [valid](Self::is_valid).
    pub fn item_entities(&self, d: &mut Drawing) -> Option<Vec<Entity>> {
        let items = self.items()?;
        let common = Common {
            layer: "0".into(),
            color: cadcraft_color::Color::ByBlock,
            linetype: "ByBlock".into(),
            lineweight: Lineweight::ByBlock,
            ..Common::default()
        };
        let mut out = Vec::with_capacity(items.len());
        for it in items {
            let ins = Insert {
                block: self.block_for(it.i, it.j, it.level).to_string(),
                insert: it.insert,
                scale: Vec3::new(1.0, 1.0, 1.0),
                rotation: it.rotation,
                attribs: Vec::new(),
                cols: 1,
                rows: 1,
                col_spacing: 0.0,
                row_spacing: 0.0,
            };
            out.push(Entity { handle: d.new_handle(), common: common.clone(), kind: EntityKind::Insert(ins) });
        }
        Some(out)
    }

    /// Take the block names from the array block's items `ents` (in the order [`items`]
    /// lists them): the source block is whatever the items that are not replaced refer to, so
    /// the parameters follow blocks renamed on paste, on INSERT of a file or by other programs
    /// (which may rename anonymous blocks). `false` when the items don't match the parameters
    /// (another program changed the block), so the array should be treated as a plain block.
    ///
    /// [`items`]: Self::items
    pub fn sync_names<'a>(&mut self, ents: impl Iterator<Item = &'a Entity>) -> bool {
        let n = self.item_count();
        let ents: Vec<&Entity> = ents.take(usize::try_from(n).unwrap_or(usize::MAX).saturating_add(1)).collect();
        if ents.len() as u64 != n {
            return false;
        }
        let Some(items) = self.items() else { return false };
        // Index the replacement entries by placement so each item is matched in O(1) instead of
        // scanning the whole `replaced` vector per item, which is quadratic when both the items
        // and the replacements are large (CWE-407). Last entry wins, as `.rev().find` did.
        let mut last_replaced: std::collections::HashMap<(u32, u32, u32), usize> = std::collections::HashMap::with_capacity(self.replaced.len());
        for (idx, r) in self.replaced.iter().enumerate() {
            last_replaced.insert((r.i, r.j, r.level), idx);
        }
        let mut source: Option<&str> = None;
        for (it, e) in items.iter().zip(&ents) {
            let EntityKind::Insert(ins) = &e.kind else { return false };
            match last_replaced.get(&(it.i, it.j, it.level)).and_then(|&idx| self.replaced.get_mut(idx)) {
                Some(r) => r.block.clone_from(&ins.block),
                None => match source {
                    None => source = Some(&ins.block),
                    Some(s) if s.eq_ignore_ascii_case(&ins.block) => {}
                    Some(_) => return false,
                },
            }
        }
        if let Some(s) = source {
            self.source = s.to_string();
        }
        true
    }
}

impl Drawing {
    /// The parameters of `e` when it is an associative array (a block reference to an array
    /// block whose items match its parameters), with the block names its items use.
    pub fn array_params(&self, e: &Entity) -> Option<ArrayParams> {
        let EntityKind::Insert(i) = &e.kind else { return None };
        let b = self.block(&i.block)?;
        let mut p = b.array.clone()?;
        p.sync_names(b.entities.iter().map(|e| e.as_ref())).then_some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(cols: u32, rows: u32) -> ArrayParams {
        ArrayParams {
            source: "*U1".into(),
            kind: ArrayKind::Rect(RectArray { cols, rows, col_spacing: 2.0, row_spacing: 3.0, axis_angle: 0.0, row_elevation: 0.0 }),
            levels: 2,
            level_spacing: 5.0,
            replaced: Vec::new(),
            editing: None,
        }
    }

    #[test]
    fn rect_items_go_column_row_level() {
        let items = rect(3, 2).items().unwrap_or_default();
        assert_eq!(items.len(), 12);
        let last = items.last().copied().map(|i| (i.i, i.j, i.level, i.insert));
        assert_eq!(last, Some((2, 1, 1, Vec3::new(4.0, 3.0, 5.0))));
    }

    #[test]
    fn polar_rows_go_outwards() {
        let p = ArrayParams {
            source: "*U1".into(),
            kind: ArrayKind::Polar(PolarArray {
                center: Vec2::new(-10.0, 0.0),
                items: 4,
                fill: 360.0,
                rotate: true,
                rows: 2,
                row_spacing: 1.0,
                row_elevation: 0.0,
            }),
            levels: 1,
            level_spacing: 0.0,
            replaced: Vec::new(),
            editing: None,
        };
        let items = p.items().unwrap_or_default();
        assert_eq!(items.len(), 8);
        // Item 1 of row 0 is a quarter turn round: base (0,0) → (-10,10).
        assert!(items[1].insert.xy().near(Vec2::new(-10.0, 10.0), 1e-9));
        // Row 1 starts one unit further out.
        assert!(items[4].insert.xy().near(Vec2::new(1.0, 0.0), 1e-9));
    }

    #[test]
    fn hostile_params_are_invalid() {
        let mut p = rect(u32::MAX, u32::MAX);
        assert!(!p.is_valid() && p.items().is_none());
        p = rect(0, 1);
        assert!(!p.is_valid());
        p = rect(2, 2);
        p.level_spacing = f64::NAN;
        assert!(!p.is_valid());
    }

    fn path(points: &[(f64, f64)], closed: bool, count: u32, divide: bool, spacing: f64, align: bool) -> ArrayParams {
        let first = points.first().map_or(Vec2::ZERO, |p| Vec2::new(p.0, p.1));
        let points = points.iter().map(|p| Vec2::new(p.0, p.1) - first).collect();
        ArrayParams {
            source: "*U1".into(),
            kind: ArrayKind::Path(PathArray { points, closed, count, divide, spacing, align }),
            levels: 1,
            level_spacing: 0.0,
            replaced: Vec::new(),
            editing: None,
        }
    }

    #[test]
    fn path_items_divide_the_path_and_turn_with_it() {
        // An L: 10 along +X, then 10 along +Y; 5 items divide its 20 units into steps of 5.
        let items = path(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], false, 5, true, 0.0, true).items().unwrap_or_default();
        let at: Vec<(Vec2, f64)> = items.iter().map(|i| (i.insert.xy(), i.rotation)).collect();
        assert_eq!(at.len(), 5);
        assert!(at[1].0.near(Vec2::new(5.0, 0.0), 1e-9) && at[1].1.abs() < 1e-9);
        assert!(at[2].0.near(Vec2::new(10.0, 0.0), 1e-9));
        assert!(at[3].0.near(Vec2::new(10.0, 5.0), 1e-9) && (at[3].1 - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        assert!(at[4].0.near(Vec2::new(10.0, 10.0), 1e-9));
        // Without alignment the items keep their orientation.
        let flat = path(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], false, 5, true, 0.0, false).items().unwrap_or_default();
        assert!(flat.iter().all(|i| i.rotation == 0.0));
    }

    #[test]
    fn closed_path_items_do_not_repeat_the_start() {
        let sq = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        let items = path(&sq, true, 4, true, 0.0, true).items().unwrap_or_default();
        assert_eq!(items.len(), 4);
        assert!(items[1].insert.xy().near(Vec2::new(4.0, 0.0), 1e-9));
        assert!(items[3].insert.xy().near(Vec2::new(0.0, 4.0), 1e-9));
    }

    #[test]
    fn path_items_at_a_fixed_spacing_stop_on_the_path() {
        let items = path(&[(0.0, 0.0), (10.0, 0.0)], false, 3, false, 4.0, true).items().unwrap_or_default();
        assert!(items[2].insert.xy().near(Vec2::new(8.0, 0.0), 1e-9));
        // More items than fit stay on the end of an open path.
        let items = path(&[(0.0, 0.0), (10.0, 0.0)], false, 5, false, 4.0, true).items().unwrap_or_default();
        assert!(items[4].insert.xy().near(Vec2::new(10.0, 0.0), 1e-9));
    }

    #[test]
    fn hostile_paths_are_invalid() {
        assert!(!path(&[(0.0, 0.0), (0.0, 0.0)], false, 3, true, 0.0, true).is_valid(), "no length");
        assert!(!path(&[(0.0, 0.0), (f64::NAN, 1.0)], false, 3, true, 0.0, true).is_valid());
        assert!(!path(&[(0.0, 0.0), (1.0, 0.0)], false, 0, true, 0.0, true).is_valid());
        assert!(!path(&[(0.0, 0.0), (1.0, 0.0)], false, 3, false, 0.0, true).is_valid(), "spacing needs a length");
        assert!(!path(&[(0.0, 0.0), (1.0, 0.0)], false, 3, false, f64::NAN, true).is_valid());
        assert!(path(&[(0.0, 0.0), (1.0, 0.0)], false, u32::MAX, true, 0.0, true).items().is_none());
        let long: Vec<(f64, f64)> = (0..=MAX_PATH_POINTS).map(|i| (i as f64, 0.0)).collect();
        assert!(!path(&long, false, 3, true, 0.0, true).is_valid());
    }

    #[test]
    fn hostile_path_with_many_levels_places_the_path_once() {
        // 20,000 vertices, one item per level, 100,000 levels: every limit holds on its own,
        // and rebuilding the path for each level would visit about 2e9 segments.
        let long: Vec<(f64, f64)> = (0..MAX_PATH_POINTS).map(|i| (i as f64, 0.0)).collect();
        let mut p = path(&long, false, 1, true, 0.0, true);
        p.levels = 100_000;
        p.level_spacing = 1.0;
        let started = std::time::Instant::now();
        let items = p.items().unwrap_or_default();
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "took {:?}", started.elapsed());
        assert_eq!(items.len(), 100_000);
        assert!(items.iter().enumerate().all(|(l, i)| i.level as usize == l && i.insert.xy() == Vec2::ZERO));
    }

    #[test]
    fn path_levels_repeat_the_same_placement() {
        let mut p = path(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], false, 3, true, 0.0, true);
        p.levels = 3;
        p.level_spacing = 2.0;
        let items = p.items().unwrap_or_default();
        assert_eq!(items.len(), 9);
        for (k, item) in items.iter().enumerate() {
            let first = items.get(k % 3).copied();
            assert_eq!(Some((item.insert.xy(), item.rotation)), first.map(|f| (f.insert.xy(), f.rotation)));
            assert_eq!(item.insert.z, 2.0 * (k / 3) as f64);
        }
    }
}
