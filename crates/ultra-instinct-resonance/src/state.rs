//! What a caller can see about a region without ranking it.
//!
//! [`RegionState`] reads the same flags and the same viewport test that the
//! locate penalties use, so `observe` and `locate` cannot disagree about
//! whether a control is disabled or off screen.

use ultra_instinct_core::{InteractionRegion, Rect, Zone};
use ultra_instinct_geometry::{is_fully_offscreen, normalize, zones};

/// Whether the control accepts input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Availability {
    Enabled,
    Disabled,
}

impl Availability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

/// How reachable the region is on screen. A ladder: when several flags are
/// set, the most limiting one wins, in the order hidden, offscreen,
/// occluded, visible. Each step down is less reachable by a click.
///
/// Zero size is not a step here; locate penalizes it separately and the
/// region's rect already shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Visibility {
    Visible,
    Occluded,
    Offscreen,
    Hidden,
}

impl Visibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Occluded => "occluded",
            Self::Offscreen => "offscreen",
            Self::Hidden => "hidden",
        }
    }
}

/// Availability and visibility of one region in one manifold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegionState {
    availability: Availability,
    visibility: Visibility,
}

impl RegionState {
    /// Offscreen means the offscreen flag or a rect fully outside `viewport`,
    /// the same test as the locate penalty.
    pub fn of(viewport: Rect, region: &InteractionRegion) -> Self {
        let flags = region.flags();
        let availability = if flags.disabled() {
            Availability::Disabled
        } else {
            Availability::Enabled
        };
        let visibility = if flags.hidden() {
            Visibility::Hidden
        } else if flags.offscreen() || is_fully_offscreen(region.rect(), viewport) {
            Visibility::Offscreen
        } else if flags.occluded() {
            Visibility::Occluded
        } else {
            Visibility::Visible
        };
        Self {
            availability,
            visibility,
        }
    }

    pub const fn availability(self) -> Availability {
        self.availability
    }

    pub const fn visibility(self) -> Visibility {
        self.visibility
    }
}

/// The first position zone `target` is in and `other` is not, in the order
/// left, right, top, bottom, center. Passing it as a locate `position` gives
/// `target` the geometric credit and withholds it from `other`. `None` when
/// every zone of `target` is shared, or the viewport cannot normalize.
pub fn separating_zone(viewport: Rect, target: Rect, other: Rect) -> Option<Zone> {
    let target_zones = zones(normalize(target, viewport).ok()?);
    let other_zones = zones(normalize(other, viewport).ok()?);
    target_zones
        .into_iter()
        .find(|zone| !other_zones.contains(zone))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_core::{
        Action, RegionFlags, RegionId, RegionParts, Role, SourceMask, UnitInterval,
    };

    fn region(flags: &str, y: f64) -> InteractionRegion {
        InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new("r").unwrap(),
            role: Role::Button,
            label: "Save".into(),
            rect: Rect::try_new(16.0, y, 80.0, 30.0).unwrap(),
            actions: vec![Action::Click],
            parent: None,
            sources: SourceMask::DOM,
            flags: RegionFlags::parse_list(flags).unwrap(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap()
    }

    fn state(flags: &str, y: f64) -> (&'static str, &'static str) {
        let viewport = Rect::try_viewport(0.0, 0.0, 1440.0, 900.0).unwrap();
        let state = RegionState::of(viewport, &region(flags, y));
        (state.availability().as_str(), state.visibility().as_str())
    }

    #[test]
    fn flags_and_geometry_map_onto_the_ladder() {
        assert_eq!(state("", 40.0), ("enabled", "visible"));
        assert_eq!(state("disabled", 40.0), ("disabled", "visible"));
        assert_eq!(state("occluded", 40.0), ("enabled", "occluded"));
        assert_eq!(state("offscreen", 40.0), ("enabled", "offscreen"));
        assert_eq!(state("hidden", 40.0), ("enabled", "hidden"));
        // Geometry alone puts a region off screen, as the locate penalty does.
        assert_eq!(state("", 900.0), ("enabled", "offscreen"));
        assert_eq!(state("", 871.0), ("enabled", "visible"));
    }

    #[test]
    fn the_most_limiting_visibility_wins() {
        assert_eq!(state("occluded,offscreen", 40.0).1, "offscreen");
        assert_eq!(state("occluded,hidden", 40.0).1, "hidden");
        assert_eq!(state("offscreen,hidden", 40.0).1, "hidden");
        assert_eq!(state("disabled,hidden", 40.0), ("disabled", "hidden"));
        assert!(Visibility::Visible < Visibility::Occluded);
        assert!(Visibility::Occluded < Visibility::Offscreen);
        assert!(Visibility::Offscreen < Visibility::Hidden);
    }

    #[test]
    fn separating_zone_names_a_zone_only_the_target_is_in() {
        let viewport = Rect::try_viewport(0.0, 0.0, 1280.0, 800.0).unwrap();
        // Live drive t8: quick-reply Send (left) and docked Compose Send (bottom).
        let reply = Rect::try_new(345.0, 380.0, 80.0, 36.0).unwrap();
        let compose = Rect::try_new(740.0, 752.0, 80.0, 36.0).unwrap();
        assert_eq!(separating_zone(viewport, reply, compose), Some(Zone::Left));
        assert_eq!(
            separating_zone(viewport, compose, reply),
            Some(Zone::Bottom)
        );
        // Same zones: nothing separates them.
        let twin = Rect::try_new(360.0, 380.0, 80.0, 36.0).unwrap();
        assert_eq!(separating_zone(viewport, reply, twin), None);
        // Order is left, right, top, bottom, center.
        let top_left = Rect::try_new(10.0, 10.0, 20.0, 20.0).unwrap();
        let middle = Rect::try_new(630.0, 390.0, 20.0, 20.0).unwrap();
        assert_eq!(
            separating_zone(viewport, top_left, middle),
            Some(Zone::Left)
        );
        assert_eq!(
            separating_zone(viewport, middle, top_left),
            Some(Zone::Center)
        );
        let flat = Rect::try_new(0.0, 0.0, 0.0, 0.0).unwrap();
        assert_eq!(separating_zone(flat, reply, compose), None);
    }
}
