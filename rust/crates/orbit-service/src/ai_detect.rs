// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Zero-code detection of AI workloads.
//!
//! The core promise of Orbit's AI support is to learn as much as possible
//! about a process **without instrumenting it or changing its code**. The
//! cheapest, most reliable signal is what the process has already loaded and
//! opened: an ML framework shows up as a mapped shared library
//! (`libtorch`, `libtensorflow`, `jaxlib`, `onnxruntime`), and GPU use shows
//! up as the CUDA/ROCm runtime being mapped and the GPU device nodes being
//! open (`/dev/nvidia*`, `/dev/kfd`). None of that requires attaching to,
//! pausing, or modifying the target.
//!
//! This module reads `/proc/<pid>/maps` and `/proc/<pid>/fd` and classifies
//! the process. It is the foundation the rest of the AI work builds on:
//! knowing a process is "PyTorch + CUDA" lets Orbit offer to auto-hook the
//! framework's hot entry points ([`suggested_hooks`]) and label the process
//! in the viewer, still with no change to the target.

use std::collections::BTreeSet;

/// An ML framework recognised from a process's loaded libraries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Framework {
    PyTorch,
    TensorFlow,
    Jax,
    OnnxRuntime,
}

impl Framework {
    pub fn label(self) -> &'static str {
        match self {
            Framework::PyTorch => "PyTorch",
            Framework::TensorFlow => "TensorFlow",
            Framework::Jax => "JAX",
            Framework::OnnxRuntime => "ONNX Runtime",
        }
    }
}

/// The GPU compute stack a process has loaded, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuStack {
    /// CUDA runtime or driver (`libcudart`, `libcuda`, `libcudnn`, …).
    Cuda,
    /// AMD ROCm/HIP (`libamdhip64`, `librocm`, `libhsa`).
    Rocm,
}

/// What a process is, as far as its loaded modules and open devices tell us.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AiWorkload {
    pub frameworks: Vec<Framework>,
    /// A GPU compute library is mapped.
    pub gpu_stack: Option<GpuStack>,
    /// A GPU device node is open (`/dev/nvidia*` or `/dev/kfd`).
    pub gpu_device_open: bool,
}

impl AiWorkload {
    /// Is the process using a GPU? Either a compute library is loaded or a GPU
    /// device is open -- either is a strong, zero-code signal.
    pub fn uses_gpu(&self) -> bool {
        self.gpu_stack.is_some() || self.gpu_device_open
    }

    /// Anything AI-relevant at all.
    pub fn is_ai(&self) -> bool {
        !self.frameworks.is_empty() || self.uses_gpu()
    }

    /// A one-line summary for the log and the instrumentation status, e.g.
    /// `PyTorch + NVIDIA GPU (CUDA)`.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = self.frameworks.iter().map(|f| f.label().to_string()).collect();
        match self.gpu_stack {
            Some(GpuStack::Cuda) => parts.push("NVIDIA GPU (CUDA)".into()),
            Some(GpuStack::Rocm) => parts.push("AMD GPU (ROCm)".into()),
            None if self.gpu_device_open => parts.push("GPU device open".into()),
            None => {}
        }
        if parts.is_empty() {
            "no AI framework or GPU detected".into()
        } else {
            parts.join(" + ")
        }
    }
}

/// Classify a process from the paths of its loaded modules. Pure, so the
/// recognition rules are unit-tested without a live process.
pub fn classify_modules<S: AsRef<str>>(module_paths: &[S]) -> (Vec<Framework>, Option<GpuStack>) {
    let mut fws: BTreeSet<Framework> = BTreeSet::new();
    let mut cuda = false;
    let mut rocm = false;
    for p in module_paths {
        let p = p.as_ref();
        // Match on the file name so a directory called "torch" on the path
        // does not trigger; the signal is the library itself.
        let file = p.rsplit('/').next().unwrap_or(p);
        if file.contains("libtorch") || file.contains("libc10") || file.starts_with("_C.") && p.contains("/torch/") {
            fws.insert(Framework::PyTorch);
        }
        if file.contains("libtensorflow") || file.contains("_pywrap_tensorflow") {
            fws.insert(Framework::TensorFlow);
        }
        if file.contains("libjax") || file.contains("jaxlib") || file.contains("libxla") {
            fws.insert(Framework::Jax);
        }
        if file.contains("libonnxruntime") || file.contains("onnxruntime") {
            fws.insert(Framework::OnnxRuntime);
        }
        if file.contains("libcuda") || file.contains("libcudart") || file.contains("libcudnn")
            || file.contains("libcublas") || file.contains("libnvinfer")
        {
            cuda = true;
        }
        if file.contains("libamdhip") || file.contains("librocm") || file.contains("libhsa") {
            rocm = true;
        }
    }
    let gpu = if cuda {
        Some(GpuStack::Cuda)
    } else if rocm {
        Some(GpuStack::Rocm)
    } else {
        None
    };
    (fws.into_iter().collect(), gpu)
}

/// Detect a running process's AI workload from `/proc`, without touching it.
#[cfg(target_os = "linux")]
pub fn detect(pid: u32) -> AiWorkload {
    let modules = mapped_modules(pid);
    let refs: Vec<&str> = modules.iter().map(String::as_str).collect();
    let (frameworks, gpu_stack) = classify_modules(&refs);
    AiWorkload { frameworks, gpu_stack, gpu_device_open: gpu_device_open(pid) }
}

#[cfg(not(target_os = "linux"))]
pub fn detect(_pid: u32) -> AiWorkload {
    AiWorkload::default()
}

/// The distinct shared-library paths mapped into a process.
#[cfg(target_os = "linux")]
fn mapped_modules(pid: u32) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    if let Ok(maps) = std::fs::read_to_string(format!("/proc/{pid}/maps")) {
        for line in maps.lines() {
            // The path is the last field, present only for file-backed maps.
            if let Some(path) = line.split_whitespace().nth(5) {
                if path.starts_with('/') {
                    out.insert(path.to_string());
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Whether the process holds a GPU compute device open. `/dev/nvidia*` is
/// NVIDIA; `/dev/kfd` is AMD ROCm. `/dev/dri/*` is deliberately excluded --
/// any GL/display client opens it, so it is not a compute signal.
#[cfg(target_os = "linux")]
fn gpu_device_open(pid: u32) -> bool {
    let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return false;
    };
    for entry in fds.flatten() {
        if let Ok(target) = std::fs::read_link(entry.path()) {
            let t = target.to_string_lossy();
            if t.starts_with("/dev/nvidia") || t == "/dev/kfd" {
                return true;
            }
        }
    }
    false
}

/// Hot entry points Orbit would offer to hook automatically for a framework
/// -- native symbol-name substrings, matched against the symbol index. These
/// are where a training loop spends its host time, so hooking them turns a
/// zero-code capture into named scopes without the user picking anything.
pub fn suggested_hooks(framework: Framework) -> &'static [&'static str] {
    match framework {
        Framework::PyTorch => &[
            "at::native::",       // the ATen kernels
            "torch::autograd::",  // backward pass
            "c10::",              // dispatcher / tensor core
            "cudaLaunchKernel",   // host-side kernel launches
        ],
        Framework::TensorFlow => &[
            "tensorflow::OpKernel::Compute",
            "tensorflow::",
            "cudaLaunchKernel",
        ],
        Framework::Jax => &["xla::", "jax::", "cudaLaunchKernel"],
        Framework::OnnxRuntime => &["onnxruntime::", "Ort::"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_frameworks_and_gpu_from_module_paths() {
        let torch_cuda = [
            "/usr/lib/python3/dist-packages/torch/lib/libtorch_cpu.so",
            "/usr/lib/python3/dist-packages/torch/lib/libc10.so",
            "/usr/lib/x86_64-linux-gnu/libcudart.so.12",
            "/lib/x86_64-linux-gnu/libc.so.6",
        ];
        let (fw, gpu) = classify_modules(&torch_cuda);
        assert_eq!(fw, vec![Framework::PyTorch]);
        assert_eq!(gpu, Some(GpuStack::Cuda));

        let (fw, gpu) = classify_modules(&["/x/libtensorflow_framework.so.2", "/x/libcudnn.so.9"]);
        assert_eq!(fw, vec![Framework::TensorFlow]);
        assert_eq!(gpu, Some(GpuStack::Cuda));

        let (fw, gpu) = classify_modules(&["/opt/rocm/lib/libamdhip64.so"]);
        assert!(fw.is_empty());
        assert_eq!(gpu, Some(GpuStack::Rocm));

        // A plain process: nothing AI.
        let (fw, gpu) = classify_modules(&["/lib/x86_64-linux-gnu/libc.so.6", "/usr/bin/python3.14"]);
        assert!(fw.is_empty() && gpu.is_none());
    }

    #[test]
    fn summary_reads_naturally() {
        let w = AiWorkload {
            frameworks: vec![Framework::PyTorch],
            gpu_stack: Some(GpuStack::Cuda),
            gpu_device_open: true,
        };
        assert!(w.is_ai() && w.uses_gpu());
        assert_eq!(w.summary(), "PyTorch + NVIDIA GPU (CUDA)");
        assert_eq!(AiWorkload::default().summary(), "no AI framework or GPU detected");
        assert!(!AiWorkload::default().is_ai());
    }

    #[test]
    fn every_framework_suggests_at_least_one_hook() {
        for fw in [Framework::PyTorch, Framework::TensorFlow, Framework::Jax, Framework::OnnxRuntime] {
            assert!(!suggested_hooks(fw).is_empty(), "{} has no suggested hooks", fw.label());
        }
    }

    // --- End-to-end against real processes (Linux) ---------------------------

    /// A plain process is not an AI workload: no false positives from
    /// detecting a live pid's `/proc`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_plain_process_is_not_flagged() {
        use std::process::{Command, Stdio};
        let mut child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        // Give it a moment to map its libraries.
        std::thread::sleep(std::time::Duration::from_millis(150));
        let w = detect(child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert!(!w.is_ai(), "a plain `sleep` must not look like AI: {w:?}");
        assert!(!w.uses_gpu());
    }

    /// Zero-code GPU detection against a real GPU process. Spawns a Python that
    /// loads `libcuda` and calls `cuInit` (which opens `/dev/nvidia*`), then
    /// detects it purely from `/proc` -- no attach, no code in the target that
    /// Orbit put there. Skips where there is no usable NVIDIA GPU, so it is a
    /// no-op on CI without one and a real proof on a box with one.
    #[cfg(target_os = "linux")]
    #[test]
    fn detects_a_real_gpu_process_without_touching_it() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};

        if !std::path::Path::new("/dev/nvidia0").exists() {
            eprintln!("AI-DETECT TEST SKIPPED: no /dev/nvidia0 (no NVIDIA GPU)");
            return;
        }
        // Loads the driver, inits it (opens the device), prints "ready", waits.
        // Top-level statements only -- no indented blocks -- so the source
        // survives being a Rust literal. A missing libcuda raises and exits
        // non-zero (no "ready"), which the caller treats as "skip".
        let script = concat!(
            "import ctypes, sys, time\n",
            "cu = ctypes.CDLL('libcuda.so.1')\n",
            "sys.exit(3) if cu.cuInit(0) != 0 else None\n",
            "sys.stderr.write('ready\\n'); sys.stderr.flush()\n",
            "time.sleep(10)\n",
        );
        let mut child = match Command::new("python3")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                eprintln!("AI-DETECT TEST SKIPPED: no python3");
                return;
            }
        };
        // Wait for the child to signal the GPU is initialised, or bail if it
        // could not (no libcuda, no permission) -- either way, skip, do not
        // fail, since that is an environment limitation, not a bug.
        let ready = {
            let mut line = String::new();
            let stderr = child.stderr.take().expect("piped stderr");
            BufReader::new(stderr).read_line(&mut line).is_ok() && line.trim() == "ready"
        };
        if !ready {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("AI-DETECT TEST SKIPPED: libcuda/cuInit unavailable here");
            return;
        }

        let w = detect(child.id());
        let _ = child.kill();
        let _ = child.wait();

        assert!(w.uses_gpu(), "must detect GPU use zero-code: {w:?}");
        // libcuda is mapped and the device is open; both are the strong signals.
        assert_eq!(w.gpu_stack, Some(GpuStack::Cuda), "libcuda should classify as CUDA: {w:?}");
        assert!(w.gpu_device_open, "cuInit opens /dev/nvidia*: {w:?}");
    }
}
