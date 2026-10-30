#!/bin/bash
# Shared AT-SPI harness for ui-smoke.sh and first-run-smoke.sh.
#
# The desktop accessibility bus is preferred, but a socket file is not proof
# that it answers: a launcher that crashed leaves the socket behind and every
# client then fails with "AT-SPI: Unable to open bus connection" while the
# script-level file test still passes. `ensure_atspi_bus` verifies the bus with a
# real D-Bus call and, when it does not answer, starts a private accessibility
# bus plus the registry inside the harness work directory. The caller exports
# AT_SPI_BUS_ADDRESS, which libatspi reads for both the app and ui_smoke.py, and
# cleans up ATSPI_BUS_PID/ATSPI_REGISTRY_PID in its trap.
#
# The private fallback needs `dbus-daemon` and the AT-SPI registry from
# `at-spi2-core`. It fails with a named error instead of returning success with
# a bus nobody is listening to, so a clean machine reports the missing package
# rather than a later "no applications" AT-SPI error.

ATSPI_REGISTRY="${ATSPI_REGISTRY:-/usr/libexec/at-spi2-registryd}"

atspi_bus_answers() {
    local address="$1" path
    [ -n "$address" ] || return 1
    case "$address" in
        unix:path=*) ;;
        *) return 1 ;;
    esac
    path="${address#unix:path=}"
    path="${path%%,*}"
    [ -S "$path" ] || return 1
    # `dbus-send --address` opens a peer connection without sending Hello, so
    # the bus rejects the Ping. Going through the session variable makes
    # dbus-send register first, which is what a real client does.
    DBUS_SESSION_BUS_ADDRESS="$address" dbus-send --session \
        --type=method_call --print-reply --reply-timeout=2000 \
        --dest=org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1
}

# True while a process is neither gone nor a zombie. `kill -0` succeeds for a
# zombie, which would make a crashed registry look healthy.
atspi_process_alive() {
    local pid="${1:-}" stat
    [ -n "$pid" ] || return 1
    [ -r "/proc/$pid/stat" ] || return 1
    stat=$(<"/proc/$pid/stat")
    [ -n "$stat" ] || return 1
    stat=${stat##*) }
    case "${stat%% *}" in
        Z|X|x) return 1 ;;
    esac
    return 0
}

start_private_atspi_bus() {
    local root="$1" out
    if ! command -v dbus-daemon >/dev/null 2>&1; then
        echo "  · dbus-daemon is not installed; cannot start a private accessibility bus" >&2
        return 1
    fi
    if [ ! -x "$ATSPI_REGISTRY" ]; then
        echo "  · the AT-SPI registry is missing at $ATSPI_REGISTRY" >&2
        echo "    install at-spi2-core (and gir1.2-atspi-2.0 for the client)" >&2
        return 1
    fi
    mkdir -p "$root"
    cat > "$root/bus.conf" <<XML
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=$root/bus</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
  <apparmor mode="disabled"/>
</busconfig>
XML
    out="$(dbus-daemon --config-file="$root/bus.conf" --print-address=1 --print-pid=1 --fork)" \
        || return 1
    ATSPI_BUS_PID="$(printf '%s\n' "$out" | sed -n 2p)"
    export AT_SPI_BUS_ADDRESS="$(printf '%s\n' "$out" | sed -n 1p)"
    # The registry owns the accessible tree a client enumerates. Without it
    # GTK can register but pyatspi sees no applications.
    AT_SPI_BUS_ADDRESS="$AT_SPI_BUS_ADDRESS" \
        "$ATSPI_REGISTRY" >>"$root/registry.log" 2>&1 &
    ATSPI_REGISTRY_PID=$!
    sleep 0.5
    if ! atspi_process_alive "$ATSPI_REGISTRY_PID"; then
        echo "  · the AT-SPI registry did not stay up; see $root/registry.log" >&2
        tail -5 "$root/registry.log" >&2 || true
        return 1
    fi
    if ! atspi_bus_answers "$AT_SPI_BUS_ADDRESS"; then
        echo "  · the private accessibility bus does not answer" >&2
        return 1
    fi
}

ensure_atspi_bus() {
    local work="$1"
    local address="${AT_SPI_BUS_ADDRESS:-unix:path=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/at-spi/bus}"
    if atspi_bus_answers "$address"; then
        export AT_SPI_BUS_ADDRESS="$address"
        return 0
    fi
    echo "  · the session accessibility bus does not answer; starting a private one" >&2
    start_private_atspi_bus "$work/at-spi"
}
