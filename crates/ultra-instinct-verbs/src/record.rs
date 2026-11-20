//! Session recording: `aui open --record <file>` makes the session daemon
//! keep a frame of the page after each answered tool call.
//!
//! Frames are `browser_screenshot` calls with a `path`, so they go through
//! the same masking as `aui shot` (`capture_hidden`: once a secret was
//! typed, sensitive field values are hidden around the capture). They land
//! in `<file>.frames/` as `f_000001.png`, `f_000002.png`, … (`.jpg` when the
//! byte cap forces the JPEG fallback), at most one per [`FRAME_GAP`]. When
//! the daemon ends, `finish` writes an
//! `index.ffconcat` list and a `manifest.json` there, and when `ffmpeg` is
//! on `PATH` assembles them into `<file>`; without it the frame sequence
//! stands on its own.
//!
//! Recording is passive observation: it never changes what a verb answers,
//! and a failed frame is skipped, never an error.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Frames closer together than this collapse into one: a fast `batch` does
/// not turn into a slide show.
const FRAME_GAP: Duration = Duration::from_millis(400);

/// The tail a last frame holds when nothing follows it.
const LAST_FRAME_SECS: f64 = 1.0;

/// One session's recording.
pub(crate) struct Recorder {
    /// `<file>.frames/`: `f_000001.png`, `f_000002.png`, …
    dir: PathBuf,
    /// The playable file `finish` writes when `ffmpeg` runs.
    file: PathBuf,
    next: u32,
    last: Option<Instant>,
    /// (file name, unix millis) per frame, for the concat durations.
    frames: Vec<(String, u64)>,
}

impl Recorder {
    /// The frames directory for `file`: `<file>.frames/`, kept 0700 (frames
    /// are page content, secret fields already hidden).
    pub(crate) fn dir_of(file: &Path) -> PathBuf {
        PathBuf::from(format!("{}.frames", file.display()))
    }

    /// Create the frames directory ahead of the daemon (called by the verb
    /// that spawns it, so a bad path fails `aui open`, not the session).
    pub(crate) fn prepare(file: &Path) -> io::Result<()> {
        let dir = Self::dir_of(file);
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// Open the recording; makes the frames dir when `prepare` did not
    /// (a `daemon --record` run by hand), so frames always have a home.
    /// `file` is the playable target.
    pub(crate) fn open(file: &Path) -> io::Result<Self> {
        Self::prepare(file)?;
        Ok(Self {
            dir: Self::dir_of(file),
            file: file.to_path_buf(),
            next: 1,
            last: None,
            frames: Vec::new(),
        })
    }

    /// After a request the daemon answered: a successful `tools/call`
    /// other than `browser_screenshot` itself takes a frame, throttled.
    /// Best-effort: a failed or refused frame changes nothing.
    pub(crate) fn after_reply(
        &mut self,
        server: &mut ultra_instinct_mcp::Server,
        request: &Value,
        ok: bool,
    ) {
        if !ok || request["method"] != "tools/call" {
            return;
        }
        let name = request["params"]["name"].as_str().unwrap_or_default();
        if name.is_empty() || name == "browser_screenshot" {
            return;
        }
        if self.last.is_some_and(|at| at.elapsed() < FRAME_GAP) {
            return;
        }
        let Some(cdp) = request["params"]["arguments"]["cdp"].as_str() else {
            return;
        };
        let stem = format!("f_{:06}", self.next);
        let written = self.dir.join(format!("{stem}.png"));
        let args = json!({
            "path": written.display().to_string(),
            "cdp": cdp,
        });
        let Ok(body) = server.call_tool("browser_screenshot", &args) else {
            return;
        };
        // Over the byte cap the tool falls back to JPEG; name the frame for
        // what it holds so `ffmpeg` and image viewers read it right.
        let frame = match body["format"].as_str() {
            Some("jpeg") => {
                let jpg = self.dir.join(format!("{stem}.jpg"));
                match std::fs::rename(&written, &jpg) {
                    Ok(()) => format!("{stem}.jpg"),
                    Err(_) => format!("{stem}.png"),
                }
            }
            _ => format!("{stem}.png"),
        };
        self.next += 1;
        self.frames.push((frame, now_millis()));
        self.last = Some(Instant::now());
    }

    /// The recording is over: write the concat list and manifest, then try
    /// `ffmpeg` for the playable file. Never fails the daemon.
    pub(crate) fn finish(self) {
        let _ = std::fs::write(
            self.dir.join("index.ffconcat"),
            concat_list(&self.dir, &self.frames),
        );
        let video = if self.frames.is_empty() {
            "none"
        } else {
            self.encode()
        };
        let manifest = json!({
            "file": self.file.display().to_string(),
            "frames_dir": self.dir.display().to_string(),
            "frames": self.frames.len(),
            "video": video,
        });
        let _ = std::fs::write(
            self.dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        );
    }

    /// `ffmpeg -f concat` the frames into `self.file`; "ok", "ffmpeg-failed"
    /// or "no-ffmpeg" (not on `PATH`).
    fn encode(&self) -> &'static str {
        if let Some(parent) = self.file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let list = self.dir.join("index.ffconcat");
        // yuv420p needs even dimensions; an odd-sized viewport is scaled
        // down a pixel rather than failing the whole encode.
        let out = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
            ])
            .arg(&list)
            .args(["-vf", "scale='max(2,trunc(iw/2)*2)':'max(2,trunc(ih/2)*2)'"])
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg(&self.file)
            .output();
        match out {
            Ok(out) if out.status.success() => "ok",
            Ok(_) => "ffmpeg-failed",
            Err(_) => "no-ffmpeg",
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The ffmpeg concat-demuxer list for `frames`: each file held for the gap
/// to the next (the last for [`LAST_FRAME_SECS`], then repeated once, as the
/// demuxer wants).
fn concat_list(dir: &Path, frames: &[(String, u64)]) -> String {
    let mut out = String::from("ffconcat version 1.0\n");
    // `file '...'`: a `'` in the path is written `'\''` (ffconcat's escape).
    let quoted = |name: &str| dir.join(name).display().to_string().replace('\'', "'\\''");
    for (i, (name, at)) in frames.iter().enumerate() {
        let secs = match frames.get(i + 1) {
            Some((_, next)) => next.saturating_sub(*at) as f64 / 1000.0,
            None => LAST_FRAME_SECS,
        };
        out.push_str(&format!(
            "file '{}'\nduration {:.3}\n",
            quoted(name),
            secs.max(0.05)
        ));
    }
    if let Some((name, _)) = frames.last() {
        out.push_str(&format!("file '{}'\n", quoted(name)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frames_dir_is_the_file_plus_frames() {
        assert_eq!(
            Recorder::dir_of(Path::new("/tmp/demo.mp4")),
            PathBuf::from("/tmp/demo.mp4.frames")
        );
    }

    #[test]
    fn the_concat_list_holds_each_frame_until_the_next() {
        let dir = Path::new("/tmp/rec.mp4.frames");
        let frames = vec![
            ("f_000001.png".to_owned(), 1_000u64),
            ("f_000002.png".to_owned(), 1_500u64),
            ("f_000003.png".to_owned(), 3_000u64),
        ];
        let list = concat_list(dir, &frames);
        let frame = |name: &str| dir.join(name).display().to_string();
        assert!(list.starts_with("ffconcat version 1.0\n"), "{list}");
        assert!(list.contains(&format!(
            "file '{}'\nduration 0.500\n",
            frame("f_000001.png")
        )));
        assert!(list.contains(&format!(
            "file '{}'\nduration 1.500\n",
            frame("f_000002.png")
        )));
        assert!(list.contains(&format!(
            "file '{}'\nduration 1.000\n",
            frame("f_000003.png")
        )));
        // The demuxer wants the last file repeated with no duration.
        assert!(list
            .trim_end()
            .ends_with(&format!("file '{}'", frame("f_000003.png"))));
    }

    #[test]
    fn an_empty_recording_writes_no_concat_entries() {
        let list = concat_list(Path::new("/tmp/x.frames"), &[]);
        assert_eq!(list, "ffconcat version 1.0\n");
    }

    /// No answered call, no frames: the manifest still lands, `video` none.
    #[test]
    fn an_empty_recording_still_writes_its_manifest() {
        let dir = std::env::temp_dir().join(format!("aui-rec-{}", std::process::id()));
        let file = dir.join("none.mp4");
        Recorder::prepare(&file).unwrap();
        Recorder::open(&file).unwrap().finish();
        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(Recorder::dir_of(&file).join("manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["frames"], 0);
        assert_eq!(manifest["video"], "none");
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
