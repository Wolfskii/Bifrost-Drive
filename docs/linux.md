# Linux

Linux support includes read-only FUSE mounts backed by the shared `StorageProvider` contract and a Secret Service credential adapter using keyring. Native RPM and AppImage builds mount enabled connections directly at `~/<connection name>` for access from Dolphin, Nautilus, and other file managers. The chosen connection name is also used as the mounted filesystem label. Bifrost refuses to mount over a non-empty directory. Install the FUSE 3 runtime and ensure `/dev/fuse` is available. FUSE works independently of the host filesystem, including Btrfs subvolumes, because it attaches a separate virtual filesystem at the mountpoint. The mount deliberately advertises read-only permissions until local mutation semantics and Linux-native acceptance tests are complete.

The credential adapter requires a Secret Service provider. On KDE Plasma, enable **Use KWallet for the Secret Service interface** in KDE Wallet settings, apply the change, and unlock the default wallet; installing GNOME Keyring is not required when using KWallet. On other desktops, install and start a provider such as `gnome-keyring` and unlock its default collection. Bifrost detects the current Linux desktop and distribution so recovery guidance prioritizes the active wallet and only offers a matching package command when appropriate. RPM is intended for RPM-based distributions; Arch users should use the AppImage. Both formats require a Secret Service provider in the host session.

Release CI produces two x86_64 desktop packages:

- AppImage for portable use across compatible distributions such as Ubuntu and Arch Linux.
- RPM for Fedora, RHEL-compatible, and other RPM-based distributions.

The Linux launcher disables WebKitGTK's DMA-BUF renderer unless the user explicitly overrides `WEBKIT_DISABLE_DMABUF_RENDERER`. This avoids blank webviews observed with some Arch Linux Wayland and GPU combinations. Run the AppImage from a terminal to inspect WebKitGTK diagnostics if rendering still fails.

The Linux tray currently uses the AppIndicator backend supplied by Tauri's `tray-icon` dependency. Some Fedora releases print a non-fatal deprecation warning from `libayatana-appindicator` when the tray starts; this is emitted by the dependency, not Bifrost. The tray remains enabled for minimize-to-tray behavior and will move to the replacement backend when the dependency exposes it.

The AppImage remains the Tauri updater target; RPM packages are updated by installing a newer release.

Install a downloaded package with the matching command:

```text
chmod +x Bifrost*.AppImage && ./Bifrost*.AppImage
sudo dnf install ./Bifrost*.rpm
```
