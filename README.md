# AVD Linux Passkeys

Connect to Azure Virtual Desktop (AVD) from Ubuntu with Entra sign-in and WebAuthn
(security key / phone passkey) support. **Remmina (master, patched) is the preferred
client**; `avd-connect` (xfreerdp / sdl-freerdp) is the command-line fallback.

Not affiliated with or endorsed by Microsoft. "Azure", "Entra" and "Windows App" are
Microsoft trademarks.

## What this is

- **Remmina** (GTK client, built from master into `/usr/local`) with one patch that routes
  the Entra sign-in through FreeRDP's AAD helper and Flatpak Chrome (phone passkeys work
  there). Launch it with `bin/remmina-avd`; create profiles with `bin/avd-profile`. See
  [Remmina (preferred client)](#remmina-preferred-client).
- FreeRDP master built from source into `/usr/local`; Remmina links it. Its `rdpewa`
  channel provides WebAuthn redirection; the phone-passkey helper owns the PIN/QR/touch
  window, so no FreeRDP dialog is needed for WebAuthn.
- `bin/avd-connect` launches `xfreerdp` (or `sdl-freerdp` with `AVD_CLIENT=sdl`) from an
  `.rdpw` file, for scripting or when Remmina is not wanted.
- Phone passkeys for in-session WebAuthn: `helper/` (Rust + libwebauthn) answers the
  session host's CTAP requests with a phone over hybrid/caBLE (QR code) or a USB key,
  and FreeRDP calls it through patch 0003. Works with both clients. See Status.
- `setup.sh` reproduces the whole build, Remmina included.

## Why not the alternatives

**Stock Remmina.** Ubuntu's Remmina 1.4.43 has no ARM gateway support, no FIDO PIN prompt
and no WebAuthn toggle. Remmina master has all three, but its Entra login is an embedded
WebKitGTK view, and WebKitGTK has no WebAuthn implementation (WebKit bug 205350), so
passkey sign-in fails; hence the patched build above. Flathub's Remmina bundles FreeRDP
3.20 without rdpewa.

**Microsoft's web client.** It has no WebAuthn redirection (Windows App platform
comparison lists the web browser as unsupported).

**Chrome relay for in-session requests.** The session host sends a CTAP request with a
32-byte `clientDataHash`. A browser's WebAuthn API builds its own clientDataJSON and
cannot sign a supplied hash (Palo Alto Unit 42 forked Chromium to work around this).
Chromium's `remoteDesktopClientOverride` is also gated by the
`WebAuthenticationRemoteDesktopAllowedOrigins` policy, documented only for
Windows/macOS/Android and only for affiliated users.

## Remmina (preferred client)

Quick start: `./setup.sh`, then `bin/avd-profile ~/Downloads/session.rdpw <user> <domain> [name]`,
then start "Azure Virtual Desktop (Remmina)" from the app menu (or `bin/remmina-avd`) and
open the profile.

Remmina master, built from `~/git/Remmina` on branch `avd-external-aad-login` into
`/usr/local` (Ubuntu's 1.4.43 in `/usr/bin` is untouched), with one patch
(`patches/remmina/0001-…`): a profile option that routes the Entra ID sign-in through
FreeRDP's exported AAD auth helper (system browser, passkey-capable) instead of Remmina's
embedded WebKitGTK view, plus an optional browser command. Everything else AVD needs is
already in Remmina master: ARM gateway transport, RDSTLS password prompt with keyring
storage, FIDO2 PIN dialog, "Share WebAuthn" toggle, multi-monitor. In-session passkeys use
the same `freerdp-webauthn-helper`, because Remmina links the `/usr/local` libfreerdp.

- `bin/remmina-avd`: launches `/usr/local/bin/remmina` with the helper environment
  (`FREERDP_RDPEWA_*`). Desktop entry: `config/avd-remmina.desktop` ("Azure Virtual Desktop
  (Remmina)", installed to `~/.local/share/applications`).
- `bin/avd-profile <rdpw> <user> <domain> [name]`: writes `~/.local/share/remmina/avd_<name>.remmina`
  from the AVD `.rdpw` with gateway, load-balance info, sign-in browser, WebAuthn and multimon set.
- `setup.sh` clones/patches/builds/installs Remmina too (`--no-remmina` skips it).
- Profile notes: quality 9 (anything lower drops wallpaper and shows drag outlines), scale
  "dynamic resolution" + viewport fullscreen (a scaled first frame looks squished until
  you toggle fullscreen), `dvc=rdpecam` for camera redirection, `sound=local` also enables
  the microphone. Remmina profiles can only be edited while disconnected. The system
  Remmina 1.4.43 (`/usr/bin/remmina`) cannot connect to AVD; `remmina-avd` refuses to start
  while it is running.

## Command-line client: sdl-freerdp / xfreerdp (fallback)

`xfreerdp` runs through XWayland, which on this NVIDIA machine shows ghosting and late
repaints. FreeRDP's maintained SDL3 client runs natively on Wayland and has clipboard
support (the SDL2 one has none and is deprecated). Ubuntu 24.04 ships no SDL3, so
`setup.sh` clones and builds SDL 3.4.18 and SDL_ttf 3.2.2 into `/usr/local` first, then
builds FreeRDP with `WITH_CLIENT_SDL3=ON WITH_CLIENT_SDL2=OFF`; the binary is still
`/usr/local/bin/sdl-freerdp`. Select it with `AVD_CLIENT=sdl`. Its Windows password comes
from `bin/avd-askpass` (keyring, then zenity) via FreeRDP's `FREERDP_ASKPASS` hook, so no
in-client credential dialog appears.

Keyboard shortcuts (SDL3 client, from `config/sdl-freerdp.json`, linked to
`~/.config/freerdp/sdl-freerdp.json`): the modifier is **Ctrl+Alt** (FreeRDP's default is
Right Shift, which makes a right-shifted capital D disconnect the session). Ctrl+Alt+Enter
toggles fullscreen, Ctrl+Alt+M minimizes, Ctrl+Alt+R toggles window resizing, Ctrl+Alt+G
toggles keyboard/mouse grab, Ctrl+Alt+D disconnects. The connection-progress popup is
disabled there and via `-sdl-connection-dialog`.

## Layout

```
setup.sh                         deps, clone, patch, build, install, link launcher
bin/remmina-avd                  preferred: Remmina launcher with the helper environment
bin/avd-profile                  writes a Remmina profile from an .rdpw
config/avd-remmina.desktop       desktop entry for remmina-avd
bin/avd-connect                  fallback CLI launcher (symlinked to ~/.local/bin/avd-connect)
patches/0001-rdpewa-allow-multiple-channel-instances.patch
patches/0002-xdg-aad-helper-browser-override.patch
patches/0003-rdpewa-external-webauthn-helper.patch
patches/0004-sdl2-auth-dialog-handle-rdstls.patch
helper/                          Rust phone-passkey helper (freerdp-webauthn-helper)
```

The FreeRDP checkout lives in `~/git/FreeRDP` (build dir `~/git/FreeRDP/build`,
Unix Makefiles generator).

## Setup

```
./setup.sh              # full run
./setup.sh --no-deps    # skip apt-get
./setup.sh --update     # git pull --ff-only first
```

The script uses `sudo` for `apt-get`, `cmake --install` and `ldconfig`. It installs
dependencies, ensures Flathub and `com.google.Chrome` are present, clones FreeRDP if
missing, applies the patches (skipping ones already applied), configures with
`-DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr/local -DCHANNEL_RDPEWA=ON
-DWITH_AAD=ON -DWITH_CLIENT_SDL=ON -DWITH_CLIENT_SDL2=ON -DWITH_XDG_AAD_AUTH_HELPER=ON ...`,
builds, installs, symlinks `avd-connect` into `~/.local/bin`, and removes the old
`~/.local/bin/fido2-pinentry` and `~/.local/share/freerdp-chrome-config`.

`--update` pulls only if the working tree differs from upstream solely by these patches.
It reverses the patches, pulls, and the next step re-applies them. If a patch no longer
applies (for example upstream merged a fix), remove it from `patches/`.

`WITH_FIDO2` and `WITH_RDPEWA` are not real FreeRDP options. The real switch is
`CHANNEL_RDPEWA=ON`; libfido2 is detected automatically.

## Connecting (CLI fallback)

The preferred route is Remmina (above); this section covers `avd-connect`.

```
avd-connect <session.rdpw> [username] [domain] [extra freerdp args...]
avd-connect ~/Downloads/session.rdpw <user> <DOMAIN>
```

Windows password (the RDSTLS logon after the gateway redirect): with `xfreerdp` the
launcher reads it from the GNOME keyring if stored, otherwise asks with a zenity dialog,
and hands it to FreeRDP over stdin (`/from-stdin`), never on the command line. To store it:

```
secret-tool store --label='AVD password' service avd-connect user <user>
```

Download the `.rdpw` from the AVD web client. The launcher works on a sanitized copy
(CRs stripped, a `redirectwebauthn:i:1dynamic` run-on line split); the original is never
modified.

| Variable | Effect |
|---|---|
| `AVD_CLIENT` | `xfreerdp` (default) or `sdl`. |
| `AVD_WINDOWED=1` | Stay windowed instead of multi-monitor fullscreen (`use multimon:i:1` in the .rdpw). |
| `AVD_VIDEO=1` | Keep video-optimized remoting (geometry/video channels) enabled; off by default because AVD reopens the geometry channel and the client logs errors. |
| `AVD_IDENTIFY_AS_WINDOWS` | Default 0 (experiment; no effect on the host's hybrid filtering): advertise a Windows 11 client (RDP OS type, client build `AVD_CLIENT_BUILD`=26100, gateway user agents via `/tune:`) (RDP OS type, gateway user agents via `/tune:`) so the host offers the phone/QR passkey option; set 0 to send FreeRDP's own identity. `AVD_CLIENT_UA` overrides the user-agent string. |
| `AVD_GFX` | Graphics codec: `AVC444` (default), `AVC420`, `RFX`, `progressive`, or `off` for FreeRDP defaults. |
| `AVD_ASYNC_UPDATE` | Default 1: `+async-update` (screen updates off the X11 event loop). |
| `AVD_ASYNC_CHANNELS=1` | Add `+async-channels`. |
| `AVD_GDI` | `hw` (XRender) or `sw` rendering backend; unset = FreeRDP default. |
| `AVD_NETWORK` | `/network:` profile, e.g. `lan` or `auto`. |
| `AVD_NO_KEYRING=1` | Do not read the Windows password from the keyring. |
| `AVD_BROWSER` | Browser command for Entra sign-in. Default `flatpak run com.google.Chrome`. Exported as `FREERDP_AAD_BROWSER`. |
| `AVD_CERT_IGNORE=1` | Pass `/cert:ignore`. Off by default; try without it first. |
| `AVD_DEBUG=1` | Log rdpewa and AAD channels at DEBUG. |
| `AVD_SDL_VIDEODRIVER` | SDL backend; defaults to `wayland` on Wayland sessions (NVIDIA + XWayland GLX crash). |
| `AVD_SDL_SOFTWARE=1` | Force SDL software rendering. |
| `AVD_WEBAUTHN_API_VERSION` | WebAuthn API version advertised to the host when the helper is present (default 6; 4 = FreeRDP default, no phone option). |
| `AVD_WEBAUTHN_IUVPAA` | Answer to the host's "platform authenticator available?" query when the helper is present (default 0). |
| `AVD_WEBAUTHN_HELPER` | Path to `freerdp-webauthn-helper`. Default `helper/target/release/freerdp-webauthn-helper`; used only if it exists and is executable. |

## How the pieces fit

Sign-in:

1. `sdl-freerdp` contacts the ARM gateway and needs an Entra token.
2. It starts the AAD auth helper (`/usr/local/libexec`, xdg variant).
3. The helper opens the login URL with the command in `FREERDP_AAD_BROWSER`
   (Flatpak Chrome) and registers `freerdp-aad-handler.desktop` as the
   `x-scheme-handler/ms-appx-web` handler.
4. After sign-in, Entra redirects to `ms-appx-web://...`; the desktop handler passes the
   result back to the helper and on to FreeRDP, which connects.

In session:

1. The Windows host opens the WebAuthn dynamic channel (`rdpewa`).
2. FreeRDP forwards CTAP requests to libfido2, which talks to a USB/NFC security key.
3. With the helper: one GTK window (QR for phone, or touch the key, PIN entry).
   Without it (libfido2 fallback): the SDL client shows its own PIN dialog; `xfreerdp`
   prompts in the terminal.

Phone passkeys (patch 0003): when `FREERDP_RDPEWA_HELPER` is set, rdpewa hands each
MakeCredential/GetAssertion request to `freerdp-webauthn-helper` as one JSON line on
stdin (base64 CTAP2 request map, rpId, flags, timeout, `filterHybrid`). The helper
shows one GTK window with a QR code (scan with an Android or iPhone camera) and
"or touch your security key", races the hybrid and USB transports, handles the USB PIN
itself, and returns the raw CTAP2 response map, which FreeRDP sends back unchanged.
It works at the CTAP level with the host-supplied `clientDataHash`, so it does not
depend on the session host version. Without the variable, or if the helper cannot be
started, the libfido2 path is used unchanged. Protocol details: `helper/README.md`.

## Patches

- `0001-rdpewa-allow-multiple-channel-instances.patch`: non-test hunks of
  [FreeRDP PR #13757](https://github.com/FreeRDP/FreeRDP/pull/13757), fixing
  [issue #13741](https://github.com/FreeRDP/FreeRDP/issues/13741). AVD/W365 hosts open
  the WebAuthn channel up to three times concurrently; the generic DVC guard rejects the
  second open ("Channel already connected"). The patch adds a per-plugin
  `allowMultipleInstances` flag, enabled only for rdpewa. Drop it when upstream merges a
  final version.
- `0002-xdg-aad-helper-browser-override.patch`: `xdg-aad-helper.py` honors
  `FREERDP_AAD_BROWSER` (shell-split command; `%u` placeholder or URL appended) and falls
  back to `xdg-open`. This avoids overriding `XDG_CONFIG_HOME` for the whole process and
  leaves the system default browser alone. Candidate for upstreaming.
- `0003-rdpewa-external-webauthn-helper.patch`: adds `rdpewa_helper.c/.h` (spawn the
  helper named by `FREERDP_RDPEWA_HELPER`, JSON over pipes, base64 CTAP payloads), hooks
  it into the rdpewa worker thread ahead of libfido2, and decodes the MS-RDPEWA 3.0
  `filterHybridTransport` request key. Linux-only source file (`if(NOT WIN32)`).
- `0005-rdpecam-encode-option-and-realtime-preset.patch`: camera channel `encode:<0|1>`
  argument (0 = send the webcam's MJPEG/raw samples unchanged instead of x264-encoding on
  the CPU) and the "veryfast" x264 preset for realtime camera encoding. Candidate PR.
- `0004-sdl2-auth-dialog-handle-rdstls.patch`: the SDL2 client's credential dialog had no
  case for `AUTH_RDSTLS`, the prompt the AVD gateway redirect uses to collect the Windows
  password, so it returned failure instantly and the connection died with
  "error retrieving ARM configuration". Ports the SDL3 client's handling. Upstream bug;
  candidate PR.

## License

GPL-3.0-or-later (see `LICENSE`) for the scripts, configs and `helper/`. Each patch under
`patches/` is a derivative of its upstream project and stays under that project's license:
`patches/*.patch` are FreeRDP (Apache-2.0), `patches/remmina/` are Remmina
(GPL-2.0-or-later with the OpenSSL exception). The helper links libwebauthn
(LGPL-2.1-or-later) and GTK (LGPL).

## Troubleshooting

- `Channel already connected` / error 548 in-session: patch 0001 is missing or
  FreeRDP was not rebuilt after applying it. Run `./setup.sh --no-deps`.
- Browser sign-in completes but nothing returns to FreeRDP: check
  `xdg-mime query default x-scheme-handler/ms-appx-web`. It must print
  `freerdp-aad-handler.desktop`. If not, run
  `xdg-mime default freerdp-aad-handler.desktop x-scheme-handler/ms-appx-web`
  and check `~/.config/mimeapps.list`.
- Black frames / late repaints in fullscreen: try in this order, one change per session:
  `AVD_GFX=RFX` (RemoteFX instead of H.264), `AVD_GDI=hw` (XRender painting), `AVD_NETWORK=lan`,
  `AVD_ASYNC_CHANNELS=1`. The XWayland + NVIDIA path is also a suspect; `AVD_CLIENT=sdl` runs
  natively on Wayland for comparison. RDP Shortpath (UDP) is not available in FreeRDP, so the
  session always runs over the TCP/WebSocket gateway.
- Camera feed slow/choppy: by default FreeRDP re-encodes the webcam to H.264 with x264 on
  the CPU. Patch 0005 adds `encode:0` to the `rdpecam` channel (MJPEG passthrough) and a
  realtime preset when encoding. The launcher sets it via the `.rdp` key
  `encode redirected video capture:i:0`; in Remmina use `dvc=rdpecam,encode:0`.
  `AVD_CAMERA_ENCODE=1` / `encode:1` restores H.264 if the host rejects MJPEG.
- Camera: needs the build with `CHANNEL_RDPECAM_CLIENT=ON` (setup.sh does this; V4L backend,
  H.264 via FFmpeg). The .rdpw's `camerastoredirect:s:*` then redirects any `/dev/video*`
  camera; check `ls /dev/video*` on the Linux side first. Microphone redirection works
  through the `audin` channel with the PulseAudio/PipeWire backend (`audiocapturemode:i:1`).
- Wrong browser opens: set `AVD_BROWSER`, and confirm patch 0002 is applied.
- Certificate errors: retry with `AVD_CERT_IGNORE=1` (insecure; only if needed).
- Debugging: `AVD_DEBUG=1 avd-connect ...` enables
  `/log-filters:com.freerdp.channels.rdpewa.client:DEBUG,com.freerdp.client.common.aadauth:DEBUG,com.freerdp.aad:DEBUG`
  at log level INFO. Extra FreeRDP args can be appended to the command line.
- `X Error of failed request: BadValue ... X_GLXCreateContext` right after start: SDL2
  chose the X11 backend (XWayland) and the NVIDIA driver cannot create a GLX context
  there. The launcher now sets `SDL_VIDEODRIVER=wayland` on Wayland sessions. If the
  window stays black, try `AVD_SDL_SOFTWARE=1` (software rendering) or
  `AVD_SDL_VIDEODRIVER=x11` (the launcher then also sets `__GLX_VENDOR_LIBRARY_NAME=mesa`,
  which is what makes GLX work on XWayland with the NVIDIA driver).
- `error retrieving ARM configuration` right after the Chrome sign-in, with no other error:
  the client could not collect the Windows password (RDSTLS). Patch 0004 fixes the SDL2
  dialog; otherwise run with `AVD_DEBUG=1` and look at the `core.gateway.arm` lines.
- `geometry.client ... Channel already connected` errors after logon: AVD reopens the
  video-geometry channel too; harmless for sign-in (only affects multimedia redirection).
- Helper window says "Phone sign-in not available on this machine": the host sent
  `filterHybridTransport`, which it does when the client advertises a WebAuthn API version
  below 6. The launcher exports `FREERDP_RDPEWA_API_VERSION=6` whenever the helper is found;
  check that the installed FreeRDP has the current patch 0003.
- Phone QR window never appears, only a PIN prompt: the helper was not found. The launcher
  prints a warning with the path it looked at; build it with `cd helper && cargo build --release`.
- No PIN dialog: make sure the client is `sdl-freerdp` (the X11 client has no FIDO PIN
  dialog).

## Status / TODO

Done: Remmina master with external Entra sign-in (preferred client), FreeRDP master build, rdpewa with the multi-instance fix, Chrome sign-in via
`FREERDP_AAD_BROWSER`, YubiKey in-session via libfido2 and the SDL PIN dialog.

Phone passkeys in-session: working (2026-10-17). The AVD session host attaches
`filterHybridTransport: true` to every real GetAssertion/MakeCredential request from this
client (it is `false` on the preliminary IUVPAA/GET_CREDENTIALS queries), independent of the
advertised WebAuthn API version, client identity, platform-authenticator answer, browser mode
or site. The host also probes a dozen undocumented Windows-App-only dynamic channels (e.g.
`com.microsoft.rdc.dvc.cpc.1`, `Microsoft::Windows::RDS::Notify`), which is where that
capability is presumably negotiated. Since the channel format is authenticator-agnostic,
the helper offers the phone path regardless of the flag (host-pool owner's decision) and the
host accepts the resulting assertion. `FREERDP_WEBAUTHN_HONOR_HYBRID_FILTER=1` restores
honouring the flag.

Helper verification done:

- `freerdp-webauthn-helper` (`helper/`): libwebauthn 0.10 CTAP2 layer with the
  host-supplied hash, GTK3 window, hybrid + USB race, USB PIN flow. `cargo test`
  passes; `--test-get <rpId>` / `--test-make <rpId>` exercise it without FreeRDP
  (`--test-make` creates a real resident credential on the phone/key).
- Patch 0003 compiled into the FreeRDP build; the launcher exports
  `FREERDP_RDPEWA_HELPER` automatically when `helper/target/release/freerdp-webauthn-helper`
  exists.

Verification order: (1) `cd helper && cargo run --example cable_spike` against the
phone; (2) `helper/target/release/freerdp-webauthn-helper --test-get login.microsoft.com`
(expect a QR window; scanning will fail at the RP, that only proves the transport);
(3) in-session login.microsoftonline.com -> security key -> QR window -> phone; then the
same with the YubiKey; then Cancel; (4) `AVD_WEBAUTHN_HELPER=/nonexistent avd-connect ...`
to confirm the libfido2 fallback still works.

Known gaps: no credential picker when a key holds several passkeys for the RP (first is
used); no built-in UV (fingerprint) on USB keys, PIN/touch only; iPhone hybrid is only
regression-tested upstream in libwebauthn; SIGTERM gives no reply line.

Out of scope: Chrome relay.
