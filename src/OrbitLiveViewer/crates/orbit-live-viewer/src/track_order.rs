// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A track order you can share as a text file.
//!
//! One pattern per line, matched against a process's name (`comm`), the
//! basename of its executable, and its full path; a pattern with a `/` in
//! it is tried against the path only. `*` matches any run of characters
//! and `?` one; matching ignores case. Blank lines and lines starting with
//! `#` are comments. Every process a line matches comes first on the rail,
//! in the order of the lines (the first matching line counts); everything
//! else keeps the viewer's own order behind them. The text lives in the
//! service's settings (`track_order`), so it holds across sessions, and it
//! loads from and saves to a plain file, which is what makes the layout of
//! a big trace something to hand to a colleague.

/// The parsed file: the patterns, in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    patterns: Vec<String>,
}

impl Rules {
    pub fn parse(text: &str) -> Rules {
        Rules {
            patterns: text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(|line| line.to_string())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn len(&self) -> usize {
        self.patterns.len()
    }

    /// The position of the first line matching one of a process's names,
    /// or `None` when the file says nothing about it. `names` is whatever
    /// the process is known by: comm, exe basename, full path.
    pub fn rank<'a>(&self, names: impl IntoIterator<Item = &'a str> + Clone) -> Option<usize> {
        self.patterns.iter().position(|pattern| {
            let path_only = pattern.contains('/');
            names.clone().into_iter().any(|name| {
                if path_only && !name.contains('/') {
                    return false;
                }
                glob_match(pattern, name)
            })
        })
    }
}

/// `*` any run, `?` one character, case-insensitive; no character classes.
/// Iterative with backtracking to the last `*`, so a long name and a
/// pattern with several stars cost their product, not an exponential.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// The viewer's side: the text as edited, its parsed rules, and a file
/// pick in flight.
pub struct State {
    pub text: String,
    pub rules: Rules,
    /// The text last sent to (or received from) the service, so a settings
    /// reply carrying what we already have does not clobber an edit.
    pub synced: String,
    pub message: String,
    pub import: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
}

impl Default for State {
    fn default() -> Self {
        State { text: String::new(), rules: Rules::default(), synced: String::new(), message: String::new(), import: None }
    }
}

impl State {
    /// Re-parses after an edit. Returns whether the rules changed.
    pub fn reparse(&mut self) -> bool {
        let rules = Rules::parse(&self.text);
        if rules != self.rules {
            self.rules = rules;
            true
        } else {
            false
        }
    }

    /// A settings reply from the service: adopt its text unless it is the
    /// one we last exchanged (an echo of our own PUT) or we are mid-edit.
    pub fn from_settings(&mut self, text: &str) {
        if text == self.synced {
            return;
        }
        self.synced = text.to_string();
        self.text = text.to_string();
        self.reparse();
    }

    pub fn receive(&mut self) {
        let result = self.import.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            self.import = None;
            match result {
                Ok(text) => {
                    if text.is_empty() {
                        return;
                    }
                    self.text = text;
                    self.reparse();
                    self.message.clear();
                }
                Err(error) => self.message = error,
            }
        }
    }
}

pub const FILENAME: &str = "track-order.txt";

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(inline_js = r#"
export function orbitPickTrackOrder() {
  return new Promise((resolve, reject) => {
    const input = document.createElement('input');
    input.type = 'file'; input.accept = '.txt,text/plain'; input.multiple = false;
    input.style.display = 'none'; document.body.appendChild(input);
    input.oncancel = () => { input.remove(); resolve(''); };
    input.onchange = async () => {
      try {
        const file = (input.files || [])[0];
        if (!file) { resolve(''); return; }
        if (file.size > 1024 * 1024) throw new Error('Track order file exceeds 1 MB');
        resolve(await file.text());
      } catch (e) { reject(String(e)); } finally { input.remove(); }
    };
    input.click();
  });
}
export function orbitSaveTrackOrder(name, text) {
  const url = URL.createObjectURL(new Blob([text], {type: 'text/plain'}));
  const a = document.createElement('a'); a.href = url; a.download = name;
  document.body.appendChild(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn orbitPickTrackOrder() -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;
    #[wasm_bindgen(catch)]
    fn orbitSaveTrackOrder(name: &str, text: &str) -> Result<(), wasm_bindgen::JsValue>;
}

/// Picks a file; the text arrives on the receiver (empty when cancelled).
pub fn open(ctx: egui::Context) -> std::sync::mpsc::Receiver<Result<String, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = match orbitPickTrackOrder().await {
            Ok(value) => Ok(value.as_string().unwrap_or_default()),
            Err(e) => Err(format!("Could not open the track order file: {e:?}")),
        };
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    #[cfg(not(target_arch = "wasm32"))]
    {
        let result = match rfd::FileDialog::new().add_filter("Track order", &["txt"]).pick_file() {
            Some(path) => std::fs::read_to_string(path).map_err(|e| e.to_string()),
            None => Ok(String::new()),
        };
        let _ = tx.send(result);
        ctx.request_repaint();
    }
    rx
}

pub fn save(text: &str) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    orbitSaveTrackOrder(FILENAME, text).map_err(|e| format!("Could not save the track order file: {e:?}"))?;
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = rfd::FileDialog::new().set_file_name(FILENAME).save_file() {
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_runs_single_characters_and_ignore_case() {
        assert!(glob_match("TP_ThirdPerson*", "tp_thirdperson-linux-shipping"));
        assert!(glob_match("*shipping", "TP_ThirdPerson-Linux-Shipping"));
        assert!(glob_match("orbit-serv?ce", "orbit-service"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
        assert!(!glob_match("orbit", "orbit-service"));
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
    }

    #[test]
    fn a_file_orders_matches_first_in_line_order_and_skips_comments() {
        let rules = Rules::parse("# the game first\nTP_ThirdPerson*\n\n  orbit-service  \n*/bin/python3*\n");
        assert_eq!(rules.len(), 3);
        assert_eq!(rules.rank(["TP_ThirdPerson-", "TP_ThirdPerson-Linux-Shipping", "/x/TP_ThirdPerson-Linux-Shipping"]), Some(0));
        assert_eq!(rules.rank(["orbit-service", "orbit-service", "/usr/bin/orbit-service"]), Some(1));
        // A pattern with a slash is tried against the path only: the comm
        // "python3" alone does not match it.
        assert_eq!(rules.rank(["python3", "python3", "/usr/bin/python3.12"]), Some(2));
        assert_eq!(rules.rank(["python3"]), None);
        assert_eq!(rules.rank(["bash", "bash", "/bin/bash"]), None);
    }

    #[test]
    fn the_first_matching_line_wins() {
        let rules = Rules::parse("*\norbit-service\n");
        assert_eq!(rules.rank(["orbit-service"]), Some(0));
    }

    #[test]
    fn a_settings_echo_does_not_clobber_an_edit() {
        let mut state = State::default();
        state.from_settings("a\n");
        assert_eq!(state.rules.len(), 1);
        state.text = "a\nb\n".into();
        assert!(state.reparse());
        // The service replies with what we last synced: the edit stays.
        state.from_settings("a\n");
        assert_eq!(state.text, "a\nb\n");
        // Something new from another browser is adopted.
        state.from_settings("c\n");
        assert_eq!(state.text, "c\n");
        assert_eq!(state.rules.len(), 1);
    }
}
