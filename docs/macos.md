# macOS

macOS Keychain access is implemented in `platforms/macos/credentials` and selected by the desktop host on macOS. A native Swift `NSFileProviderReplicatedExtension` package now maps shared remote items, enumerates directories, and fetches remote content through an injected transport boundary. The package includes application-group entitlements and macOS CI compilation. Connecting the transport to the Rust service, signing the extension, and Finder acceptance remain macOS-native release work.

No macOS filesystem support is claimed by the current build.
