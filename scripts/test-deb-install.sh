#!/bin/bash
# Install, upgrade and remove the built .deb the way a user does.
#
# The package is system-wide and its maintainer scripts write under /etc, so
# this script requires root and must run in a disposable container (or VM):
#   docker run --rm -v "$PWD:$PWD" -w "$PWD" ubuntu:26.04 \
#     bash -c 'apt-get update && apt-get install -y dpkg-dev && ./scripts/test-deb-install.sh'
#
# It proves that postinst writes the per-user polkit rule, prerm/postrm keep the
# user config, an upgrade replaces the package without losing the rule, and a
# remove/purge leaves no generated rule behind.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${TROMMEL_VERSION:-$(cd "$ROOT" && cargo metadata --no-deps --format-version 1 --locked 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])' 2>/dev/null || echo 0.0.0)}"
DEB="${1:-$ROOT/dist/trommel_${VERSION}_amd64.deb}"

[ "$(id -u)" -eq 0 ] || { echo "error: run as root in a disposable container" >&2; exit 2; }
command -v apt-get >/dev/null || { echo "error: apt-get is required" >&2; exit 2; }
command -v dpkg-deb >/dev/null || { echo "error: dpkg-deb is required" >&2; exit 2; }
[ -f "$DEB" ] || { echo "missing $DEB; run scripts/package-deb.sh" >&2; exit 1; }
# apt-get only treats an argument as a local file when it is absolute or starts
# with "./"; resolve it once so both the default and a user-supplied path work.
DEB="$(cd "$(dirname "$DEB")" && pwd)/$(basename "$DEB")"

WORK="$(mktemp -d)"
TEST_USER="trommeltest"
FAILS=0
fail() { echo "error: $*" >&2; FAILS=$((FAILS + 1)); }

cleanup() {
    rm -rf "$WORK"
}
trap cleanup EXIT

useradd -m -s /bin/bash "$TEST_USER" 2>/dev/null || true
CONFIG_DIR="/home/$TEST_USER/.config/trommel"
AUTOSTART_LINK="/home/$TEST_USER/.config/systemd/user/graphical-session.target.wants/trommeld.service"
mkdir -p "$CONFIG_DIR"
printf '{"schema_version":3}\n' > "$CONFIG_DIR/config.json"
chown -R "$TEST_USER:$TEST_USER" "/home/$TEST_USER/.config"

apt_install() {
    local deb="$1" log="$2"
    DEBIAN_FRONTEND=noninteractive SUDO_USER="$TEST_USER" \
        apt-get install -y --no-install-recommends "$deb" > "$log" 2>&1 || {
        tail -40 "$log" >&2
        fail "apt-get install failed: $deb"
        return 1
    }
}

rule_file() { echo /etc/polkit-1/rules.d/50-trommel.rules; }

# --- install -----------------------------------------------------------------
apt_install "$DEB" "$WORK/install.log" || exit 1
INSTALLED_VERSION="$(dpkg-query -W -f='${Version}' trommel 2>/dev/null || true)"
[ "$INSTALLED_VERSION" = "$VERSION" ] || fail "installed version is '$INSTALLED_VERSION', expected $VERSION"

for path in \
    /usr/bin/trommel /usr/bin/trommeld /usr/libexec/trommel/trommel-oc \
    /usr/share/applications/io.github.nayvok.Trommel.desktop \
    /usr/share/metainfo/io.github.nayvok.Trommel.metainfo.xml \
    /usr/share/polkit-1/actions/io.github.nayvok.trommel.policy \
    /usr/share/trommel/50-trommel.rules.in \
    /usr/lib/systemd/user/trommeld.service \
    /usr/lib/systemd/user/trommel-autostart.service \
    /usr/share/dbus-1/services/io.github.nayvok.Trommel.Daemon.service
do
    [ -e "$path" ] || fail "missing after install: $path"
done

[ "$(stat -c '%a' /usr/libexec/trommel/trommel-oc)" = "755" ] \
    || fail "helper is not mode 755 after install"
[ "$(stat -c '%U' /usr/libexec/trommel/trommel-oc)" = "root" ] \
    || fail "helper is not root-owned after install"

RULE="$(rule_file)"
[ -f "$RULE" ] || fail "postinst did not write the per-user polkit rule"
grep -q "subject.user !== \"$TEST_USER\"" "$RULE" 2>/dev/null \
    || fail "per-user polkit rule does not name $TEST_USER"
[ -f "$CONFIG_DIR/config.json" ] || fail "install removed the user config"
[ -L "$AUTOSTART_LINK" ] || fail "postinst did not enable trommeld.service at login"
[ "$(readlink "$AUTOSTART_LINK" 2>/dev/null)" = "/usr/lib/systemd/user/trommeld.service" ] \
    || fail "autostart link points at the wrong unit"

# A locally built package must not carry the builder's home directory.
if grep -aq '/home/' /usr/bin/trommel /usr/bin/trommeld; then
    fail "builder home path embedded in the installed binaries"
fi

# --- upgrade -----------------------------------------------------------------
# Rebuild the same payload with a higher version so dpkg runs the real upgrade
# path (old prerm with "upgrade", new postinst configure).
cp "$DEB" "$WORK/upgrade.deb"
dpkg-deb -R "$WORK/upgrade.deb" "$WORK/pkg"
UPGRADE_VERSION="${VERSION}+test1"
sed -i "s/^Version: .*/Version: $UPGRADE_VERSION/" "$WORK/pkg/DEBIAN/control"
dpkg-deb --build --root-owner-group "$WORK/pkg" "$WORK/upgrade.deb" >/dev/null

apt_install "$WORK/upgrade.deb" "$WORK/upgrade.log" || exit 1
INSTALLED_VERSION="$(dpkg-query -W -f='${Version}' trommel 2>/dev/null || true)"
[ "$INSTALLED_VERSION" = "$UPGRADE_VERSION" ] \
    || fail "upgrade left version '$INSTALLED_VERSION', expected $UPGRADE_VERSION"
[ -f "$RULE" ] || fail "upgrade lost the per-user polkit rule"
grep -q "subject.user !== \"$TEST_USER\"" "$RULE" 2>/dev/null \
    || fail "upgrade rewrote the rule for the wrong user"
[ -f "$CONFIG_DIR/config.json" ] || fail "upgrade removed the user config"
[ -L "$AUTOSTART_LINK" ] || fail "upgrade removed the login autostart link"

# --- remove ------------------------------------------------------------------
SUDO_USER="$TEST_USER" apt-get remove -y trommel > "$WORK/remove.log" 2>&1 || {
    tail -40 "$WORK/remove.log" >&2
    fail "apt-get remove failed"
}
[ ! -e /usr/bin/trommel ] || fail "binary left behind after remove"
[ ! -e "$RULE" ] || fail "per-user polkit rule left behind after remove"
[ -f "$CONFIG_DIR/config.json" ] || fail "remove deleted the user config"
[ ! -e "$AUTOSTART_LINK" ] || fail "login autostart link left behind after remove"

# --- purge -------------------------------------------------------------------
SUDO_USER="$TEST_USER" apt-get purge -y trommel > "$WORK/purge.log" 2>&1 || {
    tail -40 "$WORK/purge.log" >&2
    fail "apt-get purge failed"
}
[ ! -e "$RULE" ] || fail "per-user polkit rule left behind after purge"
[ -f "$CONFIG_DIR/config.json" ] || fail "purge deleted the user config"
[ ! -e "$AUTOSTART_LINK" ] || fail "login autostart link left behind after purge"
dpkg-query -W trommel >/dev/null 2>&1 && fail "package is still registered after purge"

if [ "$FAILS" -ne 0 ]; then
    echo "install/upgrade/remove check failed: $DEB" >&2
    exit 1
fi
echo "install/upgrade/remove ok: $DEB"
