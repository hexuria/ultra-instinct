//! `ultra-instinct lessons --diary <dir> --store <path>` — distill the
//! battle diaries under `<dir>` into the versioned lesson store at
//! `<path>` (created when missing, merged when present).

use std::path::Path;

use ultra_instinct_dojo::{learn_diary, load_lessons, save_lessons};

use crate::CliError;

pub fn lessons_command(args: &[String]) -> Result<String, CliError> {
    let mut diary: Option<String> = None;
    let mut store_path: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let (name, value) = match arg.split_once('=') {
            Some((flag, v)) => (flag, Some(v.to_owned())),
            None => (arg.as_str(), None),
        };
        let slot = match name {
            "--diary" => &mut diary,
            "--store" => &mut store_path,
            _ => return Err(CliError::UnknownFlag(arg.clone())),
        };
        if slot.is_some() {
            return Err(CliError::DuplicateFlag(match name {
                "--diary" => "--diary",
                _ => "--store",
            }));
        }
        index += 1;
        let value = value
            .or_else(|| args.get(index).cloned())
            .ok_or(CliError::MissingValue(match name {
                "--diary" => "--diary",
                _ => "--store",
            }))?;
        *slot = Some(value);
        index += 1;
    }
    let diary = diary.ok_or(CliError::MissingValue("--diary"))?;
    let store_path = store_path.ok_or(CliError::MissingValue("--store"))?;

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

    let store_file = Path::new(&store_path);
    let mut store = load_lessons(store_file).map_err(|e| CliError::Io {
        path: store_path.clone(),
        message: e.to_string(),
    })?;
    for file in &files {
        learn_diary(&mut store, file).map_err(|e| CliError::Io {
            path: file.display().to_string(),
            message: e.to_string(),
        })?;
    }
    save_lessons(&store, store_file).map_err(|e| CliError::Io {
        path: store_path.clone(),
        message: e.to_string(),
    })?;

    let words: usize = store.words.values().map(Vec::len).sum();
    let moves: usize = store.moves.values().map(Vec::len).sum();
    let trust_pairs: usize = store.trust.values().map(|m| m.len()).sum();
    Ok(format!(
        "lessons {}: {} diaries learned — {} places, {} words, {} moves, {} trust pairs\n",
        store_path,
        files.len(),
        store.places.len(),
        words,
        moves,
        trust_pairs,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lessons_requires_diary_and_store() {
        assert!(matches!(
            lessons_command(&[]).unwrap_err(),
            CliError::MissingValue("--diary")
        ));
        assert!(matches!(
            lessons_command(&["--diary".to_owned(), "/tmp".to_owned()]).unwrap_err(),
            CliError::MissingValue("--store")
        ));
    }
}
