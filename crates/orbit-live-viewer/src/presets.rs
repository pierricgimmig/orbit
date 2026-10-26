// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Portable hook selections. Runtime IDs and installation paths never leave
//! the current process; imports resolve exact names against its symbol table.
use crate::net::FunctionHit;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Function {
    pub module: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub version: u32,
    pub name: String,
    pub functions: Vec<Function>,
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

impl Preset {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 16 * 1024 * 1024 {
            return Err("Preset exceeds 16 MB".into());
        }
        let mut preset: Self =
            serde_json::from_str(text).map_err(|e| format!("Invalid preset: {e}"))?;
        if preset.version != 1 {
            return Err(format!("Unsupported preset version {}", preset.version));
        }
        if preset.name.trim().is_empty() {
            return Err("Preset needs a name".into());
        }
        if preset.functions.len() > 100_000 {
            return Err("Preset exceeds 100,000 functions".into());
        }
        for f in &preset.functions {
            if f.module.is_empty() || f.module != basename(&f.module) || f.name.is_empty() {
                return Err("Each function needs a module filename (no directories) and an exact symbol name".into());
            }
        }
        preset.functions.sort();
        preset.functions.dedup();
        Ok(preset)
    }

    pub fn from_selection(
        name: &str,
        selected: &[FunctionHit],
        catalogue: &[FunctionHit],
    ) -> Result<Self, String> {
        if selected.is_empty() {
            return Err("Hook some functions before saving a preset".into());
        }
        let by_id: HashMap<_, _> = catalogue.iter().map(|f| (f.function_id, f)).collect();
        let functions: BTreeSet<_> = selected
            .iter()
            .map(|f| {
                let f = by_id.get(&f.function_id).copied().unwrap_or(f);
                Function {
                    module: basename(&f.module).into(),
                    name: f.name.clone(),
                }
            })
            .collect();
        let preset = Self {
            version: 1,
            name: name.trim().into(),
            functions: functions.into_iter().collect(),
        };
        // Validate exports too: a report row without a resolved module must not
        // silently produce an unusable preset.
        Self::parse(&serde_json::to_string(&preset).map_err(|e| e.to_string())?)
    }

    pub fn filename(&self) -> String {
        let stem: String = self
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        format!(
            "{}.orbit-preset.json",
            stem.chars().take(100).collect::<String>()
        )
    }
}

#[derive(Default)]
pub struct Report {
    pub name: String,
    pub matched: usize,
    pub added: usize,
    pub missing: Vec<Function>,
    pub ambiguous: Vec<Function>,
}

/// Union, by current-process ID. A name that resolves to multiple addresses
/// is not portable enough to choose safely; aliases of the same ID are fine.
pub fn apply(
    presets: &[Preset],
    catalogue: &[FunctionHit],
    selected: &mut Vec<FunctionHit>,
) -> Vec<Report> {
    let mut symbols: HashMap<(&str, &str), Vec<&FunctionHit>> = HashMap::new();
    for f in catalogue.iter().filter(|f| f.function_id != 0) {
        symbols
            .entry((basename(&f.module), &f.name))
            .or_default()
            .push(f);
    }
    let mut selected_ids: HashSet<_> = selected.iter().map(|f| f.function_id).collect();
    presets
        .iter()
        .map(|p| {
            let mut report = Report {
                name: p.name.clone(),
                ..Default::default()
            };
            for key in &p.functions {
                match symbols.get(&(key.module.as_str(), key.name.as_str())) {
                    None => report.missing.push(key.clone()),
                    Some(found) if found.iter().any(|f| f.function_id != found[0].function_id) => {
                        report.ambiguous.push(key.clone())
                    }
                    Some(found) => {
                        report.matched += 1;
                        if selected_ids.insert(found[0].function_id) {
                            selected.push(found[0].clone());
                            report.added += 1;
                        }
                    }
                }
            }
            report
        })
        .collect()
}

pub struct State {
    pub open: bool,
    pub name: String,
    pub loaded: Vec<Preset>,
    pub pending: bool,
    pub generation: u64,
    pub resolving: bool,
    pub reports: Vec<Report>,
    pub message: String,
    pub import: Option<std::sync::mpsc::Receiver<Result<Vec<Preset>, String>>>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            name: "instrumentation".into(),
            loaded: Vec::new(),
            pending: false,
            generation: 0,
            resolving: false,
            reports: Vec::new(),
            message: String::new(),
            import: None,
        }
    }
}
impl State {
    pub fn queue(&mut self) {
        self.generation += 1;
        self.pending = !self.loaded.is_empty();
        self.resolving = false;
        self.reports.clear();
    }
    pub fn receive(&mut self) {
        let result = self.import.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            self.import = None;
            match result {
                Ok(presets) => {
                    if presets.is_empty() {
                        return;
                    }
                    for p in presets {
                        if !self.loaded.contains(&p) {
                            self.loaded.push(p);
                        }
                    }
                    self.queue();
                    self.message.clear();
                }
                Err(error) => self.message = error,
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(inline_js = r#"
export function orbitPickPresets() {
  return new Promise((resolve, reject) => {
    const input = document.createElement('input');
    input.type = 'file'; input.accept = '.json'; input.multiple = true;
    input.style.display = 'none'; document.body.appendChild(input);
    input.oncancel = () => { input.remove(); resolve('[]'); };
    input.onchange = async () => {
      try {
        const files = Array.from(input.files || []);
        if (files.some(f => f.size > 16 * 1024 * 1024)) throw new Error('Preset exceeds 16 MB');
        resolve(JSON.stringify(await Promise.all(files.map(f => f.text()))));
      } catch (e) { reject(String(e)); } finally { input.remove(); }
    };
    input.click();
  });
}
export function orbitSavePreset(name, text) {
  const url = URL.createObjectURL(new Blob([text], {type: 'application/json'}));
  const a = document.createElement('a'); a.href = url; a.download = name;
  document.body.appendChild(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn orbitPickPresets() -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;
    #[wasm_bindgen(catch)]
    fn orbitSavePreset(name: &str, text: &str) -> Result<(), wasm_bindgen::JsValue>;
}

pub fn open(ctx: egui::Context) -> std::sync::mpsc::Receiver<Result<Vec<Preset>, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = match orbitPickPresets().await {
            Ok(value) => {
                serde_json::from_str::<Vec<String>>(&value.as_string().unwrap_or_default())
                    .map_err(|e| e.to_string())
                    .and_then(|files| files.iter().map(|f| Preset::parse(f)).collect())
            }
            Err(e) => Err(format!("Could not open presets: {e:?}")),
        };
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    #[cfg(not(target_arch = "wasm32"))]
    {
        let result = rfd::FileDialog::new()
            .add_filter("Orbit presets", &["json"])
            .pick_files()
            .unwrap_or_default()
            .iter()
            .map(|path| {
                let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                Preset::parse(&text)
            })
            .collect();
        let _ = tx.send(result);
        ctx.request_repaint();
    }
    rx
}

pub fn save(preset: &Preset) -> Result<(), String> {
    let text = serde_json::to_string_pretty(preset).map_err(|e| e.to_string())?;
    #[cfg(target_arch = "wasm32")]
    orbitSavePreset(&preset.filename(), &text)
        .map_err(|e| format!("Could not save preset: {e:?}"))?;
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = rfd::FileDialog::new()
        .set_file_name(preset.filename())
        .save_file()
    {
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hit(id: u64, module: &str, name: &str) -> FunctionHit {
        FunctionHit {
            function_id: id,
            module: module.into(),
            name: name.into(),
            size: 0,
            safety: String::new(),
            safety_reason: String::new(),
        }
    }
    #[test]
    fn portable_export_relocates_ids_paths_and_keeps_overloads() {
        let preset = Preset::from_selection(
            "physics",
            &[
                hit(1, "/home/alice/libPhysics.so", "step(int)"),
                hit(2, "C:\\alice\\Game.dll", "step(float)"),
            ],
            &[],
        )
        .unwrap();
        let json = serde_json::to_string(&preset).unwrap();
        assert!(!json.contains("alice") && !json.contains("function_id"));
        let p = Preset::parse(&json).unwrap();
        let mut selected = vec![];
        let reports = apply(
            &[p],
            &[
                hit(99, "/opt/bob/libPhysics.so", "step(int)"),
                hit(100, "D:\\bob\\Game.dll", "step(float)"),
            ],
            &mut selected,
        );
        assert_eq!(reports[0].matched, 2);
        assert_eq!(
            selected
                .iter()
                .map(|f| f.function_id)
                .collect::<HashSet<_>>(),
            HashSet::from([99, 100])
        );
    }
    #[test]
    fn composition_is_idempotent_and_preserves_manual_selection() {
        let catalogue = vec![
            hit(1, "Game", "physics"),
            hit(2, "Game", "shared"),
            hit(3, "Game", "rendering"),
            hit(4, "Game", "manual"),
        ];
        let a = Preset::from_selection("physics", &catalogue[..2], &[]).unwrap();
        let b = Preset::from_selection("rendering", &catalogue[1..3], &[]).unwrap();
        let mut selected = vec![catalogue[3].clone()];
        let reports = apply(&[a.clone(), b.clone()], &catalogue, &mut selected);
        assert_eq!(reports.iter().map(|r| r.added).sum::<usize>(), 3);
        assert_eq!(selected.len(), 4);
        assert_eq!(
            apply(&[a, b], &catalogue, &mut selected)
                .iter()
                .map(|r| r.added)
                .sum::<usize>(),
            0
        );
    }
    #[test]
    fn missing_and_ambiguous_names_never_choose_an_address() {
        let p = Preset::from_selection(
            "test",
            &[
                hit(1, "Game", "missing"),
                hit(2, "Game", "same"),
                hit(3, "Game", "work(int)"),
            ],
            &[],
        )
        .unwrap();
        let mut selected = vec![];
        let r = apply(
            &[p],
            &[
                hit(4, "Game", "same"),
                hit(5, "Game", "same"),
                hit(6, "Other", "work(int)"),
                hit(7, "Game", "work(float)"),
            ],
            &mut selected,
        );
        assert_eq!(r[0].missing.len(), 2);
        assert_eq!(r[0].ambiguous.len(), 1);
        assert!(selected.is_empty());
    }
    #[test]
    fn invalid_files_fail_before_mutation() {
        for json in [
            r#"{"version":2,"name":"x","functions":[]}"#,
            r#"{"version":1,"name":"x","functions":[{"module":"/path/Game","name":"work"}]}"#,
            r#"{"version":1,"name":"","functions":[]}"#,
            r#"{"version":1,"name":"x","functions":[],"pid":1}"#,
        ] {
            assert!(Preset::parse(json).is_err(), "{json}");
        }
    }
}
