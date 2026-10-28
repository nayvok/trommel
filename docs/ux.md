# Trommel UX map

This document is the inventory of every visible surface, the navigation rules
they follow and the reason each one is a window, a dialog or a page. Update it
when a screen is added, removed or moved.

## Surfaces

| Surface | Kind | Opened from | Purpose |
| --- | --- | --- | --- |
| Main window | `AdwApplicationWindow` | app start | Header (refresh, start/stop, menu), four pages, unsaved-changes banner, toasts |
| Dashboard | page | switcher | Live hashrate, per-GPU cards, totals, shares, warnings |
| Presets | page | switcher | Preset list; each row opens the preset editor |
| Miners | page | switcher | Installed and supported miners, A/B benchmark |
| Settings | page | switcher | Behaviour, GPU defaults, detected-GPU table, GNOME integration, actions |
| Preset editor | modal window | preset row / “New preset” | Edit one preset: miner, version, pool, wallet, fees, options |
| Miner install | modal window | Miners row, preset editor, quick setup | Pick a version, download or import a binary |
| GPU tuning | modal window | Dashboard card, settings table | Per-card power/clocks and copy targets; temperature/fan are global/live |
| Quick setup | modal window | “Quick setup” button | Three steps for a first start: cards → miner → wallet |
| Apply-and-restart alert | `AdwAlertDialog` | Save/apply while mining | Explicit restart/stop choice for a running miner |
| Delete-preset alert | `AdwAlertDialog` | Preset editor | Destructive confirmation |
| About / log / file chooser | GNOME dialogs | menu, install dialog | Standard platform surfaces |

## Responsive layout

The window opens at 680×760 and stays usable down to ~400 px; the per-card settings rows use a short GPU name and keep the full name in the tooltip. Navigation lives in a single
`AdwViewSwitcherBar` at the bottom: the header keeps the app title, so the
switcher is never duplicated at the top and never disappears on a narrow
window. GPU cards wrap through the dashboard's 900 px clamp (one to three
columns); dialogs have their own scroll area and a size that fits small
screens. Page changes animate through `AdwViewStack`.

## Navigation rules

1. **The main window never swaps its content for a sub-screen.** Secondary
   screens are modal windows that block the main window, like the Nautilus
   preferences dialog. There are no “Back” buttons inside pages.
2. Wizards are the only surfaces with Back/Next: they are modal and keep a
   Cancel/close button in the header on every step.
3. A page is a place you return to, not a step in a flow. Drilling down always
   opens a dialog; closing the dialog leaves the page exactly as it was.
4. Destructive or restart-triggering actions ask first (`AdwAlertDialog`):
   apply-and-restart/stop while mining, delete a preset, overwrite a profile.
5. Toasts confirm background work; errors are shown in the dialog that caused
   them (installer, editor) and never silently swallowed.
6. Keyboard: `Ctrl+Q` quits; `Esc` closes modal windows and dialogs.

## One install flow

Miner installation exists in one place: **Miner install** (modal). The Miners
page, the preset editor and the quick-setup wizard only pass a miner id, a
suggested version and a refresh callback. The dialog owns:

- the version list (catalogue versions plus already-installed custom builds),
- an editable version name, so a user can install a build that is not in the
  catalogue yet,
- download fields (HTTPS URL, optional SHA-256, archive/binary names),
- import of a local binary,
- the installed/not-installed state and the final toast.

The catalogue is a convenience, not a gate: miners release often, so the user
can always type a version and paste a URL from the upstream release page.

## Theme

Trommel is a libadwaita app: it uses libadwaita's widget styling and inherits
the desktop GTK theme (Yaru on Ubuntu, Adwaita on stock GNOME). There is no
per-app theme switch on purpose - forcing `GTK_THEME` makes libadwaita skip its
stylesheet and breaks padding. **Settings → Behavior → Appearance** only picks
System/Light/Dark and applies immediately.

## Colour and emphasis

- Controls and navigation stay neutral; only **informational values** get
  colour: hashrate (accent), installed state (success), missing install or
  warning (warning/error), temperatures (warm/hot).
- Descriptions and secondary metadata stay `dim-label`.
- Colour never carries meaning on its own: the text says “installed”,
  “rejected”, “not installed” in addition to the colour.

## Quick setup details

- Step 1 lists cards with their stock limit and power limit; ticking a card is
  the only way to include it.
- “Set 80 %” fills the power rows of ticked cards, expands them and reports the
  applied values in the subtitle, so the button has a visible result.
- Step 2 installs via the shared Miner install dialog; step 3 validates wallet
  and pool address before starting.
- The footer status line is neutral, `warning` for validation errors and
  `accent` while starting.

## Empty and error states

- Service offline: pill says “offline”, start is disabled, refresh keeps
  retrying; the note row explains that `trommeld` is not running.
- No GPU: cards area and settings table show one explanatory row, not an empty
  box.
- No preset: dashboard offers “Quick setup”.
