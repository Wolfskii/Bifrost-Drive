# Security Policy

## Supported Versions

Bifrost Drive is pre-release. Security fixes apply to the latest commit on `develop` and the latest published stable release once releases begin.

## Reporting a Vulnerability

Please use a private GitHub Security Advisory for this repository. Do not disclose credentials, exploit details, or sensitive logs in a public issue. If private advisories are unavailable, contact the repository maintainers through GitHub before public disclosure.

## Security Requirements

- Secrets belong in the native credential store, never plaintext SQLite.
- TLS certificate validation and SSH host verification are enabled by default.
- Logs redact secrets and never include file contents.
- Cache and temporary files use restrictive permissions and atomic writes.
- Tauri capabilities and CSP are kept narrow.
- Dependencies are audited in CI and updates are reviewed.
- Release artifacts are checksummed and signing is supported through protected CI secrets.

See [docs/security.md](docs/security.md) for implementation details and current limitations.
