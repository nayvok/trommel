#!/bin/bash
# Install Trommel from GitHub Releases.
#
#   curl -fsSLO https://github.com/nayvok/trommel/releases/latest/download/trommel-install.sh
#   bash trommel-install.sh
#
# No apt repository is used: the script downloads the .deb of the latest
# release, verifies its SHA-256 and installs it with apt. Run it as your desktop
# user; apt asks for the sudo password once.
set -euo pipefail

REPO="${TROMMEL_REPO:-nayvok/trommel}"
BASE="${TROMMEL_DOWNLOAD_BASE:-https://github.com/$REPO/releases/latest/download}"
DEB="trommel_amd64.deb"
SHA="$DEB.sha256"
APT_FLAGS="${TROMMEL_APT_FLAGS:-}"

say() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

command -v apt-get >/dev/null || die "this installer supports Debian/Ubuntu (apt-get)"
command -v sha256sum >/dev/null || die "sha256sum is required"
if command -v curl >/dev/null; then
    download() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null; then
    download() { wget -qO "$2" "$1"; }
else
    die "curl or wget is required"
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

say "Downloading $DEB"
download "$BASE/$DEB" "$TMP/$DEB" || die "cannot download $BASE/$DEB (is the release published?)"
download "$BASE/$SHA" "$TMP/$SHA" || die "cannot download $BASE/$SHA"
( cd "$TMP" && sha256sum -c "$SHA" ) || die "SHA-256 verification failed"

say "Installing $DEB"
if [ "$(id -u)" -eq 0 ]; then
    # shellcheck disable=SC2086
    apt-get install -y $APT_FLAGS "$TMP/$DEB"
else
    command -v sudo >/dev/null || die "sudo is required for the package installation"
    # shellcheck disable=SC2086
    sudo apt-get install -y $APT_FLAGS "$TMP/$DEB"
fi

say "Done"
cat <<'TXT'
Trommel is installed. Open it from the application menu (or run "trommel").
The GNOME panel applet is included; log out and back in on Wayland, then enable
it in Trommel -> Settings -> GNOME integration.
Config: ~/.config/trommel/config.json
TXT
