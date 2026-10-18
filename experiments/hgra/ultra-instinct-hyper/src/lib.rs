//! Deterministic bipolar hypervectors.
//!
//! Every symbol is a vector in `{-1, +1}^d`. `d` is one of 512, 1024, 2048, or
//! 4096. The default is 2048. Bits come from a fixed FNV-1a seed expanded by
//! SplitMix64 over `namespace || symbol || version`. There is no learned
//! embedding and no process-random hasher.
//!
//! Algebra:
//! - **binding** is element-wise multiply, which is commutative on bipolar vectors
//! - **bundling** is a weighted sum followed by a sign. A zero component becomes
//!   `+1` (the documented tie-break)
//! - **permutation** is a left rotation used to encode a relation without a new
//!   random vector
//! - **similarity** is cosine. For bipolar vectors that is the normalized dot
//!   product `dot / dims`, in `[-1, 1]`. A vector compared with itself is `1`.

#![forbid(unsafe_code)]

use std::fmt;

/// Item-memory version mixed into every seed.
pub const ENCODER_VERSION: u32 = 1;

/// Allowed dimensionalities. Other widths cannot be constructed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Dims {
    D512 = 512,
    D1024 = 1024,
    D2048 = 2048,
    D4096 = 4096,
}

impl Dims {
    pub const DEFAULT: Self = Self::D2048;

    pub const fn get(self) -> usize {
        self as usize
    }

    pub fn try_from_usize(value: usize) -> Result<Self, HyperError> {
        match value {
            512 => Ok(Self::D512),
            1024 => Ok(Self::D1024),
            2048 => Ok(Self::D2048),
            4096 => Ok(Self::D4096),
            other => Err(HyperError::UnsupportedDims(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HyperError {
    UnsupportedDims(usize),
    EmptySymbol,
    DimMismatch { left: usize, right: usize },
    InvalidComponent { index: usize, value: i8 },
    EmptyBundle,
    NonFiniteWeight,
}

impl fmt::Display for HyperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedDims(n) => {
                write!(f, "dims {n} are not one of 512, 1024, 2048, 4096")
            }
            Self::EmptySymbol => f.write_str("namespace and symbol must not be empty"),
            Self::DimMismatch { left, right } => {
                write!(f, "hypervector dims {left} and {right} differ")
            }
            Self::InvalidComponent { index, value } => {
                write!(f, "component {index} is {value}, expected -1 or +1")
            }
            Self::EmptyBundle => f.write_str("cannot bundle an empty set of vectors"),
            Self::NonFiniteWeight => f.write_str("bundle weight must be finite"),
        }
    }
}

impl std::error::Error for HyperError {}

/// A dense bipolar vector. Components are private so a `0` cannot leak in.
#[derive(Clone, PartialEq, Eq)]
pub struct BipolarVector {
    dims: Dims,
    data: Vec<i8>,
}

impl BipolarVector {
    pub fn try_from_bipolar(data: Vec<i8>) -> Result<Self, HyperError> {
        let dims = Dims::try_from_usize(data.len())?;
        for (index, value) in data.iter().copied().enumerate() {
            if value != -1 && value != 1 {
                return Err(HyperError::InvalidComponent { index, value });
            }
        }
        Ok(Self { dims, data })
    }

    pub const fn dims(&self) -> Dims {
        self.dims
    }

    pub fn as_slice(&self) -> &[i8] {
        &self.data
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl fmt::Debug for BipolarVector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let preview: Vec<i8> = self.data.iter().copied().take(8).collect();
        f.debug_struct("BipolarVector")
            .field("dims", &self.dims.get())
            .field("head", &preview)
            .finish()
    }
}

/// Deterministic encoder for one dimensionality and item-memory version.
#[derive(Clone, Debug)]
pub struct Encoder {
    dims: Dims,
    version: u32,
}

impl Encoder {
    pub fn new(dims: Dims) -> Self {
        Self {
            dims,
            version: ENCODER_VERSION,
        }
    }

    pub fn with_version(dims: Dims, version: u32) -> Self {
        Self { dims, version }
    }

    pub const fn dims(&self) -> Dims {
        self.dims
    }

    pub const fn version(&self) -> u32 {
        self.version
    }

    /// Encode `namespace` and `symbol` at this encoder's version.
    pub fn encode(&self, namespace: &str, symbol: &str) -> Result<BipolarVector, HyperError> {
        if namespace.is_empty() || symbol.is_empty() {
            return Err(HyperError::EmptySymbol);
        }
        let mut state = seed(namespace, symbol, self.version);
        let mut data = vec![0i8; self.dims.get()];
        let mut filled = 0usize;
        while filled < data.len() {
            let bits = splitmix64(&mut state);
            for shift in 0..64 {
                if filled >= data.len() {
                    break;
                }
                let bit = (bits >> shift) & 1;
                data[filled] = if bit == 1 { 1 } else { -1 };
                filled += 1;
            }
        }
        debug_assert!(data.iter().all(|v| *v == -1 || *v == 1));
        Ok(BipolarVector {
            dims: self.dims,
            data,
        })
    }
}

fn seed(namespace: &str, symbol: &str, version: u32) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    fn mix(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    mix(&mut hash, namespace.as_bytes());
    mix(&mut hash, &[0x1f]);
    mix(&mut hash, symbol.as_bytes());
    mix(&mut hash, &[0x1f]);
    mix(&mut hash, &version.to_le_bytes());
    mix(&mut hash, &[0x1f]);
    // Frozen hash tag (pua-hv1 precedent): the seed is part of the format, not the brand.
    mix(&mut hash, b"hyper-use-hv1");
    if hash == 0 {
        0xA5A5_A5A5_A5A5_A5A5
    } else {
        hash
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Element-wise multiply. Commutative for bipolar values.
pub fn bind(left: &BipolarVector, right: &BipolarVector) -> Result<BipolarVector, HyperError> {
    if left.len() != right.len() {
        return Err(HyperError::DimMismatch {
            left: left.len(),
            right: right.len(),
        });
    }
    let data = left
        .as_slice()
        .iter()
        .zip(right.as_slice())
        .map(|(a, b)| a * b)
        .collect();
    Ok(BipolarVector {
        dims: left.dims,
        data,
    })
}

/// Weighted sum, then sign. A component that sums to exactly `0` becomes `+1`.
pub fn bundle(parts: &[(BipolarVector, f64)]) -> Result<BipolarVector, HyperError> {
    let Some((first, _)) = parts.first() else {
        return Err(HyperError::EmptyBundle);
    };
    let dims = first.dims;
    let width = first.len();
    let mut acc = vec![0.0f64; width];
    for (vector, weight) in parts {
        if vector.len() != width {
            return Err(HyperError::DimMismatch {
                left: width,
                right: vector.len(),
            });
        }
        if !weight.is_finite() {
            return Err(HyperError::NonFiniteWeight);
        }
        for (slot, component) in acc.iter_mut().zip(vector.as_slice()) {
            *slot += *weight * f64::from(*component);
        }
    }
    let data = acc
        .into_iter()
        .map(|sum| {
            if sum > 0.0 {
                1
            } else if sum < 0.0 {
                -1
            } else {
                1
            }
        })
        .collect();
    Ok(BipolarVector { dims, data })
}

/// Left-rotate by `shift` positions (`shift` is taken modulo the width).
/// `[a, b, c]` rotated by 1 becomes `[b, c, a]`.
pub fn permute(vector: &BipolarVector, shift: usize) -> BipolarVector {
    let width = vector.len();
    let shift = if width == 0 { 0 } else { shift % width };
    let mut data = vec![0i8; width];
    for (index, value) in vector.as_slice().iter().copied().enumerate() {
        data[(index + width - shift) % width] = value;
    }
    BipolarVector {
        dims: vector.dims,
        data,
    }
}

/// Non-zero rotation distance for a relation name, in `1..dims`.
pub fn relation_shift(relation: &str, dims: Dims) -> usize {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in relation.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let width = dims.get();
    if width <= 1 {
        return 0;
    }
    (hash as usize % (width - 1)) + 1
}

/// Cosine similarity. For bipolar vectors this is `dot(a, b) / dims`.
pub fn cosine(left: &BipolarVector, right: &BipolarVector) -> Result<f64, HyperError> {
    if left.len() != right.len() {
        return Err(HyperError::DimMismatch {
            left: left.len(),
            right: right.len(),
        });
    }
    let dot: i32 = left
        .as_slice()
        .iter()
        .zip(right.as_slice())
        .map(|(a, b)| i32::from(*a) * i32::from(*b))
        .sum();
    Ok(f64::from(dot) / left.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_dims_and_bad_components_are_errors() {
        assert_eq!(
            Dims::try_from_usize(256),
            Err(HyperError::UnsupportedDims(256))
        );
        assert_eq!(
            Dims::try_from_usize(256).unwrap_err().to_string(),
            "dims 256 are not one of 512, 1024, 2048, 4096"
        );
        let err = BipolarVector::try_from_bipolar(vec![0; 512]).unwrap_err();
        assert_eq!(err, HyperError::InvalidComponent { index: 0, value: 0 });
        assert_eq!(
            BipolarVector::try_from_bipolar(vec![2; 512]).unwrap_err(),
            HyperError::InvalidComponent { index: 0, value: 2 }
        );
        assert_eq!(Dims::try_from_usize(7), Err(HyperError::UnsupportedDims(7)));
    }

    #[test]
    fn empty_symbol_and_empty_bundle_are_errors() {
        let encoder = Encoder::new(Dims::D512);
        assert_eq!(encoder.encode("", "button"), Err(HyperError::EmptySymbol));
        assert_eq!(encoder.encode("role", ""), Err(HyperError::EmptySymbol));
        assert_eq!(
            encoder.encode("role", "").unwrap_err().to_string(),
            "namespace and symbol must not be empty"
        );
        assert_eq!(bundle(&[]), Err(HyperError::EmptyBundle));
    }

    #[test]
    fn encoder_is_deterministic_binding_commutes_and_self_similarity_is_max() {
        let symbols = [
            "button",
            "Settings",
            "left",
            "click",
            "parent",
            "near",
            "small",
            "navigation",
            "profile",
            "help",
        ];
        let namespaces = [
            "role", "label", "position", "action", "relation", "shape", "key",
        ];
        for dims in [Dims::D512, Dims::D1024, Dims::D2048] {
            let encoder = Encoder::new(dims);
            for namespace in namespaces {
                for symbol in symbols {
                    let once = encoder.encode(namespace, symbol).unwrap();
                    let twice = encoder.encode(namespace, symbol).unwrap();
                    assert_eq!(once, twice, "{namespace}:{symbol}");
                    assert!(once.as_slice().iter().all(|v| *v == -1 || *v == 1));
                    assert_eq!(cosine(&once, &once).unwrap(), 1.0);
                }
            }
            let left = encoder.encode("role", "button").unwrap();
            let right = encoder.encode("label", "settings").unwrap();
            assert_eq!(bind(&left, &right).unwrap(), bind(&right, &left).unwrap());
            let flipped =
                BipolarVector::try_from_bipolar(left.as_slice().iter().map(|v| -v).collect())
                    .unwrap();
            assert_eq!(cosine(&left, &flipped).unwrap(), -1.0);
        }
    }

    #[test]
    fn version_and_namespace_change_the_vector() {
        let a = Encoder::new(Dims::D512);
        let b = Encoder::with_version(Dims::D512, 2);
        assert_ne!(
            a.encode("role", "button").unwrap(),
            b.encode("role", "button").unwrap()
        );
        assert_ne!(
            a.encode("role", "button").unwrap(),
            a.encode("label", "button").unwrap()
        );
        assert_ne!(
            a.encode("role", "button").unwrap(),
            a.encode("role", "link").unwrap()
        );
    }

    #[test]
    fn bundle_tie_breaks_toward_positive_one() {
        let pos = BipolarVector::try_from_bipolar(vec![1; 512]).unwrap();
        let neg = BipolarVector::try_from_bipolar(vec![-1; 512]).unwrap();
        let bundled = bundle(&[(pos, 1.0), (neg, 1.0)]).unwrap();
        assert!(bundled.as_slice().iter().all(|v| *v == 1));
        assert_eq!(bundle(&[]).unwrap_err(), HyperError::EmptyBundle);
        let err = bundle(&[(
            BipolarVector::try_from_bipolar(vec![1; 512]).unwrap(),
            f64::NAN,
        )])
        .unwrap_err();
        assert_eq!(err, HyperError::NonFiniteWeight);
        assert_eq!(err.to_string(), "bundle weight must be finite");
    }

    #[test]
    fn permutation_is_a_rotation_and_not_the_identity() {
        let encoder = Encoder::new(Dims::D512);
        let vector = encoder.encode("role", "button").unwrap();
        let shifted = permute(&vector, 1);
        assert_ne!(shifted, vector);
        assert_eq!(permute(&vector, 0), vector);
        assert_eq!(permute(&vector, vector.len()), vector);
        assert_eq!(shifted.as_slice()[0], vector.as_slice()[1]);
        assert_eq!(shifted.as_slice()[vector.len() - 1], vector.as_slice()[0]);
        let shift = relation_shift("parent", Dims::D512);
        assert!((1..512).contains(&shift));
        assert_ne!(permute(&vector, shift), vector);
    }

    #[test]
    fn dim_mismatch_is_reported() {
        let a = BipolarVector::try_from_bipolar(vec![1; 512]).unwrap();
        let b = BipolarVector::try_from_bipolar(vec![1; 1024]).unwrap();
        assert!(matches!(
            bind(&a, &b),
            Err(HyperError::DimMismatch {
                left: 512,
                right: 1024
            })
        ));
        assert_eq!(
            cosine(&a, &b).unwrap_err(),
            HyperError::DimMismatch {
                left: 512,
                right: 1024
            }
        );
    }

    #[test]
    fn bind_is_commutative_and_its_own_inverse() {
        let encoder = Encoder::new(Dims::D512);
        let left = encoder.encode("role", "button").unwrap();
        let right = encoder.encode("label", "settings").unwrap();
        let bound = bind(&left, &right).unwrap();
        assert_eq!(bound, bind(&right, &left).unwrap());
        assert_eq!(bind(&bound, &right).unwrap(), left);
        assert_eq!(bind(&bound, &left).unwrap(), right);
        assert!(bind(&left, &left)
            .unwrap()
            .as_slice()
            .iter()
            .all(|v| *v == 1));
        // Quarter-weights are exact dyadics. Adding 0.25 twice matches one 0.5,
        // in either order. Neighbor coalescing in the ranker depends on this.
        let twice = bundle(&[(left.clone(), 0.25), (left.clone(), 0.25)]).unwrap();
        let once = bundle(&[(left.clone(), 0.5)]).unwrap();
        assert_eq!(twice, once);
        let flipped = bundle(&[(right.clone(), 0.25), (left.clone(), 0.25)]).unwrap();
        let flipped_back = bundle(&[(left.clone(), 0.25), (right, 0.25)]).unwrap();
        assert_eq!(flipped, flipped_back);
        let wide = Encoder::new(Dims::D4096);
        let once = wide.encode("role", "button").unwrap();
        assert_eq!(once, wide.encode("role", "button").unwrap());
        assert_eq!(cosine(&once, &once).unwrap(), 1.0);
    }

    #[test]
    fn property_binding_commutes_across_a_fixed_corpus() {
        let encoder = Encoder::new(Dims::D512);
        let corpus = ["a", "b", "c", "settings", "left", "button", "1", "near"];
        for left_symbol in corpus {
            for right_symbol in corpus {
                let left = encoder.encode("n", left_symbol).unwrap();
                let right = encoder.encode("m", right_symbol).unwrap();
                assert_eq!(bind(&left, &right).unwrap(), bind(&right, &left).unwrap());
                let bound = bind(&left, &right).unwrap();
                assert_eq!(cosine(&bound, &bound).unwrap(), 1.0);
                assert_eq!(encoder.encode("n", left_symbol).unwrap(), left);
            }
        }
    }
}
