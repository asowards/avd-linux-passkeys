// SPDX-License-Identifier: GPL-3.0-or-later
//! Wire protocol (JSON on stdin/stdout) and CTAP2 CBOR request parsing / response building.
//!
//! This module intentionally does not depend on libwebauthn so it can be unit tested in isolation.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ciborium::Value;
use serde::Deserialize;
use serde_json::json;

// Windows WEBAUTHN_CTAP_TRANSPORT_* bits.
pub const TRANSPORT_USB: u32 = 0x1;
pub const TRANSPORT_HYBRID: u32 = 0x20;

// Windows CTAPCLT_* flag hints.
pub const FLAG_CLIENT_PIN_REQUIRED: u32 = 0x0010_0000;
pub const FLAG_UV_REQUIRED: u32 = 0x0040_0000;
pub const FLAG_UV_PREFERRED: u32 = 0x0080_0000;
pub const FLAG_UV_NOT_REQUIRED: u32 = 0x0100_0000;

// HRESULTs.
pub const NTE_USER_CANCELLED: u32 = 0x8009_0036;
pub const NTE_NOT_FOUND: u32 = 0x8009_0011;
pub const E_FAIL: u32 = 0x8000_4005;
/// ERROR_CANCELLED. Documented choice: used for timeouts (ctapStatus 0x2F).
pub const ERROR_CANCELLED: u32 = 0x8007_04C7;

pub const CTAP_USER_CANCEL: u8 = 0x2D;
pub const CTAP_NO_CREDENTIALS: u8 = 0x2E;
pub const CTAP_USER_ACTION_TIMEOUT: u8 = 0x2F;

/// Accepts integers written as JSON floats (WinPR's JSON writer emits `1.0` for `1`).
fn lenient_u64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| *f >= 0.0 && f.fract() == 0.0).map(|f| f as u64))
            .ok_or_else(|| serde::de::Error::custom(format!("not a non-negative integer: {n}"))),
        serde_json::Value::Null => Ok(0),
        other => Err(serde::de::Error::custom(format!("expected a number, got {other}"))),
    }
}

fn lenient_u32<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let v = lenient_u64(d)?;
    u32::try_from(v).map_err(|_| serde::de::Error::custom(format!("{v} does not fit in u32")))
}

#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    #[serde(default, deserialize_with = "lenient_u32")]
    pub protocol: u32,
    pub command: String,
    #[serde(rename = "rpId")]
    pub rp_id: String,
    #[serde(rename = "ctapRequest")]
    pub ctap_request: String,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub flags: u32,
    #[serde(default, rename = "timeoutMs", deserialize_with = "lenient_u64")]
    pub timeout_ms: u64,
    #[serde(default, rename = "filterHybrid")]
    pub filter_hybrid: bool,
}

impl Request {
    pub fn timeout_ms_or_default(&self) -> u64 {
        if self.timeout_ms == 0 {
            120_000
        } else {
            self.timeout_ms
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetParams {
    pub rp_id: String,
    pub client_data_hash: Vec<u8>,
    pub allow: Vec<Vec<u8>>,
    pub up: Option<bool>,
    pub uv: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakeParams {
    pub client_data_hash: Vec<u8>,
    pub rp_id: String,
    pub rp_name: Option<String>,
    pub user_id: Vec<u8>,
    pub user_name: Option<String>,
    pub user_display_name: Option<String>,
    pub algs: Vec<i32>,
    pub exclude: Vec<Vec<u8>>,
    pub rk: bool,
    pub uv: Option<bool>,
}

/// Errors returned by the helper, mapped onto the reply error fields.
#[derive(Debug, Clone)]
pub enum HelperError {
    Cancelled,
    Timeout,
    Ctap(u8, String),
    Other(String),
}

impl HelperError {
    /// Errors from which the other transport cannot recover (versus "this transport is unusable").
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            HelperError::Cancelled | HelperError::Timeout | HelperError::Ctap(..)
        )
    }

    pub fn ctap_status(&self) -> u8 {
        match self {
            HelperError::Cancelled => CTAP_USER_CANCEL,
            HelperError::Timeout => CTAP_USER_ACTION_TIMEOUT,
            HelperError::Ctap(c, _) => *c,
            HelperError::Other(_) => 0x7F,
        }
    }

    pub fn hresult(&self) -> u32 {
        match self {
            HelperError::Cancelled => NTE_USER_CANCELLED,
            HelperError::Timeout => ERROR_CANCELLED,
            HelperError::Ctap(CTAP_USER_CANCEL, _) => NTE_USER_CANCELLED,
            HelperError::Ctap(CTAP_NO_CREDENTIALS, _) => NTE_NOT_FOUND,
            _ => E_FAIL,
        }
    }

    pub fn message(&self) -> String {
        match self {
            HelperError::Cancelled => "cancelled by user".into(),
            HelperError::Timeout => "operation timed out".into(),
            HelperError::Ctap(c, m) => format!("CTAP error 0x{c:02X}: {m}"),
            HelperError::Other(m) => m.clone(),
        }
    }
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

pub fn error_reply(e: &HelperError) -> String {
    json!({
        "status": "error",
        "ctapStatus": e.ctap_status(),
        "hresult": e.hresult(),
        "message": e.message(),
    })
    .to_string()
}

pub fn ok_reply(ctap_response: &[u8], provider: &str, uv_status: u8, transports: u32) -> String {
    json!({
        "status": "ok",
        "ctapResponse": B64.encode(ctap_response),
        "provider": provider,
        "uvStatus": uv_status,
        "transports": transports,
    })
    .to_string()
}

pub fn b64_decode(s: &str) -> Result<Vec<u8>, HelperError> {
    B64.decode(s.trim())
        .map_err(|e| HelperError::Other(format!("invalid base64: {e}")))
}

pub fn b64_encode(b: &[u8]) -> String {
    B64.encode(b)
}

// ---------------------------------------------------------------------------------------------
// CBOR helpers

fn bad(msg: impl Into<String>) -> HelperError {
    HelperError::Other(msg.into())
}

fn decode_map(bytes: &[u8]) -> Result<Vec<(Value, Value)>, HelperError> {
    let v: Value = ciborium::from_reader(bytes).map_err(|e| bad(format!("invalid CBOR: {e}")))?;
    match v {
        Value::Map(m) => Ok(m),
        _ => Err(bad("CTAP request is not a CBOR map")),
    }
}

fn int_key(v: &Value) -> Option<i128> {
    match v {
        Value::Integer(i) => Some((*i).into()),
        _ => None,
    }
}

fn get_int<'a>(map: &'a [(Value, Value)], key: i128) -> Option<&'a Value> {
    map.iter()
        .find(|(k, _)| int_key(k) == Some(key))
        .map(|(_, v)| v)
}

fn get_text<'a>(map: &'a [(Value, Value)], key: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(k, _)| matches!(k, Value::Text(t) if t == key))
        .map(|(_, v)| v)
}

fn as_bytes(v: &Value) -> Option<Vec<u8>> {
    match v {
        Value::Bytes(b) => Some(b.clone()),
        _ => None,
    }
}

fn as_text(v: &Value) -> Option<String> {
    match v {
        Value::Text(t) => Some(t.clone()),
        _ => None,
    }
}

fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn descriptor_ids(v: Option<&Value>) -> Vec<Vec<u8>> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|d| match d {
                Value::Map(m) => get_text(m, "id").and_then(as_bytes),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

fn option_bool(opts: Option<&Value>, name: &str) -> Option<bool> {
    match opts {
        Some(Value::Map(m)) => get_text(m, name).and_then(as_bool),
        _ => None,
    }
}

pub fn parse_get_assertion(bytes: &[u8]) -> Result<GetParams, HelperError> {
    let m = decode_map(bytes)?;
    let rp_id = get_int(&m, 1)
        .and_then(as_text)
        .ok_or_else(|| bad("getAssertion: missing rpId (1)"))?;
    let client_data_hash = get_int(&m, 2)
        .and_then(as_bytes)
        .ok_or_else(|| bad("getAssertion: missing clientDataHash (2)"))?;
    let opts = get_int(&m, 5);
    Ok(GetParams {
        rp_id,
        client_data_hash,
        allow: descriptor_ids(get_int(&m, 3)),
        up: option_bool(opts, "up"),
        uv: option_bool(opts, "uv"),
    })
}

pub fn parse_make_credential(bytes: &[u8]) -> Result<MakeParams, HelperError> {
    let m = decode_map(bytes)?;
    let client_data_hash = get_int(&m, 1)
        .and_then(as_bytes)
        .ok_or_else(|| bad("makeCredential: missing clientDataHash (1)"))?;
    let rp = match get_int(&m, 2) {
        Some(Value::Map(rp)) => rp,
        _ => return Err(bad("makeCredential: missing rp (2)")),
    };
    let rp_id = get_text(rp, "id")
        .and_then(as_text)
        .ok_or_else(|| bad("makeCredential: missing rp.id"))?;
    let user = match get_int(&m, 3) {
        Some(Value::Map(u)) => u,
        _ => return Err(bad("makeCredential: missing user (3)")),
    };
    let user_id = get_text(user, "id")
        .and_then(as_bytes)
        .ok_or_else(|| bad("makeCredential: missing user.id"))?;
    let algs = match get_int(&m, 4) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|p| match p {
                Value::Map(pm) => get_text(pm, "alg").and_then(|v| match v {
                    Value::Integer(i) => i32::try_from(i128::from(*i)).ok(),
                    _ => None,
                }),
                _ => None,
            })
            .collect(),
        _ => vec![],
    };
    let opts = get_int(&m, 7);
    Ok(MakeParams {
        client_data_hash,
        rp_id,
        rp_name: get_text(rp, "name").and_then(as_text),
        user_id,
        user_name: get_text(user, "name").and_then(as_text),
        user_display_name: get_text(user, "displayName").and_then(as_text),
        algs,
        exclude: descriptor_ids(get_int(&m, 5)),
        rk: option_bool(opts, "rk").unwrap_or(false),
        uv: option_bool(opts, "uv"),
    })
}

fn int(i: i64) -> Value {
    Value::Integer(i.into())
}

fn encode(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::into_writer(v, &mut out).expect("CBOR encoding to Vec cannot fail");
    out
}

fn descriptor(id: &[u8]) -> Value {
    Value::Map(vec![
        (Value::Text("id".into()), Value::Bytes(id.to_vec())),
        (Value::Text("type".into()), Value::Text("public-key".into())),
    ])
}

/// Builds the CTAP2 getAssertion response map (1 credential, 2 authData, 3 signature, 4 user).
pub fn build_get_assertion_response(
    credential_id: Option<&[u8]>,
    auth_data: &[u8],
    signature: &[u8],
    user_id: Option<&[u8]>,
) -> Vec<u8> {
    let mut map = Vec::new();
    if let Some(id) = credential_id {
        map.push((int(1), descriptor(id)));
    }
    map.push((int(2), Value::Bytes(auth_data.to_vec())));
    map.push((int(3), Value::Bytes(signature.to_vec())));
    if let Some(uid) = user_id {
        map.push((
            int(4),
            Value::Map(vec![(Value::Text("id".into()), Value::Bytes(uid.to_vec()))]),
        ));
    }
    encode(&Value::Map(map))
}

/// Builds the CTAP2 makeCredential response map (1 fmt, 2 authData, 3 attStmt).
/// `att_stmt_cbor` is the already CBOR-encoded attestation statement.
pub fn build_make_credential_response(
    fmt: &str,
    auth_data: &[u8],
    att_stmt_cbor: &[u8],
) -> Result<Vec<u8>, HelperError> {
    let att: Value = ciborium::from_reader(att_stmt_cbor)
        .map_err(|e| bad(format!("attestation statement is not valid CBOR: {e}")))?;
    Ok(encode(&Value::Map(vec![
        (int(1), Value::Text(fmt.to_string())),
        (int(2), Value::Bytes(auth_data.to_vec())),
        (int(3), att),
    ])))
}

// ---------------------------------------------------------------------------------------------
// Test harness request fabrication

pub fn fabricate_get_request(rp_id: &str, hash: &[u8]) -> Request {
    let opts = Value::Map(vec![(Value::Text("up".into()), Value::Bool(true))]);
    let map = Value::Map(vec![
        (int(1), Value::Text(rp_id.into())),
        (int(2), Value::Bytes(hash.to_vec())),
        (int(5), opts),
    ]);
    Request {
        protocol: 1,
        command: "getAssertion".into(),
        rp_id: rp_id.into(),
        ctap_request: b64_encode(&encode(&map)),
        flags: FLAG_UV_PREFERRED,
        timeout_ms: 120_000,
        filter_hybrid: false,
    }
}

pub fn fabricate_make_request(rp_id: &str, hash: &[u8]) -> Request {
    let text = |s: &str| Value::Text(s.into());
    let map = Value::Map(vec![
        (int(1), Value::Bytes(hash.to_vec())),
        (
            int(2),
            Value::Map(vec![(text("id"), text(rp_id)), (text("name"), text(rp_id))]),
        ),
        (
            int(3),
            Value::Map(vec![
                (text("id"), Value::Bytes(b"test-user-id".to_vec())),
                (text("name"), text("test")),
                (text("displayName"), text("test")),
            ]),
        ),
        (
            int(4),
            Value::Array(vec![Value::Map(vec![
                (text("alg"), int(-7)),
                (text("type"), text("public-key")),
            ])]),
        ),
        (int(7), Value::Map(vec![(text("rk"), Value::Bool(true))])),
    ]);
    Request {
        protocol: 1,
        command: "makeCredential".into(),
        rp_id: rp_id.into(),
        ctap_request: b64_encode(&encode(&map)),
        flags: FLAG_UV_PREFERRED,
        timeout_ms: 120_000,
        filter_hybrid: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sample_get_assertion() {
        let req = fabricate_get_request("login.microsoft.com", &[7u8; 32]);
        let bytes = b64_decode(&req.ctap_request).unwrap();
        let p = parse_get_assertion(&bytes).unwrap();
        assert_eq!(p.rp_id, "login.microsoft.com");
        assert_eq!(p.client_data_hash, vec![7u8; 32]);
        assert!(p.allow.is_empty());
        assert_eq!(p.up, Some(true));
        assert_eq!(p.uv, None);
    }

    #[test]
    fn parse_get_assertion_with_allow_list_and_uv() {
        let t = |s: &str| Value::Text(s.into());
        let m = Value::Map(vec![
            (int(1), t("example.com")),
            (int(2), Value::Bytes(vec![1; 32])),
            (int(3), Value::Array(vec![descriptor(&[9, 9, 9]), descriptor(&[8])])),
            (int(5), Value::Map(vec![(t("up"), Value::Bool(true)), (t("uv"), Value::Bool(true))])),
            (int(6), Value::Bytes(vec![0; 16])),
            (int(7), int(2)),
        ]);
        let p = parse_get_assertion(&encode(&m)).unwrap();
        assert_eq!(p.allow, vec![vec![9, 9, 9], vec![8]]);
        assert_eq!(p.uv, Some(true));
    }

    #[test]
    fn parse_sample_make_credential() {
        let req = fabricate_make_request("login.microsoft.com", &[3u8; 32]);
        let bytes = b64_decode(&req.ctap_request).unwrap();
        let p = parse_make_credential(&bytes).unwrap();
        assert_eq!(p.rp_id, "login.microsoft.com");
        assert_eq!(p.user_id, b"test-user-id".to_vec());
        assert_eq!(p.user_name.as_deref(), Some("test"));
        assert_eq!(p.algs, vec![-7]);
        assert!(p.rk);
        assert!(p.exclude.is_empty());
        assert_eq!(p.client_data_hash, vec![3u8; 32]);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_get_assertion(&[0xFF, 0x00]).is_err());
        assert!(parse_make_credential(&encode(&int(1))).is_err());
    }

    #[test]
    fn get_response_roundtrip() {
        let bytes = build_get_assertion_response(Some(&[1, 2]), &[0xAA; 37], &[0xBB; 70], Some(&[5]));
        let m = decode_map(&bytes).unwrap();
        assert_eq!(get_int(&m, 2).and_then(as_bytes).unwrap(), vec![0xAA; 37]);
        assert_eq!(get_int(&m, 3).and_then(as_bytes).unwrap(), vec![0xBB; 70]);
        match get_int(&m, 1).unwrap() {
            Value::Map(d) => {
                assert_eq!(get_text(d, "id").and_then(as_bytes).unwrap(), vec![1, 2]);
                assert_eq!(get_text(d, "type").and_then(as_text).unwrap(), "public-key");
            }
            _ => panic!(),
        }
        assert!(get_int(&m, 4).is_some());
        let no_user = decode_map(&build_get_assertion_response(None, &[1], &[2], None)).unwrap();
        assert!(get_int(&no_user, 4).is_none());
        assert!(get_int(&no_user, 1).is_none());
    }

    #[test]
    fn make_response_splices_att_stmt() {
        let att = encode(&Value::Map(vec![(Value::Text("alg".into()), int(-7))]));
        let bytes = build_make_credential_response("packed", &[1, 2, 3], &att).unwrap();
        let m = decode_map(&bytes).unwrap();
        assert_eq!(get_int(&m, 1).and_then(as_text).unwrap(), "packed");
        assert_eq!(get_int(&m, 2).and_then(as_bytes).unwrap(), vec![1, 2, 3]);
        assert!(matches!(get_int(&m, 3), Some(Value::Map(a)) if a.len() == 1));
    }

    #[test]
    fn base64_roundtrip_and_replies() {
        let data = vec![0u8, 255, 1, 2, 3];
        assert_eq!(b64_decode(&b64_encode(&data)).unwrap(), data);
        let ok: serde_json::Value = serde_json::from_str(&ok_reply(&data, "Hybrid", 1, TRANSPORT_HYBRID)).unwrap();
        assert_eq!(ok["status"], "ok");
        assert_eq!(ok["transports"], 0x20);
        assert_eq!(ok["provider"], "Hybrid");
        let err: serde_json::Value = serde_json::from_str(&error_reply(&HelperError::Cancelled)).unwrap();
        assert_eq!(err["ctapStatus"], 0x2D);
        assert_eq!(err["hresult"].as_u64().unwrap(), NTE_USER_CANCELLED as u64);
        let nf = HelperError::Ctap(0x2E, "x".into());
        assert_eq!(nf.hresult(), NTE_NOT_FOUND);
        assert_eq!(HelperError::Timeout.hresult(), ERROR_CANCELLED);
        assert_eq!(HelperError::Ctap(0x31, "".into()).hresult(), E_FAIL);
    }

    #[test]
    fn request_accepts_float_integers() {
        let r: Request = serde_json::from_str(
            r#"{"protocol":1.0,"command":"getAssertion","rpId":"x","ctapRequest":"oA==","flags":8388608.0,"timeoutMs":120000.0,"filterHybrid":false}"#,
        )
        .unwrap();
        assert_eq!(r.protocol, 1);
        assert_eq!(r.flags, 0x0080_0000);
        assert_eq!(r.timeout_ms, 120_000);
    }
}
