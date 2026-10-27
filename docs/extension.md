# GNOME Shell extension

UUID: `trommel@nayvok.github.io` (GNOME Shell 50). It has **no** `prefs.js`; all
settings live in the app.

## Settings

GSettings schema `org.gnome.shell.extensions.trommel`:

- `panel-mode`: `icon`, `hashrate`, `temperature`, `both`.
- `quick-toggle-enabled`: show a start/stop toggle in Quick Settings.
- `live-activity-enabled`: show the live hashrate/temperature card.
- `notifications-enabled`: show daemon notifications.

The app edits the same schema from **Settings → GNOME integration** and shows
whether Extension Manager currently has the extension enabled.

## Install/enable

- Debian: files under `/usr/share/gnome-shell/extensions/trommel@nayvok.github.io/`;
  the app offers **Enable** on first run.
- Developer install: `./install.sh` packs with `gnome-extensions pack`, installs
  and calls `gnome-extensions enable`, then verifies `gnome-extensions info`.
  It does not hide failures with `|| true`.
- If the panel icon does not appear, log out and back in on Wayland, then run
  `gnome-extensions info trommel@nayvok.github.io`.

## Why the old extension did not work

The previous install left it installed but disabled (`enabled: false`, state
`INITIALIZED`) because the enable step was swallowed by `|| true` and the
session was restarted. The new flow checks the result and prints diagnostics.
The extension also used a non-standard namespace and had its own prefs; both are
fixed here.

## Troubleshooting

```bash
gnome-extensions info trommel@nayvok.github.io
gsettings get org.gnome.shell enabled-extensions
journalctl --user -b | grep -i -E 'trommel|JS ERROR'
systemctl --user status trommeld
```

The daemon does not depend on the extension; mining still works from the app or
CLI if the panel applet is disabled.
