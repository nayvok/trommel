# GNOME Shell extension

UUID: `trommel@nayvok.github.io`, GNOME Shell 50. It is a thin D-Bus client:
the daemon owns the state, the extension only displays it and sends Start/Stop/
Toggle/OpenApp. There is deliberately **no `prefs.js`** — settings live in the
app.

## Files

| File | Purpose |
| --- | --- |
| `extension/trommel@nayvok.github.io/extension.js` | Entire extension: panel button, menu, Quick Settings toggle |
| `metadata.json` | UUID, name, `shell-version: ["50"]`, settings schema, version |
| `stylesheet.css` | `trommel-*` classes for the popup |
| `icons/` | Panel/symbolic icons |
| `LICENSE` | GPL-3.0-or-later (derived from GNOME Shell) |

The pack step (`gnome-extensions pack`) includes `icons`, `LICENSE` and the
stylesheet; `install.sh`, `scripts/package-deb.sh` and the release workflow all
use the same source directory, so there is no second copy to drift.

## How it works

```
extension (GJS) ── Gio.DBusProxy ──> io.github.nayvok.Trommel.Daemon
   │                                   /io/github/nayvok/Trommel
   ├── methods Refresh/Toggle/OpenApp
   ├── signal StatusChanged(json)
   ├── signal Notification(summary, body)
   ├── bus-name owner watch (reconnect/offline state)
   └── GSettings org.gnome.shell.extensions.trommel
```

- `TrommelToggle` is a `QuickSettings.QuickToggle`; it appears only when
  `quick-toggle-enabled` is true and mirrors `status.state.running` from the
  status document after every refresh.
- The panel button owns the popup (the menu is created from the button, not from
  a non-existent indicator menu object). Panel text follows `panel-mode`:
  `icon`, `hashrate`, `temperature` or `both`.
- The popup shows the state pill, the action button, a live card (hashrate,
  power/temperature, GPU totals) controlled by `live-activity-enabled`, miner and
  fee, shares, pool balance and **Open Trommel**.
- `Notification` signals are forwarded to the desktop only when
  `notifications-enabled` is true.
- If the daemon is not running, the popup says so; it does not crash and the
  app/CLI can start the daemon later. The proxy reconnects when the owner
  appears or disappears.
- The extension never applies GPU tuning and never spawns a miner.

## Settings

Schema `org.gnome.shell.extensions.trommel` (installed from
`data/schemas/…gschema.xml`, also compiled by the `.deb`):

| Key | Type | Default | App control |
| --- | --- | --- | --- |
| `panel-mode` | enum `icon\|hashrate\|temperature\|both` | `both` | Settings → GNOME integration |
| `quick-toggle-enabled` | bool | `true` | Settings → GNOME integration |
| `live-activity-enabled` | bool | `true` | Settings → GNOME integration |
| `notifications-enabled` | bool | `true` | Settings → GNOME integration |

The app edits the same schema and shows whether Extension Manager currently has
the extension enabled. There is no separate extension preferences window.

## Install and enable

- **Debian package:** files are installed under
  `/usr/share/gnome-shell/extensions/trommel@nayvok.github.io/`; the app offers
  **Enable** in **Settings → GNOME integration**.
- **Developer install:** `./install.sh` packs with `gnome-extensions pack`,
  installs and calls `gnome-extensions enable`, then verifies the result with
  `gnome-extensions info`. It does not swallow failures with `|| true`.
- **Extension Manager:** the extension is listed there; enabling/disabling it
  outside the app is supported, and the app reflects the state.
- On Wayland a newly installed extension needs a log-out/log-in before GNOME
  Shell loads it.

## Troubleshooting

```bash
gnome-extensions info trommel@nayvok.github.io
gsettings get org.gnome.shell enabled-extensions
gsettings get org.gnome.shell.extensions.trommel panel-mode
journalctl --user -b | grep -i -E 'trommel|JS ERROR'
systemctl --user status trommeld
```

- **Not listed** — check the install path and `metadata.json`'s `shell-version`;
  repack and reinstall.
- **Listed but disabled** — enable it in Extension Manager or the app; the
  state is reported by `gnome-extensions info` (`ENABLED` vs `INITIALIZED`).
- **Enabled but no icon** — log out and back in on Wayland, then check the
  journal for `JS ERROR`.
- **Popup says “service is not running”** — start `trommeld` or open the app;
  D-Bus activation starts it on demand.
- **Settings do nothing** — both the app and the extension must see the same
  schema; verify with `gsettings list-keys
  org.gnome.shell.extensions.trommel`.

The daemon does not depend on the extension: mining still works from the app or
CLI when the panel applet is disabled.
