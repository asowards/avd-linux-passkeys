// SPDX-License-Identifier: GPL-3.0-or-later
//! Hybrid (caBLE) spike, adapted from libwebauthn 0.10.0 examples/ceremony/webauthn_cable.rs.
//!
//! Modes (human with a phone required; shows a QR code in the terminal):
//!   cargo run --example cable_spike                    MakeCredential (rp example.org)
//!   cargo run --example cable_spike -- --get <rpId>    high-level GetAssertion (webauthn_get_assertion)
//!   cargo run --example cable_spike -- --raw <rpId>    CTAP2-level GetAssertion with a caller-supplied
//!                                                      32-byte clientDataHash (what the RDP helper needs)
//! Add RUST_LOG=libwebauthn=debug for protocol logs.
use std::error::Error;

use libwebauthn::ops::webauthn::{
    DatFilePublicSuffixList, GetAssertionRequest, JsonFormat, MakeCredentialRequest,
    OriginValidation, RelatedOrigins, RequestOrigin, RequestSettings, WebAuthnIDLResponse as _,
};
use libwebauthn::proto::ctap2::{
    Ctap2, Ctap2GetAssertionRequest,
};
use libwebauthn::transport::cable::is_available;
use libwebauthn::transport::cable::qr_code_device::{
    CableQrCodeDevice, CableTransports, QrCodeOperationHint,
};
use libwebauthn::transport::{Channel as _, ChannelSettings, Device};
use libwebauthn::webauthn::WebAuthn;
use qrcode::render::unicode;
use qrcode::QrCode;
use std::time::Duration;

#[path = "common/mod.rs"]
mod common;

const MAKE_CREDENTIAL_REQUEST: &str = r#"
{
    "rp": { "id": "example.org", "name": "Example Relying Party" },
    "user": {
        "id": "MTIzNDU2NzgxMjM0NTY3ODEyMzQ1Njc4MTIzNDU2Nzg",
        "name": "Mario Rossi",
        "displayName": "Mario Rossi"
    },
    "challenge": "MTIzNDU2NzgxMjM0NTY3ODEyMzQ1Njc4MTIzNDU2Nzg",
    "pubKeyCredParams": [ {"type": "public-key", "alg": -7} ],
    "timeout": 120000,
    "excludeCredentials": [],
    "authenticatorSelection": { "residentKey": "discouraged", "userVerification": "preferred" },
    "attestation": "none"
}
"#;

fn get_request_json(rp_id: &str) -> String {
    format!(
        r#"{{
    "challenge": "MTIzNDU2NzgxMjM0NTY3ODEyMzQ1Njc4MTIzNDU2Nzg",
    "timeout": 120000,
    "rpId": "{rp_id}",
    "allowCredentials": [],
    "userVerification": "preferred"
}}"#
    )
}

enum Mode {
    Make,
    Get(String),
    Raw(String),
}

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn Error>> {
    common::setup_logging();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match args.as_slice() {
        [f, rp] if f == "--get" => Mode::Get(rp.clone()),
        [f, rp] if f == "--raw" => Mode::Raw(rp.clone()),
        [] => Mode::Make,
        _ => return Err("usage: cable_spike [--get <rpId> | --raw <rpId>]".into()),
    };

    if !is_available().await {
        eprintln!("No Bluetooth adapter found. Cable/Hybrid transport is unavailable.");
        return Err("Cable transport not available".into());
    }

    let rp_for_origin = match &mode {
        Mode::Make => "example.org".to_string(),
        Mode::Get(rp) | Mode::Raw(rp) => rp.clone(),
    };
    let request_origin: RequestOrigin = format!("https://{rp_for_origin}")
        .as_str()
        .try_into()
        .expect("Invalid origin");
    let psl = DatFilePublicSuffixList::from_system_file().expect(
        "PSL not available; install the publicsuffix-list package or pass an explicit path",
    );
    let settings = RequestSettings {
        origin: OriginValidation::Validate {
            public_suffix_list: &psl,
            related_origins: RelatedOrigins::Disabled,
        },
    };

    let hint = match mode {
        Mode::Make => QrCodeOperationHint::MakeCredential,
        _ => QrCodeOperationHint::GetAssertionRequest,
    };
    let mut device: CableQrCodeDevice =
        CableQrCodeDevice::new_transient(hint, CableTransports::CloudAssistedOrLocal)?;

    println!("Created QR code, awaiting for advertisement.");
    let qr_string = device.qr_code.to_string();
    println!("{qr_string}");
    let qr_code = QrCode::new(qr_string).unwrap();
    let image = qr_code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .build();
    println!("{}", image);

    let mut channel = device.channel(ChannelSettings::default()).await.unwrap();
    println!("Channel established {:?}", channel);

    let state_recv = channel.get_ux_update_receiver();
    tokio::spawn(common::handle_cable_updates(state_recv));

    match mode {
        Mode::Make => {
            let request =
                MakeCredentialRequest::prepare(&request_origin, MAKE_CREDENTIAL_REQUEST, &settings)
                    .await
                    .expect("Failed to parse request JSON");
            let response = retry_user_errors!(channel.webauthn_make_credential(&request)).unwrap();
            let response_json = response
                .to_json_string(&request, JsonFormat::Prettified)
                .expect("Failed to serialize MakeCredential response");
            println!("WebAuthn MakeCredential response (JSON):\n{response_json}");
        }
        Mode::Get(rp) => {
            let request =
                GetAssertionRequest::prepare(&request_origin, &get_request_json(&rp), &settings)
                    .await
                    .expect("Failed to parse request JSON");
            let response = retry_user_errors!(channel.webauthn_get_assertion(&request)).unwrap();
            for assertion in &response.assertions {
                let json = assertion
                    .to_json_string(&request, JsonFormat::Prettified)
                    .expect("Failed to serialize assertion");
                println!("WebAuthn GetAssertion response (JSON):\n{json}");
            }
        }
        Mode::Raw(rp) => {
            // CTAP2 layer: we supply the clientDataHash ourselves (here an arbitrary 32 bytes).
            let info = channel.ctap2_get_info().await?;
            println!("getInfo: {info:#?}");
            let req = Ctap2GetAssertionRequest {
                relying_party_id: rp,
                client_data_hash: serde_bytes::ByteBuf::from(vec![0x42u8; 32]),
                allow: vec![],
                extensions: None,
                options: None, // phone performs UV itself; no pinUvAuthParam from us
                pin_auth_param: None,
                pin_auth_proto: None,
            };
            let resp = channel
                .ctap2_get_assertion(&req, Duration::from_secs(120))
                .await?;
            println!("credential_id: {:?}", resp.credential_id);
            println!(
                "authData ({} bytes): {:02x?}",
                resp.authenticator_data.to_response_bytes()?.len(),
                resp.authenticator_data.to_response_bytes()?
            );
            println!("signature: {:02x?}", resp.signature.as_slice());
            println!("user: {:?}", resp.user);
            println!("credentials_count: {:?}", resp.credentials_count);
        }
    }

    Ok(())
}
