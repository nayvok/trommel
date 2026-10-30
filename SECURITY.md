# Security policy

Please report vulnerabilities privately to the maintainer at
`yanyushkov@gmail.com` rather than in a public issue. Include reproduction steps, the affected
version, and whether the polkit helper, a miner binary or the D-Bus service is
involved.

Supported version: the current `0.8.x` line. The project does not distribute
miner binaries; report vulnerabilities in those binaries to their upstream
authors.

Never include seed phrases, private keys or full wallet files in a report. A
public wallet address is not a secret but can be redacted.

The trust model, the root helper contract, download checks and redaction rules
are documented in [docs/security.md](docs/security.md).
