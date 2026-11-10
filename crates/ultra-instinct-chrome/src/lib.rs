//! Find and start a local Chrome / Chromium with a DevTools port (std only).
//!
//! Search order: `ULTRA_INSTINCT_CHROME`, a Chrome for Testing install under
//! `~/.cache/ultra-instinct/chrome` (the `@puppeteer/browsers` layout), the
//! usual macOS / Linux / Windows install paths, then `PATH` names.
//! [`launch`] starts it on `--remote-debugging-port=0` and reads the port
//! Chrome writes to `<user-data-dir>/DevToolsActivePort`.
//!
//! Re-exported as `ultra_instinct_browser::launch`.

#![forbid(unsafe_code)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Env var naming the Chrome binary to use; it wins over every other path.
pub const CHROME_ENV: &str = "ULTRA_INSTINCT_CHROME";
/// `PATH` names tried last, in this order.
pub const CHROME_PATH_NAMES: [&str; 5] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "chrome",
];
/// Binaries inside a Chrome for Testing platform dir (`chrome-<platform>/`).
const CFT_BINARIES: [&str; 3] = [
    "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
    "chrome",
    "chrome.exe",
];

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LaunchError {
    /// `ULTRA_INSTINCT_CHROME` is set but is not a file.
    BadOverride(PathBuf),
    NotFound,
    Io(String),
    /// Chrome exited before writing `DevToolsActivePort`.
    Exited(String),
    Timeout(Duration),
    /// The profile's `SingletonLock` names a live Chrome (this pid).
    ProfileInUse(u32),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadOverride(path) => write!(f, "{CHROME_ENV}={} is not a file", path.display()),
            Self::NotFound => write!(
                f,
                "no Chrome or Chromium found; run `aui install` or set {CHROME_ENV}"
            ),
            Self::Io(message) => write!(f, "chrome launch: {message}"),
            Self::Exited(status) => write!(f, "chrome exited before DevTools was ready ({status})"),
            Self::Timeout(after) => {
                write!(f, "chrome wrote no DevToolsActivePort within {after:?}")
            }
            Self::ProfileInUse(pid) => {
                write!(f, "profile in use by a running Chrome (pid {pid})")
            }
        }
    }
}

impl std::error::Error for LaunchError {}

/// `~/.cache/ultra-instinct/chrome`, the Chrome for Testing install root.
pub fn chrome_cache_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".cache/ultra-instinct/chrome"))
}

/// Everyday user-data dirs of Chrome (stable / Beta / Canary or Dev),
/// Chromium, Brave and Edge under `$HOME` (`$XDG_CONFIG_HOME` on Linux).
/// A DevTools port must not open on one of these: it is the user's own
/// logged-in profile.
pub fn default_user_data_dirs() -> Vec<PathBuf> {
    let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    env("HOME").map_or_else(Vec::new, |home| {
        default_user_data_dirs_in(Path::new(&home), env("XDG_CONFIG_HOME").map(PathBuf::from))
    })
}

fn default_user_data_dirs_in(home: &Path, xdg_config: Option<PathBuf>) -> Vec<PathBuf> {
    let (base, names) = if cfg!(target_os = "macos") {
        let names = [
            "Google/Chrome",
            "Google/Chrome Beta",
            "Google/Chrome Canary",
        ];
        let more = ["Chromium", "BraveSoftware/Brave-Browser", "Microsoft Edge"];
        (home.join("Library/Application Support"), [names, more])
    } else {
        let names = [
            "google-chrome",
            "google-chrome-beta",
            "google-chrome-unstable",
        ];
        let more = ["chromium", "BraveSoftware/Brave-Browser", "microsoft-edge"];
        (
            xdg_config.unwrap_or_else(|| home.join(".config")),
            [names, more],
        )
    };
    names.iter().flatten().map(|name| base.join(name)).collect()
}

/// Whether `path` is one of [`default_user_data_dirs`]: canonical paths
/// when both exist, else the paths as written.
pub fn is_default_user_data_dir(path: &Path) -> bool {
    is_one_of(path, &default_user_data_dirs())
}

fn is_one_of(path: &Path, dirs: &[PathBuf]) -> bool {
    dirs.iter()
        .any(|dir| match (path.canonicalize(), dir.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => path == dir,
        })
}

/// The pid at the end of a `SingletonLock` link target, `<hostname>-<pid>`.
fn singleton_lock_pid(target: &str) -> Option<u32> {
    target
        .rsplit_once('-')?
        .1
        .parse()
        .ok()
        .filter(|pid| *pid != 0)
}

/// The pid of a live Chrome holding `profile` (its `SingletonLock` link
/// names a pid that `kill -0` reaches). A dangling or unparsable lock is
/// stale: `None`. Unix only; elsewhere always `None`.
fn profile_holder(profile: &Path) -> Option<u32> {
    let target = std::fs::read_link(profile.join("SingletonLock")).ok()?;
    let pid = singleton_lock_pid(target.to_str()?)?;
    let alive = cfg!(unix)
        && Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
    alive.then_some(pid)
}

/// Find a Chrome binary on this machine, in the module's search order.
pub fn find_chrome() -> Result<PathBuf, LaunchError> {
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let system = system_paths(env);
    find_chrome_in(
        std::env::var_os(CHROME_ENV).as_deref(),
        chrome_cache_dir().as_deref(),
        &system,
        std::env::var_os("PATH").as_deref(),
    )
}

/// The search behind [`find_chrome`] with every input passed in.
pub fn find_chrome_in(
    override_path: Option<&OsStr>,
    cache_root: Option<&Path>,
    system: &[PathBuf],
    path_var: Option<&OsStr>,
) -> Result<PathBuf, LaunchError> {
    if let Some(raw) = override_path.filter(|raw| !raw.is_empty()) {
        let path = PathBuf::from(raw);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(LaunchError::BadOverride(path))
        };
    }
    if let Some(found) = cache_root.and_then(find_in_cache) {
        return Ok(found);
    }
    if let Some(found) = system.iter().find(|path| path.is_file()) {
        return Ok(found.clone());
    }
    let dirs: Vec<PathBuf> = path_var
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    for name in CHROME_PATH_NAMES {
        for dir in &dirs {
            for file in [name.to_owned(), format!("{name}.exe")] {
                let candidate = dir.join(file);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Err(LaunchError::NotFound)
}

/// `<root>/chrome/<platform>-<version>/chrome-<platform>/<binary>`, newest version first.
fn find_in_cache(root: &Path) -> Option<PathBuf> {
    let mut versions = children(&root.join("chrome"));
    versions.sort_by_key(|dir| std::cmp::Reverse(version_key(dir)));
    versions
        .iter()
        .flat_map(|dir| children(dir))
        .find_map(|platform| {
            CFT_BINARIES
                .iter()
                .map(|binary| platform.join(binary))
                .find(|path| path.is_file())
        })
}

fn children(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// Digit runs of the dir name, so `131.0.1` sorts above `99.0.1`.
fn version_key(dir: &Path) -> Vec<u64> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.split(|c: char| !c.is_ascii_digit())
        .filter_map(|run| run.parse().ok())
        .collect()
}

fn system_paths(env: impl Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        let mut paths = vec![
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        ];
        if let Some(home) = env("HOME") {
            paths.push(home.join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"));
        }
        paths
    } else if cfg!(windows) {
        ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(|var| env(var))
            .map(|base| base.join("Google\\Chrome\\Application\\chrome.exe"))
            .collect()
    } else {
        [
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    }
}

/// `chrome --version`, e.g. `Google Chrome 131.0.6778.85`.
pub fn chrome_version(binary: &Path) -> Result<String, LaunchError> {
    let out = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map_err(|err| LaunchError::Io(err.to_string()))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if text.is_empty() {
        return Err(LaunchError::Exited(out.status.to_string()));
    }
    Ok(text)
}

/// The port on the first line of a `DevToolsActivePort` file.
pub fn parse_devtools_active_port(body: &str) -> Option<u16> {
    body.lines()
        .next()?
        .trim()
        .parse()
        .ok()
        .filter(|port| *port != 0)
}

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    /// Binary to start; `None` runs [`find_chrome`].
    pub binary: Option<PathBuf>,
    /// Show a window instead of `--headless=new`.
    pub headed: bool,
    /// `--user-data-dir`; `None` makes a fresh temp profile removed on drop.
    pub profile_dir: Option<PathBuf>,
    /// Leave Chrome running (and the temp profile on disk) when dropped.
    pub keep_alive: bool,
    /// How long to wait for `DevToolsActivePort`.
    pub timeout: Duration,
    /// `--window-size` as (width, height). `None`: [`DEFAULT_WINDOW`]
    /// headless; a maximized window headed.
    pub window_size: Option<(u32, u32)>,
}

/// The headless window when [`LaunchOptions::window_size`] is `None`.
pub const DEFAULT_WINDOW: (u32, u32) = (1440, 900);

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            binary: None,
            headed: false,
            profile_dir: None,
            keep_alive: false,
            timeout: Duration::from_secs(20),
            window_size: None,
        }
    }
}

/// The window flags `launch` passes for `options`.
fn window_args(options: &LaunchOptions) -> Vec<String> {
    match (options.window_size, options.headed) {
        (Some((width, height)), _) => vec![format!("--window-size={width},{height}")],
        (None, true) => vec!["--start-maximized".to_owned()],
        (None, false) => {
            let (width, height) = DEFAULT_WINDOW;
            vec![format!("--window-size={width},{height}")]
        }
    }
}

/// How long a drop waits for Chrome on a kept profile to quit after SIGTERM.
const QUIT_WAIT: Duration = Duration::from_secs(5);

/// A Chrome this process started. Dropping it ends Chrome unless
/// [`LaunchOptions::keep_alive`] was set: a temp profile is killed at once
/// and removed; a kept profile (`profile_dir`) is first asked to quit
/// (SIGTERM, unix) and given up to 5 s to flush cookies and storage, then
/// killed.
#[derive(Debug)]
pub struct ChromeProcess {
    child: Child,
    port: u16,
    binary: PathBuf,
    profile: PathBuf,
    temp_profile: bool,
    keep_alive: bool,
}

impl ChromeProcess {
    /// `http://127.0.0.1:<port>`, the value `--cdp` takes.
    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
    pub fn binary(&self) -> &Path {
        &self.binary
    }
    pub fn profile_dir(&self) -> &Path {
        &self.profile
    }
}

impl Drop for ChromeProcess {
    fn drop(&mut self) {
        if self.keep_alive {
            return;
        }
        if !self.temp_profile && cfg!(unix) {
            let asked = Command::new("kill")
                .args(["-TERM", &self.child.id().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            let started = Instant::now();
            while asked && started.elapsed() < QUIT_WAIT {
                if !matches!(self.child.try_wait(), Ok(None)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.temp_profile {
            // Chrome helpers may still be flushing; a second try usually lands.
            if std::fs::remove_dir_all(&self.profile).is_err() {
                std::thread::sleep(Duration::from_millis(200));
                let _ = std::fs::remove_dir_all(&self.profile);
            }
        }
    }
}

/// Start Chrome and wait until its DevTools port is known.
pub fn launch(options: &LaunchOptions) -> Result<ChromeProcess, LaunchError> {
    let binary = match &options.binary {
        Some(binary) => binary.clone(),
        None => find_chrome()?,
    };
    let io = |err: std::io::Error| LaunchError::Io(err.to_string());
    let (profile, temp_profile) = match &options.profile_dir {
        Some(dir) => (dir.clone(), false),
        None => {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let name = format!("ultra-instinct-chrome-{}-{nanos}", std::process::id());
            (std::env::temp_dir().join(name), true)
        }
    };
    std::fs::create_dir_all(&profile).map_err(io)?;
    // Another live Chrome on this profile would write the port file we read.
    if let Some(pid) = profile_holder(&profile) {
        return Err(LaunchError::ProfileInUse(pid));
    }
    let port_file = profile.join("DevToolsActivePort");
    // A profile reused after a crash still holds the old port.
    let _ = std::fs::remove_file(&port_file);
    let mut command = Command::new(&binary);
    if !options.headed {
        command.arg("--headless=new");
    }
    command
        .arg("--remote-debugging-port=0")
        .arg(format!("--user-data-dir={}", profile.display()))
        .args(window_args(options))
        .args([
            "--no-first-run",
            "--no-default-browser-check",
            "about:blank",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            if temp_profile {
                let _ = std::fs::remove_dir_all(&profile);
            }
            return Err(io(err));
        }
    };
    let mut process = ChromeProcess {
        child,
        port: 0,
        binary,
        profile,
        temp_profile,
        keep_alive: false,
    };
    let started = Instant::now();
    loop {
        if let Some(port) = std::fs::read_to_string(&port_file)
            .ok()
            .as_deref()
            .and_then(parse_devtools_active_port)
        {
            process.port = port;
            process.keep_alive = options.keep_alive;
            return Ok(process);
        }
        if let Ok(Some(status)) = process.child.try_wait() {
            return Err(LaunchError::Exited(status.to_string()));
        }
        if started.elapsed() >= options.timeout {
            return Err(LaunchError::Timeout(options.timeout));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ui-launch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
        path.to_path_buf()
    }

    #[test]
    fn parses_devtools_active_port() {
        assert_eq!(
            parse_devtools_active_port("53117\n/devtools/browser/abc\n"),
            Some(53117)
        );
        assert_eq!(parse_devtools_active_port(" 9222 \r\n"), Some(9222));
        assert_eq!(parse_devtools_active_port(""), None);
        assert_eq!(parse_devtools_active_port("0\n/devtools/browser/x"), None);
        assert_eq!(parse_devtools_active_port("port\n"), None);
        assert_eq!(parse_devtools_active_port("70000\n"), None);
    }

    #[test]
    fn search_order_is_override_cache_system_path() {
        let dir = scratch("order");
        let bin = dir.join("bin");
        let on_path = touch(&bin.join("chromium"));
        let path_var = std::env::join_paths([&bin]).unwrap();
        let system = touch(&dir.join("sys/Google Chrome"));
        let cache = dir.join("cache");
        let old = touch(&cache.join("chrome/linux-99.0.1/chrome-linux64/chrome"));
        let new = touch(&cache.join("chrome/linux-131.0.2/chrome-linux64/chrome"));
        let custom = touch(&dir.join("custom/chrome"));
        let systems = [dir.join("missing"), system.clone()];
        let find = |o: Option<&OsStr>, c: Option<&Path>, s: &[PathBuf]| {
            find_chrome_in(o, c, s, Some(&path_var))
        };
        assert_eq!(
            find(Some(custom.as_os_str()), Some(&cache), &systems),
            Ok(custom)
        );
        assert_eq!(find(None, Some(&cache), &systems), Ok(new.clone()));
        std::fs::remove_file(&new).unwrap();
        assert_eq!(find(None, Some(&cache), &systems), Ok(old));
        assert_eq!(find(None, None, &systems), Ok(system));
        assert_eq!(find(None, None, &[]), Ok(on_path));
        assert_eq!(find(Some(OsStr::new("")), None, &[]), find(None, None, &[]));
        let missing = dir.join("nope");
        assert_eq!(
            find(Some(missing.as_os_str()), Some(&cache), &systems),
            Err(LaunchError::BadOverride(missing))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_names_are_tried_in_order_and_dirs_ignored() {
        let dir = scratch("path");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(a.join("google-chrome")).unwrap();
        let chrome = touch(&a.join("chrome"));
        let chromium = touch(&b.join("chromium-browser"));
        let path_var = std::env::join_paths([&a, &b]).unwrap();
        assert_eq!(
            find_chrome_in(None, None, &[], Some(&path_var)),
            Ok(chromium)
        );
        std::fs::remove_dir_all(&b).unwrap();
        assert_eq!(find_chrome_in(None, None, &[], Some(&path_var)), Ok(chrome));
        let empty = std::env::join_paths([dir.join("none")]).unwrap();
        assert_eq!(
            find_chrome_in(None, None, &[], Some(&empty)),
            Err(LaunchError::NotFound)
        );
        assert_eq!(
            find_chrome_in(None, None, &[], None),
            Err(LaunchError::NotFound)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mac_chrome_for_testing_layout_is_found() {
        let dir = scratch("cft-mac");
        let app = touch(&dir.join(
            "chrome/mac_arm-131.0.6778.85/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        ));
        assert_eq!(find_chrome_in(None, Some(&dir), &[], None), Ok(app));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Headless: desktop size; headed: maximized; a given size wins.
    #[test]
    fn the_window_is_desktop_sized_headless_and_maximized_headed() {
        let headless = LaunchOptions::default();
        assert_eq!(window_args(&headless), ["--window-size=1440,900"]);
        let headed = LaunchOptions {
            headed: true,
            ..LaunchOptions::default()
        };
        assert_eq!(window_args(&headed), ["--start-maximized"]);
        for headed in [false, true] {
            let sized = LaunchOptions {
                headed,
                window_size: Some((390, 844)),
                ..LaunchOptions::default()
            };
            assert_eq!(window_args(&sized), ["--window-size=390,844"]);
        }
    }

    #[test]
    fn singleton_lock_targets_name_a_pid() {
        assert_eq!(singleton_lock_pid("my-mac.local-4242"), Some(4242));
        assert_eq!(singleton_lock_pid("host-7"), Some(7));
        for stale in ["", "host", "host-", "host-x12", "host-0"] {
            assert_eq!(singleton_lock_pid(stale), None, "{stale}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_profile_held_by_a_live_pid_refuses_and_a_stale_lock_does_not() {
        let dir = scratch("lock");
        let lock = dir.join("SingletonLock");
        assert_eq!(profile_holder(&dir), None);
        let me = std::process::id();
        std::os::unix::fs::symlink(format!("host-{me}"), &lock).unwrap();
        assert_eq!(profile_holder(&dir), Some(me));
        let options = LaunchOptions {
            binary: Some(PathBuf::from("/nonexistent/ultra-instinct-chrome")),
            profile_dir: Some(dir.clone()),
            ..LaunchOptions::default()
        };
        assert_eq!(launch(&options).unwrap_err(), LaunchError::ProfileInUse(me));
        std::fs::remove_file(&lock).unwrap();
        std::os::unix::fs::symlink("host-not-a-pid", &lock).unwrap();
        assert_eq!(profile_holder(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_user_data_dirs_are_found_lexically_or_through_links() {
        let dir = scratch("defaults");
        let home = dir.join("home");
        let dirs = default_user_data_dirs_in(&home, None);
        assert_eq!(dirs.len(), 6);
        let chrome = if cfg!(target_os = "macos") {
            home.join("Library/Application Support/Google/Chrome")
        } else {
            home.join(".config/google-chrome")
        };
        assert_eq!(dirs[0], chrome);
        assert!(is_one_of(&chrome, &dirs));
        assert!(is_one_of(
            Path::new(&format!("{}/", chrome.display())),
            &dirs
        ));
        assert!(!is_one_of(&dir.join("shop-profile"), &dirs));
        assert!(!is_one_of(&chrome.join("Default"), &dirs));
        if !cfg!(target_os = "macos") {
            let xdg = default_user_data_dirs_in(&home, Some(dir.join("xdg")));
            assert_eq!(xdg[3], dir.join("xdg/chromium"));
        }
        #[cfg(unix)]
        {
            std::fs::create_dir_all(&chrome).unwrap();
            let link = dir.join("link");
            std::os::unix::fs::symlink(&chrome, &link).unwrap();
            assert!(is_one_of(&link, &dirs));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_binary_is_an_io_error() {
        let options = LaunchOptions {
            binary: Some(PathBuf::from("/nonexistent/ultra-instinct-chrome")),
            ..LaunchOptions::default()
        };
        assert!(matches!(launch(&options), Err(LaunchError::Io(_))));
    }

    #[test]
    #[ignore = "starts a real Chrome"]
    fn live_launch_gives_an_endpoint_and_cleans_up() {
        let chrome = launch(&LaunchOptions::default()).expect("launch chrome");
        let profile = chrome.profile_dir().to_path_buf();
        assert!(chrome.endpoint().starts_with("http://127.0.0.1:"));
        let pages = ultra_instinct_cdp::page_targets(&chrome.endpoint()).expect("json/list");
        assert!(!pages.is_empty());
        drop(chrome);
        assert!(!profile.exists());
    }
}
