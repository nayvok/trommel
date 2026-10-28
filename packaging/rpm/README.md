# RPM (not released)

`trommel.spec.in` is a template, not a release artifact. The `.deb` package is
the supported format. The template does not yet install every asset the Debian
package does; a Fedora/openSUSE test pass must confirm:

- `/usr/bin/trommel`, `/usr/bin/trommeld`
- `/usr/libexec/trommel/trommel-oc` and the polkit action/rule
- systemd user units and the D-Bus activation service
- GSettings schema and the GNOME extension
- `Requires: gtk4, libadwaita, polkit, systemd, dbus-daemon`
- `Recommends: nvidia-driver, gnome-shell`

Do not publish an RPM without testing install/upgrade/removal and the extension
on the target distribution. Update `docs/packaging.md` when it becomes supported.
