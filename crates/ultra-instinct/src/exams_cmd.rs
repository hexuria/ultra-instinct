//! `ultra-instinct exams --diary <dir> [--lessons <store>]` — the belt
//! exam report (issue #49, work item 6): remote policy calls per 100
//! recorded decisions (the dojo's progress metric, which must fall over
//! time) plus the belt each pairing and learned move holds.
//!
//! With `--lessons` the store is read from `<store>`; without it the
//! store is distilled from the diaries themselves, so an exam always has
//! something to grade. Deterministic and offline.

use std::collections::BTreeSet;
use std::path::Path;

use ultra_instinct_dojo::{examine, learn_diary, load_lessons, read_diary, LessonStore};

use crate::CliError;

pub fn exams_command(args: &[String]) -> Result<String, CliError> {
    let mut diary: Option<String> = None;
    let mut lessons_path: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let (name, value) = match arg.split_once('=') {
            Some((flag, v)) => (flag, Some(v.to_owned())),
            None => (arg.as_str(), None),
        };
        let slot = match name {
            "--diary" => &mut diary,
            "--lessons" => &mut lessons_path,
            _ => return Err(CliError::UnknownFlag(arg.clone())),
        };
        if slot.is_some() {
            return Err(CliError::DuplicateFlag(match name {
                "--diary" => "--diary",
                _ => "--lessons",
            }));
        }
        index += 1;
        let value = value
            .or_else(|| args.get(index).cloned())
            .ok_or(CliError::MissingValue(match name {
                "--diary" => "--diary",
                _ => "--lessons",
            }))?;
        *slot = Some(value);
        index += 1;
    }
    let diary = diary.ok_or(CliError::MissingValue("--diary"))?;

    let dir = Path::new(&diary);
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| CliError::Io {
            path: diary.clone(),
            message: e.to_string(),
        })?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension().and_then(|e| e.to_str()) == Some("jsonl")).then_some(path)
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(CliError::Io {
            path: diary.clone(),
            message: "no .jsonl diaries".to_owned(),
        });
    }

    // Diaries come first in file order; each is one run.
    let mut diaries = Vec::with_capacity(files.len());
    for file in &files {
        diaries.push(read_diary(file).map_err(|e| CliError::Io {
            path: file.display().to_string(),
            message: e.to_string(),
        })?);
    }

    let store = match lessons_path {
        Some(path) => load_lessons(Path::new(&path)).map_err(|e| CliError::Io {
            path,
            message: e.to_string(),
        })?,
        None => {
            let mut store = LessonStore::default();
            for file in &files {
                learn_diary(&mut store, file).map_err(|e| CliError::Io {
                    path: file.display().to_string(),
                    message: e.to_string(),
                })?;
            }
            store
        }
    };

    let report = examine(&diaries, &store);
    let mut out = String::new();
    out.push_str(&format!(
        "exams: {} decisions — {} remote calls ({} per 100)\n",
        report.decisions, report.remote_calls, report.remote_per_100
    ));
    let held: BTreeSet<&str> = report.belt_histogram.keys().copied().collect();
    let mut histogram = String::from("belts:");
    for name in ["white", "orange", "blue", "ultra"] {
        histogram.push_str(&format!(
            " {} {}",
            name,
            report.belt_histogram.get(name).copied().unwrap_or(0)
        ));
    }
    if held.is_empty() {
        histogram.push_str(" (no pairings yet)");
    }
    out.push_str(&histogram);
    out.push('\n');
    for (key, belts) in &report.move_belts {
        out.push_str(&format!("move {key}: {}\n", belts.join(", ")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exams_requires_a_diary_dir() {
        assert!(matches!(
            exams_command(&[]).unwrap_err(),
            CliError::MissingValue("--diary")
        ));
    }
}
