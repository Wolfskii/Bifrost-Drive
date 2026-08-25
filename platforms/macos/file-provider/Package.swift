// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "BifrostFileProvider",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "BifrostFileProvider", targets: ["BifrostFileProvider"])
    ],
    targets: [
        .target(name: "BifrostFileProvider"),
        .testTarget(
            name: "BifrostFileProviderTests",
            dependencies: ["BifrostFileProvider"]
        )
    ]
)
