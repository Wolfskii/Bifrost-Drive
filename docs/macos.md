# macOS

macOS Keychain access is implemented in `platforms/macos/credentials` and selected by the desktop host on macOS. A native Swift `NSFileProviderReplicatedExtension` package now maps shared remote items, enumerates directories, and fetches remote content through an injected transport boundary. The package includes application-group entitlements and macOS CI compilation. Connecting the transport to the Rust service, signing the extension, and Finder acceptance remain macOS-native release work.

Development release bundles use an ad-hoc code signature so downloaded Apple Silicon builds satisfy the platform's executable-signing requirement. They are not notarized and macOS may still require the user to approve Bifrost Drive in **System Settings > Privacy & Security** before opening it. Public distribution without that approval requires Developer ID signing and Apple notarization.

No macOS filesystem support is claimed by the current build.
