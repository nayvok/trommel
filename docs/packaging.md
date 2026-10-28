# Packaging status

| Format | v1 status | Why |
|---|---|---|
| `.deb` | **Released/tested target** | Full host access for `nvidia-smi`, polkit helper, systemd user unit, GSettings and the GNOME extension. |
| `.rpm` | Spec template, not released | Needs a Fedora/openSUSE test pass; paths and dependencies differ. |
| AppImage | Deferred | Read-only mount cannot install the daemon, polkit helper or extension; a portable tarball + installer is the better fallback. |
| Flatpak | Deferred | Sandbox lacks `/dev/nvidia*` and `/proc/driver/nvidia`, cannot use host `nvidia-smi`, the polkit helper, systemd user unit or install the extension, and downloads miner binaries at runtime. A GUI-only Flatpak is a different product. |

## Debian package

```bash
./scripts/package-deb.sh
sudo apt install ./dist/trommel_0.3.0_amd64.deb
```

Layout: `/usr/bin/{trommel,trommeld}`, `/usr/libexec/trommel/trommel-oc`,
polkit action/rule, systemd user units, D-Bus service, desktop/metainfo/icons,
GSettings schema and the extension under `/usr/share/gnome-shell/extensions/`.

`postinst` compiles schemas and refreshes desktop/icon caches. It intentionally
does **not** enable the user service or the extension for a user: the first app
launch offers the extension, and D-Bus activation starts the daemon on demand.

The package depends on GTK4/libadwaita/glib, `pkexec`/polkit, user D-Bus and
systemd; NVIDIA and GNOME Shell are recommendations. It is built/tested on
Ubuntu 26.04; use a disposable VM/container for verification.

## Flatpak notes for the future

A self-hosted Flatpak would need `--device=all`, broad host filesystem access,
`flatpak-spawn --host` for `nvidia-smi` and a host-side helper installation.
That defeats the sandbox and is not Flathub-eligible. Revisit only with a proper
host bridge/portal design.
