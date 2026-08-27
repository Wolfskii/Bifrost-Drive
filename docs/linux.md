# Linux

Linux support includes a read-only FUSE adapter backed by the shared `StorageProvider` contract and a Secret Service credential adapter using keyring. Install `libfuse3-dev` and ensure `/dev/fuse` is available before mounting. The FUSE mount deliberately advertises read-only permissions until local mutation semantics and Linux-native acceptance tests are complete. Mount permissions, file watching, cache paths, and disconnect behavior remain Linux-native acceptance concerns.

The credential adapter requires a Secret Service provider. On Arch Linux, install `gnome-keyring` and `libsecret`, ensure the keyring starts in the user desktop session, and unlock the default collection. RPM is intended for RPM-based distributions; Arch users should prefer the AppImage or Flatpak, but either still requires a Secret Service provider in the host session.

No Linux filesystem mount support is claimed by the current build.

Release CI produces three x86_64 desktop packages:

- AppImage for portable use across compatible distributions such as Ubuntu and Arch Linux.
- RPM for Fedora, RHEL-compatible, and other RPM-based distributions.
- Flatpak using the supported GNOME 50 runtime for distribution-independent desktop dependencies.

The Linux launcher disables WebKitGTK's DMA-BUF renderer unless the user explicitly overrides `WEBKIT_DISABLE_DMABUF_RENDERER`. This avoids blank webviews observed with some Arch Linux Wayland and GPU combinations. Run the AppImage from a terminal to inspect WebKitGTK diagnostics if rendering still fails.

The Linux tray currently uses the AppIndicator backend supplied by Tauri's `tray-icon` dependency. Some Fedora releases print a non-fatal deprecation warning from `libayatana-appindicator` when the tray starts; this is emitted by the dependency, not Bifrost. The tray remains enabled for minimize-to-tray behavior and will move to the replacement backend when the dependency exposes it.

The Flatpak grants network, Wayland/X11, GPU, home-directory, notification, and Secret Service access. The AppImage remains the Tauri updater target; RPM and Flatpak packages are updated through their package format or by installing a newer release.

Install a downloaded package with the matching command:

```text
chmod +x Bifrost*.AppImage && ./Bifrost*.AppImage
sudo dnf install ./Bifrost*.rpm
flatpak install --user ./Bifrost*.flatpak
```
