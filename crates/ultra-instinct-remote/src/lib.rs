//! Remote decision transports for `ultra_instinct_policy::RemotePolicy` and the HTTP
//! text model for `ultra_instinct_policy::ModelTextResolver`.
//!
//! Split out of `ultra-instinct` so model SDKs and the HTTP client live in one leaf
//! crate. Every module is behind a feature: `jev` (System One), `clef`
//! (Cloudflare Workers AI), `model-text` (HTTP text model). The closed remote
//! wire contract is `ultra_instinct_policy`'s; nothing here can widen a reply.
#![forbid(unsafe_code)]

#[cfg(feature = "clef")]
pub mod clef;
#[cfg(feature = "model-text")]
pub mod http_text;
#[cfg(all(feature = "jev", feature = "model-text"))]
pub mod second;
#[cfg(feature = "jev")]
pub mod typesafe;

/// The System One SDK, for callers that ask it directly (`ultra-instinct`'s live
/// fixture compare), so no other crate depends on it.
#[cfg(feature = "jev")]
pub use typesafe_sdk;
