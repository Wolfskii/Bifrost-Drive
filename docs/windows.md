# Windows

Windows 11 22H2+ on NTFS is the first-release target. The planned filesystem integration uses the Windows Cloud Files API (CFAPI) and Desktop Bridge packaging so Explorer can display placeholders and hydrate files on demand.

Current status: Tauri desktop shell and Windows compilation are available. CFAPI registration, callbacks, hydration, Explorer status, and production packaging are **Planned** and require native Windows 11 validation.

Windows development requires Visual Studio Build Tools, the Windows SDK, WebView2, and a clean test account or VM. CFAPI tests must cover registration cleanup, hydration cancellation, process restart, crash recovery, pinning, dehydration, and uninstall.
