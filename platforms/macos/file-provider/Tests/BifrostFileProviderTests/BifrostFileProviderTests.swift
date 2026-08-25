import XCTest
@testable import BifrostFileProvider

final class BifrostFileProviderTests: XCTestCase {
    func testRemoteItemMapsToFileProviderMetadata() {
        let remote = BifrostRemoteItem(
            identifier: "docs/report.txt",
            parentIdentifier: "docs",
            filename: "report.txt",
            isDirectory: false,
            size: 42,
            modifiedAt: Date(timeIntervalSince1970: 1)
        )
        let item = BifrostFileProviderItem(remote: remote)

        XCTAssertEqual(item.itemIdentifier.rawValue, "docs/report.txt")
        XCTAssertEqual(item.parentItemIdentifier.rawValue, "docs")
        XCTAssertEqual(item.filename, "report.txt")
        XCTAssertEqual(item.documentSize?.int64Value, 42)
        XCTAssertTrue(item.capabilities.contains(.allowsReading))
    }
}
