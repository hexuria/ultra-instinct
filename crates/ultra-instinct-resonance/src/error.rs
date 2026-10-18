use std::fmt;

use ultra_instinct_geometry::GeometryError;
#[cfg(feature = "hgra")]
use ultra_instinct_hyper::HyperError;

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ResonanceError {
    #[cfg(feature = "hgra")]
    Hyper(HyperError),
    Geometry(GeometryError),
    /// Positive basis points did not sum to 100. `sum` is the integer total.
    WeightsDoNotSum {
        sum: u32,
    },
}

impl fmt::Display for ResonanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "hgra")]
            Self::Hyper(err) => write!(f, "hypervector error: {err}"),
            Self::Geometry(err) => write!(f, "geometry error: {err}"),
            Self::WeightsDoNotSum { sum } => {
                write!(
                    f,
                    "positive weights sum to {sum} basis points, expected 100"
                )
            }
        }
    }
}

impl std::error::Error for ResonanceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            #[cfg(feature = "hgra")]
            Self::Hyper(err) => Some(err),
            Self::Geometry(err) => Some(err),
            Self::WeightsDoNotSum { .. } => None,
        }
    }
}

#[cfg(feature = "hgra")]
impl From<HyperError> for ResonanceError {
    fn from(value: HyperError) -> Self {
        Self::Hyper(value)
    }
}

impl From<GeometryError> for ResonanceError {
    fn from(value: GeometryError) -> Self {
        Self::Geometry(value)
    }
}
