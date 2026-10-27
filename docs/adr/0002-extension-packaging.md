# ADR 0002: Bundle the extension, enable with consent

Status: accepted for v1.

The extension is shipped in the same package and is visible in Extension
Manager. It has no `prefs.js`; the app and extension share
`org.gnome.shell.extensions.trommel`, so settings live in the app.

A Debian post-install script cannot safely enable an extension for a specific
user. The first app launch offers an Enable action; the developer installer
enables it and verifies `gnome-extensions info`. Extension Manager remains the
source of truth for enabled/disabled state.
