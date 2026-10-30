# Flatpak (deferred)

Flatpak cannot run the host `nvidia-smi`, access `/dev/nvidia*` and
`/proc/driver/nvidia` without broad device permissions, install a polkit helper
or a systemd user unit. Downloading miner binaries at runtime is also outside
Flathub policy.

A future design would need a host bridge/portal; a GUI-only Flatpak is a
different product. See `docs/packaging.md`.
