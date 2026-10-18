//! Rank four fixtures with `WeightedMatcher` and print one JSON document.
//!
//! This is fixture id and action agreement. It is not a Browser Use comparison.
//! Live System One choices run only with `--features jev` and `ULTRA_INSTINCT_JEV=1`.
//!
//! ```text
//! cargo run -p ultra-instinct --example fixture_compare
//! ```

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    match compare(&dir) {
        Ok(json) => {
            print!("{json}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("ultra-instinct: {err}");
            ExitCode::from(1)
        }
    }
}

fn compare(dir: &std::path::Path) -> Result<String, ultra_instinct::CompareError> {
    #[cfg(feature = "jev")]
    let report = ultra_instinct::fixture_compare_live(dir)?;
    #[cfg(not(feature = "jev"))]
    let report = ultra_instinct::fixture_compare(dir)?;
    Ok(report.render())
}
