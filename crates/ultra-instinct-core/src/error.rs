use std::fmt;

/// Failures while constructing core values or a manifold.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreError {
    /// Region or parent identifier was empty.
    EmptyId,
    /// Identifier contained whitespace or a control character.
    InvalidId,
    /// A coordinate was NaN or infinite.
    NonFiniteCoordinate,
    /// Width or height was negative.
    NegativeExtent,
    /// A viewport must have positive width and height.
    NonPositiveViewport,
    /// Two regions were inserted with the same id.
    DuplicateRegion(String),
    /// A unit-interval value was outside `[0, 1]` or non-finite.
    StabilityOutOfRange,
    /// Source mask bits outside the known set.
    UnknownSourceBits(u8),
    /// Query text was empty or had no alphanumeric token.
    EmptyQueryText,
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId => f.write_str("region id must not be empty"),
            Self::InvalidId => {
                f.write_str("region id must not contain whitespace or control characters")
            }
            Self::NonFiniteCoordinate => f.write_str("coordinate must be finite"),
            Self::NegativeExtent => f.write_str("width and height must be non-negative"),
            Self::NonPositiveViewport => f.write_str("viewport width and height must be positive"),
            Self::DuplicateRegion(id) => write!(f, "duplicate region id `{id}`"),
            Self::StabilityOutOfRange => {
                f.write_str("temporal stability must be a finite value in [0, 1]")
            }
            Self::UnknownSourceBits(bits) => {
                write!(f, "unknown source bits 0b{bits:b}")
            }
            Self::EmptyQueryText => {
                f.write_str("locate text must contain at least one alphanumeric token")
            }
        }
    }
}

impl std::error::Error for CoreError {}

/// Failures while parsing a `.manifold` fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FixtureError {
    /// A single line could not be parsed. `line` is 1-based.
    Line { line: usize, message: String },
    /// The fixture never declared a viewport.
    MissingViewport,
    /// More than one viewport directive.
    DuplicateViewport { line: usize },
    /// The same region id appeared twice.
    DuplicateRegion { line: usize, id: String },
    /// `write_fixture` cannot emit a viewport whose origin is not `(0, 0)`.
    UnsupportedViewportOrigin,
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Line { line, message } => write!(f, "fixture line {line}: {message}"),
            Self::MissingViewport => f.write_str("fixture is missing a viewport directive"),
            Self::DuplicateViewport { line } => {
                write!(f, "fixture line {line}: duplicate viewport")
            }
            Self::DuplicateRegion { line, id } => {
                write!(f, "fixture line {line}: duplicate region id `{id}`")
            }
            Self::UnsupportedViewportOrigin => {
                f.write_str("fixture format cannot represent a viewport whose origin is not (0, 0)")
            }
        }
    }
}

impl std::error::Error for FixtureError {}
