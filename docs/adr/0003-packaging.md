# ADR 0003: Debian first, no Flatpak in v1

Status: accepted for v1.

The product needs host `nvidia-smi`, a polkit helper, a systemd user daemon and
a GNOME Shell extension. Flatpak's sandbox blocks or complicates all four, and
downloading miner binaries at runtime is outside Flathub policy. AppImage has
the same daemon/helper/extension problem. RPM is testable only on another
distribution.

Therefore `.deb` is the released target for v1. RPM/AppImage/Flatpak have
status documents and no release artifacts until a tested integration exists.
