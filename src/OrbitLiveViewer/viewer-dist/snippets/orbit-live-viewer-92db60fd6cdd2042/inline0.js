// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.


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
