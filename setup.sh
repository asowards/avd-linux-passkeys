#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build and install FreeRDP master (sdl-freerdp with rdpewa/WebAuthn redirection) for AVD.
# Idempotent; safe to re-run.
#
# Usage: ./setup.sh [--no-deps] [--update]
#   --no-deps  skip apt-get install
#   --update   git pull --ff-only in the FreeRDP checkout (only if the working tree
#              differs from upstream solely by this project's patches)
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FREERDP_DIR="$HOME/git/FreeRDP"
BUILD_DIR="$FREERDP_DIR/build"
PREFIX=/usr/local

DO_DEPS=1; DO_UPDATE=0; DO_REMMINA=1
for a in "$@"; do
    case "$a" in
        --no-deps) DO_DEPS=0 ;;
        --update)  DO_UPDATE=1 ;;
        --no-remmina) DO_REMMINA=0 ;;
        -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
        *) echo "Unknown option: $a" >&2; exit 2 ;;
    esac
done

log() { printf '\n==> %s\n' "$*"; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

PATCHES=("$PROJECT_DIR"/patches/*.patch)
[ -e "${PATCHES[0]}" ] || die "no patches found in $PROJECT_DIR/patches"

# 1. Dependencies
if [ "$DO_DEPS" = 1 ]; then
    log "Installing build/runtime dependencies"
    sudo apt-get install -y \
        build-essential cmake git pkg-config \
        libssl-dev libx11-dev libxext-dev libxinerama-dev libxcursor-dev libxdamage-dev \
        libxv-dev libxkbfile-dev libasound2-dev libcups2-dev libxml2-dev libpam0g-dev \
        libfido2-dev libcbor-dev libpcsclite-dev libjson-c-dev libpulse-dev libusb-1.0-0-dev \
        libavcodec-dev libavutil-dev libswscale-dev libkrb5-dev \
        libsdl2-dev libsdl2-ttf-dev libwebkit2gtk-4.1-dev \
        libgtk-3-dev libdbus-1-dev libudev-dev libv4l-dev \
        libwayland-dev libxkbcommon-dev libdecor-0-dev libegl1-mesa-dev libgles-dev libdrm-dev libgbm-dev \
        libibus-1.0-dev libfreetype-dev libharfbuzz-dev libxi-dev libxrandr-dev libxss-dev libxfixes-dev \
        libsecret-tools zenity flatpak python3 \
        libgcrypt20-dev libsodium-dev libjson-glib-dev libsecret-1-dev libssh-dev libvte-2.91-dev \
        libayatana-appindicator3-dev libavahi-ui-gtk3-dev libavahi-client-dev intltool libsoup-3.0-dev
else
    log "Skipping dependency install (--no-deps)"
fi

# 2. Flathub + Chrome
log "Checking Flatpak Chrome"
if ! flatpak remotes --columns=name 2>/dev/null | grep -qx flathub; then
    flatpak remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
fi
if ! flatpak info com.google.Chrome >/dev/null 2>&1; then
    flatpak install -y flathub com.google.Chrome
else
    echo "com.google.Chrome already installed"
fi

# 2b. SDL3 + SDL3_ttf (Ubuntu 24.04 has no packages; FreeRDP's maintained SDL3 client needs them)
SDL_TAG=release-3.4.18
SDLTTF_TAG=release-3.2.2
build_sdl() {
    local name="$1" url="$2" tag="$3" dir="$HOME/git/$1"; shift 3
    if [ ! -d "$dir/.git" ]; then
        log "Cloning $name ($tag)"
        git clone --depth 1 --branch "$tag" "$url" "$dir"
    fi
    log "Building $name"
    cmake -S "$dir" -B "$dir/build" -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$PREFIX" "$@"
    cmake --build "$dir/build" -j"$(nproc)"
    log "Installing $name to $PREFIX (sudo)"
    sudo cmake --install "$dir/build"
}
if ! pkg-config --atleast-version=3.2 sdl3 2>/dev/null; then
    build_sdl SDL https://github.com/libsdl-org/SDL.git "$SDL_TAG" \
        -DSDL_SHARED=ON -DSDL_STATIC=OFF -DSDL_TEST_LIBRARY=OFF -DSDL_TESTS=OFF -DSDL_EXAMPLES=OFF
    build_sdl SDL_ttf https://github.com/libsdl-org/SDL_ttf.git "$SDLTTF_TAG"
    sudo ldconfig
else
    log "SDL3 $(pkg-config --modversion sdl3) already installed"
fi

# 3. FreeRDP source
if [ ! -d "$FREERDP_DIR/.git" ]; then
    log "Cloning FreeRDP into $FREERDP_DIR"
    mkdir -p "$(dirname "$FREERDP_DIR")"
    git clone https://github.com/FreeRDP/FreeRDP.git "$FREERDP_DIR"
elif [ "$DO_UPDATE" = 1 ]; then
    log "Updating FreeRDP checkout"
    cd "$FREERDP_DIR"
    if [ -n "$(git diff --name-only)" ]; then
        # The tree must differ from HEAD only by our patches: every patch that is
        # applied must be reversible, and reversing all of them must leave a clean tree.
        for p in "${PATCHES[@]}"; do
            git apply --reverse --check "$p" 2>/dev/null \
                || die "working tree has changes that are not exactly this project's patches (failed: $(basename "$p")). Resolve manually (git status / git diff), then re-run."
        done
        # Reverse in reverse order, then confirm nothing else is left.
        for ((i=${#PATCHES[@]}-1; i>=0; i--)); do git apply --reverse "${PATCHES[$i]}"; done
        if [ -n "$(git diff --name-only)" ]; then
            for p in "${PATCHES[@]}"; do git apply "$p" 2>/dev/null || true; done
            die "tree has extra modifications beyond the patches; patches re-applied, nothing pulled."
        fi
    fi
    git pull --ff-only
else
    log "FreeRDP checkout exists; leaving it alone (use --update to pull)"
fi

# 4. Patches
log "Applying patches"
cd "$FREERDP_DIR"
for p in "${PATCHES[@]}"; do
    name="$(basename "$p")"
    if git apply --reverse --check "$p" >/dev/null 2>&1; then
        echo "already applied: $name"
    elif git apply --check "$p" >/dev/null 2>&1; then
        git apply "$p"
        echo "applied:         $name"
    else
        cat >&2 <<MSG
ERROR: $name does not apply cleanly to $FREERDP_DIR.
Either upstream changed the same code (the patch may already be merged in a different
form, or needs rebasing), or the tree has local changes. To investigate:
    cd $FREERDP_DIR && git status && git apply --check -v $p
If upstream already contains the fix, delete the patch from $PROJECT_DIR/patches and re-run.
MSG
        exit 1
    fi
done

# 5. Configure, build, install
log "Configuring"
cmake -S "$FREERDP_DIR" -B "$BUILD_DIR" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$PREFIX" \
    -DCHANNEL_RDPEWA=ON \
    -DCHANNEL_RDPECAM_CLIENT=ON \
    -DWITH_AAD=ON \
    -DWITH_CLIENT_SDL=ON \
    -DWITH_CLIENT_SDL3=ON \
    -DWITH_CLIENT_SDL2=OFF \
    -DWITH_XDG_AAD_AUTH_HELPER=ON \
    -DWITH_WEBVIEW_AAD_AUTH_HELPER=ON \
    -DWITH_X11=ON \
    -DWITH_WAYLAND=ON \
    -DWITH_PCSC=ON \
    -DWITH_PULSE=ON \
    -DWITH_ALSA=ON \
    -DWITH_CUPS=ON \
    -DWITH_FFMPEG=ON \
    -DWITH_SWSCALE=ON \
    -DWITH_KRB5=ON

log "Building"
cmake --build "$BUILD_DIR" -j"$(nproc)"

log "Installing to $PREFIX (sudo)"
sudo cmake --install "$BUILD_DIR"
sudo ldconfig

# 5b. Phone-passkey helper (optional; needs a Rust toolchain >= 1.88)
if command -v cargo >/dev/null 2>&1; then
    log "Building freerdp-webauthn-helper (phone passkeys in-session)"
    (cd "$PROJECT_DIR/helper" && cargo build --release) \
        || echo "WARNING: helper build failed; in-session WebAuthn falls back to libfido2 (USB key only)"
else
    log "cargo not found; skipping freerdp-webauthn-helper (install rustup to enable phone passkeys)"
fi

# 5c. Remmina from master with the external-browser Entra sign-in patch (installs to /usr/local;
#     Ubuntu's packaged Remmina in /usr/bin is left alone). Skip with --no-remmina.
if [ "${DO_REMMINA:-1}" = 1 ]; then
    REMMINA_DIR="$HOME/git/Remmina"
    if [ ! -d "$REMMINA_DIR/.git" ]; then
        log "Cloning Remmina"
        git clone https://gitlab.com/Remmina/Remmina.git "$REMMINA_DIR"
    fi
    cd "$REMMINA_DIR"
    if ! git rev-parse --verify avd-external-aad-login >/dev/null 2>&1; then
        git checkout -q -b avd-external-aad-login master
    else
        git checkout -q avd-external-aad-login
    fi
    for p in "$PROJECT_DIR"/patches/remmina/*.patch; do
        if git apply --reverse --check "$p" >/dev/null 2>&1; then
            echo "already applied: $(basename "$p")"
        else
            git am -q "$p" || die "Remmina patch $(basename "$p") failed to apply (git am --abort to reset)"
            echo "applied:         $(basename "$p")"
        fi
    done
    log "Configuring and building Remmina"
    PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig" cmake -S "$REMMINA_DIR" -B "$REMMINA_DIR/build" \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$PREFIX" -DCMAKE_PREFIX_PATH="$PREFIX" \
        -DWITH_FREERDP3=ON -DWITH_RDP_AUTH_AAD=ON -DWITH_LIBVNCSERVER=OFF -DWITH_GVNC=OFF \
        -DWITH_NEWS=OFF -DWITH_STATS=OFF -DWITH_TIP=OFF -DWITH_MANPAGES=OFF -DWITH_X2GO=OFF -DWITH_WWW=OFF
    cmake --build "$REMMINA_DIR/build" -j"$(nproc)"
    log "Installing Remmina to $PREFIX (sudo)"
    sudo cmake --install "$REMMINA_DIR/build"
    cd "$PROJECT_DIR"
fi

# 6. Launcher symlink
log "Linking avd-connect"
mkdir -p "$HOME/.local/bin"
LINK="$HOME/.local/bin/avd-connect"
if [ -e "$LINK" ] && [ ! -L "$LINK" ]; then
    mv "$LINK" "$LINK.bak.$(date +%Y%m%d%H%M%S)"
    echo "backed up existing file to $LINK.bak.*"
fi
ln -sfn "$PROJECT_DIR/bin/avd-connect" "$LINK"
ln -sfn "$PROJECT_DIR/bin/remmina-avd" "$HOME/.local/bin/remmina-avd"
ln -sfn "$PROJECT_DIR/bin/avd-profile" "$HOME/.local/bin/avd-profile"
chmod +x "$PROJECT_DIR/bin/avd-connect"
echo "$LINK -> $PROJECT_DIR/bin/avd-connect"

# 6b. SDL client preferences (Ctrl+Alt shortcuts, no connection dialog)
mkdir -p "$HOME/.config/freerdp"
ln -sfn "$PROJECT_DIR/config/sdl-freerdp.json" "$HOME/.config/freerdp/sdl-freerdp.json"
echo "~/.config/freerdp/sdl-freerdp.json -> $PROJECT_DIR/config/sdl-freerdp.json"

# 7. Remove remnants of the old setup
log "Cleaning up old setup"
for old in "$HOME/.local/bin/fido2-pinentry" "$HOME/.local/share/freerdp-chrome-config"; do
    if [ -e "$old" ] || [ -L "$old" ]; then
        echo "removing $old"
        rm -rf -- "$old"
    fi
done

# 8. Done
cat <<MSG

Done.

Connect with:
    avd-connect /path/to/session.rdpw [username] [domain]
e.g.
    avd-connect ~/Downloads/session.rdpw <user> <DOMAIN>

Make sure ~/.local/bin is in PATH. Optional env vars: AVD_BROWSER, AVD_CERT_IGNORE=1,
AVD_DEBUG=1, AVD_WEBAUTHN_HELPER. See README.md.
MSG
