# Windows

Windows 11 22H2+ on NTFS is the first-release target. CFAPI lets Explorer display sync-root placeholders and hydrate files on demand. A separate WinFsp adapter provides genuine local drive-letter mounts with provider-backed filesystem operations. A connection uses its selected drive letter when configured; otherwise it uses a CFAPI sync folder, and the desktop does not register both presentations at once.

Current status: Tauri desktop shell, Windows GUI-subsystem builds, per-machine Program Files installation, native start-on-boot registration, CFAPI registration, placeholder transfer, provider-backed hydration, local close/delete/rename routing, writable WinFsp callbacks, persisted drive selection, startup remounting, provider capacity reporting where available, hidden attributes for remote dotfiles, notifications, updater configuration, and signed-package workflow support are implemented. Real drive acceptance covers read, write, create, rename, delete, directory operations, and unmount against an in-memory provider. Explorer acceptance against each live provider remains required. CFAPI folders are not aliased to drive letters.

Windows development requires Visual Studio Build Tools, the Windows SDK, WebView2, and a clean test account or VM. Building the native WinFsp feature additionally requires WinFsp 2.1 or later, installed from an elevated terminal with `choco install winfsp -y`. End users do not install this prerequisite separately: the Bifrost NSIS installer bundles the official unmodified WinFsp 2.1.25156 MSI, verifies its pinned checksum during packaging, and installs it silently when needed. Bifrost does not remove WinFsp during uninstall because other applications may share it.

WinFsp - Windows File System Proxy, Copyright (C) Bill Zissimopoulos. Source and license: <https://github.com/winfsp/winfsp>.

CFAPI tests must cover registration cleanup, hydration cancellation, process restart, crash recovery, pinning, dehydration, and uninstall. WinFsp tests must cover read, staged write and flush, create, truncate, delete, rename, directory operations, restart, and unmount cleanup.
