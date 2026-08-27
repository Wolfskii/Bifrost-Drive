import FileProvider
import Foundation
import AppKit

private let groupIdentifier = "group.com.bifrost.drive"

private struct DesiredDomain: Decodable {
    let identifier: String
    let displayName: String
}

private func writeCString(_ value: String, to buffer: UnsafeMutablePointer<CChar>?, capacity: Int) -> Int32 {
    guard let buffer, capacity > 0 else { return -1 }
    let bytes = Array(value.utf8CString)
    guard bytes.count <= capacity else { return -2 }
    bytes.withUnsafeBufferPointer { source in
        buffer.update(from: source.baseAddress!, count: bytes.count)
    }
    return 0
}

@_cdecl("bifrost_file_provider_group_container")
public func bifrostFileProviderGroupContainer(
    _ buffer: UnsafeMutablePointer<CChar>?,
    _ capacity: Int
) -> Int32 {
    guard let url = FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: groupIdentifier
    ) else {
        _ = writeCString("Application Group container is unavailable", to: buffer, capacity: capacity)
        return -1
    }
    return writeCString(url.path, to: buffer, capacity: capacity)
}

@_cdecl("bifrost_file_provider_sync_domains")
public func bifrostFileProviderSyncDomains(
    _ domainsJSON: UnsafePointer<CChar>?,
    _ errorBuffer: UnsafeMutablePointer<CChar>?,
    _ errorCapacity: Int
) -> Int32 {
    guard let domainsJSON else {
        _ = writeCString("Domain payload is missing", to: errorBuffer, capacity: errorCapacity)
        return -1
    }

    do {
        let desired = try JSONDecoder().decode(
            [DesiredDomain].self,
            from: Data(String(cString: domainsJSON).utf8)
        )
        let desiredByID = Dictionary(uniqueKeysWithValues: desired.map { ($0.identifier, $0) })

        let fetch = DispatchSemaphore(value: 0)
        var existing: [NSFileProviderDomain] = []
        var operationError: Error?
        NSFileProviderManager.getDomainsWithCompletionHandler { domains, error in
            existing = domains
            operationError = error
            fetch.signal()
        }
        fetch.wait()
        if let operationError { throw operationError }

        for domain in existing where desiredByID[domain.identifier.rawValue] == nil {
            let removal = DispatchSemaphore(value: 0)
            NSFileProviderManager.remove(domain) { error in
                operationError = error
                removal.signal()
            }
            removal.wait()
            if let operationError { throw operationError }
        }

        let existingIDs = Set(existing.map { $0.identifier.rawValue })
        for domain in desired where !existingIDs.contains(domain.identifier) {
            let addition = DispatchSemaphore(value: 0)
            let fileProviderDomain = NSFileProviderDomain(
                identifier: NSFileProviderDomainIdentifier(rawValue: domain.identifier),
                displayName: domain.displayName
            )
            NSFileProviderManager.add(fileProviderDomain) { error in
                operationError = error
                addition.signal()
            }
            addition.wait()
            if let operationError { throw operationError }
        }
        return 0
    } catch {
        _ = writeCString(error.localizedDescription, to: errorBuffer, capacity: errorCapacity)
        return -1
    }
}

@_cdecl("bifrost_file_provider_add_domain")
public func bifrostFileProviderAddDomain(
    _ identifier: UnsafePointer<CChar>?,
    _ displayName: UnsafePointer<CChar>?,
    _ errorBuffer: UnsafeMutablePointer<CChar>?,
    _ errorCapacity: Int
) -> Int32 {
    guard let identifier, let displayName else {
        _ = writeCString("Domain identity is missing", to: errorBuffer, capacity: errorCapacity)
        return -1
    }
    let domain = NSFileProviderDomain(
        identifier: NSFileProviderDomainIdentifier(rawValue: String(cString: identifier)),
        displayName: String(cString: displayName)
    )
    let fetch = DispatchSemaphore(value: 0)
    var existing: [NSFileProviderDomain] = []
    var operationError: Error?
    NSFileProviderManager.getDomainsWithCompletionHandler { domains, error in
        existing = domains
        operationError = error
        fetch.signal()
    }
    fetch.wait()
    if let operationError {
        _ = writeCString(operationError.localizedDescription, to: errorBuffer, capacity: errorCapacity)
        return -1
    }
    if existing.contains(where: { $0.identifier == domain.identifier }) {
        return 0
    }
    let completion = DispatchSemaphore(value: 0)
    NSFileProviderManager.add(domain) { error in
        operationError = error
        completion.signal()
    }
    completion.wait()
    if let operationError {
        _ = writeCString(operationError.localizedDescription, to: errorBuffer, capacity: errorCapacity)
        return -1
    }
    return 0
}

@_cdecl("bifrost_file_provider_remove_domain")
public func bifrostFileProviderRemoveDomain(
    _ identifier: UnsafePointer<CChar>?,
    _ errorBuffer: UnsafeMutablePointer<CChar>?,
    _ errorCapacity: Int
) -> Int32 {
    guard let identifier else {
        _ = writeCString("Domain identity is missing", to: errorBuffer, capacity: errorCapacity)
        return -1
    }
    let wanted = String(cString: identifier)
    let fetch = DispatchSemaphore(value: 0)
    var domains: [NSFileProviderDomain] = []
    var operationError: Error?
    NSFileProviderManager.getDomainsWithCompletionHandler { values, error in
        domains = values
        operationError = error
        fetch.signal()
    }
    fetch.wait()
    if let operationError {
        _ = writeCString(operationError.localizedDescription, to: errorBuffer, capacity: errorCapacity)
        return -1
    }
    guard let domain = domains.first(where: { $0.identifier.rawValue == wanted }) else {
        return 0
    }
    let removal = DispatchSemaphore(value: 0)
    NSFileProviderManager.remove(domain) { error in
        operationError = error
        removal.signal()
    }
    removal.wait()
    if let operationError {
        _ = writeCString(operationError.localizedDescription, to: errorBuffer, capacity: errorCapacity)
        return -1
    }
    return 0
}

@_cdecl("bifrost_file_provider_open_domain")
public func bifrostFileProviderOpenDomain(
    _ identifier: UnsafePointer<CChar>?,
    _ errorBuffer: UnsafeMutablePointer<CChar>?,
    _ errorCapacity: Int
) -> Int32 {
    guard let identifier else {
        _ = writeCString("Domain identity is missing", to: errorBuffer, capacity: errorCapacity)
        return -1
    }
    let domain = NSFileProviderDomain(
        identifier: NSFileProviderDomainIdentifier(rawValue: String(cString: identifier)),
        displayName: "Bifrost Drive"
    )
    guard let manager = NSFileProviderManager(for: domain) else {
        _ = writeCString("File Provider domain is not registered", to: errorBuffer, capacity: errorCapacity)
        return -1
    }
    let lookup = DispatchSemaphore(value: 0)
    var visibleURL: URL?
    var operationError: Error?
    manager.getUserVisibleURL(for: .rootContainer) { url, error in
        visibleURL = url
        operationError = error
        lookup.signal()
    }
    lookup.wait()
    if let operationError {
        _ = writeCString(operationError.localizedDescription, to: errorBuffer, capacity: errorCapacity)
        return -1
    }
    guard let visibleURL else {
        _ = writeCString("Finder location is unavailable", to: errorBuffer, capacity: errorCapacity)
        return -1
    }
    NSWorkspace.shared.open(visibleURL)
    return 0
}
