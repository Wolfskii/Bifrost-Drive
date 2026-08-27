# macOS File Provider

This directory contains the native `NSFileProviderReplicatedExtension` for macOS 13 and later. The Swift package maps provider-neutral remote metadata into Finder items and uses an application-group request broker to route enumeration, hydration, upload, rename, directory creation, and deletion through the Rust `StorageProvider` implementations in the running Bifrost host.

`project.yml` generates the Xcode extension and host-bridge targets. `build.sh` stages `BifrostFileProvider.appex` and the bridge dylib before Tauri embeds and signs them. Run `swift test --package-path platforms/macos/file-provider` and `platforms/macos/file-provider/build.sh` on macOS before packaging.

The host app and extension must be signed by the same Apple team with the `group.com.bifrost.drive` application group. The extension bundle identifier is `com.bifrost.drive.file-provider`. Ad-hoc signing can verify bundle structure but cannot provide production File Provider or application-group authorization; configure the Apple App IDs, profiles, Developer ID identity, and notarization before distributing Finder integration.
