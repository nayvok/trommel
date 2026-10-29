# Design

This document is the UX and visual specification for Trommel. It is written for
whoever adds a screen, a row or a warning: the goal is that new work feels like
the rest of the app without a design review. The code lives in
`crates/cli/src/gui.rs`; the component map is in [components.md](components.md).

Russian mirror: [design.ru.md](design.ru.md).

## Product posture

Trommel is a **control room**, not a dashboard for spectators and not a miner.
It should feel like a quiet system utility:

- **The safe action is the obvious one.** Defaults are conservative (no clock
  locks, driver-auto power, one miner), and the risky action is always explicit
  and named.
- **Show state, not decoration.** Hashrate, temperature, power and fees are the
  product. Colour and motion only clarify state.
- **Progressive disclosure.** Simple mode shows power, temperature and Start.
  Advanced controls exist but do not compete with the first run.
- **One place per job.** Miner installation has one dialog; a preset has one
  editor; navigation is one view switcher in the header bar.
- **No lies.** A disabled control says why; an excluded card is never shown as
  mining; a failed restore is reported; a fee that cannot change is shown
  read-only; an unapplied draft is visible.

## Platform foundations

- GTK 4.22 and libadwaita 1.9 (`gtk4` `v4_22`, `libadwaita` `v1_9` features).
- GNOME 50 / Ubuntu 26.04 is the tested target; Wayland is the supported
  session. The window must remain usable at ~400 px width.
- The UI is built from libadwaita preferences widgets (`AdwPreferencesPage`,
  `AdwPreferencesGroup`, `AdwActionRow`, `AdwSpinRow`, `AdwSwitchRow`,
  `AdwComboRow`, `AdwBanner`, `AdwToastOverlay`, `AdwAlertDialog`,
  `AdwViewStack`, `AdwViewSwitcherTitle`, `AdwViewSwitcherBar`, `AdwClamp`,
  `AdwToolbarView`, `AdwWrapBox`, `AdwDialog`).
- No custom navigation model, no page-swapping main window, no web view.

## Visual system

### Theme

Trommel uses libadwaita's own widget styling and inherits the desktop GTK theme
(Yaru on Ubuntu, Adwaita on stock GNOME). **There is deliberately no per-app GTK
theme switch:** forcing `GTK_THEME` makes libadwaita skip its stylesheet and
breaks spacing and colours.

**Settings → Behavior → Appearance** only chooses the colour scheme —
System, Light or Dark — and applies immediately. `TROMMEL_COLOR_SCHEME`
overrides it for screenshots and tests.

![Dashboard in the light scheme](images/dashboard-en-light.png)

### Layout and spacing

| Constant | Value |
| --- | --- |
| Default window | 880 × 760 |
| Minimum window | 400 × 420 |
| Dashboard content clamp | 900 px (cards widen beyond the reading column) |
| Dashboard margins | 18 px top, 24 px bottom, 12 px sides |
| GPU cards | `AdwWrapBox` with fill justify, 10 px gaps, 240 px minimum |
| Page transition | `AdwViewStack`, 220 ms |
| Preferences reading width | libadwaita default (~600 px) |

Do not set a minimum width from a settings row: the per-card table is one
compact card per GPU precisely so Settings does not force a wide window.

### Type and emphasis

- Use the built-in `title-*`, `heading`, `body`, `caption`, `numeric` and
  `dim-label` classes. The only custom sizes are the hero hashrate
  (`.trommel-hero-hashrate`, 2.6 em, weight 800), the hero caption, metric labels
  and chips; do not add more.
- Numbers use tabular figures (`font-feature-settings: "tnum"`) so live values
  do not jitter.
- Labels are sentence case. All-caps only for short status pill text and the
  hero caption.

### Cards, pills and chips

Custom CSS lives in `install_css()`:

| Class | Use |
| --- | --- |
| `.trommel-hero` | Dashboard hashrate hero card: accent tint, 1 px accent border, 16 px radius |
| `.trommel-hero-caption` | Small letter-spaced HASHRATE caption |
| `.trommel-hero-hashrate` | Large hashrate value |
| `.trommel-hero-line`, `.trommel-hero-rule`, `.trommel-hero-meta` | Preset line, separator and uptime/restarts row |
| `.trommel-gpu-card` | Dashboard GPU card: rounded 12 px, card background, soft shadow |
| `.trommel-gpu-card-wide` | Single-GPU card: wider padding, fills the dashboard row |
| `.trommel-gpu-stat` | Live-value bubble inside a dashboard GPU card: tinted, 10 px radius |
| `.trommel-gpu-stat-value`, `-label`, `-hint` | Big tabular value, small label, muted target/lock hint |
| `.trommel-gpu-title` | Card title weight |
| `.trommel-gpu-chip` | Custom/default badge and compact live chip (pill, tabular) |
| `.trommel-pill` + `-running`, `-external`, `-stopped`, `-offline` | Status pill |
| `.trommel-hot`, `.trommel-warm`, `.trommel-muted` | Temperature and mute emphasis |
| `button.pill` | Compact rounded action button (26 px min height) |

Rounded corners are 12 px for cards and 999 px for pills. Shadows are reserved
for the dashboard GPU cards; Settings uses libadwaita's standard preference
groups and rows.

### Colour

Colour is **informational, never sole carrier**:

- hashrate and accent actions — `accent`;
- running — `success`; external/manual mining — `warning`;
- offline — `error`;
- temperature — `warning` (warm), `error` (hot);
- installed — `success`; missing install — `warning`/`error`;
- warnings — `warning`; secondary metadata — `dim-label`/muted.

Every coloured value has a word next to it (“installed”, “not installed”,
“rejected”, “offline”). Never encode a state only as a colour.

### Motion and feedback

- Page switches animate; dialogs use the platform transition.
- Toasts confirm background work: applied, installed, restored, copied
  diagnostics, benchmark winner.
- Dialogs own errors they caused (installer, editor, tuning); a failed
  background action surfaces a toast with a **Details** button. The details
  window keeps the full text selectable and copyable.
- The dashboard GPU card is not clickable: only **Tune** and its participation
  switch are actionable, and a sparkline only adds a tooltip, so the card itself
  has no hover treatment. Turning the switch off is an applied change: while
  mining it opens the Apply alert, and Cancel restores the switch.
- Dashboard **Start** is disabled while every detected GPU is off; the
  warnings row and the button tooltip explain it.
- No spinners for work that completes in under a frame; the Start button changes
  its label to “Starting…” and disables while the daemon works.

## Navigation

1. **The main window never swaps its content for a sub-screen.** Secondary
   surfaces are modal `AdwDialog`s presented inside the main window, like the
   Nautilus preferences dialog. There are no “Back” buttons inside pages.
2. **One navigation surface.** An `AdwViewSwitcherTitle` in the header bar:
   the four pages are the title widget while the window is wide enough to show
   their icons and labels. When it is not, the title returns and the
   `AdwViewSwitcherBar` reveals itself at the bottom, so the switcher is never
   clipped and never duplicated. The header bar carries navigation and window
   chrome only (refresh, switcher, menu): the primary Start/Stop action lives on
   the dashboard, which keeps the one-line switcher usable down to roughly
   850 px, so the 880×760 default is one line in both locales; a narrower
   window stacks the labels under the icons and keeps them in the header.
3. **Wizards are the only Back/Next surfaces.** They are modal dialogs and keep
   Cancel/Back/Next in the footer on every step.
4. **A page is a place you return to, not a step in a flow.** Drilling down
   always opens a dialog; closing it leaves the page as it was.
5. **Destructive or restart-triggering actions ask first** with an
   `AdwAlertDialog`: apply/restart while mining, delete a preset, overwrite a
   profile.
6. **Keyboard:** `Ctrl+Q` quits; `Esc` closes dialogs. Dialog headers carry the
   title and the close control only; the close button is dropped when an explicit
   Cancel/Close lives in the footer, so a dialog never shows two dismiss
   controls.

## Surface inventory

| Surface | Kind | Opened from | Purpose |
| --- | --- | --- | --- |
| Main window | `AdwApplicationWindow` | app start | Header (view switcher, refresh, menu), four pages, banners |
| Dashboard | page | view switcher | Hero hashrate, GPU cards, totals, shares, pool, warnings |
| Presets | page | view switcher | Preset list; rows open the editor |
| Miners | page | view switcher | Catalogue, installed versions, install dialog, benchmark |
| Settings | page | view switcher | Behaviour, appearance, actions |
| Preset editor | `AdwDialog` | preset row / New preset | One preset: miner, version, pool, wallet, fees, options |
| Miner install | `AdwDialog` | Miners row, editor, wizard | Version pick, HTTPS download, local import |
| GPU tuning | `AdwDialog` | Dashboard card | Per-card power/clocks; copy to similar/all |
| Quick setup | `AdwDialog` | Quick setup button / Start on a pristine install | Cards → miner → wallet → start |
| Apply alert | `AdwAlertDialog` | apply while mining | Apply and restart / apply and stop / cancel |
| Delete-preset alert | `AdwAlertDialog` | preset editor | Destructive confirmation |
| About / log / file chooser | platform | menu, installer | Standard surfaces |

## Screen specifications

### Dashboard

- Hero card: a small letter-spaced HASHRATE caption, the large value, status
  pill (`mining` / `external` / `stopped` / `offline`), a line with preset ·
  miner · version · total fee, a separator, and a metadata line with uptime,
  restarts and the miner data source. The card is accent-tinted with a soft
  shadow; the value uses the accent colour only while a hashrate is known.
  The card closes with the primary **Start/Stop** button (play/stop icon, pill,
  suggested while stopped and destructive while mining) in its footer, right
  aligned with the metadata line: the brightest control of the app is anchored
  to the card it acts on instead of floating in the header.
- GPU cards: title (name + index), a switch that includes the card in the
  active **mining session** (off = excluded: not passed to the miner, not
  tuned), six value bubbles (power, core, memory, temperature, fan, load; the
  configured target/lock is a small hint and turns hot when the live value
  differs), a **mining**/**excluded** chip, a custom/default chip and a
  **Tune** button. The participation chip describes the applied session, not
  the draft switch; after a switch change it keeps saying `mining` until Apply. Each bubble ends
  with a **sparkline** on the bubble floor: the metric's last five minutes as a
  thin accent line over a soft accent wash, rounded with the bubble. The wash
  reaches the rounded floor and the plot keeps only a 2 px margin, so the graph
  is grounded in the bubble instead of floating above its bottom edge. The line
  follows the value colour, so a warm or hot bubble also draws warm or hot.
  History lives in the GUI (it starts empty and fills at the poll rate), not in
  the daemon status. A one-GPU rig uses a single full-width card with a 3×2
  bubble grid; 2+ cards go into a wrap box and split the row evenly (240 px
  minimum per card). Core and memory clocks are labelled separately.
- Aggregate rows: cards (`1 of 2 mining`, or `selected` while stopped),
  rig power, max temperature, accepted and rejected shares, pool balance, the
  configured pool endpoint with its fee, and warnings. Power sums `power.draw`
  over every detected card (an excluded card still counts, since it still
  draws) and the temperature is the hottest card's reading. The cards, power
  and temperature rows carry a tooltip with one line per card, so a two-GPU
  aggregate can be checked without opening the JSON. J/TH stays in the status
  JSON for agents, but the dashboard does not show an efficiency row for now.
- When stopped, show **numeric placeholders** (`0 TH/s`, `0.000 PRL` or `0 PRL`
  without a wallet) and still name the configured pool endpoint. No lone dashes
  as placeholders.
- Warnings are one row with the joined warning text and a warning badge; the
  same list is in the status document.
- The secondary **Quick setup** button (flat pill, below the footer) appears
  until a wallet and an installed miner exist and opens the same wizard as
  Start does on an unfinished setup.

### Presets

- Rows are activatable `AdwActionRow`s in a preferences group; the chevron is a
  real button and opens the editor.
- Row subtitle: coin, pool, miner and version. The active preset gets an
  “Active” badge; inactive rows get a **Use** button.
- **New preset** creates a copy of the active preset and opens the editor.
- **Use** applies the selection (with the restart/stop dialog while mining).

### Preset editor (modal)

- Header: the preset name and “Preset” subtitle only (no close button: Cancel
  dismisses). The body scrolls; a footer holds **Cancel** on the left and
  **Save** on the right, and the page ends with the **Duplicate**/**Delete…**
  group.
- Sections: identity (name), miner/version + **Install…**, pool and address,
  wallet/worker, fees (miner read-only unless the catalogue allows an override;
  pool fee editable), miner options, extra arguments. The coin/algorithm and the
  GPU profile are not exposed here.
- Miner options are rebuilt from the selected miner's descriptors. Values whose
  keys the new miner does not define are dropped when the preset is saved.
- Simple mode hides miner options, extra arguments and the miner fee row but
  keeps the summary.
- Save applies immediately: stopped → save; running → apply alert. Delete asks
  for confirmation, names the preset, and explains that the daemon keeps mining
  with the previously applied preset until another one is applied.

### Miners

- Compact rows: name, short description, installed versions and a chevron that
  opens the install dialog. The upstream source link lives in the dialog.
- Benchmark controls (minutes + **Run benchmark**) run the sequential A/B test;
  a toast reports the best effective hashrate.
- A row with no installed versions shows no version suffix; its hint points to
  the same install dialog.

### Miner install (modal, single source)

Owns the whole install flow for the Miners page, the preset editor and the
wizard. Two groups, in order:

1. **Version** — version list from the catalogue **plus** already-installed
   custom builds, and an editable version name, so a newer upstream build can
   be installed before the catalogue knows it.
2. **Installation** — the state row (installed path, or “not installed”),
   **Import binary** for a local executable, **Release page** when the
   catalogue lists a homepage, and a **Download source** expander holding the
   HTTPS URL, optional SHA-256, archive kind (`tar.gz`, `zip`, raw) and binary
   name. The expander is collapsed because a catalogue version fills those
   fields; its subtitle shows the URL, so nothing is hidden before the primary
   action.

The group description carries the HTTPS-only/SHA-256 note; errors appear in
the footer status and success toasts and refreshes the caller. There is no
separate “verified” badge in the dialog. The footer keeps only **Cancel**, the
status line and **Install**, which fits a 400 px window; the short label and the
collapsed source rows keep the dialog inside a 760 px window without scrolling
to its own footer.

The catalogue is a convenience, not a gate. Never add a second install UI.

### Settings

Groups, in order:

1. **Behavior** — restart on crash, restore stock on stop, notifications,
   temperature limit and resume temperature (global; one miner process gets one
   temperature policy), start at login, simple mode, poll interval, language
   (restart required), appearance (immediate).
2. **Actions** — **Save and apply**, **Copy diagnostics**, **Restore stock**.
   Buttons stay vertically centred in their rows. **Restore stock** asks first:
   snapshot target, or the driver default when it differs; a pre-existing tuned
   limit and the last restore error are explained. **Restore stock** opens a
   confirmation with the snapshot target and, when it differs, the driver
   default; it explains a pre-existing tuned limit and the last restore error.

Rules:

- The dashboard GPU participation switch and the tuning dialog apply immediately
  (draft + apply flow); Cancel in the restart/stop alert must restore the switch
  and the dirty banner.
- When applied GPU settings are still on the cards while the miner is stopped,
  the dashboard shows an **oc banner** with a **Restore stock** action; the same
  state is also a status warning. The banner is not dismissible into a silent
  state.
- `Settings → Power limit` intentionally accepts `0` as driver auto; do not
  clamp it to a low wattage.
- The per-card dialog does not pretend to control fan or temperature; those are
  global/live values.

### GPU tuning (modal)

- Header: card name only. The footer holds a **Presets** menu
  (**Recommended**, **Profile default**) on the left and **Save** on the right;
  the close button dismisses without saving.
- A **Now** row shows live power/core/memory/temperature so the target can be
  compared with reality.
- Power limit, core lock + value, memory lock + value. A clock row appears only
  while its lock is on; with the lock off the subtitle names the current driver
  value.
- **Recommended** = 80 % of the stock power limit, both locks off; the tooltip
  says exactly that. It is a safe starting point, not an optimum. **Profile
  default** removes the per-card override and is shown only when one exists.
- **Apply to** appears only with more than one card: similar GPUs or all GPUs.
- Save goes through the normal apply flow; a single-GPU dialog has no empty
  “apply to” section.

### Quick setup (modal wizard)

Three steps, each with a status line and a footer carrying Cancel/Back on the
left and Next/Start mining on the right; the header shows the wizard title and
the current step as its subtitle:

1. **Cards** — each detected card: participation switch, its own power limit, stock
   limit in the subtitle. **Apply 80 %** fills the ticked cards, expands them
   and writes the resulting watts into each row; **Use auto** clears the limit.
   A card without a tick is not touched.
2. **Miner** — pick a miner/version; **Install…** opens the shared install
   dialog; installed state is shown in the row. One installation serves every
   preset.
3. **Wallet** — preset name, wallet, worker, pool and address. Validation
   explains missing pieces instead of failing silently.

**Start mining** saves the draft and starts. With a running miner it shows the
normal restart/stop alert; it never restarts silently. The wizard starts each
card from its own override or driver auto, never from an unrelated default
profile (this prevented a 300 W default from being applied to a 165 W card).

### Alerts

- **Apply while mining**: heading “Apply changes?”, body explains that
  GPU/miner changes need a restart; responses **Cancel**, **Apply and stop**
  (destructive), **Apply and restart** (default). The alert is the only way the
  GUI sends `restart`/`stop`.
- **Delete preset**: names the preset, confirms, and explains what the daemon
  will keep using.

## States

| State | Design |
| --- | --- |
| Service offline | Pill “offline”, Start disabled, a note explains that `trommeld` is not running; refresh keeps retrying |
| No GPU | One explanatory row in cards and settings, not an empty box |
| No preset | Hero says “No preset”; settings still render |
| No wallet / no miner | Start opens Quick setup; dashboard offers the same wizard |
| Miner stopped | Numeric placeholders, configured endpoint still named |
| Miner running | Live values, uptime, fee breakdown, warnings; Apply requires the alert |
| GPU excluded | Chip `excluded`, live temperature/power still visible, Totals counts it out; Start refuses when the miner cannot select devices |
| External miner | “mining (started manually)” pill in warning colour; Start explains that the external process must be stopped |
| Missing miner install | Install state in warning colour, Start blocked, install dialog offered |
| Lock not honoured | A warning with requested and actual MHz |
| Load error | Error text in the row/dialog that caused it; no silent fallback |

## Copy and localisation

- English is the source string; Russian is a translation: `tr("English",
  "Русский")` in Rust. Never concatenate translated fragments; use a full
  sentence per language.
- Use the user's terms: *miner*, *pool*, *wallet*, *preset*, *power limit*,
  *clock lock*. Avoid internal words such as “snapshot”, “supervisor”,
  “restart-required diff” in user-facing copy.
- Error messages say what failed and what to do next (“GPU helper did not
  answer in time; check the polkit rule”).
- Numbers are formatted with `trommel_core::util` (`125.3 TH/s`, `00:12:34`),
  with tabular figures in the UI.
- The catalogue carries `*_en`/`*_ru` descriptions, help and pool notes.

## Accessibility and keyboard

- Every interactive row is a real button/switch with an accessible label; the
  chevrons are buttons, not images.
- Do not rely on hover: tooltips repeat what is visible, not replace it.
- Toggle rows use `AdwSwitchRow`/`AdwActionRow` activation semantics; a switch
  never looks enabled while its value row is stale.
- Colour contrast follows libadwaita; muted text is only for secondary metadata.
- The window is usable at 400 px: no horizontal scrolling, no clipped action
  buttons, dialogs scroll instead of growing beyond the screen.
- AT-SPI limitations (spin rows may not expose their value) are handled in
  `scripts/ui-smoke.sh` by changing the profile through a switch; see
  [verification.md](verification.md).

## Screenshots

`scripts/screenshots.sh` renders all images in `docs/images/` from a fake
two-GPU rig with deterministic XDG directories. Conventions:

- English dark is the primary set. The dashboard is captured in dark and light
  for both languages, so GitHub can pick the viewer's scheme; the remaining
  surfaces are English dark plus Russian settings.
- File naming: `<surface>-<lang>-<scheme>.png`.
- Use the GUI test hooks (`TROMMEL_SCREENSHOT`, `TROMMEL_OPEN_*`,
  `TROMMEL_WIZARD_STEP`, …) instead of hand-cropping.
- Screenshots in the user guide must match the current layout; regenerate them
  when a surface changes, and never reference an image the script does not
  produce.

## Implementation notes

- Keep all custom CSS in `install_css()`; do not scatter `set_margin` magic
  where a libadwaita widget already implements the pattern.
- Rebuild GPU cards only when the status or GPU-profile signature changes
  (`gpu_sig`) to avoid flicker and lost focus. The signature covers every value
  shown in a bubble, so a clock or fan change is never left stale.
- Sparkline history is a GUI-side ring buffer (`History`: five minutes per GPU
  UUID, keyed by metric). `apply_status` appends one sample per status and
  `update_sparklines` redraws the registered areas, so a redraw never rebuilds
  a card.
- Widget callbacks mutate the draft only; they never talk to D-Bus directly.
  Settings switches write into the draft as they are toggled, so the global
  unsaved-changes banner and the Settings **Save and apply** button behave the
  same.
- New dialogs are `AdwDialog`, never a separate `GtkWindow`: they inherit the
  window style and use libadwaita's spring presentation instead of the
  compositor's window-open animation. They must be modal, close on `Esc`, and
  keep their header for the title and the close control, and put every action
  in a footer below the content (secondary on the left, primary on the right).
- Shared UI primitives: `label()`, `spin_row()`, `switch_row()`, `sync_lock_row()`,
  `gpu_stat()`, `gpu_chip()`, `set_status_text()`, `string_list()`, `clear_box()`,
  `clear_flow()`, `dialog_footer()`, `dialog_body()`. Reuse them rather than
  duplicating styling.
