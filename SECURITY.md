# Security policy

Please report vulnerabilities privately to the maintainer at
`yanyushkov@gmail.com` rather than in a public issue. Include reproduction steps,
the affected version, and whether the polkit helper, a miner binary or the
D-Bus service is involved.

Supported version: the current `0.8.x` line. The project does not distribute
miner binaries; report vulnerabilities in those binaries to their upstream
authors.

Never include seed phrases, private keys or full wallet files in a report. A
public wallet address is not a secret but can be redacted.

## Trust model in brief

- The GUI, CLI, daemon and guard run as the desktop user. Only
  `trommel-oc` is privileged, invoked through pkexec, and it accepts one
  `nvidia-smi` operation per GPU with validated UUIDs and numeric ranges — no
  paths, shell text or arbitrary commands.
- `trommeld` owns the miner and the snapshot. It never runs as root, never kills
  an external miner and never writes outside the user's directories.
- Miner downloads require HTTPS, are size-limited, optionally checksummed, and
  are extracted with absolute paths and `..` rejected. A missing checksum is
  recorded as unverified, never invented.
- Wallets are redacted from diagnostics and the MCP read tools; the status
  document and daemon log do not contain them. Local history has no wallet,
  pool URL or IP and is never uploaded.
- The D-Bus service is a same-user interface: any process running as the user can
  call `GetConfig` and read the wallet. Do not expose it on the system bus or a
  socket.
- There is no telemetry, analytics, update check or account. The daemon calls
  the PearlHash account API for the configured wallet and the miner's local API
  on `127.0.0.1`.

The full boundary table, helper contract, download rules and known limitations
are in [docs/architecture.md](docs/architecture.md#security-model).
