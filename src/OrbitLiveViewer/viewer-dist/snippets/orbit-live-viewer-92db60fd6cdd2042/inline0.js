// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.


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
