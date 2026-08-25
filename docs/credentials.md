# Credentials

The shared credential contract is present; native stores are **Planned**. Windows will use Credential Manager, with DPAPI only where justified. macOS will use Keychain. Linux will use Secret Service/libsecret where available.

SQLite contains credential references and non-secret labels only. Secret values are redacted from `Debug` and `Display`, are never sent in UI DTOs, and must not appear in logs or crash reports.
