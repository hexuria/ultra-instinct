//! One JSONL file per run, written line-buffered so a crash keeps every
//! completed line. `read_diary` is the untrusted-input counterpart:
//! field-validated, schema-checked, used by the replay arena.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::DojoError;
use crate::line::{parse_line, DiaryLine};

/// Appends diary lines to `<dir>/<unix_ms>-<slug>.jsonl` (slug = first words
/// of the goal). Creates `<dir>` when missing. Every `write` ends with a
/// newline and a flush: a torn last line is the only loss a crash can cause.
pub struct DiaryWriter {
    writer: BufWriter<File>,
    path: PathBuf,
}

impl DiaryWriter {
    /// Create a new diary file inside `dir`.
    ///
    /// # Errors
    /// [`DojoError::Io`] when `dir` can't be created or the file can't be
    /// opened.
    pub fn create(dir: &Path, goal: &str) -> Result<Self, DojoError> {
        fs::create_dir_all(dir).map_err(|e| DojoError::Io {
            path: dir.to_path_buf(),
            message: format!("create diary dir: {e}"),
        })?;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let slug = slugify(goal);
        let mut path = dir.join(format!("{millis}-{slug}.jsonl"));
        // Two runs starting the same millisecond with the same goal must not
        // share a file — that would interleave two runs' lines.
        for n in 2u32.. {
            if !path.exists() {
                break;
            }
            path = dir.join(format!("{millis}-{slug}-{n}.jsonl"));
        }
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(|e| DojoError::Io {
                path: path.clone(),
                message: format!("create diary: {e}"),
            })?;
        Ok(Self {
            writer: BufWriter::new(file),
            path,
        })
    }

    /// Path of the diary file being written.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one line.
    ///
    /// # Errors
    /// [`DojoError::Io`] on write/flush failure.
    pub fn write(&mut self, line: &DiaryLine) -> Result<(), DojoError> {
        let json = line.to_json();
        self.writer
            .write_all(json.as_bytes())
            .and_then(|()| self.writer.write_all(b"\n"))
            .and_then(|()| self.writer.flush())
            .map_err(|e| DojoError::Io {
                path: self.path.clone(),
                message: format!("write diary: {e}"),
            })
    }
}

/// Read every line of a diary file.
///
/// # Errors
/// [`DojoError::Io`] when the file can't be read; [`DojoError::Parse`] /
/// [`DojoError::Schema`] on the first malformed line (a partially-written
/// final line fails the parse — the caller decides whether to tolerate it).
pub fn read_diary(path: &Path) -> Result<Vec<DiaryLine>, DojoError> {
    let text = fs::read_to_string(path).map_err(|e| DojoError::Io {
        path: path.to_path_buf(),
        message: format!("read diary: {e}"),
    })?;
    let mut lines = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        lines.push(parse_line(trimmed, index + 1)?);
    }
    Ok(lines)
}

fn slugify(goal: &str) -> String {
    let mut slug = String::with_capacity(24);
    for ch in goal.chars() {
        if slug.len() >= 24 {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "run".to_owned()
    } else {
        slug.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::{OutcomeLine, RunLine};

    #[test]
    fn writer_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("ultra-instinct-dojo-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut writer = DiaryWriter::create(&dir, "send a message").unwrap();
        writer
            .write(&DiaryLine::Run(RunLine {
                goal: "send a message".to_owned(),
                clauses: vec!["send a message".to_owned()],
                policy: "instinct".to_owned(),
            }))
            .unwrap();
        writer
            .write(&DiaryLine::Outcome(OutcomeLine {
                kind: "done".to_owned(),
                reason: "goal satisfied".to_owned(),
                steps: 1,
                policy_calls: 1,
                stale_discards: 0,
                duration_ms: 3,
            }))
            .unwrap();
        drop(writer);

        let files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
            .collect();
        assert_eq!(files.len(), 1);
        let lines = read_diary(&files[0].path()).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(matches!(&lines[0], DiaryLine::Run(r) if r.goal == "send a message"));
        assert!(matches!(&lines[1], DiaryLine::Outcome(o) if o.kind == "done"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_rejects_unknown_schema() {
        let err = parse_line(r#"{"schema":99,"type":"run"}"#, 1).unwrap_err();
        assert!(matches!(err, DojoError::Schema { found: 99 }));
    }

    #[test]
    fn parse_rejects_missing_field() {
        let err = parse_line(r#"{"schema":1,"type":"run","goal":"x"}"#, 1).unwrap_err();
        assert!(matches!(err, DojoError::Parse { line: 1, .. }));
    }
}
