# macOS

macOS Keychain access is implemented in `platforms/macos/credentials` and selected by the desktop host on macOS. A native Swift `NSFileProviderReplicatedExtension` is embedded at `Bifrost Drive.app/Contents/PlugIns/BifrostFileProvider.appex`. Each mounted connection is registered as a Finder domain using its chosen name. An application-group broker routes Finder enumeration, hydration, upload, rename, directory creation, and deletion to the shared Rust storage providers while Bifrost is running.

Development release bundles use an ad-hoc code signature so downloaded Apple Silicon builds satisfy the platform's executable-signing requirement. They are not notarized, and ad-hoc approval does not grant the File Provider and application-group authorization needed for Finder integration. Production Finder support requires host and extension App IDs under the same Apple team, the `group.com.bifrost.drive` application group, matching provisioning, Developer ID signing, and notarization.

The implementation and package layout are complete, but signed Finder acceptance remains required on a provisioned macOS host before release support is claimed.
