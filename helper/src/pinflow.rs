// SPDX-License-Identifier: GPL-3.0-or-later
//! CTAP2 operation execution on an already-open channel, including manual PIN/UV handling for
//! USB (HID) authenticators.

use std::fmt::Display;
use std::time::Duration;

use libwebauthn::fido::AuthenticatorDataFlags;
use libwebauthn::pin::{
    pin_hash, PinUvAuthProtocol, PinUvAuthProtocolOne, PinUvAuthProtocolTwo,
};
use libwebauthn::proto::ctap2::{
    Ctap2, Ctap2AuthTokenPermissionRole, Ctap2ClientPinRequest, Ctap2CredentialType,
    Ctap2GetAssertionRequest, Ctap2GetInfoResponse, Ctap2MakeCredentialOptions,
    Ctap2MakeCredentialRequest, Ctap2PublicKeyCredentialDescriptor,
    Ctap2PublicKeyCredentialRpEntity, Ctap2PublicKeyCredentialType,
    Ctap2PublicKeyCredentialUserEntity,
};
use libwebauthn::webauthn::error::PlatformError;
use libwebauthn::webauthn::WebAuthnError;
use serde_bytes::ByteBuf;
use unicode_normalization::UnicodeNormalization;

use crate::protocol::*;
use crate::ui::UiHandle;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Op {
    Get(GetParams, u32),
    Make(MakeParams, u32),
}

impl Op {
    pub fn rp_id(&self) -> &str {
        match self {
            Op::Get(p, _) => &p.rp_id,
            Op::Make(p, _) => &p.rp_id,
        }
    }

    /// Whether a PIN should be used when the (USB) authenticator has one set.
    fn wants_pin(&self) -> bool {
        match self {
            Op::Make(..) => true,
            Op::Get(p, flags) => {
                if p.uv == Some(true)
                    || flags & (FLAG_UV_REQUIRED | FLAG_CLIENT_PIN_REQUIRED) != 0
                {
                    true
                } else {
                    flags & FLAG_UV_NOT_REQUIRED == 0
                }
            }
        }
    }
}

pub struct Outcome {
    pub ctap_response: Vec<u8>,
    pub uv: bool,
}

pub fn herr<E: Display>(e: WebAuthnError<E>) -> HelperError {
    match e {
        WebAuthnError::Ctap(c) => HelperError::Ctap(u8::from(c), format!("{c:?}")),
        WebAuthnError::Platform(PlatformError::Cancelled) => HelperError::Cancelled,
        WebAuthnError::Platform(p) => HelperError::Other(format!("platform error: {p}")),
        WebAuthnError::Transport(t) => HelperError::Other(format!("transport error: {t}")),
        #[allow(unreachable_patterns)]
        _ => HelperError::Other("unknown libwebauthn error".into()),
    }
}

fn descriptors(ids: &[Vec<u8>]) -> Vec<Ctap2PublicKeyCredentialDescriptor> {
    ids.iter()
        .map(|id| Ctap2PublicKeyCredentialDescriptor {
            id: ByteBuf::from(id.clone()),
            r#type: Ctap2PublicKeyCredentialType::PublicKey,
            transports: None,
        })
        .collect()
}

type PinAuth = (Vec<u8>, u32);

async fn send<C: Ctap2>(
    ch: &mut C,
    op: &Op,
    pin: Option<&PinAuth>,
    timeout: Duration,
) -> Result<Outcome, HelperError> {
    match op {
        Op::Get(p, _) => {
            let req = Ctap2GetAssertionRequest {
                relying_party_id: p.rp_id.clone(),
                client_data_hash: ByteBuf::from(p.client_data_hash.clone()),
                allow: descriptors(&p.allow),
                extensions: None,
                options: None,
                pin_auth_param: pin.map(|(a, _)| ByteBuf::from(a.clone())),
                pin_auth_proto: pin.map(|(_, v)| *v),
            };
            let r = ch.ctap2_get_assertion(&req, timeout).await.map_err(herr)?;
            let ad = r
                .authenticator_data
                .to_response_bytes()
                .map_err(|e| HelperError::Other(format!("authData encoding: {e:?}")))?;
            let uv = r
                .authenticator_data
                .flags
                .contains(AuthenticatorDataFlags::USER_VERIFIED);
            let resp = build_get_assertion_response(
                r.credential_id.as_ref().map(|c| c.id.as_slice()),
                &ad,
                r.signature.as_slice(),
                r.user.as_ref().map(|u| u.id.as_slice()),
            );
            Ok(Outcome {
                ctap_response: resp,
                uv,
            })
        }
        Op::Make(p, _) => {
            let req = Ctap2MakeCredentialRequest {
                hash: ByteBuf::from(p.client_data_hash.clone()),
                relying_party: Ctap2PublicKeyCredentialRpEntity::new(
                    &p.rp_id,
                    p.rp_name.as_deref().unwrap_or(&p.rp_id),
                ),
                user: Ctap2PublicKeyCredentialUserEntity {
                    id: ByteBuf::from(p.user_id.clone()),
                    name: p.user_name.clone(),
                    display_name: p.user_display_name.clone(),
                },
                algorithms: p
                    .algs
                    .iter()
                    .map(|a| {
                        Ctap2CredentialType::new(
                            Ctap2PublicKeyCredentialType::PublicKey,
                            (*a).into(),
                        )
                    })
                    .collect(),
                exclude: if p.exclude.is_empty() {
                    None
                } else {
                    Some(descriptors(&p.exclude))
                },
                extensions: None,
                options: if p.rk {
                    Some(Ctap2MakeCredentialOptions {
                        require_resident_key: Some(true),
                        deprecated_require_user_verification: None,
                    })
                } else {
                    None
                },
                pin_auth_param: pin.map(|(a, _)| ByteBuf::from(a.clone())),
                pin_auth_proto: pin.map(|(_, v)| *v),
                enterprise_attestation: None,
            };
            let r = ch.ctap2_make_credential(&req, timeout).await.map_err(herr)?;
            let ad = r
                .authenticator_data
                .to_response_bytes()
                .map_err(|e| HelperError::Other(format!("authData encoding: {e:?}")))?;
            let uv = r
                .authenticator_data
                .flags
                .contains(AuthenticatorDataFlags::USER_VERIFIED);
            // libwebauthn's cbor::to_vec is crate-private; its statement types are plain serde
            // structs, so ciborium produces the same CBOR.
            let mut att = Vec::new();
            ciborium::into_writer(&r.attestation_statement, &mut att)
                .map_err(|e| HelperError::Other(format!("attStmt encoding: {e}")))?;
            Ok(Outcome {
                ctap_response: build_make_credential_response(&r.format, &ad, &att)?,
                uv,
            })
        }
    }
}

/// Runs `op` over a phone (hybrid) channel: the phone performs its own user verification, so we
/// send no PIN parameters.
pub async fn run_hybrid<C: Ctap2>(
    ch: &mut C,
    op: &Op,
    timeout: Duration,
) -> Result<Outcome, HelperError> {
    send(ch, op, None, timeout).await
}

/// Runs `op` over a USB authenticator, handling PIN entry via the UI when needed.
pub async fn run_hid<C: Ctap2>(
    ch: &mut C,
    op: &Op,
    ui: &UiHandle,
    timeout: Duration,
) -> Result<Outcome, HelperError> {
    let info = ch.ctap2_get_info().await.map_err(herr)?;
    let mut need_pin = info.option_enabled("clientPin") && op.wants_pin();
    let mut pin_auth: Option<PinAuth> = None;
    let mut tried_pin = false;
    loop {
        if need_pin && pin_auth.is_none() {
            ui.status("Enter PIN");
            pin_auth = Some(obtain_pin_auth(ch, &info, op, ui, timeout).await?);
            tried_pin = true;
        }
        ui.status("Touch the key");
        match send(ch, op, pin_auth.as_ref(), timeout).await {
            Ok(mut o) => {
                if pin_auth.is_some() {
                    o.uv = true;
                }
                return Ok(o);
            }
            Err(HelperError::Ctap(0x36, _)) if !tried_pin => {
                need_pin = true;
            }
            Err(HelperError::Ctap(CTAP_USER_ACTION_TIMEOUT, _)) => {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

async fn obtain_pin_auth<C: Ctap2>(
    ch: &mut C,
    info: &Ctap2GetInfoResponse,
    op: &Op,
    ui: &UiHandle,
    timeout: Duration,
) -> Result<PinAuth, HelperError> {
    let protos = info.pin_auth_protos.clone().unwrap_or_else(|| vec![1]);
    let proto: Box<dyn PinUvAuthProtocol> = if protos.contains(&2) {
        Box::new(PinUvAuthProtocolTwo::new())
    } else {
        Box::new(PinUvAuthProtocolOne::new())
    };
    let (hash, role, rp): (&[u8], _, _) = match op {
        Op::Get(p, _) => (
            &p.client_data_hash,
            Ctap2AuthTokenPermissionRole::GET_ASSERTION,
            p.rp_id.as_str(),
        ),
        Op::Make(p, _) => (
            &p.client_data_hash,
            Ctap2AuthTokenPermissionRole::MAKE_CREDENTIAL,
            p.rp_id.as_str(),
        ),
    };
    let mut prefix: Option<String> = None;
    for _ in 0..3 {
        let retries = ch
            .ctap2_client_pin(
                &Ctap2ClientPinRequest::new_get_pin_retries(Some(proto.version())),
                timeout,
            )
            .await
            .ok()
            .and_then(|r| r.pin_retries);
        let pin = ui.ask_pin(retries, prefix.as_deref()).await?;
        let pin: String = pin.nfc().collect();
        match pin_token(ch, info, &*proto, &pin, role, rp, timeout).await {
            Ok(token) => {
                let auth = proto
                    .authenticate(&token, hash)
                    .map_err(|e| HelperError::Other(format!("pinAuth: {e}")))?;
                return Ok((auth, proto.version() as u32));
            }
            // 0x31 PIN_INVALID
            Err(HelperError::Ctap(0x31, _)) => prefix = Some("Incorrect PIN".into()),
            Err(e) => return Err(e),
        }
    }
    Err(HelperError::Ctap(0x31, "PINInvalid".into()))
}

async fn pin_token<C: Ctap2>(
    ch: &mut C,
    info: &Ctap2GetInfoResponse,
    proto: &dyn PinUvAuthProtocol,
    pin: &str,
    role: Ctap2AuthTokenPermissionRole,
    rp: &str,
    timeout: Duration,
) -> Result<Vec<u8>, HelperError> {
    let pe = |e: PlatformError| HelperError::Other(format!("PIN protocol: {e}"));
    let ka = ch
        .ctap2_client_pin(
            &Ctap2ClientPinRequest::new_get_key_agreement(proto.version()),
            timeout,
        )
        .await
        .map_err(herr)?
        .key_agreement
        .ok_or_else(|| HelperError::Other("no keyAgreement in response".into()))?;
    let (my_pub, shared) = proto.encapsulate(&ka).map_err(pe)?;
    let enc = proto
        .encrypt(&shared, &pin_hash(pin.as_bytes()))
        .map_err(pe)?;
    let req = if info.option_enabled("pinUvAuthToken") {
        Ctap2ClientPinRequest::new_get_pin_token_with_perm(
            proto.version(),
            my_pub,
            &enc,
            role,
            Some(rp),
        )
    } else {
        Ctap2ClientPinRequest::new_get_pin_token(proto.version(), my_pub, &enc)
    };
    let tok = ch
        .ctap2_client_pin(&req, timeout)
        .await
        .map_err(herr)?
        .pin_uv_auth_token
        .ok_or_else(|| HelperError::Other("no pinUvAuthToken in response".into()))?;
    proto.decrypt(&shared, &tok).map_err(pe)
}
