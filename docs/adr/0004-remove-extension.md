# ADR 0004: Remove the GNOME Shell extension

Status: accepted. Supersedes [ADR 0002](0002-extension-packaging.md).

## Context

ADR 0002 shipped a GNOME Shell 50 applet (`trommel@nayvok.github.io`) inside the
same package. It was a thin D-Bus client with no `prefs.js`, sharing
`org.gnome.shell.extensions.trommel` with the app. The applet existed to show
miner events (crash, error, overheat, restart) and to offer a panel popup and a
Quick Settings toggle.

In practice the applet duplicated a status contract the app and CLI already
render, coupled the repository to one shell version, and was the only consumer
of the `Notification` D-Bus signal. Users who do not run GNOME Shell 50 could
not use it at all, and it added a second, GSettings-only settings surface to
maintain.

## Decision

Remove the applet, its `data/schemas/org.gnome.shell.extensions.trommel.gschema.xml`
schema, the **Settings → GNOME integration** group and the extension step of the
package and `install.sh`. Desktop notifications move into the daemon, which
calls `org.freedesktop.Notifications.Notify` on the session bus (bounded by a
timeout, failures ignored). The `Notification` D-Bus signal remains as an
optional integration surface, and `behavior.notifications` still gates both
paths.

`install.sh --no-extension` is kept as a deprecated no-op so existing scripts do
not fail; `uninstall.sh` keeps the legacy applet cleanup for old developer
installs.

## Consequences

- Notifications work on any freedesktop desktop, not only with the applet.
- A Debian postinst cannot safely edit another user's dconf, so an upgrade from
  a release that shipped the applet leaves its UUID in
  `org.gnome.shell enabled-extensions`. The package manager removes the files;
  the stale entry is deleted once in Extensions or with `gsettings`.
- The app no longer shells out to `gnome-extensions` and no longer reads a
  GSettings schema; `data/` carries only the desktop entry, metainfo and icons.
- Reintroducing a panel applet later needs a new ADR: it would be a new client,
  not a restored copy of this one.
