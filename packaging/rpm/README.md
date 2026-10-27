# RPM (not released in v1)

A spec template will be added after a Fedora/openSUSE test pass. The `.deb`
package is the supported v1 format. Paths and dependencies to verify:

- `/usr/bin/trommel`, `/usr/bin/trommeld`
- `/usr/libexec/trommel/trommel-oc`
- systemd user units, D-Bus service, polkit action/rule
- GSettings schema and GNOME extension
- `Requires: gtk4, libadwaita, polkit, systemd, dbus-daemon`
- `Recommends: nvidia-driver, gnome-shell`

Do not publish an RPM without testing install/upgrade/removal and the extension
on the target distribution.
