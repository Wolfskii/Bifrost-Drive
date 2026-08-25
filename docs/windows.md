# Windows

Windows 11 22H2+ on NTFS is the first-release target. The planned filesystem integration uses the Windows Cloud Files API (CFAPI) and Desktop Bridge packaging so Explorer can display placeholders and hydrate files on demand.

Current status: Tauri desktop shell, Windows GUI-subsystem builds, per-machine Program Files installation, native start-on-boot registration, CFAPI registration, placeholder transfer, provider-backed hydration, local close/delete/rename routing, notifications, updater configuration, and signed-package workflow support are implemented. Connections can optionally expose their CFAPI folder through an available per-user drive letter. Native Windows 11 Explorer acceptance, Desktop Bridge validation, and protected release secrets remain required.

Windows development requires Visual Studio Build Tools, the Windows SDK, WebView2, and a clean test account or VM. CFAPI tests must cover registration cleanup, hydration cancellation, process restart, crash recovery, pinning, dehydration, and uninstall.
