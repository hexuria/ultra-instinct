//! Property tests for the bipolar algebra. This is the chaos layer for the
//! encoder and bind. It is not a second implementation of ranking.

use proptest::prelude::*;
use ultra_instinct_hyper::{bind, bundle, cosine, Dims, Encoder, HyperError};

fn dims() -> impl Strategy<Value = Dims> {
    prop_oneof![
        Just(Dims::D512),
        Just(Dims::D1024),
        Just(Dims::D2048),
        Just(Dims::D4096),
    ]
}

fn symbol() -> impl Strategy<Value = String> {
    proptest::collection::vec(any::<char>(), 1..16).prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn encoder_is_deterministic_and_self_similarity_is_maximal(
        width in dims(),
        namespace in "[a-z][a-z0-9]{0,7}",
        left in "[A-Za-z0-9]{1,12}",
        right in "[A-Za-z0-9]{1,12}",
    ) {
        let encoder = Encoder::new(width);
        let once = encoder.encode(&namespace, &left).unwrap();
        let twice = encoder.encode(&namespace, &left).unwrap();
        prop_assert_eq!(&once, &twice);
        let bipolar = once.as_slice().iter().all(|v| *v == -1 || *v == 1);
        prop_assert!(bipolar);
        let self_sim = cosine(&once, &once).unwrap();
        prop_assert_eq!(self_sim, 1.0);
        let other = encoder.encode(&namespace, &right).unwrap();
        let cross = cosine(&once, &other).unwrap();
        prop_assert!(self_sim >= cross);
        let bound = bind(&once, &other).unwrap();
        prop_assert_eq!(&bind(&other, &once).unwrap(), &bound);
        prop_assert_eq!(&bind(&bound, &other).unwrap(), &once);
    }

    #[test]
    fn random_symbols_never_panic(namespace in symbol(), name in symbol()) {
        for width in [Dims::D512, Dims::D1024, Dims::D2048, Dims::D4096] {
            let encoder = Encoder::new(width);
            let encoded = encoder.encode(&namespace, &name);
            match encoded {
                Ok(vector) => {
                    prop_assert_eq!(cosine(&vector, &vector).unwrap(), 1.0);
                    let bipolar = vector.as_slice().iter().all(|component| *component == -1 || *component == 1);
                    prop_assert!(bipolar);
                }
                Err(HyperError::EmptySymbol) => prop_assert!(false, "strategy excludes empty"),
                Err(other) => prop_assert!(false, "unexpected {}", other),
            }
        }
    }

    #[test]
    fn empty_symbols_are_the_empty_error(namespace in "\\PC{0,8}", name in "\\PC{0,8}") {
        let encoder = Encoder::new(Dims::D512);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            encoder.encode(&namespace, &name)
        }));
        prop_assert!(result.is_ok(), "encode panicked");
        match result.unwrap() {
            Ok(vector) => {
                prop_assert!(!namespace.is_empty() && !name.is_empty());
                prop_assert_eq!(cosine(&vector, &vector).unwrap(), 1.0);
            }
            Err(HyperError::EmptySymbol) => {
                prop_assert!(namespace.is_empty() || name.is_empty());
            }
            Err(other) => prop_assert!(false, "unexpected {}", other),
        }
    }
}

#[test]
fn bundle_of_nothing_is_the_empty_error() {
    assert_eq!(bundle(&[]).unwrap_err(), HyperError::EmptyBundle);
}
