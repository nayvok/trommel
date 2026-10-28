# Security model

- **No secrets in the repository.** Wallets are user input; seed phrases/private
  keys must never be requested or stored. Test fixtures use `prl1test…`.
- **Wallet is public but personal.** Status JSON and logs do not include it; the
  daemon log redacts addresses when logging settings.
- **Root helper is narrow.** `/usr/libexec/trommel/trommel-oc` is root-owned and
  accepts only a `GPU-…` UUID plus numeric `--pl/--lgc/--lmc` values in ranges.
  It runs `nvidia-smi` directly; no shell, paths or arbitrary commands.
- **Polkit.** The developer installer writes a per-user rule for the annotated
  action `io.github.nayvok.trommel.oc`. The Debian `postinst` writes the same
  kind of rule for `SUDO_USER`; the shipped `/usr/share` rule is only a local
  fallback. The helper is the only privileged program and validates the GPU
  UUID plus numeric ranges. Remove the rule to require a password prompt.
- **Miner downloads.** HTTPS only, 512 MiB limit, optional SHA-256, path
  traversal/symlink checks in archives, atomic install into
  `~/.local/share/trommel/miners/`. Unverified/custom binaries are marked.
- **No telemetry.** The app only calls the selected pool's public API and
  downloads a miner when the user asks.
- **One miner.** The supervisor never spawns a second child and does not kill
  an external miner without user action.
- **Flathub.** Flatpak is not shipped in v1 because the required host access
  would break the sandbox.
