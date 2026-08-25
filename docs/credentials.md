# Credentials

The shared credential contract and Windows Credential Manager adapter are implemented. Keychain and Secret Service/libsecret adapters are **Planned**. DPAPI is reserved for a justified Windows-specific envelope rather than used as a plaintext fallback.

SQLite contains credential references and non-secret labels only. Secret values are redacted from `Debug` and `Display`, are never sent in UI DTOs, and must not appear in logs or crash reports.
