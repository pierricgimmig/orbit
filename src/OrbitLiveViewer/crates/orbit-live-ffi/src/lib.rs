//! C ABI for embedding [`orbit_live_server`] in OrbitService.

use std::ffi::{c_char, c_int, CStr};
use std::net::SocketAddr;
use std::os::raw::c_void;
use std::path::PathBuf;
use std::ptr;
use std::slice;
use std::sync::{Arc, Mutex, OnceLock};

use orbit_live_server::{ControlHooks, LiveService, ServerConfig};

struct FfiState {
    // Held so the multi-thread runtime (and HTTP server) stay alive.
    #[allow(dead_code)]
    runtime: tokio::runtime::Runtime,
    service: Arc<LiveService>,
}

static STATE: OnceLock<Mutex<Option<FfiState>>> = OnceLock::new();

fn state() -> &'static Mutex<Option<FfiState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

#[repr(C)]
pub struct OrbitLiveServerConfig {
    pub http_port: u16,
    pub ring_buffer_bytes: u64,
    pub spill_path: *const c_char,
}

#[repr(C)]
pub struct OrbitLiveCallbacks {
    pub user_data: *mut c_void,
    pub list_processes_json: Option<
        unsafe extern "C" fn(user_data: *mut c_void, out: *mut c_char, out_len: usize) -> c_int,
    >,
    pub start_capture:
        Option<unsafe extern "C" fn(user_data: *mut c_void, json: *const c_char) -> c_int>,
    pub stop_capture: Option<unsafe extern "C" fn(user_data: *mut c_void) -> c_int>,
    pub load_symbols: Option<unsafe extern "C" fn(user_data: *mut c_void, pid: u32) -> c_int>,
    pub symbols_status_json: Option<
        unsafe extern "C" fn(user_data: *mut c_void, pid: u32, out: *mut c_char, out_len: usize) -> c_int,
    >,
    pub search_functions_json: Option<
        unsafe extern "C" fn(
            user_data: *mut c_void,
            pid: u32,
            query: *const c_char,
            limit: u32,
            out: *mut c_char,
            out_len: usize,
        ) -> c_int,
    >,
}

unsafe impl Send for OrbitLiveCallbacks {}
unsafe impl Sync for OrbitLiveCallbacks {}

fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

fn call_json_out(
    func: unsafe extern "C" fn(*mut c_void, *mut c_char, usize) -> c_int,
    user_data: usize,
    buf_len: usize,
    what: &str,
) -> Result<String, String> {
    let mut buf = vec![0u8; buf_len];
    let rc = unsafe { func(user_data as *mut c_void, buf.as_mut_ptr() as *mut c_char, buf.len()) };
    if rc != 0 {
        return Err(format!("{what} failed ({rc})"));
    }
    let s = unsafe { CStr::from_ptr(buf.as_ptr() as *const c_char) };
    Ok(s.to_string_lossy().into_owned())
}

/// Start the HTTP server on a background Tokio runtime. 0 on success.
#[no_mangle]
pub extern "C" fn orbit_live_server_start(config: *const OrbitLiveServerConfig) -> c_int {
    if config.is_null() {
        return -1;
    }
    let cfg = unsafe { &*config };
    let mut sc = ServerConfig::default();
    sc.bind = SocketAddr::from(([0, 0, 0, 0], cfg.http_port));
    sc.ring_buffer_bytes = if cfg.ring_buffer_bytes == 0 {
        orbit_live_server::DEFAULT_RING_BYTES
    } else {
        cfg.ring_buffer_bytes
    };
    sc.spill_path = cstr(cfg.spill_path).filter(|s| !s.is_empty()).map(PathBuf::from);

    let service = match LiveService::new(sc) {
        Ok(s) => s,
        Err(_) => return -2,
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("orbit-live")
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return -3,
    };
    let svc = Arc::clone(&service);
    runtime.spawn(async move {
        let _ = orbit_live_server::http::serve(svc).await;
    });
    let mut guard = state().lock().unwrap();
    *guard = Some(FfiState { runtime, service });
    0
}

#[no_mangle]
pub extern "C" fn orbit_live_server_stop() {
    let mut guard = state().lock().unwrap();
    *guard = None;
}

fn with_service<F: FnOnce(&LiveService)>(f: F) -> c_int {
    let guard = state().lock().unwrap();
    match guard.as_ref() {
        Some(s) => {
            f(&s.service);
            0
        }
        None => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn orbit_live_server_set_callbacks(cb: OrbitLiveCallbacks) -> c_int {
    let user_data = cb.user_data as usize;
    let list = cb.list_processes_json;
    let start = cb.start_capture;
    let stop = cb.stop_capture;
    let load = cb.load_symbols;
    let status = cb.symbols_status_json;
    let search = cb.search_functions_json;
    with_service(move |svc| {
        let hooks = ControlHooks {
            list_processes_json: Box::new(move || {
                if let Some(func) = list {
                    call_json_out(func, user_data, 1 << 20, "list_processes")
                } else {
                    Ok("[]".into())
                }
            }),
            start_capture: Box::new(move |json: &str| {
                if let Some(func) = start {
                    let c = std::ffi::CString::new(json).map_err(|e| e.to_string())?;
                    let rc = unsafe { func(user_data as *mut c_void, c.as_ptr()) };
                    if rc != 0 {
                        return Err(format!("start_capture failed ({rc})"));
                    }
                }
                Ok(())
            }),
            stop_capture: Box::new(move || {
                if let Some(func) = stop {
                    let rc = unsafe { func(user_data as *mut c_void) };
                    if rc != 0 {
                        return Err(format!("stop_capture failed ({rc})"));
                    }
                }
                Ok(())
            }),
            load_symbols: Box::new(move |pid| {
                if let Some(func) = load {
                    let rc = unsafe { func(user_data as *mut c_void, pid) };
                    if rc != 0 {
                        return Err(format!("load_symbols failed ({rc})"));
                    }
                }
                Ok(())
            }),
            symbols_status_json: Box::new(move |pid| {
                if let Some(func) = status {
                    let mut buf = vec![0u8; 4096];
                    let rc = unsafe {
                        func(
                            user_data as *mut c_void,
                            pid,
                            buf.as_mut_ptr() as *mut c_char,
                            buf.len(),
                        )
                    };
                    if rc != 0 {
                        return Err(format!("symbols_status failed ({rc})"));
                    }
                    let s = unsafe { CStr::from_ptr(buf.as_ptr() as *const c_char) };
                    Ok(s.to_string_lossy().into_owned())
                } else {
                    Ok(r#"{"pid":0,"status":"idle","function_count":0,"module_count":0,"error":""}"#.into())
                }
            }),
            search_functions_json: Box::new(move |pid, q, limit| {
                if let Some(func) = search {
                    let c = std::ffi::CString::new(q).map_err(|e| e.to_string())?;
                    let mut buf = vec![0u8; 1 << 16];
                    let rc = unsafe {
                        func(
                            user_data as *mut c_void,
                            pid,
                            c.as_ptr(),
                            limit,
                            buf.as_mut_ptr() as *mut c_char,
                            buf.len(),
                        )
                    };
                    if rc != 0 {
                        return Err(format!("search_functions failed ({rc})"));
                    }
                    let s = unsafe { CStr::from_ptr(buf.as_ptr() as *const c_char) };
                    Ok(s.to_string_lossy().into_owned())
                } else {
                    Ok(r#"{"pid":0,"status":"idle","functions":[]}"#.into())
                }
            }),
        };
        svc.set_hooks(hooks);
    })
}

#[no_mangle]
pub extern "C" fn orbit_live_intern_or_insert(text: *const c_char, len: u32) -> u32 {
    if text.is_null() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(text as *const u8, len as usize) };
    let s = std::str::from_utf8(bytes).unwrap_or("");
    let guard = state().lock().unwrap();
    match guard.as_ref() {
        Some(st) => st.service.intern_string(s),
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn orbit_live_ingest_api_scope_start(
    pid: u32,
    tid: u32,
    timestamp_ns: u64,
    color_rgba: u32,
    name_id: u32,
) {
    let _ = with_service(|svc| {
        svc.ingest_scope_start(pid, tid, timestamp_ns, color_rgba, name_id);
    });
}

#[no_mangle]
pub extern "C" fn orbit_live_ingest_api_scope_stop(pid: u32, tid: u32, timestamp_ns: u64) {
    let _ = with_service(|svc| {
        svc.ingest_scope_stop(pid, tid, timestamp_ns);
    });
}

#[no_mangle]
pub extern "C" fn orbit_live_ingest_function_call(
    pid: u32,
    tid: u32,
    name_id: u32,
    duration_ns: u64,
    end_timestamp_ns: u64,
    depth: i32,
) {
    let _ = with_service(|svc| {
        let ev = svc
            .pairer
            .lock()
            .function_call(pid, tid, name_id, duration_ns, end_timestamp_ns, depth);
        svc.push_event(ev);
    });
}

#[no_mangle]
pub extern "C" fn orbit_live_ingest_sample_stack(
    pid: u32,
    tid: u32,
    timestamp_ns: u64,
    duration_ns: u64,
    name_ids: *const u32,
    depth_count: u32,
) {
    if name_ids.is_null() || depth_count == 0 {
        return;
    }
    let ids = unsafe { slice::from_raw_parts(name_ids, depth_count as usize) };
    let _ = with_service(|svc| {
        let evs = svc
            .pairer
            .lock()
            .sample_stack(pid, tid, timestamp_ns, duration_ns, ids);
        svc.push_events(&evs);
    });
}

#[no_mangle]
pub extern "C" fn orbit_live_ingest_scheduling_slice(
    pid: u32,
    tid: u32,
    core: i32,
    duration_ns: u64,
    out_timestamp_ns: u64,
) {
    let _ = with_service(|svc| {
        let ev = svc
            .pairer
            .lock()
            .scheduling_slice(pid, tid, core, duration_ns, out_timestamp_ns);
        svc.push_event(ev);
    });
}

#[no_mangle]
pub extern "C" fn orbit_live_ingest_thread_state_slice(
    pid: u32,
    tid: u32,
    thread_state: u32,
    duration_ns: u64,
    end_timestamp_ns: u64,
) {
    let _ = with_service(|svc| {
        let ev = svc.pairer.lock().thread_state_slice(
            pid,
            tid,
            thread_state,
            duration_ns,
            end_timestamp_ns,
        );
        svc.push_event(ev);
    });
}

#[no_mangle]
pub extern "C" fn orbit_live_mark_capture_started(pid: u32, start_ns: u64) {
    let _ = with_service(|svc| svc.mark_capture_started(pid, start_ns));
}

#[no_mangle]
pub extern "C" fn orbit_live_mark_capture_finished() {
    let _ = with_service(|svc| svc.mark_capture_finished());
}

#[allow(dead_code)]
fn _ptr() -> *const c_void {
    ptr::null()
}
