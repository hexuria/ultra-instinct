//! Viewport-relative geometry for ultra-instinct.
//!
//! Pixel rectangles are normalized by the viewport (`x / width`, `y / height`)
//! and then classified into symbolic atoms. Screen space uses a top-left origin
//! with `y` increasing downward, so [`Relation::Above`] means a smaller `y`.
//!
//! Thresholds (version 1):
//! - left / right split at `1/3` and `2/3` of the viewport
//! - center is the closed middle third on both axes
//! - small area `< 0.01`, medium `< 0.08`, otherwise large (normalized area)
//! - near when the gap between rectangles is `<= 0.08` viewport units
//! - aligned when centers differ by `<= 0.05` on that axis

#![forbid(unsafe_code)]

use std::fmt;

use ultra_instinct_core::{InteractionManifold, Rect, RegionId, Relation, SizeClass, Zone};

/// Normalized rectangle. Components are fractions of the viewport, not pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizedRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl NormalizedRect {
    pub const fn x(self) -> f64 {
        self.x
    }
    pub const fn y(self) -> f64 {
        self.y
    }
    pub const fn w(self) -> f64 {
        self.w
    }
    pub const fn h(self) -> f64 {
        self.h
    }

    pub fn center(self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn area(self) -> f64 {
        self.w * self.h
    }

    pub fn right(self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(self) -> f64 {
        self.y + self.h
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GeometryError {
    NonPositiveViewport,
    UnknownRegion(String),
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPositiveViewport => f.write_str("viewport width and height must be positive"),
            Self::UnknownRegion(id) => write!(f, "unknown region `{id}`"),
        }
    }
}

impl std::error::Error for GeometryError {}

pub const ZONE_LOW: f64 = 1.0 / 3.0;
pub const ZONE_HIGH: f64 = 2.0 / 3.0;
pub const SMALL_AREA_MAX: f64 = 0.01;
pub const MEDIUM_AREA_MAX: f64 = 0.08;
pub const NEAR_GAP: f64 = 0.08;
pub const ALIGN_TOLERANCE: f64 = 0.05;

/// `nx = (x - viewport.x) / viewport.width`, and the same for `y` and extents.
pub fn normalize(rect: Rect, viewport: Rect) -> Result<NormalizedRect, GeometryError> {
    if viewport.width() <= 0.0 || viewport.height() <= 0.0 {
        return Err(GeometryError::NonPositiveViewport);
    }
    Ok(NormalizedRect {
        x: (rect.x() - viewport.x()) / viewport.width(),
        y: (rect.y() - viewport.y()) / viewport.height(),
        w: rect.width() / viewport.width(),
        h: rect.height() / viewport.height(),
    })
}

/// Symbolic position atoms for a normalized center.
///
/// A region is never both left and right. Center requires both axes to sit in
/// the closed middle third, so it does not overlap a side atom.
pub fn zones(rect: NormalizedRect) -> Vec<Zone> {
    let (cx, cy) = rect.center();
    let mut found = Vec::with_capacity(3);
    if cx < ZONE_LOW {
        found.push(Zone::Left);
    }
    if cx > ZONE_HIGH {
        found.push(Zone::Right);
    }
    if cy < ZONE_LOW {
        found.push(Zone::Top);
    }
    if cy > ZONE_HIGH {
        found.push(Zone::Bottom);
    }
    let mid_x = (ZONE_LOW..=ZONE_HIGH).contains(&cx);
    let mid_y = (ZONE_LOW..=ZONE_HIGH).contains(&cy);
    if mid_x && mid_y {
        found.push(Zone::Center);
    }
    found
}

pub fn size_class(rect: NormalizedRect) -> SizeClass {
    let area = rect.area();
    if area < SMALL_AREA_MAX {
        SizeClass::Small
    } else if area < MEDIUM_AREA_MAX {
        SizeClass::Medium
    } else {
        SizeClass::Large
    }
}

/// True when the rectangle lies completely outside the viewport's closed bounds.
pub fn is_fully_offscreen(rect: Rect, viewport: Rect) -> bool {
    rect.right() <= viewport.x()
        || rect.x() >= viewport.right()
        || rect.bottom() <= viewport.y()
        || rect.y() >= viewport.bottom()
}

/// Geometric relations of `a` relative to `b` (not parent/child).
///
/// Equal rectangles overlap and are near, but neither contains the other.
/// Touching edges with zero intersection area are near when the gap is inside
/// [`NEAR_GAP`]. Results follow a fixed relation order.
pub fn spatial_relations(a: NormalizedRect, b: NormalizedRect) -> Vec<Relation> {
    let overlap_w = (a.right().min(b.right()) - a.x().max(b.x())).max(0.0);
    let overlap_h = (a.bottom().min(b.bottom()) - a.y().max(b.y())).max(0.0);
    let overlaps = overlap_w > 0.0 && overlap_h > 0.0;
    let a_inside = a.x() >= b.x()
        && a.y() >= b.y()
        && a.right() <= b.right()
        && a.bottom() <= b.bottom()
        && a.area() < b.area();
    let b_inside = b.x() >= a.x()
        && b.y() >= a.y()
        && b.right() <= a.right()
        && b.bottom() <= a.bottom()
        && b.area() < a.area();
    let gap_x = if a.right() < b.x() {
        b.x() - a.right()
    } else if b.right() < a.x() {
        a.x() - b.right()
    } else {
        0.0
    };
    let gap_y = if a.bottom() < b.y() {
        b.y() - a.bottom()
    } else if b.bottom() < a.y() {
        a.y() - b.bottom()
    } else {
        0.0
    };
    let gap = (gap_x * gap_x + gap_y * gap_y).sqrt();
    let (acx, acy) = a.center();
    let (bcx, bcy) = b.center();

    let mut rels = Vec::new();
    if a.bottom() <= b.y() {
        rels.push(Relation::Above);
    }
    if a.y() >= b.bottom() {
        rels.push(Relation::Below);
    }
    if a_inside {
        rels.push(Relation::Inside);
    }
    if b_inside {
        rels.push(Relation::Contains);
    }
    if overlaps {
        rels.push(Relation::Overlaps);
    }
    if gap <= NEAR_GAP {
        rels.push(Relation::Near);
    }
    if (acx - bcx).abs() <= ALIGN_TOLERANCE {
        rels.push(Relation::AlignedX);
    }
    if (acy - bcy).abs() <= ALIGN_TOLERANCE {
        rels.push(Relation::AlignedY);
    }
    rels
}

/// Geometric relations plus `Parent` / `Child` from the manifold link.
pub fn relations(
    manifold: &InteractionManifold,
    from: &RegionId,
    to: &RegionId,
) -> Result<Vec<Relation>, GeometryError> {
    if from == to {
        return Ok(Vec::new());
    }
    let left = manifold
        .get(from)
        .ok_or_else(|| GeometryError::UnknownRegion(from.to_string()))?;
    let right = manifold
        .get(to)
        .ok_or_else(|| GeometryError::UnknownRegion(to.to_string()))?;
    let a = normalize(left.rect(), manifold.viewport())?;
    let b = normalize(right.rect(), manifold.viewport())?;
    let mut rels = Vec::new();
    if left.parent() == Some(to) {
        rels.push(Relation::Parent);
    }
    if right.parent() == Some(from) {
        rels.push(Relation::Child);
    }
    rels.extend(spatial_relations(a, b));
    Ok(rels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_core::{
        Action, InteractionRegion, Rect, RegionFlags, RegionId, RegionParts, Role, SourceMask,
        UnitInterval, Zone,
    };

    fn vp() -> Rect {
        Rect::try_viewport(0.0, 0.0, 100.0, 100.0).unwrap()
    }

    fn n(x: f64, y: f64, w: f64, h: f64) -> NormalizedRect {
        normalize(Rect::try_new(x, y, w, h).unwrap(), vp()).unwrap()
    }

    #[test]
    fn normalizes_against_viewport_origin() {
        let viewport = Rect::try_viewport(10.0, 20.0, 200.0, 100.0).unwrap();
        let rect = Rect::try_new(110.0, 70.0, 50.0, 25.0).unwrap();
        let norm = normalize(rect, viewport).unwrap();
        assert!((norm.x() - 0.5).abs() < 1e-12);
        assert!((norm.y() - 0.5).abs() < 1e-12);
        assert!((norm.w() - 0.25).abs() < 1e-12);
        assert!((norm.h() - 0.25).abs() < 1e-12);
    }

    #[test]
    fn rejects_non_positive_viewport() {
        let rect = Rect::try_new(0.0, 0.0, 1.0, 1.0).unwrap();
        let bad = Rect::try_new(0.0, 0.0, 0.0, 10.0).unwrap();
        assert_eq!(
            normalize(rect, bad),
            Err(GeometryError::NonPositiveViewport)
        );
    }

    #[test]
    fn zones_and_sizes_match_the_documented_thresholds() {
        let left_top = n(0.0, 0.0, 10.0, 10.0);
        assert_eq!(zones(left_top), vec![Zone::Left, Zone::Top]);
        assert_eq!(size_class(left_top), SizeClass::Medium);

        let center = n(40.0, 40.0, 20.0, 20.0);
        assert_eq!(zones(center), vec![Zone::Center]);
        assert_eq!(size_class(center), SizeClass::Medium);
        let center_small = n(48.0, 48.0, 4.0, 4.0);
        assert_eq!(zones(center_small), vec![Zone::Center]);
        assert_eq!(size_class(center_small), SizeClass::Small);

        let right_bottom = n(80.0, 80.0, 40.0, 40.0);
        assert_eq!(zones(right_bottom), vec![Zone::Right, Zone::Bottom]);
        assert_eq!(size_class(right_bottom), SizeClass::Large);

        let tiny = n(0.0, 40.0, 5.0, 5.0);
        assert_eq!(size_class(tiny), SizeClass::Small);
    }

    #[test]
    fn spatial_atoms_above_inside_overlap_near_aligned() {
        let a = n(0.0, 0.0, 10.0, 10.0);
        let below = n(0.0, 16.0, 10.0, 10.0);
        let rels = spatial_relations(a, below);
        assert!(rels.contains(&Relation::Above));
        assert!(rels.contains(&Relation::AlignedX));
        assert!(rels.contains(&Relation::Near));
        assert!(!rels.contains(&Relation::Below));
        assert!(spatial_relations(below, a).contains(&Relation::Below));

        let outer = n(0.0, 0.0, 50.0, 50.0);
        let inner = spatial_relations(a, outer);
        assert!(inner.contains(&Relation::Inside));
        assert!(inner.contains(&Relation::Overlaps));
        assert!(spatial_relations(outer, a).contains(&Relation::Contains));

        let overlap = n(5.0, 5.0, 10.0, 10.0);
        assert!(spatial_relations(a, overlap).contains(&Relation::Overlaps));

        let far = n(0.0, 80.0, 10.0, 10.0);
        let far_rels = spatial_relations(a, far);
        assert!(far_rels.contains(&Relation::Above));
        assert!(!far_rels.contains(&Relation::Near));
        assert!(far_rels.contains(&Relation::AlignedX));
    }

    #[test]
    fn offscreen_requires_the_rect_to_clear_the_viewport() {
        let viewport = vp();
        let outside = Rect::try_new(-50.0, 0.0, 10.0, 10.0).unwrap();
        let partial = Rect::try_new(-5.0, 0.0, 10.0, 10.0).unwrap();
        let inside = Rect::try_new(1.0, 1.0, 10.0, 10.0).unwrap();
        assert!(is_fully_offscreen(outside, viewport));
        assert!(!is_fully_offscreen(partial, viewport));
        assert!(!is_fully_offscreen(inside, viewport));
    }

    #[test]
    fn parent_relation_comes_from_the_manifold_link() {
        let parent = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new("nav").unwrap(),
            role: Role::Navigation,
            label: "Sidebar".into(),
            rect: Rect::try_new(0.0, 0.0, 30.0, 100.0).unwrap(),
            actions: vec![Action::Scroll],
            parent: None,
            sources: SourceMask::DOM,
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap();
        let child = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new("settings").unwrap(),
            role: Role::Button,
            label: "Settings".into(),
            rect: Rect::try_new(2.0, 10.0, 20.0, 8.0).unwrap(),
            actions: vec![Action::Click],
            parent: Some(RegionId::try_new("nav").unwrap()),
            sources: SourceMask::DOM,
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap();
        let manifold = InteractionManifold::try_new(vp(), vec![parent, child], 0).unwrap();
        let rels = relations(
            &manifold,
            &RegionId::try_new("settings").unwrap(),
            &RegionId::try_new("nav").unwrap(),
        )
        .unwrap();
        assert!(rels.contains(&Relation::Parent));
        assert!(rels.contains(&Relation::Inside));
        let missing = relations(
            &manifold,
            &RegionId::try_new("missing").unwrap(),
            &RegionId::try_new("nav").unwrap(),
        )
        .unwrap_err();
        assert_eq!(missing, GeometryError::UnknownRegion("missing".into()));
    }
}
