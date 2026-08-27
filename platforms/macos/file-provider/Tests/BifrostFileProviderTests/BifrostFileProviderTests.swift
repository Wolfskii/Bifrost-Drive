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

    func testRootItemUsesFileProviderRootIdentifier() {
        let remote = BifrostRemoteItem(
            identifier: "",
            parentIdentifier: "",
            filename: "Team Files",
            isDirectory: true,
            size: nil,
            modifiedAt: nil
        )
        let item = BifrostFileProviderItem(remote: remote)

        XCTAssertEqual(item.itemIdentifier, .rootContainer)
        XCTAssertTrue(item.capabilities.contains(.allowsContentEnumerating))
    }

    func testRemoteItemDecodesRustIsoTimestamp() throws {
        let data = Data(
            #"{"identifier":"report.txt","parentIdentifier":"","filename":"report.txt","isDirectory":false,"size":42,"modifiedAt":"2026-08-27T14:30:00Z"}"#.utf8
        )
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        let item = try decoder.decode(BifrostRemoteItem.self, from: data)

        XCTAssertEqual(item.identifier, "report.txt")
        XCTAssertNotNil(item.modifiedAt)
    }
}
