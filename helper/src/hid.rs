// SPDX-License-Identifier: GPL-3.0-or-later
//! USB security key (HID) transport. Polls for devices every second.

use std::time::Duration;

use libwebauthn::transport::hid::list_devices;
use libwebauthn::transport::{Device, ChannelSettings};

use crate::pinflow::{herr, run_hid, Op, Outcome};
use crate::protocol::HelperError;
use crate::ui::UiHandle;

/// Returns on success or on a terminal (CTAP-level) error; transport-level problems are retried.
pub async fn run(op: &Op, ui: &UiHandle, timeout: Duration) -> Result<Outcome, HelperError> {
    loop {
        match list_devices().await {
            Ok(mut devices) => {
                for dev in devices.iter_mut() {
                    tracing::debug!("trying HID device {dev}");
                    let attempt = async {
                        let mut ch = dev.channel(ChannelSettings::default()).await.map_err(herr)?;
                        run_hid(&mut ch, op, ui, timeout).await
                    }
                    .await;
                    match attempt {
                        Ok(o) => return Ok(o),
                        Err(e) if e.is_terminal() => return Err(e),
                        Err(e) => tracing::debug!("HID device skipped: {e}"),
                    }
                }
            }
            Err(e) => tracing::debug!("listing HID devices failed: {e:?}"),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
