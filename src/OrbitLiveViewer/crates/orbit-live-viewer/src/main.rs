// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The live viewer as a native window: the same eframe app the browser
//! runs, talking to a running orbit-service over HTTP and a WebSocket.
//!
//!     orbit-live-viewer-native [--url http://127.0.0.1:44766]
//!     orbit-live-viewer-native --capture path/to/capture.orbit-stream
//!
//! The wasm pack remains the shipped UI; this binary exists so the app
//! compiles and runs natively too -- for development without a browser, for
//! profiling the viewer itself under native tools, and as the basis for a
//! desktop build should one be wanted (docs/native-viewer.md weighs that).

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    let mut url: Option<String> = None;
    let mut capture: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--url" => url = args.next(),
            "--capture" => capture = args.next(),
            "-h" | "--help" => {
                println!(
                    "orbit-live-viewer-native [--url http://host:port] [--capture FILE]\n\n  \
                     --url      the orbit-service to connect to (default {})\n  \
                     --capture  open a saved capture stream file instead of a service",
                    orbit_live_viewer::DEFAULT_SERVICE_URL
                );
                return Ok(());
            }
            other if other.starts_with("http://") => url = Some(other.to_string()),
            other => capture = Some(other.to_string()),
        }
    }
    if let Some(url) = &url {
        orbit_live_viewer::set_service_url(url);
    }
    if let Some(path) = capture {
        // The app reads this the way the web page reads `?capture=`.
        orbit_live_viewer::set_capture_path(&path);
    }
    let title = format!("Orbit live viewer -- {}", orbit_live_viewer::service_url());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1600.0, 1000.0]).with_title(title),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "orbit-live-viewer",
        options,
        Box::new(|cc| Ok(Box::new(orbit_live_viewer::OrbitLiveApp::new(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {}
