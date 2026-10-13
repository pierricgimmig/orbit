// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! OpenAI-compatible stand-in for screenshots and local demos.
//!
//! Prints a base URL, then waits. Point the service at it:
//!
//! ```text
//! ORBIT_AI_PROVIDER=ollama
//! ORBIT_AI_BASE_URL=<printed url>
//! ```
//!
//! The replies quote the capture summary the service sends. They are not a
//! model. `cargo run --example grounded_openai --features mock-server`.

fn main() {
    let server = orbit_live_insight::testing::spawn_grounded();
    println!("{}", server.base_url);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
