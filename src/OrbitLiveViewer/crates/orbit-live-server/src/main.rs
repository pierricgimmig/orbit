//! Standalone live-viewer service (same HTTP stack OrbitService embeds).
//!
//! Useful when you only want the viewer + demo producer, or to exercise the
//! ring/stream without building the C++ capture stack.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use orbit_live_server::{http, LiveService, ServerConfig, DEFAULT_HTTP_PORT, DEFAULT_RING_BYTES};

fn parse_args() -> ServerConfig {
    let mut cfg = ServerConfig {
        bind: SocketAddr::from(([0, 0, 0, 0], DEFAULT_HTTP_PORT)),
        ring_buffer_bytes: DEFAULT_RING_BYTES,
        spill_path: None,
        dev_self_profile: orbit_live_server::env_dev_self(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--http-port" | "--http_port" => {
                let port: u16 = args
                    .next()
                    .expect("missing port")
                    .parse()
                    .expect("invalid port");
                cfg.bind.set_port(port);
            }
            "--ring-buffer-bytes" | "--ring_buffer_bytes" => {
                cfg.ring_buffer_bytes = parse_size(&args.next().expect("missing size"));
            }
            "--spill-path" | "--spill_path" => {
                let p = args.next().expect("missing path");
                if !p.is_empty() {
                    cfg.spill_path = Some(PathBuf::from(p));
                }
            }
            "--dev-self-profile" | "--dev_self_profile" => {
                cfg.dev_self_profile = true;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                print_help();
                std::process::exit(2);
            }
        }
    }
    cfg
}

fn parse_size(s: &str) -> u64 {
    let s = s.trim();
    let (num, mul) = if let Some(rest) = s.strip_suffix(['k', 'K']) {
        (rest, 1024u64)
    } else if let Some(rest) = s.strip_suffix(['m', 'M']) {
        (rest, 1024 * 1024)
    } else if let Some(rest) = s.strip_suffix(['g', 'G']) {
        (rest, 1024 * 1024 * 1024)
    } else {
        (s, 1)
    };
    num.parse::<u64>().expect("invalid size") * mul
}

fn print_help() {
    eprintln!(
        "orbit-live-service — WASM live viewer HTTP server\n\
         \n\
         --http-port PORT              listen port (default {DEFAULT_HTTP_PORT})\n\
         --ring-buffer-bytes N[kKmMgG] in-process ring size (default {DEFAULT_RING_BYTES})\n\
         --spill-path DIR              optional overflow serialize directory\n\
         --dev-self-profile            dogfood: emit viewer/service scopes into the ring\n\
         \n\
         Open http://127.0.0.1:<port>/ and click Demo, or ?dev=1 / Dev in the\n\
         transport bar to profile the viewer. ORBIT_LIVE_DEV=1 is the same as\n\
         --dev-self-profile. OrbitService can also register control hooks."
    );
}

#[tokio::main]
async fn main() {
    let cfg = parse_args();
    eprintln!(
        "Orbit live viewer on http://{}/  ring={} bytes  spill={:?}",
        cfg.bind, cfg.ring_buffer_bytes, cfg.spill_path
    );
    let service = LiveService::new(cfg).expect("create live service");
    let svc = Arc::clone(&service);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        crate::demo_stop_and_exit(&svc);
    });
    http::serve(service).await.expect("http serve");
}

fn demo_stop_and_exit(svc: &LiveService) {
    orbit_live_server::demo::stop(svc);
    std::process::exit(0);
}
