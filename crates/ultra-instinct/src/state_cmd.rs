//! `run --load-state <path>` / `--save-state <path>`: reuse a saved login.
//!
//! The state loads before the first observe: cookies and the current origin's
//! storage first, then (with `--url`) the target origin's storage once the
//! page has navigated there. The state saves when the run ends, successful
//! or not. Output names counts only, never cookie or storage values.

use std::path::Path;

use ultra_instinct_browser::storage_state::{
    apply_origin_storage, load_state, read_state_file, save_state, write_state_file,
    BorrowedTransport, StateError, StorageState,
};
use ultra_instinct_browser::{BrowserSession, CdpTransport};

use crate::CliError;

/// Page-side failures are `Browser`; file, format, and permission failures
/// are `Config` (their message already names the file).
fn state_err(path: &str, err: StateError) -> CliError {
    match err {
        StateError::Browser(err) => CliError::Browser(format!("state {path}: {err}")),
        other => CliError::Config(other.to_string()),
    }
}

/// Read `path` and apply it to the page the transport drives now.
pub(crate) fn load<T: CdpTransport>(
    transport: &mut T,
    path: &str,
) -> Result<(StorageState, String), CliError> {
    let state = read_state_file(Path::new(path)).map_err(|e| state_err(path, e))?;
    let mut session = BrowserSession::new(BorrowedTransport(transport));
    let report = load_state(&mut session, &state).map_err(|e| state_err(path, e))?;
    let line = format!(
        "state loaded {path}: {} cookies, storage {}\n",
        report.cookies,
        report.applied.as_deref().unwrap_or("pending navigation")
    );
    Ok((state, line))
}

/// After navigation: write the storage for the origin the page is on now.
pub(crate) fn after_navigation<T: CdpTransport>(
    transport: &mut T,
    path: &str,
    state: &StorageState,
) -> Result<String, CliError> {
    let mut session = BrowserSession::new(BorrowedTransport(transport));
    let applied = apply_origin_storage(&mut session, state).map_err(|e| state_err(path, e))?;
    Ok(applied
        .map(|origin| format!("state storage applied for {origin}\n"))
        .unwrap_or_default())
}

/// Save to `path` and fold the outcome into the run's result: a save
/// failure fails a successful run, and is appended to a failed run's text.
pub(crate) fn save_after<T: CdpTransport>(
    transport: &mut T,
    path: &str,
    result: Result<String, CliError>,
) -> Result<String, CliError> {
    let mut session = BrowserSession::new(BorrowedTransport(transport));
    let saved = save_state(&mut session)
        .and_then(|state| write_state_file(Path::new(path), &state).map(|()| state))
        .map(|state| {
            format!(
                "state saved {path}: {} cookies, {} origins\n",
                state.cookies.len(),
                state.origins.len()
            )
        })
        .map_err(|e| state_err(path, e));
    match (result, saved) {
        (Ok(out), Ok(line)) => Ok(out + &line),
        (Ok(_), Err(err)) => Err(err),
        (Err(CliError::Agent(out)), saved) => Err(CliError::Agent(match saved {
            Ok(line) => out + &line,
            Err(err) => format!("{out}state not saved: {err}\n"),
        })),
        (Err(other), _) => Err(other),
    }
}

/// Put the load lines in front of a run's output (also inside a failed
/// run's transcript).
pub(crate) fn with_prefix(
    prefix: &str,
    result: Result<String, CliError>,
) -> Result<String, CliError> {
    match result {
        Ok(out) => Ok(format!("{prefix}{out}")),
        Err(CliError::Agent(out)) => Err(CliError::Agent(format!("{prefix}{out}"))),
        Err(other) => Err(other),
    }
}
