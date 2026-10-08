// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Cross-origin isolation for hosts that cannot set headers (GitHub Pages).
//
// The viewer's rayon pool needs SharedArrayBuffer, which the browser only
// enables when the document is cross-origin isolated: COOP same-origin on
// the top-level page and COEP require-corp on that page and every ancestor
// of an embedded viewer. Pages cannot send those headers. This file is both
// the registration snippet and the worker. It rewrites responses to add the
// headers, then reloads once so the document that just loaded actually
// carries them.
//
// require-corp, not credentialless. credentialless is missing in Safari, so
// threads would never start there. require-corp blocks no-cors cross-origin
// responses unless they opt in with CORP. Google Fonts does, but a service
// worker that returns the original no-cors response turns it opaque and the
// opt-in is no longer visible. Cross-origin GETs are therefore refetched
// with CORS (fonts.googleapis.com and fonts.gstatic.com send
// Access-Control-Allow-Origin: *) and handed back as a real response.
//
// If registration fails, or the browser ignores the headers, the script
// stops after one reload. The viewer then stays on its sequential lane walk.
// A page that is already isolated (orbit-service, tools/site/serve.py) does
// not register a worker.

if (typeof window === "undefined") {
  self.addEventListener("install", () => self.skipWaiting());
  self.addEventListener("activate", (event) => {
    event.waitUntil(self.clients.claim());
  });

  self.addEventListener("fetch", (event) => {
    const req = event.request;
    if (req.cache === "only-if-cached" && req.mode !== "same-origin") {
      return;
    }
    const url = new URL(req.url);
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      return;
    }
    event.respondWith(isolate(req));
  });

  async function isolate(req) {
    const sameOrigin = new URL(req.url).origin === self.location.origin;
    let response;
    try {
      response = await load(req, sameOrigin);
    } catch (err) {
      console.error("coi-serviceworker fetch failed", err);
      return fetch(req);
    }
    if (response.type === "opaqueredirect" || response.type === "opaque" || response.status === 0) {
      return response;
    }
    if (response.status === 204 || response.status === 205 || response.status === 304) {
      return response;
    }
    const headers = new Headers(response.headers);
    // fetch() already decoded the body. Leaving these on makes the browser
    // decode it a second time.
    headers.delete("Content-Encoding");
    headers.delete("Content-Length");
    if (sameOrigin) {
      headers.set("Cross-Origin-Embedder-Policy", "require-corp");
      headers.set("Cross-Origin-Opener-Policy", "same-origin");
      if (!headers.has("Cross-Origin-Resource-Policy")) {
        headers.set("Cross-Origin-Resource-Policy", "same-origin");
      }
    } else if (!headers.has("Cross-Origin-Resource-Policy")) {
      headers.set("Cross-Origin-Resource-Policy", "cross-origin");
    }
    try {
      return new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers,
      });
    } catch (err) {
      console.error("coi-serviceworker rewrite failed", err);
      return response;
    }
  }

  function load(req, sameOrigin) {
    if (sameOrigin || (req.method !== "GET" && req.method !== "HEAD")) {
      return fetch(req);
    }
    return fetch(req.url, {
      method: req.method,
      mode: "cors",
      credentials: "omit",
      redirect: "follow",
    }).catch(() => fetch(req));
  }
} else {
  (function () {
    const KEY = "orbit-coi-reloads";
    let storage;
    try {
      storage = window.sessionStorage;
    } catch (err) {
      storage = null;
    }
    if (window.crossOriginIsolated || !navigator.serviceWorker) {
      if (storage) storage.removeItem(KEY);
      return;
    }
    const reloads = storage ? Number(storage.getItem(KEY) || "0") : 0;
    const controlled = !!navigator.serviceWorker.controller;
    if ((reloads >= 1 && controlled) || reloads >= 2) {
      return;
    }
    const scriptUrl = document.currentScript && document.currentScript.src;
    if (!scriptUrl) return;

    function reload() {
      if (storage) storage.setItem(KEY, String(reloads + 1));
      location.reload();
    }

    navigator.serviceWorker.register(scriptUrl).then((reg) => {
      // Already controlling, or this registration finished activating before
      // the callback ran. Either way the next document is the isolated one.
      if (navigator.serviceWorker.controller || (reg.active && !reg.installing)) {
        reload();
        return;
      }
      const worker = reg.installing || reg.waiting;
      if (!worker) {
        reload();
        return;
      }
      if (worker.state === "activated") {
        reload();
        return;
      }
      worker.addEventListener("statechange", () => {
        if (worker.state === "activated") reload();
      });
    }).catch((err) => {
      console.warn("COOP/COEP worker failed to register; viewer stays single-threaded", err);
    });
  })();
}
