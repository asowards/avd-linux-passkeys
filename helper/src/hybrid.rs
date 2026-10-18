// SPDX-License-Identifier: GPL-3.0-or-later
//! Hybrid (caBLE / QR code) transport.

use std::time::Duration;

use libwebauthn::transport::cable::channel::{CableUpdate, CableUxUpdate};
use libwebauthn::transport::cable::qr_code_device::{
    CableQrCodeDevice, CableTransports, QrCodeOperationHint,
};
use libwebauthn::transport::{Channel, ChannelSettings, Device};

use crate::pinflow::{herr, run_hybrid, Op, Outcome};
use crate::protocol::HelperError;
use crate::ui::UiHandle;

pub async fn run(op: &Op, ui: &UiHandle, timeout: Duration) -> Result<Outcome, HelperError> {
    let hint = match op {
        Op::Get(..) => QrCodeOperationHint::GetAssertionRequest,
        Op::Make(..) => QrCodeOperationHint::MakeCredential,
    };
    // Default to the cloud-assisted (WebSocket tunnel) path only: it is what Chrome uses and
    // the best-tested one. The direct BLE L2CAP path (CTAP 2.3) stalled during
    // authentication with an Android phone here. FREERDP_WEBAUTHN_CABLE=both re-enables it.
    let transports = match std::env::var("FREERDP_WEBAUTHN_CABLE").as_deref() {
        Ok("both") | Ok("local") => CableTransports::CloudAssistedOrLocal,
        _ => CableTransports::CloudAssistedOnly,
    };
    tracing::info!("hybrid transports: {transports:?}");
    let mut device = CableQrCodeDevice::new_transient(hint, transports)
        .map_err(|e| HelperError::Other(format!("hybrid: cannot create QR device: {e:?}")))?;
    ui.show_qr(&device.qr_code.to_string());
    let mut ch = device.channel(ChannelSettings::default()).await.map_err(herr)?;

    // Subscribe before the first CTAP call.
    let mut rx = ch.get_ux_update_receiver();
    let pump = async {
        loop {
            match rx.recv().await {
                Ok(CableUxUpdate::CableUpdate(u)) => match u {
                    CableUpdate::ProximityCheck => ui.status("Checking phone proximity"),
                    CableUpdate::Connecting => ui.status("Connecting to phone"),
                    CableUpdate::Authenticating => ui.status("Verifying"),
                    CableUpdate::Connected => ui.status("Connected; confirm on your phone"),
                    CableUpdate::Error(e) => tracing::warn!("hybrid error update: {e}"),
                },
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        std::future::pending::<()>().await
    };
    ui.status("Waiting for phone");
    tokio::select! {
        r = run_hybrid(&mut ch, op, timeout) => r,
        _ = pump => unreachable!(),
    }
}
