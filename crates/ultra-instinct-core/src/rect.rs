use std::fmt;

use crate::error::CoreError;

/// A point in viewport pixels. The origin and axis direction belong to the
/// producer of the snapshot. Phase 1 fixtures use a top-left origin with `y`
/// increasing downward.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    x: f64,
    y: f64,
}

impl Point {
    pub fn try_new(x: f64, y: f64) -> Result<Self, CoreError> {
        if !x.is_finite() || !y.is_finite() {
            return Err(CoreError::NonFiniteCoordinate);
        }
        Ok(Self { x, y })
    }

    pub const fn x(self) -> f64 {
        self.x
    }

    pub const fn y(self) -> f64 {
        self.y
    }

    pub fn distance(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}

/// An axis-aligned rectangle in viewport pixels.
///
/// Width and height are non-negative. Zero extent is representable so a
/// degenerate control can be penalized instead of being refused at the type
/// boundary. Viewports use [`Rect::try_viewport`], which rejects zero area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    pub fn try_new(x: f64, y: f64, width: f64, height: f64) -> Result<Self, CoreError> {
        if ![x, y, width, height].into_iter().all(f64::is_finite) {
            return Err(CoreError::NonFiniteCoordinate);
        }
        if width < 0.0 || height < 0.0 {
            return Err(CoreError::NegativeExtent);
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub fn try_viewport(x: f64, y: f64, width: f64, height: f64) -> Result<Self, CoreError> {
        let rect = Self::try_new(x, y, width, height)?;
        if rect.width == 0.0 || rect.height == 0.0 {
            return Err(CoreError::NonPositiveViewport);
        }
        Ok(rect)
    }

    pub const fn x(self) -> f64 {
        self.x
    }

    pub const fn y(self) -> f64 {
        self.y
    }

    pub const fn width(self) -> f64 {
        self.width
    }

    pub const fn height(self) -> f64 {
        self.height
    }

    pub fn right(self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(self) -> f64 {
        self.y + self.height
    }

    pub fn center(self) -> Point {
        Point {
            x: self.x + self.width / 2.0,
            y: self.y + self.height / 2.0,
        }
    }

    /// `point` lies inside this rect (edges included).
    pub fn contains_point(self, point: Point) -> bool {
        (self.x..=self.right()).contains(&point.x())
            && (self.y..=self.bottom()).contains(&point.y())
    }

    pub fn is_zero_area(self) -> bool {
        self.width == 0.0 || self.height == 0.0
    }

    /// Quantize to thousandths of a pixel so fingerprinting ignores noise
    /// below that resolution.
    pub fn quantize_milli(self) -> (i64, i64, i64, i64) {
        (
            quantize(self.x),
            quantize(self.y),
            quantize(self.width),
            quantize(self.height),
        )
    }
}

fn quantize(value: f64) -> i64 {
    (value * 1000.0).round() as i64
}

impl fmt::Display for Rect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "({}, {}, {}×{})",
            self.x, self.y, self.width, self.height
        )
    }
}
