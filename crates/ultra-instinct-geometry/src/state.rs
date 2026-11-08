//! What a caller can see about a region.
//!
//! [`RegionState`] reads the region flags and the viewport test
//! [`is_fully_offscreen`], so `observe`, the world snapshot and `guard`
//! cannot disagree about whether a control is disabled or off screen.

use ultra_instinct_core::{InteractionRegion, Rect};

use crate::is_fully_offscreen;

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
/// Zero size is not a step here; the region's rect already shows it.
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
    /// Offscreen means the offscreen flag or a rect fully outside `viewport`.
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
        // Geometry alone puts a region off screen.
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
}
