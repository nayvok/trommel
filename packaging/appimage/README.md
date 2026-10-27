# AppImage (deferred)

An AppImage mounts the app read-only, so it cannot install the daemon, polkit
helper, systemd user unit or GNOME extension for the host. It also cannot
reliably spawn a long-lived daemon from a temporary mount.

A portable tarball plus an explicit host-helper installer is the preferred
fallback. Do not ship an AppImage that silently requires root setup.
