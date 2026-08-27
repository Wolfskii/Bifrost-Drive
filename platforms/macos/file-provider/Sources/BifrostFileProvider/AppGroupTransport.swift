import FileProvider
import Foundation

struct FileProviderRequest: Codable {
    let id: String
    let operation: String
    let connectionId: String
    let identifier: String?
    let parentIdentifier: String?
    let filename: String?
    let isDirectory: Bool?
    let pageToken: String?
    let contentFile: String?
}

struct FileProviderResponse: Codable {
    let ok: Bool
    let item: BifrostRemoteItem?
    let items: [BifrostRemoteItem]?
    let nextPageToken: String?
    let contentFile: String?
    let error: FileProviderResponseError?
}

struct FileProviderResponseError: Codable {
    let code: String
    let message: String
}

struct AppGroupTransport: BifrostFileProviderTransport {
    private static let groupIdentifier = "group.com.bifrost.drive"
    private let connectionId: String
    private let root: URL?

    init(connectionId: String) {
        self.connectionId = connectionId
        self.root = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: Self.groupIdentifier
        )?.appendingPathComponent("FileProvider", isDirectory: true)
    }

    func item(identifier: String) async throws -> BifrostRemoteItem {
        let response = try await perform(operation: "item", identifier: normalized(identifier))
        return try requiredItem(response)
    }

    func contents(identifier: String, destination: URL) async throws -> BifrostRemoteItem {
        let response = try await perform(operation: "contents", identifier: normalized(identifier))
        guard let root, let contentFile = response.contentFile else {
            throw BifrostFileProviderError.unavailable
        }
        let source = root.appendingPathComponent("payloads", isDirectory: true)
            .appendingPathComponent(contentFile, isDirectory: false)
        try FileManager.default.copyItem(at: source, to: destination)
        try? FileManager.default.removeItem(at: source)
        return try requiredItem(response)
    }

    func enumerate(
        parentIdentifier: String,
        pageToken: String?
    ) async throws -> ([BifrostRemoteItem], String?) {
        let response = try await perform(
            operation: "enumerate",
            parentIdentifier: normalized(parentIdentifier),
            pageToken: pageToken
        )
        return (response.items ?? [], response.nextPageToken)
    }

    func create(
        identifier: String,
        parentIdentifier: String,
        filename: String,
        isDirectory: Bool,
        contents: URL?
    ) async throws -> BifrostRemoteItem {
        let upload = try stage(contents)
        defer { removeStaged(upload) }
        let response = try await perform(
            operation: "create",
            identifier: normalized(identifier),
            parentIdentifier: normalized(parentIdentifier),
            filename: filename,
            isDirectory: isDirectory,
            contentFile: upload
        )
        return try requiredItem(response)
    }

    func modify(
        identifier: String,
        parentIdentifier: String,
        filename: String,
        contents: URL?
    ) async throws -> BifrostRemoteItem {
        let upload = try stage(contents)
        defer { removeStaged(upload) }
        let response = try await perform(
            operation: "modify",
            identifier: normalized(identifier),
            parentIdentifier: normalized(parentIdentifier),
            filename: filename,
            contentFile: upload
        )
        return try requiredItem(response)
    }

    func delete(identifier: String) async throws {
        _ = try await perform(operation: "delete", identifier: normalized(identifier))
    }

    private func requiredItem(_ response: FileProviderResponse) throws -> BifrostRemoteItem {
        guard let item = response.item else {
            throw BifrostFileProviderError.invalidIdentifier
        }
        return item
    }

    private func normalized(_ identifier: String) -> String {
        identifier == NSFileProviderItemIdentifier.rootContainer.rawValue ? "" : identifier
    }

    private func stage(_ source: URL?) throws -> String? {
        guard let source, let root else { return nil }
        let directory = root.appendingPathComponent("payloads", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let filename = UUID().uuidString
        try FileManager.default.copyItem(
            at: source,
            to: directory.appendingPathComponent(filename, isDirectory: false)
        )
        return filename
    }

    private func removeStaged(_ filename: String?) {
        guard let filename, let root else { return }
        try? FileManager.default.removeItem(
            at: root.appendingPathComponent("payloads", isDirectory: true)
                .appendingPathComponent(filename, isDirectory: false)
        )
    }

    private func perform(
        operation: String,
        identifier: String? = nil,
        parentIdentifier: String? = nil,
        filename: String? = nil,
        isDirectory: Bool? = nil,
        pageToken: String? = nil,
        contentFile: String? = nil
    ) async throws -> FileProviderResponse {
        guard let root else { throw BifrostFileProviderError.unavailable }
        let requests = root.appendingPathComponent("requests", isDirectory: true)
        let responses = root.appendingPathComponent("responses", isDirectory: true)
        try FileManager.default.createDirectory(at: requests, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: responses, withIntermediateDirectories: true)

        let id = UUID().uuidString
        let request = FileProviderRequest(
            id: id,
            operation: operation,
            connectionId: connectionId,
            identifier: identifier,
            parentIdentifier: parentIdentifier,
            filename: filename,
            isDirectory: isDirectory,
            pageToken: pageToken,
            contentFile: contentFile
        )
        let requestURL = requests.appendingPathComponent("\(id).json", isDirectory: false)
        let responseURL = responses.appendingPathComponent("\(id).json", isDirectory: false)
        try JSONEncoder().encode(request).write(to: requestURL, options: .atomic)

        let longOperation = ["contents", "create", "modify"].contains(operation)
        let deadline = Date().addingTimeInterval(longOperation ? 300 : 60)
        while Date() < deadline {
            if FileManager.default.fileExists(atPath: responseURL.path) {
                let data = try Data(contentsOf: responseURL)
                try? FileManager.default.removeItem(at: responseURL)
                let decoder = JSONDecoder()
                decoder.dateDecodingStrategy = .iso8601
                let response = try decoder.decode(FileProviderResponse.self, from: data)
                if response.ok { return response }
                throw providerError(response.error)
            }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        try? FileManager.default.removeItem(at: requestURL)
        throw BifrostFileProviderError.unavailable
    }

    private func providerError(_ error: FileProviderResponseError?) -> Error {
        let code: NSFileProviderError.Code
        switch error?.code {
        case "not_found":
            code = .noSuchItem
        case "not_authenticated":
            code = .notAuthenticated
        case "unsupported":
            code = .cannotSynchronize
        case "server_unreachable":
            code = .serverUnreachable
        default:
            return NSError(
                domain: "com.bifrost.drive.file-provider",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: error?.message ?? "Bifrost request failed"]
            )
        }
        return NSError(
            domain: NSFileProviderErrorDomain,
            code: code.rawValue,
            userInfo: [NSLocalizedDescriptionKey: error?.message ?? "Bifrost request failed"]
        )
    }
}
