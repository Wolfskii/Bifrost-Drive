# Credentials

The shared credential contract and native Windows Credential Manager, macOS Keychain, and Linux Secret Service adapters are implemented. DPAPI is reserved for a justified Windows-specific envelope rather than used as a plaintext fallback.

Linux requires a running Secret Service provider, such as `gnome-keyring`, with an unlocked default collection in the desktop login session. If Bifrost reports that the native credential store is unavailable, install the provider and `libsecret`, enable it for the user session, then log out and back in if necessary. No credentials are written to SQLite or an application fallback file.

SQLite contains credential references and non-secret labels only. Secret values are redacted from `Debug` and `Display`, are never sent in UI DTOs, and must not appear in logs or crash reports.
