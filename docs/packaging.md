# Packaging and releases

Trommel is distributed **only through GitHub Releases** as a Debian package.
There is no apt repository and no PPA; upgrades are new release downloads.

## Formats

| Format | Status | Why |
| --- | --- | --- |
| `.deb` | **Released** on GitHub | Needs host `nvidia-smi`, a polkit helper and a systemd user unit |
| `.rpm` | Template only (`packaging/rpm/trommel.spec.in`) | Needs a Fedora/openSUSE install/upgrade/removal test |
| AppImage | Deferred | Read-only mount cannot install the daemon, helper or user unit; a portable tarball with an explicit host-helper installer is the better fallback |
| Flatpak | Deferred | Sandbox lacks `/dev/nvidia*` and `/proc/driver/nvidia`, cannot use the host `nvidia-smi`, the polkit helper or the user unit, and downloads miner binaries at runtime. A GUI-only Flatpak would be a different product |

Do not publish an RPM/AppImage/Flatpak artifact until its integration story is
tested. The reasoning is recorded in [ADR 0003](adr/0003-packaging.md).

## Release assets

A tag `vX.Y.Z` triggers `.github/workflows/release.yml`. The workflow runs
secrets/format/clippy/tests/e2e/polkit/asset validation, builds the bundle and
publishes:

```
trommel_X.Y.Z_amd64.deb      versioned package
trommel_amd64.deb            stable alias for releases/latest/download
trommel_X.Y.Z_amd64.deb.sha256
trommel_amd64.deb.sha256
trommel-install.sh           verifies the SHA-256, then apt installs
SHA256SUMS                   all assets
```

Locally the same bundle is produced by:

```bash
./scripts/package-deb.sh     # dist/trommel_X.Y.Z_amd64.deb + stable alias + .sha256
./scripts/release-bundle.sh  # dist/release/* with the same naming as CI
./scripts/test-deb.sh        # layout validation (dpkg-deb -c)
```

Users install with:

```bash
sudo apt install ./trommel_amd64.deb
# or
curl -fsSLO https://github.com/nayvok/trommel/releases/latest/download/trommel-install.sh
bash trommel-install.sh
```

`apt remove trommel` removes the package and keeps the user config.

## Debian package layout

```
/usr/bin/trommel
/usr/bin/trommeld
/usr/libexec/trommel/trommel-oc                  root helper (0755)
/usr/share/polkit-1/actions/io.github.nayvok.trommel.policy
/usr/share/polkit-1/rules.d/50-trommel.rules     generic fallback
/usr/share/trommel/50-trommel.rules.in           per-user template
/usr/lib/systemd/user/trommeld.service
/usr/lib/systemd/user/trommel-autostart.service
/usr/share/dbus-1/services/io.github.nayvok.Trommel.Daemon.service
/usr/share/applications/io.github.nayvok.Trommel.desktop
/usr/share/metainfo/io.github.nayvok.Trommel.metainfo.xml
/usr/share/icons/hicolor/{scalable,symbolic}/apps/io.github.nayvok.Trommel*.svg
```

Dependencies: GTK4, libadwaita, glib, `libglib2.0-bin`, pkexec/polkit, user
D-Bus and systemd; NVIDIA and GNOME Shell are recommendations. The package is
built and tested on Ubuntu 26.04.

Maintainer scripts:

- `postinst` recompiles the system schema cache (which drops a schema left by an
  older package), refreshes the desktop/icon caches and writes
  `/etc/polkit-1/rules.d/50-trommel.rules` for `SUDO_USER`, so GPU tuning does
  not prompt from the user service. The shipped `/usr/share` rule is only a
  local fallback.
- `prerm` intentionally does not stop the miner.
- `postrm` removes the generated per-user rule on remove/purge.

The package intentionally does **not** enable the user service: D-Bus
activation starts the daemon on demand.

## Developer install

```bash
./install.sh             # build, install user binaries, helper and service
./install.sh --no-build  # use target/release
./install.sh --no-helper # skip the polkit helper (no OC)
```

`install.sh` installs to `~/.local/bin`, `~/.config/systemd/user`,
`~/.local/share/{applications,icons,metainfo,dbus-1/services}`, puts the helper
at `/usr/libexec/trommel/trommel-oc` and installs the per-user
polkit rule through `scripts/install-polkit-rule.sh`. It restarts `trommeld` so
an upgrade does not leave the previous process running.

Remove a developer install with `./uninstall.sh` (keeps config) or
`./uninstall.sh --purge` (removes config, state and benchmarks). The script
refuses to remove the helper while a miner is still running.

## CI workflows

| Workflow | Trigger | Checks |
| --- | --- | --- |
| `ci.yml` | push to `main`, pull request | secrets, docs links, fmt, clippy with `gui`, tests, e2e, MCP, release build, desktop/metainfo validation |
| `package.yml` | manual | builds the `.deb` and validates its layout |
| `release.yml` | tag `v*` | everything in CI plus polkit check, release bundle and GitHub Release upload |

## Versioning

One version for the whole workspace (`Cargo.toml` `[workspace.package]
version`). The release workflow derives the asset name from it. Also update
`CHANGELOG.md` and the metainfo `<release>` entry. The full checklist is in
[development.md](development.md#release-checklist).
