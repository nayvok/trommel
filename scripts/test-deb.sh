#!/bin/bash
# Layout validation for the built Debian package.
#
# Asserts every path documented in docs/packaging.md, the file modes that make
# the helper and the units work, the maintainer scripts, the polkit action and a
# path-neutral binary. The install/upgrade/remove behaviour is covered by
# scripts/test-deb-install.sh.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${TROMMEL_VERSION:-$(cd "$ROOT" && cargo metadata --no-deps --format-version 1 --locked 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])' 2>/dev/null || echo 0.0.0)}"
DEB="${1:-$ROOT/dist/trommel_${VERSION}_amd64.deb}"
[ -f "$DEB" ] || { echo "missing $DEB; run scripts/package-deb.sh" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
FAILS=0
fail() { echo "error: $*" >&2; FAILS=$((FAILS + 1)); }

LISTING="$(dpkg-deb -c "$DEB")"
check_path() {
    local path="${1#/}" mode="${2:-}"
    if ! awk -v p="./${path}" '$NF == p { found = 1 } END { exit !found }' <<< "$LISTING"; then
        fail "missing in package: /$path"
        return
    fi
    if [ -n "$mode" ] \
        && ! awk -v p="./${path}" -v m="$mode" \
            '$NF == p { ok = ($1 == m) } END { exit !ok }' <<< "$LISTING"; then
        fail "/$path has the wrong mode (expected $mode)"
    fi
}

# docs/packaging.md § Debian package layout
check_path /usr/bin/trommel -rwxr-xr-x
check_path /usr/bin/trommeld -rwxr-xr-x
check_path /usr/libexec/trommel/trommel-oc -rwxr-xr-x
check_path /usr/share/polkit-1/actions/io.github.nayvok.trommel.policy
check_path /usr/share/polkit-1/rules.d/50-trommel.rules
check_path /usr/share/trommel/50-trommel.rules.in
check_path /usr/lib/systemd/user/trommeld.service
check_path /usr/lib/systemd/user/trommel-autostart.service
check_path /usr/share/dbus-1/services/io.github.nayvok.Trommel.Daemon.service
check_path /usr/share/applications/io.github.nayvok.Trommel.desktop
check_path /usr/share/metainfo/io.github.nayvok.Trommel.metainfo.xml
check_path /usr/share/icons/hicolor/scalable/apps/io.github.nayvok.Trommel.svg
check_path /usr/share/icons/hicolor/symbolic/apps/io.github.nayvok.Trommel-symbolic.svg

# Control fields and maintainer scripts.
CONTROL="$(dpkg-deb -f "$DEB")"
grep -qxF 'Package: trommel' <<< "$CONTROL" || fail "control Package is not trommel"
grep -qxF "Version: $VERSION" <<< "$CONTROL" || fail "control Version is not $VERSION"
grep -qxF 'Architecture: amd64' <<< "$CONTROL" || fail "control Architecture is not amd64"
for dep in libgtk-4-1 libadwaita-1-0 'pkexec | policykit-1' dbus-user-session systemd; do
    grep -qF "$dep" <<< "$CONTROL" || fail "control Depends lacks $dep"
done
# GNOME is not a runtime dependency: the app is GTK4/libadwaita and its
# postinst has no schema/extension work.
if grep -qF 'gnome-shell' <<< "$CONTROL"; then
    fail "control still recommends gnome-shell"
fi
if grep -qF 'libglib2.0-bin' <<< "$CONTROL"; then
    fail "control still depends on libglib2.0-bin"
fi

dpkg-deb -e "$DEB" "$WORK/control"
for script in postinst prerm postrm; do
    [ -x "$WORK/control/$script" ] || fail "maintainer script $script is missing or not executable"
done
if grep -q 'glib-compile-schemas' "$WORK/control/postinst"; then
    fail "postinst still recompiles GLib schemas"
fi
grep -q '50-trommel.rules.in' "$WORK/control/postinst" || fail "postinst does not install the per-user rule"
grep -q 'SUDO_USER' "$WORK/control/postinst" || fail "postinst does not key the rule on SUDO_USER"

dpkg-deb -x "$DEB" "$WORK/root"
POLICY="$WORK/root/usr/share/polkit-1/actions/io.github.nayvok.trommel.policy"
grep -q 'id="io.github.nayvok.trommel.oc"' "$POLICY" || fail "polkit action id is missing"
grep -q 'org.freedesktop.policykit.exec.path">/usr/libexec/trommel/trommel-oc<' "$POLICY" \
    || fail "polkit action does not point at the packaged helper"
grep -q 'org.freedesktop.policykit.exec.allow_gui">true<' "$POLICY" \
    || fail "polkit action does not allow a GUI caller"
grep -q 'Exec=trommel' "$WORK/root/usr/share/applications/io.github.nayvok.Trommel.desktop" \
    || fail "desktop entry does not run /usr/bin/trommel"
grep -q 'Icon=io.github.nayvok.Trommel' "$WORK/root/usr/share/applications/io.github.nayvok.Trommel.desktop" \
    || fail "desktop entry does not reference the packaged icon"
grep -q 'subject.user' "$WORK/root/usr/share/trommel/50-trommel.rules.in" \
    || fail "per-user rule template does not match subject.user"

# The package must not leak the builder's home directory through panic
# locations or debug strings.
if grep -aq '/home/' "$WORK/root/usr/bin/trommel" "$WORK/root/usr/bin/trommeld"; then
    fail "builder home path embedded in the binaries (check package-deb.sh remap)"
fi

if [ "$FAILS" -ne 0 ]; then
    echo "package layout check failed: $DEB" >&2
    exit 1
fi
echo "package layout ok: $DEB"
