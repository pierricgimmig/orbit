// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The service's log: what it printed to stderr all along, now also kept in
//! a file, the way the C++ OrbitService and the Qt UI did with `ORBIT_LOG`.
//!
//! Every line has the same shape on both sinks:
//!
//! ```text
//! [2026-09-26T14:03:22.123456] [INFO ] [serve.rs:773] 3 sampling ring(s) at 1000 Hz, unwinder ready
//! [2026-09-26T14:03:24.010775] [WARN ] [viewer 7c1e orbit_live_viewer::net] WebSocket closed
//! ```
//!
//! UTC time to the microsecond, the level, where the line came from, the
//! message. The origin is `file:line` for the service's own lines, the
//! helper's name for a child process's stderr, and `viewer <page> <module>`
//! for a line the browser viewer relayed through `POST /api/log`, so one
//! file tells the story of a session from both ends.
//!
//! The file lives in `~/.orbitprofiler/logs/` (the directory the Qt UI
//! used), named `orbit-service-<utc time>-<pid>.log`, and files older than a
//! week are removed when a new one opens. Under sudo the directory is the
//! invoking user's, and the file is chowned to them, so `./rust.sh --sudo`
//! leaves a log the user can read and delete. `--log-dir <dir>` or
//! `ORBIT_LOG_DIR` picks another directory. `ORBIT_LOG=debug` (or `trace`)
//! raises the level; the default is info.
//!
//! Lines logged before the file opens (argument parsing, a bind failure)
//! are held back and written first once it does.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FILE_PREFIX: &str = "orbit-service-";
const KEEP_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const BACKLOG_CAP: usize = 256;

struct Sink {
    file: Option<File>,
    backlog: Vec<String>,
    backlog_dropped: usize,
}

static SINK: Mutex<Sink> = Mutex::new(Sink { file: None, backlog: Vec::new(), backlog_dropped: 0 });

struct Logger;
static LOGGER: Logger = Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let origin = match (record.file(), record.line()) {
            (Some(file), Some(line)) => format!("{}:{line}", basename(file)),
            _ => record.target().to_string(),
        };
        let line = format_line(now_micros(), record.level(), &origin, &record.args().to_string());
        write_line(&line, true);
    }

    fn flush(&self) {}
}

/// Installs the logger (stderr until [`open_file`] adds the file) and a
/// panic hook that gets the panic into the log before the process aborts.
pub fn install() {
    let level = match std::env::var("ORBIT_LOG").as_deref() {
        Ok("trace") => log::LevelFilter::Trace,
        Ok("debug") => log::LevelFilter::Debug,
        _ => log::LevelFilter::Info,
    };
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let line = format_line(now_micros(), log::Level::Error, "panic", &info.to_string());
        // The default hook prints to stderr itself; this only adds the file.
        write_line(&line, false);
        previous(info);
    }));
}

/// Opens the log file and writes the held-back lines to it. `explicit`
/// wins over `--log-dir` on the command line, which wins over
/// `ORBIT_LOG_DIR`, which wins over `~/.orbitprofiler/logs`.
pub fn open_file(explicit: Option<&Path>) -> Result<PathBuf, String> {
    let dir = explicit.map(Path::to_path_buf).unwrap_or_else(default_dir);
    let owner = sudo_owner();
    create_dir_all_owned(&dir, owner)?;
    prune_old(&dir);

    let now = SystemTime::now();
    let path = dir.join(format!("{FILE_PREFIX}{}-{}.log", file_stamp(now), std::process::id()));
    let file = File::options()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    #[cfg(unix)]
    if let Some((uid, gid)) = owner {
        let _ = std::os::unix::fs::chown(&path, Some(uid), Some(gid));
    }

    let mut header = vec![format!(
        "orbit-service {} pid {} uid {} on {}; log {}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        uid(),
        hostname().unwrap_or_else(|| "?".into()),
        path.display()
    )];
    header.push(format!("command line: {}", std::env::args().collect::<Vec<_>>().join(" ")));
    if let Ok(cwd) = std::env::current_dir() {
        header.push(format!("working directory: {}", cwd.display()));
    }

    let mut sink = SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = file;
    for text in header {
        let _ = file.write_all(format_line(now_micros(), log::Level::Info, "logging.rs", &text).as_bytes());
        let _ = file.write_all(b"\n");
    }
    if sink.backlog_dropped > 0 {
        let note = format!("{} earlier line(s) were not kept for the file", sink.backlog_dropped);
        let _ = file.write_all(format_line(now_micros(), log::Level::Warn, "logging.rs", &note).as_bytes());
        let _ = file.write_all(b"\n");
    }
    for line in sink.backlog.drain(..) {
        let _ = file.write_all(line.as_bytes());
        let _ = file.write_all(b"\n");
    }
    sink.file = Some(file);
    Ok(path)
}

/// One line the browser viewer sent through `POST /api/log`: the viewer's
/// own clock (`Date.now()`) and module, tagged with a short id for the page
/// so two open tabs read apart. Warnings and errors also reach stderr, so
/// the terminal shows a viewer that lost its socket; info stays in the file.
pub fn relay(page: &str, t_ms: u64, level: log::Level, target: &str, message: &str) {
    let origin = format!("viewer {page} {target}");
    let line = format_line(t_ms.saturating_mul(1000), level, &origin, message);
    write_line(&line, level <= log::Level::Warn);
}

/// Copies a child process's stderr into the log, one line at a time, under
/// the child's name. What the helper prints still reaches the terminal.
pub fn relay_stderr<R: std::io::Read + Send + 'static>(reader: R, program: &'static str) {
    std::thread::Builder::new()
        .name(format!("{program}-stderr"))
        .spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(reader).lines() {
                let Ok(text) = line else { break };
                write_line(&format_line(now_micros(), log::Level::Info, program, &text), true);
            }
        })
        .ok();
}

fn write_line(line: &str, to_stderr: bool) {
    if to_stderr {
        let stderr = std::io::stderr();
        let mut stderr = stderr.lock();
        let _ = stderr.write_all(line.as_bytes());
        let _ = stderr.write_all(b"\n");
    }
    let mut sink = SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let sink = &mut *sink;
    match sink.file.as_mut() {
        Some(file) => {
            let mut bytes = Vec::with_capacity(line.len() + 1);
            bytes.extend_from_slice(line.as_bytes());
            bytes.push(b'\n');
            // One write per line and no buffering, so a crash loses nothing
            // that was logged before it.
            let _ = file.write_all(&bytes);
        }
        None if sink.backlog.len() < BACKLOG_CAP => sink.backlog.push(line.to_owned()),
        None => sink.backlog_dropped += 1,
    }
}

fn format_line(unix_micros: u64, level: log::Level, origin: &str, message: &str) -> String {
    format!("[{}] [{:<5}] [{origin}] {message}", timestamp(unix_micros), level.as_str())
}

fn basename(file: &str) -> &str {
    file.rsplit(['/', '\\']).next().unwrap_or(file)
}

fn now_micros() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0)
}

/// `2026-09-26T14:03:22.123456`, UTC.
fn timestamp(unix_micros: u64) -> String {
    let secs = unix_micros / 1_000_000;
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
        unix_micros % 1_000_000
    )
}

/// `2026_09_26_14_03_22`, UTC: the C++ log file name's stamp.
fn file_stamp(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!("{year:04}_{month:02}_{day:02}_{:02}_{:02}_{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `civil_from_days`), so the stamp needs no date crate.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn default_dir() -> PathBuf {
    if let Some(dir) = log_dir_from_args() {
        return dir;
    }
    if let Some(dir) = std::env::var_os("ORBIT_LOG_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(home) = home_dir() {
        return home.join(".orbitprofiler").join("logs");
    }
    std::env::temp_dir().join("orbit-logs")
}

/// `--log-dir <dir>` anywhere on the command line. Scanned here rather than
/// in the argument loop because `--serve` runs from inside that loop, so a
/// flag after it would otherwise be seen too late.
fn log_dir_from_args() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--log-dir" {
            return args.next().filter(|v| !v.is_empty()).map(PathBuf::from);
        }
    }
    None
}

fn home_dir() -> Option<PathBuf> {
    // Under sudo, the invoking user's home: that is where they will look,
    // and root's home is usually not theirs to read.
    if let Some(user) = std::env::var_os("SUDO_USER").filter(|v| !v.is_empty()) {
        if let Some(home) = passwd_home(&user.to_string_lossy()) {
            return Some(home);
        }
    }
    std::env::var_os("HOME").filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn passwd_home(user: &str) -> Option<PathBuf> {
    passwd_home_in(&std::fs::read_to_string("/etc/passwd").ok()?, user)
}

fn passwd_home_in(passwd: &str, user: &str) -> Option<PathBuf> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != user {
            return None;
        }
        fields.nth(4).filter(|home| !home.is_empty()).map(PathBuf::from)
    })
}

/// The invoking user's ids when running under sudo, to hand the log file
/// (and any directory this creates) to them.
fn sudo_owner() -> Option<(u32, u32)> {
    if uid() != 0 {
        return None;
    }
    let uid = std::env::var("SUDO_UID").ok()?.parse().ok()?;
    let gid = std::env::var("SUDO_GID").ok()?.parse().ok()?;
    Some((uid, gid))
}

#[cfg(unix)]
fn uid() -> u32 {
    unsafe { libc::geteuid() }
}
#[cfg(not(unix))]
fn uid() -> u32 {
    0
}

fn hostname() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `create_dir_all`, chowning each directory it had to create, so a first
/// run under sudo does not leave the user unable to log without it.
fn create_dir_all_owned(dir: &Path, owner: Option<(u32, u32)>) -> Result<(), String> {
    let mut missing = Vec::new();
    let mut probe = dir;
    while !probe.exists() {
        missing.push(probe.to_path_buf());
        match probe.parent() {
            Some(parent) => probe = parent,
            None => break,
        }
    }
    std::fs::create_dir_all(dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    #[cfg(unix)]
    if let Some((uid, gid)) = owner {
        for created in missing {
            let _ = std::os::unix::fs::chown(&created, Some(uid), Some(gid));
        }
    }
    #[cfg(not(unix))]
    let _ = (missing, owner);
    Ok(())
}

/// Removes this program's log files older than a week, like the Qt UI did.
fn prune_old(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(FILE_PREFIX) || !name.ends_with(".log") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else { continue };
        if now.duration_since(modified).map(|age| age > KEEP_FOR).unwrap_or(false) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_utc_iso_with_microseconds() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00.000000");
        assert_eq!(timestamp(1_600_000_000_123_456), "2020-09-13T12:26:40.123456");
        // A leap day, and the last second of a year.
        assert_eq!(timestamp(1_709_164_800_000_000), "2024-02-29T00:00:00.000000");
        assert_eq!(timestamp(1_767_225_599_000_000), "2025-12-31T23:59:59.000000");
        assert_eq!(file_stamp(UNIX_EPOCH + Duration::from_secs(1_600_000_000)), "2020_09_13_12_26_40");
    }

    #[test]
    fn lines_carry_time_level_and_origin() {
        assert_eq!(
            format_line(1_600_000_000_000_000, log::Level::Warn, "serve.rs:12", "hook not armed"),
            "[2020-09-13T12:26:40.000000] [WARN ] [serve.rs:12] hook not armed"
        );
        assert_eq!(basename("/home/x/rust/crates/orbit-service/src/serve.rs"), "serve.rs");
        assert_eq!(basename("C:\\src\\serve.rs"), "serve.rs");
    }

    #[test]
    fn passwd_lookup_finds_the_home_by_user_name() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                      pierric:x:1000:1000:Pierric,,,:/home/pierric:/bin/bash\n\
                      broken line\n";
        assert_eq!(passwd_home_in(passwd, "pierric"), Some(PathBuf::from("/home/pierric")));
        assert_eq!(passwd_home_in(passwd, "root"), Some(PathBuf::from("/root")));
        assert_eq!(passwd_home_in(passwd, "nobody"), None);
    }

    #[test]
    fn old_files_of_this_program_are_pruned_and_others_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("orbit-service-2020_01_01_00_00_00-1.log");
        let recent = dir.path().join("orbit-service-2026_01_01_00_00_00-2.log");
        let foreign = dir.path().join("Orbit-2020_01_01_00_00_00-3.log");
        for path in [&old, &recent, &foreign] {
            std::fs::write(path, "x").unwrap();
        }
        let long_ago = SystemTime::now() - KEEP_FOR - Duration::from_secs(3600);
        for path in [&old, &foreign] {
            File::options().write(true).open(path).unwrap().set_modified(long_ago).unwrap();
        }
        prune_old(dir.path());
        assert!(!old.exists(), "a week-old file of ours goes");
        assert!(recent.exists(), "a recent one stays");
        assert!(foreign.exists(), "someone else's file is not ours to remove");
    }

    #[test]
    fn missing_directories_are_created_down_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a").join("b").join("logs");
        create_dir_all_owned(&deep, None).unwrap();
        assert!(deep.is_dir());
        create_dir_all_owned(&deep, None).unwrap();
    }
}
