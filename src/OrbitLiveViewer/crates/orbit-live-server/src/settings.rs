// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! User settings the service keeps on disk, so a choice made in the viewer
//! holds across browsers, sessions and restarts: `~/.config/orbit/
//! settings.json` (`$XDG_CONFIG_HOME/orbit/settings.json` when that is set;
//! the invoking user's home when the service runs under sudo, and the file
//! is handed back to that user). `GET /api/settings` reads it, `PUT` replaces
//! it and saves. Unknown keys in an older or newer file are kept as they
//! are; a missing key takes its default.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Calls a second past which a hooked function is switched off mid-capture,
/// by default. A function this hot is not something to time with a hook --
/// the sampling profile already shows it -- and hooked it costs the target
/// about a core (a microsecond of trampoline and record per call) for spans
/// no timeline can show. Ten thousand entries in any 100 ms window trips it.
pub const DEFAULT_MAX_HOOK_CALLS_PER_S: u64 = 100_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Switch off a hooked function that fires past `max_hook_calls_per_s`.
    #[serde(default = "default_true")]
    pub auto_unhook: bool,
    #[serde(default = "default_limit")]
    pub max_hook_calls_per_s: u64,
    /// Keys this build does not know, carried through a load/save cycle.
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}

fn default_limit() -> u64 {
    DEFAULT_MAX_HOOK_CALLS_PER_S
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto_unhook: true, max_hook_calls_per_s: DEFAULT_MAX_HOOK_CALLS_PER_S, other: Default::default() }
    }
}

impl Settings {
    /// The call-rate limit a capture applies: 0 when auto-unhook is off.
    pub fn effective_max_hook_calls_per_s(&self) -> u64 {
        if self.auto_unhook {
            self.max_hook_calls_per_s
        } else {
            0
        }
    }

    /// Where the file lives for this run.
    pub fn path() -> Option<PathBuf> {
        Some(config_dir()?.join("orbit").join("settings.json"))
    }

    pub fn load() -> Settings {
        Self::path().map(|p| Self::load_from(&p)).unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> Settings {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(settings) => settings,
                Err(error) => {
                    eprintln!("orbit-service: {} is not valid settings JSON ({error}); using defaults", path.display());
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) -> Result<PathBuf, String> {
        let path = Self::path().ok_or("no home directory to keep settings in")?;
        self.save_to(&path)?;
        Ok(path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            hand_back_to_invoker(dir);
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
        hand_back_to_invoker(path);
        Ok(())
    }
}

/// `$XDG_CONFIG_HOME`, else the home of the user who ran the service (the
/// sudo invoker when it runs privileged, so a setting made from the viewer
/// is the same file whether or not the service was started with sudo).
fn config_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(xdg));
    }
    invoker_home().map(|home| home.join(".config"))
}

fn invoker_home() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        if let Some(user) = std::env::var_os("SUDO_USER") {
            let user = user.to_string_lossy().into_owned();
            if let Ok(passwd) = std::fs::read_to_string("/etc/passwd") {
                for line in passwd.lines() {
                    let fields: Vec<&str> = line.split(':').collect();
                    if fields.len() >= 6 && fields[0] == user {
                        return Some(PathBuf::from(fields[5]));
                    }
                }
            }
        }
    }
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Under sudo the file would otherwise be root's, and the next unprivileged
/// service could read but not update it.
fn hand_back_to_invoker(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::chown;
        let uid = std::env::var("SUDO_UID").ok().and_then(|v| v.parse::<u32>().ok());
        let gid = std::env::var("SUDO_GID").ok().and_then(|v| v.parse::<u32>().ok());
        if let (Some(uid), Some(gid)) = (uid, gid) {
            let _ = chown(path, Some(uid), Some(gid));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_on_at_a_hundred_thousand() {
        let s = Settings::default();
        assert!(s.auto_unhook);
        assert_eq!(s.effective_max_hook_calls_per_s(), 100_000);
    }

    #[test]
    fn off_means_no_limit_whatever_the_value() {
        let s = Settings { auto_unhook: false, max_hook_calls_per_s: 5, ..Default::default() };
        assert_eq!(s.effective_max_hook_calls_per_s(), 0);
    }

    #[test]
    fn round_trips_through_a_file_and_keeps_unknown_keys() {
        let dir = std::env::temp_dir().join(format!("orbit-settings-test-{}", std::process::id()));
        let path = dir.join("orbit").join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Settings::load_from(&path), Settings::default(), "no file: defaults");
        let mut s = Settings { auto_unhook: false, max_hook_calls_per_s: 250_000, ..Default::default() };
        s.other.insert("future_knob".into(), serde_json::json!({"a": 1}));
        s.save_to(&path).unwrap();
        let back = Settings::load_from(&path);
        assert_eq!(back, s);
        assert_eq!(back.other["future_knob"]["a"], 1);
        // An older file that predates a key: the key takes its default.
        std::fs::write(&path, r#"{"max_hook_calls_per_s": 42}"#).unwrap();
        let older = Settings::load_from(&path);
        assert!(older.auto_unhook);
        assert_eq!(older.max_hook_calls_per_s, 42);
        // Garbage: defaults, not a crash.
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
