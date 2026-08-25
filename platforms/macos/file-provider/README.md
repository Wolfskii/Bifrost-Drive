# macOS File Provider

This directory contains the native `NSFileProviderReplicatedExtension` target boundary for macOS 11 and later. The Swift package maps provider-neutral remote metadata into Finder items, enumerates remote directories, and delegates content downloads through `BifrostFileProviderTransport`.

The app target must inject a transport backed by the Bifrost local service and include `Info.plist` and `BifrostFileProvider.entitlements` in the File Provider extension target. The application-group identifier must match the signed host app. Run `swift test --package-path platforms/macos/file-provider` on macOS before packaging.

Mutation callbacks forward create, modify, and delete operations to the injected transport. The default transport returns an explicit unavailable error until the host app supplies conflict-safe Rust service operations; Finder changes are never silently discarded.
