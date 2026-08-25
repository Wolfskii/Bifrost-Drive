import FileProvider
import Foundation
import os

public enum BifrostFileProviderError: Error {
    case unavailable
    case invalidIdentifier
}

public struct BifrostRemoteItem: Sendable {
    public let identifier: String
    public let parentIdentifier: String
    public let filename: String
    public let isDirectory: Bool
    public let size: Int64?
    public let modifiedAt: Date?

    public init(
        identifier: String,
        parentIdentifier: String,
        filename: String,
        isDirectory: Bool,
        size: Int64?,
        modifiedAt: Date?
    ) {
        self.identifier = identifier
        self.parentIdentifier = parentIdentifier
        self.filename = filename
        self.isDirectory = isDirectory
        self.size = size
        self.modifiedAt = modifiedAt
    }
}

public protocol BifrostFileProviderTransport: Sendable {
    func item(identifier: String) async throws -> BifrostRemoteItem
    func contents(identifier: String, destination: URL) async throws -> BifrostRemoteItem
    func enumerate(parentIdentifier: String, pageToken: String?) async throws -> ([BifrostRemoteItem], String?)
    func create(identifier: String, parentIdentifier: String, filename: String, contents: URL?) async throws -> BifrostRemoteItem
    func modify(identifier: String, contents: URL?) async throws -> BifrostRemoteItem
    func delete(identifier: String) async throws
}

public final class BifrostFileProviderItem: NSObject, NSFileProviderItem {
    public let itemIdentifier: NSFileProviderItemIdentifier
    public let parentItemIdentifier: NSFileProviderItemIdentifier
    public let filename: String
    public let typeIdentifier: String
    public let documentSize: NSNumber?
    public let contentModificationDate: Date?
    public let capabilities: NSFileProviderItemCapabilities
    public let childItemCount: NSNumber?

    public init(remote: BifrostRemoteItem) {
        self.itemIdentifier = NSFileProviderItemIdentifier(remote.identifier)
        self.parentItemIdentifier = NSFileProviderItemIdentifier(remote.parentIdentifier)
        self.filename = remote.filename
        self.typeIdentifier = remote.isDirectory ? "public.folder" : "public.data"
        self.documentSize = remote.size.map(NSNumber.init(value:))
        self.contentModificationDate = remote.modifiedAt
        self.capabilities = remote.isDirectory
            ? [.allowsContentEnumerating, .allowsAddingSubItems]
            : [.allowsReading]
        self.childItemCount = remote.isDirectory ? nil : 0
        super.init()
    }
}

public final class BifrostFileProviderExtension: NSObject, NSFileProviderReplicatedExtension {
    private let domain: NSFileProviderDomain
    private let transport: BifrostFileProviderTransport
    private let logger = Logger(subsystem: "com.bifrost.drive", category: "file-provider")

    public init(domain: NSFileProviderDomain, transport: BifrostFileProviderTransport) {
        self.domain = domain
        self.transport = transport
        super.init()
    }

    public required convenience init(domain: NSFileProviderDomain) {
        self.init(domain: domain, transport: UnavailableTransport())
    }

    public func invalidate() {
        logger.info("File Provider domain invalidated: \(self.domain.identifier.rawValue, privacy: .public)")
    }

    public func item(
        for identifier: NSFileProviderItemIdentifier,
        request: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        Task {
            do {
                let item = try await transport.item(identifier: identifier.rawValue)
                completionHandler(BifrostFileProviderItem(remote: item), nil)
            } catch {
                completionHandler(nil, error)
            }
        }
        return progress
    }

    public func fetchContents(
        for itemIdentifier: NSFileProviderItemIdentifier,
        version: NSFileProviderItemVersion?,
        request: NSFileProviderRequest,
        completionHandler: @escaping (URL?, NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        let destination = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: false)
        let progress = Progress(totalUnitCount: 1)
        Task {
            do {
                let item = try await transport.contents(identifier: itemIdentifier.rawValue, destination: destination)
                completionHandler(destination, BifrostFileProviderItem(remote: item), nil)
            } catch {
                try? FileManager.default.removeItem(at: destination)
                completionHandler(nil, nil, error)
            }
        }
        return progress
    }

    public func enumerator(
        for containerItemIdentifier: NSFileProviderItemIdentifier
    ) throws -> NSFileProviderEnumerator {
        BifrostEnumerator(parentIdentifier: containerItemIdentifier.rawValue, transport: transport)
    }

    public func createItem(
        basedOn itemTemplate: NSFileProviderItem,
        fields: NSFileProviderItemFields,
        contents url: URL?,
        options: NSFileProviderCreateItemOptions = [],
        request: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        Task {
            do {
                let remote = try await transport.create(
                    identifier: itemTemplate.itemIdentifier.rawValue,
                    parentIdentifier: itemTemplate.parentItemIdentifier.rawValue,
                    filename: itemTemplate.filename,
                    contents: url
                )
                completionHandler(BifrostFileProviderItem(remote: remote), [], false, nil)
            } catch {
                completionHandler(nil, fields, false, error)
            }
        }
        return progress
    }

    public func modifyItem(
        _ item: NSFileProviderItem,
        baseVersion version: NSFileProviderItemVersion,
        changedFields: NSFileProviderItemFields,
        contents newContents: URL?,
        options: NSFileProviderModifyItemOptions = [],
        request: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        Task {
            do {
                let remote = try await transport.modify(
                    identifier: item.itemIdentifier.rawValue,
                    contents: newContents
                )
                completionHandler(BifrostFileProviderItem(remote: remote), [], false, nil)
            } catch {
                completionHandler(nil, changedFields, false, error)
            }
        }
        return progress
    }

    public func deleteItem(
        identifier: NSFileProviderItemIdentifier,
        baseVersion version: NSFileProviderItemVersion,
        options: NSFileProviderDeleteItemOptions = [],
        request: NSFileProviderRequest,
        completionHandler: @escaping (Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        Task {
            do {
                try await transport.delete(identifier: identifier.rawValue)
                completionHandler(nil)
            } catch {
                completionHandler(error)
            }
        }
        return progress
    }

    public func materializedItemsDidChange(completionHandler: @escaping () -> Void) {
        completionHandler()
    }

    public func pendingItemsDidChange(completionHandler: @escaping () -> Void) {
        completionHandler()
    }

    public func importDidFinish(completionHandler: @escaping () -> Void) {
        completionHandler()
    }
}

private struct UnavailableTransport: BifrostFileProviderTransport {
    func item(identifier: String) async throws -> BifrostRemoteItem {
        throw BifrostFileProviderError.unavailable
    }

    func contents(identifier: String, destination: URL) async throws -> BifrostRemoteItem {
        throw BifrostFileProviderError.unavailable
    }

    func enumerate(parentIdentifier: String, pageToken: String?) async throws -> ([BifrostRemoteItem], String?) {
        throw BifrostFileProviderError.unavailable
    }

    func create(identifier: String, parentIdentifier: String, filename: String, contents: URL?) async throws -> BifrostRemoteItem {
        throw BifrostFileProviderError.unavailable
    }

    func modify(identifier: String, contents: URL?) async throws -> BifrostRemoteItem {
        throw BifrostFileProviderError.unavailable
    }

    func delete(identifier: String) async throws {
        throw BifrostFileProviderError.unavailable
    }
}

private final class BifrostEnumerator: NSObject, NSFileProviderEnumerator {
    private let parentIdentifier: String
    private let transport: BifrostFileProviderTransport

    init(parentIdentifier: String, transport: BifrostFileProviderTransport) {
        self.parentIdentifier = parentIdentifier
        self.transport = transport
        super.init()
    }

    func invalidate() {}

    func enumerateItems(
        for observer: NSFileProviderEnumerationObserver,
        startingAt page: NSFileProviderPage
    ) {
        let token = page == NSFileProviderPage.initialPage ? nil : String(data: page.rawValue, encoding: .utf8)
        Task {
            do {
                let (items, nextToken) = try await transport.enumerate(parentIdentifier: parentIdentifier, pageToken: token)
                observer.didEnumerate(items.map(BifrostFileProviderItem.init))
                if let nextToken {
                    observer.finishEnumerating(upTo: NSFileProviderPage(nextToken.data(using: .utf8)!))
                } else {
                    observer.finishEnumerating(upTo: nil)
                }
            } catch {
                observer.finishEnumeratingWithError(error)
            }
        }
    }

    func enumerateChanges(
        for observer: NSFileProviderChangeObserver,
        from syncAnchor: NSFileProviderSyncAnchor
    ) {
        observer.finishEnumeratingChanges(upTo: syncAnchor, moreComing: false)
    }

    func currentSyncAnchor(completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        completionHandler(NSFileProviderSyncAnchor(Data("bifrost-drive-v1".utf8)))
    }
}
