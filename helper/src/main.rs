// SPDX-License-Identifier: GPL-3.0-or-later
//! freerdp-webauthn-helper: answers FreeRDP rdpewa WebAuthn requests with a phone (hybrid) or
//! USB security key via libwebauthn's CTAP2 layer. See README.md.

mod hid;
mod hybrid;
mod pinflow;
mod protocol;
mod ui;

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pinflow::Op;
use protocol::*;
use rand::RngCore;
use ui::UiHandle;

static REPLIED: AtomicBool = AtomicBool::new(false);

/// Prints the single reply line (first call wins) to stdout.
fn emit(line: &str) {
    if REPLIED.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn finish(line: &str) -> ! {
    emit(line);
    std::process::exit(0);
}

struct Success {
    ctap_response: Vec<u8>,
    provider: &'static str,
    uv: bool,
    transports: u32,
}

async fn flow(req: Request, ui: UiHandle) -> Result<Success, HelperError> {
    if req.protocol != 0 && req.protocol != 1 {
        return Err(HelperError::Other(format!("unsupported protocol {}", req.protocol)));
    }
    let ctap = b64_decode(&req.ctap_request)?;
    let op = match req.command.as_str() {
        "getAssertion" => Op::Get(parse_get_assertion(&ctap)?, req.flags),
        "makeCredential" => Op::Make(parse_make_credential(&ctap)?, req.flags),
        other => return Err(HelperError::Other(format!("unknown command {other}"))),
    };
    tracing::info!("{} for {} (flags 0x{:08X})", req.command, op.rp_id(), req.flags);

    let deadline = Instant::now() + Duration::from_millis(req.timeout_ms_or_default());
    let remaining = move || deadline.saturating_duration_since(Instant::now());

    let bt_available = libwebauthn::transport::cable::is_available().await;
    tracing::info!(
        "hybrid: filterHybrid={} bluetooth_available={}",
        req.filter_hybrid, bt_available
    );
    // The host attaches filterHybridTransport to every real request from this client; by
    // the host-pool owner's decision the phone path is offered anyway. Set
    // FREERDP_WEBAUTHN_HONOR_HYBRID_FILTER=1 to go back to honouring the flag.
    let honor_filter = std::env::var("FREERDP_WEBAUTHN_HONOR_HYBRID_FILTER").as_deref() == Ok("1");
    if req.filter_hybrid && !honor_filter {
        tracing::info!("host set filterHybridTransport; offering the phone path regardless");
    }
    let use_hybrid = (!req.filter_hybrid || !honor_filter) && bt_available;
    if !use_hybrid {
        tracing::info!("hybrid transport not used (filtered or no Bluetooth)");
        ui.no_phone();
    }
    ui.status(if use_hybrid {
        "Waiting for phone or security key"
    } else {
        "Waiting for security key"
    });

    let hy = async {
        if use_hybrid {
            hybrid::run(&op, &ui, remaining()).await
        } else {
            std::future::pending().await
        }
    };
    let hd = hid::run(&op, &ui, remaining());
    tokio::pin!(hy, hd);
    let mut hy_alive = use_hybrid;
    let sleep = tokio::time::sleep(remaining());
    tokio::pin!(sleep);

    let (outcome, provider, transports) = loop {
        tokio::select! {
            r = &mut hy, if hy_alive => match r {
                Ok(o) => break (o, "Hybrid", TRANSPORT_HYBRID),
                Err(e) if e.is_terminal() => return Err(e),
                Err(e) => {
                    tracing::warn!("hybrid unavailable: {e}");
                    hy_alive = false;
                    ui.no_phone();
                    ui.status("Phone sign-in failed; use a security key");
                }
            },
            r = &mut hd => break (r?, "Hid", TRANSPORT_USB),
            _ = ui.cancel.notified() => return Err(HelperError::Cancelled),
            _ = tokio::signal::ctrl_c() => return Err(HelperError::Cancelled),
            _ = &mut sleep => return Err(HelperError::Timeout),
        }
    };
    Ok(Success {
        ctap_response: outcome.ctap_response,
        provider,
        uv: outcome.uv,
        transports,
    })
}

fn run_blocking(req: Request, ui: UiHandle) -> String {
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return error_reply(&HelperError::Other(format!("tokio runtime: {e}"))),
    };
    match rt.block_on(flow(req, ui)) {
        Ok(s) => ok_reply(&s.ctap_response, s.provider, s.uv as u8, s.transports),
        Err(e) => {
            tracing::warn!("failed: {e}");
            error_reply(&e)
        }
    }
}

fn read_request() -> Result<Request, HelperError> {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| HelperError::Other(format!("reading stdin: {e}")))?;
    // Log the request as received, minus the (large, sensitive) CTAP payload.
    if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(line.trim()) {
        if let Some(obj) = v.as_object_mut() {
            if let Some(c) = obj.get("ctapRequest").and_then(|c| c.as_str()).map(|c| c.len()) {
                obj.insert("ctapRequest".into(), serde_json::Value::String(format!("<{c} b64 chars>")));
            }
        }
        tracing::info!("request: {v}");
    }
    serde_json::from_str(line.trim())
        .map_err(|e| HelperError::Other(format!("invalid request JSON: {e}")))
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .without_time()
        .init();

    // Whatever happens, the caller gets a reply line and a clean exit.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        finish(&error_reply(&HelperError::Other(format!("internal error: {info}"))));
    }));

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut hash = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut hash);
    let request = match args.as_slice() {
        [a] if a == "--version" || a == "-V" => {
            println!("freerdp-webauthn-helper {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        [a, rp] if a == "--test-get" => Ok(fabricate_get_request(rp, &hash)),
        [a, rp] if a == "--test-make" => Ok(fabricate_make_request(rp, &hash)),
        [] => read_request(),
        _ => {
            eprintln!(
                "usage: freerdp-webauthn-helper [--version | --test-get <rpId> | --test-make <rpId>]\n\
                 (no arguments: read one JSON request line from stdin)"
            );
            std::process::exit(2);
        }
    };
    let req = match request {
        Ok(r) => r,
        Err(e) => finish(&error_reply(&e)),
    };

    match gtk::init() {
        Ok(()) => {
            let handle = ui::create(&req.rp_id);
            std::thread::spawn(move || {
                let line = run_blocking(req, handle);
                finish(&line);
            });
            gtk::main();
        }
        Err(e) => {
            tracing::warn!("GTK unavailable ({e}); continuing without UI");
            finish(&run_blocking(req, UiHandle::headless()));
        }
    }
}
