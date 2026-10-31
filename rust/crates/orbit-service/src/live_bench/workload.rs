// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Real threads doing real work: integer math at a few call depths, sequential
//! and strided memory, a shared mutex, and yields. Sampling this is meaningful.
//! The 72-core *volume* is still synthesized; this process only has the host's
//! cores, and it is here so a kernel trace (when permitted) has something real
//! to see.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct Workload {
    stop: Arc<AtomicBool>,
    joins: Vec<JoinHandle<()>>,
    pub tids: Vec<i32>,
}

impl Workload {
    pub fn spawn(threads: u32) -> Self {
        let n = threads.max(1) as usize;
        let stop = Arc::new(AtomicBool::new(false));
        let tids = Arc::new(Mutex::new(Vec::with_capacity(n)));
        let lock = Arc::new(Mutex::new(0u64));
        let mut joins = Vec::with_capacity(n);
        for i in 0..n {
            let stop = Arc::clone(&stop);
            let tids = Arc::clone(&tids);
            let lock = Arc::clone(&lock);
            joins.push(thread::Builder::new().name(format!("orbit-load-{i}")).spawn(move || {
                tids.lock().unwrap().push(current_tid());
                let mut buf = vec![0u64; 8192];
                let mut x = 0x9E37_79B9_u64.wrapping_add(i as u64);
                let mut iter = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    x = depth_c(x, &mut buf);
                    if iter % 8 == 0 {
                        let mut g = lock.lock().unwrap();
                        *g = g.wrapping_add(x);
                        x ^= *g;
                    }
                    if iter % 32 == 0 {
                        std::thread::yield_now();
                    }
                    if iter % 128 == 0 {
                        thread::sleep(Duration::from_micros(30 + (i as u64 % 7) * 10));
                    }
                    iter = iter.wrapping_add(1);
                    std::hint::black_box(x);
                }
            }).expect("spawn workload thread"));
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let tids = loop {
            let got = tids.lock().unwrap().clone();
            if got.len() >= n || std::time::Instant::now() > deadline {
                break got;
            }
            thread::sleep(Duration::from_millis(1));
        };
        Self { stop, joins, tids }
    }
}

impl Drop for Workload {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for join in self.joins.drain(..) {
            let _ = join.join();
        }
    }
}

fn current_tid() -> i32 {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: gettid takes no pointer and cannot fail.
        unsafe { libc::gettid() }
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

#[inline(never)]
fn leaf_mix(x: u64) -> u64 {
    x.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17) ^ 0xD1B5_4A32_D192_ED03
}

#[inline(never)]
fn depth_a(x: u64, buf: &mut [u64]) -> u64 {
    let i = (x as usize) % buf.len();
    // Sequential neighbour plus a strided touch, so samples see both patterns.
    let j = (i.wrapping_mul(64) + 3) % buf.len();
    buf[i] = buf[i].wrapping_add(leaf_mix(x));
    buf[j] ^= buf[i].rotate_left(5);
    buf[i] ^ buf[j]
}

#[inline(never)]
fn depth_b(x: u64, buf: &mut [u64]) -> u64 {
    let y = depth_a(x ^ buf[(x as usize) % buf.len()], buf);
    depth_a(y.wrapping_add(buf.len() as u64), buf)
}

#[inline(never)]
fn depth_c(x: u64, buf: &mut [u64]) -> u64 {
    let y = depth_b(x.wrapping_add(0x100), buf);
    depth_b(y ^ 0x55, buf) ^ depth_a(y.rotate_left(9), buf)
}
