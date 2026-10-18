# freerdp-webauthn-helper

Out-of-process WebAuthn helper for FreeRDP's `rdpewa` channel (see
`../patches/0003-rdpewa-external-webauthn-helper.patch`). For each in-session WebAuthn request
from the Windows session host it shows one GTK3 window with a hybrid/caBLE QR code (phone
passkey, Android or iPhone) and also listens for a USB FIDO2 security key. The first transport
to finish wins. It uses libwebauthn's CTAP2 layer with the caller-supplied `clientDataHash`
and returns the raw CTAP2 response map.

## Build

    cargo build --release      # target/release/freerdp-webauthn-helper
    cargo test

Needs GTK 3 development files (`pkg-config gtk+-3.0`).

## How FreeRDP invokes it

Set `FREERDP_RDPEWA_HELPER` to the absolute path of the binary before starting the FreeRDP
client:

    FREERDP_RDPEWA_HELPER=$PWD/target/release/freerdp-webauthn-helper xfreerdp3 ...

FreeRDP forks the helper per request, writes one JSON line to its stdin and reads one JSON line
from its stdout (stderr is for logs; `RUST_LOG=debug` for more, default `info`).

## Wire protocol (newline-delimited JSON)

Request:

    {"protocol":1,"command":"makeCredential"|"getAssertion","rpId":"login.microsoft.com",
     "ctapRequest":"<base64 CTAP2 request CBOR map>","flags":N,"timeoutMs":N,"filterHybrid":false}

`ctapRequest` is the CTAP2 command's CBOR map (integer keys, without the command byte).
`timeoutMs` 0 means 120000; it is enforced as an overall deadline. `flags` (CTAPCLT_* bits) are
only hints: `0x00400000` UV required, `0x00800000` UV preferred, `0x01000000` UV not required,
`0x00100000` client PIN required. `filterHybrid: true` disables the phone transport. pinAuth
parameters and extensions in the request are ignored.

Success reply:

    {"status":"ok","ctapResponse":"<base64 CTAP2 response map>","provider":"Hybrid"|"Hid",
     "uvStatus":0|1,"transports":N}

- getAssertion map: 1 credential descriptor, 2 authData (raw), 3 signature, 4 `{id}` user (if present).
- makeCredential map: 1 fmt, 2 authData (raw), 3 attStmt.
- `transports`: USB 0x1, HYBRID 0x20. `uvStatus` is 1 when the returned authData has the UV
  flag or a PIN was used.

Error reply: `{"status":"error","ctapStatus":N,"hresult":N,"message":"..."}`

| Condition | ctapStatus | hresult |
|---|---|---|
| user cancel (button / window close / phone cancel) | 0x2D | 0x80090036 NTE_USER_CANCELLED |
| timeout (overall deadline) | 0x2F | 0x800704C7 ERROR_CANCELLED (chosen over NTE_USER_CANCELLED) |
| no credentials | 0x2E | 0x80090011 NTE_NOT_FOUND |
| other CTAP error | the CTAP code | 0x80004005 E_FAIL |
| internal / parse / panic | 0x7F | 0x80004005 E_FAIL |

The helper always prints exactly one reply line and exits 0 (a panic hook prints an error reply).

## Behaviour notes

- Hybrid is used when `!filterHybrid` and a Bluetooth adapter is available. The phone does its
  own UV, so no PIN parameters are sent over hybrid. If the hybrid transport fails to start the
  helper keeps going with USB only.
- USB: devices are re-listed every second. If the key has a PIN (and the request wants UV), the
  window shows a PIN row (Enter submits, 3 tries, remaining attempts shown), the PIN is NFC
  normalised and a pinUvAuthToken is obtained (PIN protocol 2 preferred).
- If GTK cannot initialise, USB still works but PIN prompts fail with CTAP 0x36. Note GTK will
  try the Wayland default socket even with `DISPLAY` unset.
- With several credentials returned by a key, the first assertion is used (no picker).

## Test harness (no FreeRDP needed)

    freerdp-webauthn-helper --test-get  login.microsoft.com   # random hash, empty allow list
    freerdp-webauthn-helper --test-make login.microsoft.com   # rk=true, user "test"
    freerdp-webauthn-helper --version

These run the full UI flow and print the reply JSON on stdout. A makeCredential test creates a
real resident credential on the key/phone.

## Prerequisites

- Hybrid: the `bluetooth` service running (`systemctl status bluetooth`) with an adapter
  powered on; the phone needs Bluetooth and internet.
- USB key: a udev rule granting your user access to the hidraw node, e.g. libfido2's
  `70-u2f.rules` / `70-fido.rules` copied to `/etc/udev/rules.d/` (then
  `udevadm control --reload && udevadm trigger`). Not installed by this project.

## Hybrid transport selection

By default the QR code advertises only the cloud-assisted (WebSocket tunnel) transport, the
path Chrome uses. Set `FREERDP_WEBAUTHN_CABLE=both` to also offer the direct BLE L2CAP
channel (CTAP 2.3); with an Android phone that path connected but stalled at
authentication in testing. Debug with `RUST_LOG=libwebauthn=debug`.

## Host hybrid filter

The helper offers the phone (hybrid) path even when the host sets `filterHybridTransport`
(the AVD host sets it for every real request from this client). Set
`FREERDP_WEBAUTHN_HONOR_HYBRID_FILTER=1` to honour the flag instead.
