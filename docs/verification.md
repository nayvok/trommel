# Verification matrix (0.4.3)

Reproducible checks live in `scripts/e2e.sh`, `scripts/ui-smoke.sh`,
`scripts/first-run-smoke.sh` and `scripts/check-polkit.sh`. The table below
records what is automated, what was checked manually on the reference
RTX 3080 Ti rig, and what depends on a re-login (GNOME Shell extension).

| Surface | Action | Expected | How |
| --- | --- | --- | --- |
| CLI | `status`, `start`, `stop`, `toggle` | JSON/human state, stock restore | e2e + manual |
| CLI | `preset list/show/use`, `miner list/import/install` | install reaches a running daemon | e2e |
| CLI | `config path/get/reset/validate/export/diff/apply` | diff/apply modes, redaction | e2e + unit |
| CLI | `gpus`, `bench`, `diagnose`, `mcp` | GPU JSON, report, MCP smoke | unit + `test-mcp.sh` |
| Dashboard | Refresh, Start/Stop, GPU enable switch, Tune | live status, profile edit; Cancel restores the switch | UI smoke |
| Dashboard | stopped state | `0 TH/s`, numeric pool balance, endpoint, no lone dashes | first-run smoke + screenshot |
| Quick setup | Cancel, Next, Back, Apply 80 %, Use auto, Start mining | 3 steps, validation, start | UI smoke |
| First run | Start with no wallet/miner, empty config | opens Quick setup; miner marked not installed; one-GPU tune has no Apply-to | `first-run-smoke.sh` |
| Presets | preset row, chevron, New preset, Use | editor/apply; chevron is a real button | UI smoke |
| Preset editor | Save, Cancel, Duplicate, Delete…, Install… | apply, confirmation, shared install dialog | UI smoke (editor hook) |
| Miners | miner row, chevron, benchmark, install dialog | installed state, cancel; chevron is a real button | UI smoke |
| Settings | switches, power 0=auto, Save and apply, Copy diagnostics, Restore stock | aligned pill buttons, live apply, redaction, stop-first guard | UI smoke + screenshot |
| GPU tune | Now row, Recommended, Save, locks on/off, Similar/All GPUs | value rows hidden while off; Apply-to only with >1 GPU; restart/stop dialog | UI smoke + screenshot |
| Alerts | Apply and restart / Apply and stop / Cancel, delete confirmation | explicit restart/stop; Cancel reverts optimistic switch | UI smoke |
| Menu | Open log, About, Quit | opens log/about, quits | manual |
| GNOME extension | panel icon, popup, Start/Stop, Quick Settings, panel modes, notifications | mirrors daemon; menu comes from the panel button, no prefs window | manual after re-login |
| Window | resize to ~400 px | no clipping; per-card rows and action buttons readable | screenshot + manual |
| Installer | `install.sh` dev path, `.deb` postinst, GitHub installer | binaries, helper, rule, units, extension | Docker + manual |
| Real rig | turret polkit, 300 W/1620/810, start/stop | prompt-free OC, ~125 TH/s, stock restore; lock warnings stay empty | manual |

Known harness limits: AdwSpinRow spinners are not exposed as AT-SPI values, so
the UI smoke forces a profile change through the core-lock switch. Native file
choosers and the About dialog are manual. A destroyed modal frame can linger in
the AT-SPI tree after Cancel; the same click closes the window in the manual
check.
