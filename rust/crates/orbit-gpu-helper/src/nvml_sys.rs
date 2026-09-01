// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A minimal runtime binding to `libnvidia-ml`.
//!
//! Loaded with `dlopen` rather than linked, so this helper runs on machines
//! with no NVIDIA driver at all: a missing library is `Nvml::load()` returning
//! None, not a binary that refuses to start. Only the handful of entry points
//! the telemetry sampler needs are bound.

use orbit_tracing_state::nvml::{DeviceSample, ProcessMemory};
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_uint, c_void};

const NVML_SUCCESS: c_int = 0;
const NVML_ERROR_INSUFFICIENT_SIZE: c_int = 7;
/// `NVML_TEMPERATURE_GPU`.
const TEMPERATURE_GPU: c_uint = 0;
/// `NVML_CLOCK_SM` / `NVML_CLOCK_MEM`.
const CLOCK_SM: c_uint = 1;
const CLOCK_MEM: c_uint = 2;

type Device = *mut c_void;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Memory {
    total: u64,
    free: u64,
    used: u64,
}

/// `nvmlMemory_v2_t`. v1's `used` includes driver-reserved memory, which is
/// why it reads several hundred MiB above what `nvidia-smi` shows; v2 breaks
/// the reservation out into its own field so `used` is the process-visible
/// figure users recognize. Preferred when the driver exposes it.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct MemoryV2 {
    version: c_uint,
    total: u64,
    reserved: u64,
    free: u64,
    used: u64,
}

/// `NVML_STRUCT_VERSION(Memory, 2)`: the struct size in the low bits, the
/// version in bits 24+.
const MEMORY_V2_VERSION: c_uint = (std::mem::size_of::<MemoryV2>() as c_uint) | (2 << 24);

/// `nvmlProcessInfo_v3_t`: pid, used memory, then the MIG instance ids.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct ProcessInfoV3 {
    pid: c_uint,
    used_gpu_memory: u64,
    gpu_instance_id: c_uint,
    compute_instance_id: c_uint,
}

pub struct Nvml {
    handle: *mut c_void,
    device_get_count: unsafe extern "C" fn(*mut c_uint) -> c_int,
    device_get_handle: unsafe extern "C" fn(c_uint, *mut Device) -> c_int,
    device_get_utilization: unsafe extern "C" fn(Device, *mut Utilization) -> c_int,
    device_get_memory: unsafe extern "C" fn(Device, *mut Memory) -> c_int,
    device_get_memory_v2: Option<unsafe extern "C" fn(Device, *mut MemoryV2) -> c_int>,
    device_get_temperature: unsafe extern "C" fn(Device, c_uint, *mut c_uint) -> c_int,
    device_get_power: unsafe extern "C" fn(Device, *mut c_uint) -> c_int,
    device_get_clock: unsafe extern "C" fn(Device, c_uint, *mut c_uint) -> c_int,
    device_get_processes: Option<unsafe extern "C" fn(Device, *mut c_uint, *mut ProcessInfoV3) -> c_int>,
    device_get_name: Option<unsafe extern "C" fn(Device, *mut c_char, c_uint) -> c_int>,
    system_get_driver_version: Option<unsafe extern "C" fn(*mut c_char, c_uint) -> c_int>,
    shutdown: unsafe extern "C" fn() -> c_int,
}

/// Resolves a symbol, returning None when absent so a driver too old for one
/// entry point degrades that metric instead of failing the whole helper.
unsafe fn symbol(handle: *mut c_void, name: &str) -> Option<*mut c_void> {
    let cname = CString::new(name).ok()?;
    let pointer = libc::dlsym(handle, cname.as_ptr() as *const c_char);
    if pointer.is_null() {
        None
    } else {
        Some(pointer)
    }
}

macro_rules! required {
    ($handle:expr, $name:literal, $ty:ty) => {
        match symbol($handle, $name) {
            Some(pointer) => std::mem::transmute::<*mut c_void, $ty>(pointer),
            None => {
                eprintln!("orbit-gpu-helper: libnvidia-ml has no {}", $name);
                libc::dlclose($handle);
                return None;
            }
        }
    };
}

impl Nvml {
    /// Loads and initializes NVML, or returns None when the library or the
    /// driver is not present.
    pub fn load() -> Option<Nvml> {
        // SAFETY: dlopen/dlsym on a well-known library; every symbol is
        // checked before use and the signatures match the NVML headers.
        unsafe {
            let name = CString::new("libnvidia-ml.so.1").ok()?;
            let mut handle = libc::dlopen(name.as_ptr(), libc::RTLD_NOW);
            if handle.is_null() {
                let fallback = CString::new("libnvidia-ml.so").ok()?;
                handle = libc::dlopen(fallback.as_ptr(), libc::RTLD_NOW);
            }
            if handle.is_null() {
                eprintln!("orbit-gpu-helper: libnvidia-ml not found; no GPU telemetry");
                return None;
            }

            let init = required!(handle, "nvmlInit_v2", unsafe extern "C" fn() -> c_int);
            if init() != NVML_SUCCESS {
                eprintln!("orbit-gpu-helper: nvmlInit failed (no NVIDIA driver?)");
                libc::dlclose(handle);
                return None;
            }

            let nvml = Nvml {
                device_get_count: required!(handle, "nvmlDeviceGetCount_v2", unsafe extern "C" fn(*mut c_uint) -> c_int),
                device_get_handle: required!(handle, "nvmlDeviceGetHandleByIndex_v2", unsafe extern "C" fn(c_uint, *mut Device) -> c_int),
                device_get_utilization: required!(handle, "nvmlDeviceGetUtilizationRates", unsafe extern "C" fn(Device, *mut Utilization) -> c_int),
                device_get_memory: required!(handle, "nvmlDeviceGetMemoryInfo", unsafe extern "C" fn(Device, *mut Memory) -> c_int),
                device_get_memory_v2: symbol(handle, "nvmlDeviceGetMemoryInfo_v2")
                    .map(|p| std::mem::transmute::<*mut c_void, unsafe extern "C" fn(Device, *mut MemoryV2) -> c_int>(p)),
                device_get_temperature: required!(handle, "nvmlDeviceGetTemperature", unsafe extern "C" fn(Device, c_uint, *mut c_uint) -> c_int),
                device_get_power: required!(handle, "nvmlDeviceGetPowerUsage", unsafe extern "C" fn(Device, *mut c_uint) -> c_int),
                device_get_clock: required!(handle, "nvmlDeviceGetClockInfo", unsafe extern "C" fn(Device, c_uint, *mut c_uint) -> c_int),
                // Optional: absent on older drivers, and only costs the
                // per-process attribution.
                device_get_processes: symbol(handle, "nvmlDeviceGetComputeRunningProcesses_v3")
                    .map(|p| std::mem::transmute::<*mut c_void, unsafe extern "C" fn(Device, *mut c_uint, *mut ProcessInfoV3) -> c_int>(p)),
                device_get_name: symbol(handle, "nvmlDeviceGetName")
                    .map(|p| std::mem::transmute::<*mut c_void, unsafe extern "C" fn(Device, *mut c_char, c_uint) -> c_int>(p)),
                system_get_driver_version: symbol(handle, "nvmlSystemGetDriverVersion")
                    .map(|p| std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_char, c_uint) -> c_int>(p)),
                shutdown: required!(handle, "nvmlShutdown", unsafe extern "C" fn() -> c_int),
                handle,
            };
            Some(nvml)
        }
    }

    pub fn device_count(&self) -> u32 {
        let mut count: c_uint = 0;
        // SAFETY: bound entry point, out param is a live local.
        if unsafe { (self.device_get_count)(&mut count) } == NVML_SUCCESS {
            count
        } else {
            0
        }
    }

    /// Reads every metric for one device. Each is independently optional:
    /// NVML answers NOT_SUPPORTED per metric on some cards, and the sampler
    /// renders those as gaps rather than zeros.
    pub fn sample_device(&self, index: u32, timestamp_ns: u64) -> Option<DeviceSample> {
        // SAFETY: all calls are bound entry points with live out params; the
        // device handle is checked before use.
        unsafe {
            let mut device: Device = std::ptr::null_mut();
            if (self.device_get_handle)(index, &mut device) != NVML_SUCCESS || device.is_null() {
                return None;
            }

            let mut utilization = Utilization::default();
            let utilization_ok = (self.device_get_utilization)(device, &mut utilization) == NVML_SUCCESS;

            // Prefer v2: its `used` excludes driver-reserved memory and so
            // agrees with nvidia-smi. Fall back to v1 on older drivers.
            let (memory_ok, memory_used, memory_total) = match self.device_get_memory_v2 {
                Some(get_v2) => {
                    let mut memory = MemoryV2 { version: MEMORY_V2_VERSION, ..MemoryV2::default() };
                    if get_v2(device, &mut memory) == NVML_SUCCESS {
                        (true, memory.used, memory.total)
                    } else {
                        let mut v1 = Memory::default();
                        let ok = (self.device_get_memory)(device, &mut v1) == NVML_SUCCESS;
                        (ok, v1.used, v1.total)
                    }
                }
                None => {
                    let mut v1 = Memory::default();
                    let ok = (self.device_get_memory)(device, &mut v1) == NVML_SUCCESS;
                    (ok, v1.used, v1.total)
                }
            };

            let mut temperature: c_uint = 0;
            let temperature_ok =
                (self.device_get_temperature)(device, TEMPERATURE_GPU, &mut temperature) == NVML_SUCCESS;

            let mut power: c_uint = 0;
            let power_ok = (self.device_get_power)(device, &mut power) == NVML_SUCCESS;

            let mut sm_clock: c_uint = 0;
            let sm_ok = (self.device_get_clock)(device, CLOCK_SM, &mut sm_clock) == NVML_SUCCESS;
            let mut memory_clock: c_uint = 0;
            let memory_clock_ok =
                (self.device_get_clock)(device, CLOCK_MEM, &mut memory_clock) == NVML_SUCCESS;

            Some(DeviceSample {
                device_index: index,
                timestamp_ns,
                gpu_utilization_percent: utilization_ok.then_some(utilization.gpu),
                memory_utilization_percent: utilization_ok.then_some(utilization.memory),
                memory_used_bytes: memory_ok.then_some(memory_used),
                memory_total_bytes: memory_ok.then_some(memory_total),
                temperature_celsius: temperature_ok.then_some(temperature),
                power_milliwatts: power_ok.then_some(power),
                sm_clock_mhz: sm_ok.then_some(sm_clock),
                memory_clock_mhz: memory_clock_ok.then_some(memory_clock),
                processes: self.compute_processes(device),
            })
        }
    }

    /// The processes holding memory on this device. NVML wants the array
    /// sized by a first call that reports INSUFFICIENT_SIZE.
    fn compute_processes(&self, device: Device) -> Vec<ProcessMemory> {
        let Some(get_processes) = self.device_get_processes else { return Vec::new() };
        // SAFETY: the count-then-fill protocol NVML documents; the buffer is
        // sized to the count NVML asks for.
        unsafe {
            let mut count: c_uint = 0;
            let status = get_processes(device, &mut count, std::ptr::null_mut());
            if status == NVML_SUCCESS || count == 0 {
                return Vec::new(); // no processes on the device
            }
            if status != NVML_ERROR_INSUFFICIENT_SIZE {
                return Vec::new();
            }
            let mut infos = vec![ProcessInfoV3::default(); count as usize];
            if get_processes(device, &mut count, infos.as_mut_ptr()) != NVML_SUCCESS {
                return Vec::new();
            }
            infos
                .iter()
                .take(count as usize)
                .map(|info| ProcessMemory {
                    pid: info.pid as i32,
                    used_bytes: info.used_gpu_memory,
                })
                .collect()
        }
    }
}

/// Static description of a device, for the capture's GpuInfo metadata.
pub struct DeviceInfo {
    pub name: Vec<u8>,
    pub vram_total_bytes: u64,
    pub driver_version: Vec<u8>,
}

impl Nvml {
    /// Model name, VRAM size and driver version for one device.
    pub fn device_info(&self, index: u32) -> Option<DeviceInfo> {
        // SAFETY: bound entry points; buffers are sized per the NVML headers
        // and the results are read back only up to the first NUL.
        unsafe {
            let mut device: Device = std::ptr::null_mut();
            if (self.device_get_handle)(index, &mut device) != NVML_SUCCESS || device.is_null() {
                return None;
            }
            let name = self
                .device_get_name
                .and_then(|get_name| {
                    let mut buffer = [0i8; 96]; // NVML_DEVICE_NAME_V2_BUFFER_SIZE
                    (get_name(device, buffer.as_mut_ptr(), buffer.len() as c_uint) == NVML_SUCCESS)
                        .then(|| cstr_bytes(&buffer))
                })
                .unwrap_or_default();
            let driver_version = self
                .system_get_driver_version
                .and_then(|get_version| {
                    let mut buffer = [0i8; 80];
                    (get_version(buffer.as_mut_ptr(), buffer.len() as c_uint) == NVML_SUCCESS)
                        .then(|| cstr_bytes(&buffer))
                })
                .unwrap_or_default();
            let vram_total_bytes = self
                .sample_device(index, 0)
                .and_then(|sample| sample.memory_total_bytes)
                .unwrap_or(0);
            Some(DeviceInfo { name, vram_total_bytes, driver_version })
        }
    }
}

/// The bytes of a NUL-terminated buffer, up to the terminator.
fn cstr_bytes(buffer: &[c_char]) -> Vec<u8> {
    buffer
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte as u8)
        .collect()
}

impl Drop for Nvml {
    fn drop(&mut self) {
        // SAFETY: shutdown then close the handle we opened.
        unsafe {
            (self.shutdown)();
            libc::dlclose(self.handle);
        }
    }
}
