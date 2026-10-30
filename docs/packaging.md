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

A tag `vX.Y.Z` triggers `.github/workflows/release.yml`. The workflow runs the
full CI matrix (including the headless UI smokes and the MSRV check), verifies
that the tag matches the workspace version, builds and verifies the bundle, and
publishes:

```
trommel_X.Y.Z_amd64.deb      versioned package
trommel_amd64.deb            stable alias for releases/latest/download
trommel_X.Y.Z_amd64.deb.sha256
trommel_amd64.deb.sha256
trommel-install.sh           verifies the SHA-256, then apt installs
SHA256SUMS                   checksums for both debs and trommel-install.sh
```

The `.sha256` sidecars are per-file checksums; `SHA256SUMS` covers the two
packages and the installer, which is the set a user downloads.

Locally the same bundle is produced by:

```bash
./scripts/package-deb.sh            # dist/trommel_X.Y.Z_amd64.deb + alias + .sha256
./scripts/release-bundle.sh         # dist/release/*, then verifies it
./scripts/test-deb.sh               # full layout validation
./scripts/test-deb-install.sh       # install/upgrade/remove (root, disposable container)
./scripts/verify-release-bundle.sh  # assets, sidecars and SHA256SUMS
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
/usr/share/icons/hicolor/<size>/apps/io.github.nayvok.Trommel*.png
```

Dependencies: GTK4, libadwaita, glib, pkexec/polkit, user D-Bus and systemd;
the NVIDIA driver is recommended. The tray is implemented in-process with
`ksni` and adds no shared-library or GNOME dependency. GNOME Shell is **not**
required: the app is a plain GTK4/libadwaita application and its postinst does
not compile schemas or touch GNOME extensions. The tray does need a
StatusNotifierItem host — KDE Plasma natively, GNOME through the AppIndicator
extension (Ubuntu ships `ubuntu-appindicators`); without one the app still runs.
The package is built and tested on Ubuntu 26.04.

`scripts/test-deb.sh` asserts this layout (every icon path, the helper mode,
the maintainer scripts and the polkit annotation) and rejects a package whose
binaries still contain a builder home path. `scripts/test-deb-install.sh`
installs, upgrades and removes the package in a disposable root container,
including the login autostart symlink. Both run in CI.

Maintainer scripts:

- `postinst` refreshes the desktop/icon caches and writes
  `/etc/polkit-1/rules.d/50-trommel.rules` for `SUDO_USER`, so GPU tuning does
  not prompt from the user service. On the first install it also enables
  `trommeld.service` for that user (a `graphical-session.target.wants` symlink),
  so the background service and tray start at login by default. Upgrades do not
  re-enable it: a user who turned it off in Settings keeps that choice. It does
  not compile GLib schemas: Trommel ships no `gschema`, so `libglib2.0-bin` is
  not a dependency. The shipped `/usr/share` rule is only a local fallback.
- `postrm` removes the generated polkit rule and the login autostart symlink on
  remove/purge.
- `prerm` intentionally does not stop the miner.

The package enables the user service for the installing user on first install,
so the tray is present after login. The Settings switch **Start Trommel at
login** writes the same unit state. D-Bus activation can still start the daemon
on demand when autostart is off.

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
| `ci.yml` | push to `main`, pull request | secrets, docs links, fmt, clippy with `gui`, tests, GUI-feature tests, CLI-only build, e2e, MCP, release build, desktop/metainfo validation, package layout + install/upgrade/remove, MSRV |
| `ui.yml` | called by `ci.yml` and `release.yml` | headless `ui-smoke.sh` and `first-run-smoke.sh` under Xvfb with a private AT-SPI bus |
| `package.yml` | manual | builds the `.deb` and validates its layout (the same check runs on every pull request) |
| `release.yml` | tag `v*` | the CI matrix plus polkit check, tag/version check, release bundle verification, install/upgrade/remove, GitHub Release upload |

## Versioning

One version for the whole workspace (`Cargo.toml` `[workspace.package]
version`). `./scripts/check-version.sh [vX.Y.Z]` checks it against
`Cargo.lock`, `CHANGELOG.md`, the metainfo `<release>` and, when a tag is given
(or `GITHUB_REF` is a tag ref), that the tag is annotated and points at `HEAD`.
CI runs it on every push and pull request; the release workflow runs it before
building. Also update the `CHANGELOG.md` and metainfo entries. The full checklist
is below.

## Release checklist

Run from the repository root with a clean working tree.

1. **Version.** Bump `version` in `Cargo.toml`; add a dated `CHANGELOG.md`
   entry and a metainfo `<release>` entry; bump `Version:` in the RPM template
   (`packaging/rpm/trommel.spec.in`). Build once so `Cargo.lock` picks up the
   workspace version, then `./scripts/check-version.sh` must pass.
2. **Matrix.** `cargo fmt --all -- --check`, `cargo clippy --workspace
   --all-targets --features trommel/gui -- -D warnings`, `cargo test
   --workspace`, `cargo test -p trommel --features gui`, `cargo build -p
   trommel`, `cargo build -p trommel --features gui`, `cargo build -p
   trommeld`, `./scripts/test-oc-helper.sh`, `./scripts/e2e.sh`,
   `./scripts/test-mcp.sh`, `./scripts/ui-smoke.sh`,
   `./scripts/first-run-smoke.sh`, `./scripts/check-docs.sh`,
   `./scripts/check-no-secrets.sh`, `./scripts/check-polkit.sh`,
   `desktop-file-validate data/io.github.nayvok.Trommel.desktop` and
   `appstreamcli validate --explain data/io.github.nayvok.Trommel.metainfo.xml`.
   Run the UI smoke suites on a real desktop session for a release; the headless
   Xvfb job is the automated gate. Regenerate screenshots only if the UI changed.
3. **Package.** `./scripts/package-deb.sh`, `./scripts/test-deb.sh` and
   `./scripts/test-deb-install.sh` (root in a disposable container).
4. **Bundle.** `./scripts/release-bundle.sh` builds `dist/release` and verifies
   it; `(cd dist/release && sha256sum -c SHA256SUMS)` must pass.
5. **Commit and tag.** Commit everything, tag the **last** commit with an
   annotated `vX.Y.Z`, push the branch and the tag. Do not tag an intermediate
   commit. If the release workflow fails, fix on the branch, move the tag to the
   new last commit and push it again; delete the remote tag only if the release
   was not published and that is agreed.
6. **Watch the workflow.** The release workflow re-runs the matrix (including
   the headless UI smokes and the MSRV), verifies the tag/version and the
   bundle, and publishes only if they pass.
7. **Verify the published release.** Download every asset into one directory and
   run `./scripts/verify-release-bundle.sh <dir>`; check the release page for
   the `.deb`, both `.sha256` sidecars, `trommel-install.sh` and `SHA256SUMS`,
   and that `releases/latest/download/trommel_amd64.deb` resolves. Install once
   from a clean machine.

The interactive matrix in [verification.md](verification.md) is the
real-desktop gate; the headless UI job proves the click path but not the
compositor or the driver.
