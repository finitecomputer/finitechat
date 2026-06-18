import Foundation
import SwiftUI

struct NostrFollowProfile: Identifiable, Equatable, Sendable {
    let pubkey: String
    let npub: String
    let name: String?
    let username: String?
    let about: String?
    let pictureURL: String?
    let relayHint: String?

    var id: String { pubkey }

    var displayName: String {
        for candidate in [name, username] {
            if let value = candidate?.trimmingCharacters(in: .whitespacesAndNewlines),
               !value.isEmpty
            {
                return value
            }
        }
        return shortenedNpub
    }

    var hasProfileName: Bool {
        for candidate in [name, username] {
            if let value = candidate?.trimmingCharacters(in: .whitespacesAndNewlines),
               !value.isEmpty
            {
                return true
            }
        }
        return false
    }

    var shortenedNpub: String {
        guard npub.count > 18 else { return npub }
        return "\(npub.prefix(10))...\(npub.suffix(4))"
    }
}

struct NostrFollowFetchResult: Equatable, Sendable {
    let profiles: [NostrFollowProfile]
    let relayCount: Int
    let followedPubkeyCount: Int
}

final class NostrPeopleModel: ObservableObject {
    @Published private(set) var profiles: [NostrFollowProfile] = []
    @Published private(set) var isLoading = false
    @Published private(set) var statusText: String?

    private let service: NostrRelayProfileService
    private var lastLoadedAccountID: String?

    init(service: NostrRelayProfileService = NostrRelayProfileService()) {
        self.service = service
    }

    @MainActor
    func loadIfNeeded(identity: AppNostrIdentity?) async {
        guard let identity else {
            profiles = []
            statusText = nil
            lastLoadedAccountID = nil
            return
        }
        guard lastLoadedAccountID != identity.accountID || profiles.isEmpty else { return }
        await refresh(identity: identity)
    }

    @MainActor
    func refresh(identity: AppNostrIdentity?) async {
        guard let identity else { return }
        lastLoadedAccountID = identity.accountID
        isLoading = true
        statusText = nil
        do {
            let result = try await service.fetchFollowProfiles(forAccountID: identity.accountID)
            profiles = result.profiles
            if result.followedPubkeyCount == 0 {
                statusText = "No follows found on the configured Nostr relays."
            } else {
                statusText = "Loaded \(result.profiles.count) of \(result.followedPubkeyCount) follows from \(result.relayCount) relays."
            }
        } catch is CancellationError {
            return
        } catch {
            profiles = []
            statusText = "Could not load follows: \(error.localizedDescription)"
        }
        isLoading = false
    }
}

final class NostrRelayProfileService: Sendable {
    typealias EventLoader = @Sendable (
        _ relay: String,
        _ filter: NostrRelayFilter,
        _ subscriptionPrefix: String,
        _ timeoutNanoseconds: UInt64
    ) async throws -> [NostrRelayEvent]

    static let pikaProfileRelays = [
        "wss://relay.primal.net",
        "wss://nos.lol",
        "wss://relay.damus.io",
        "wss://us-east.nostr.pikachat.org",
        "wss://eu.nostr.pikachat.org",
    ]

    private let relays: [String]
    private let timeoutNanoseconds: UInt64
    private let eventLoader: EventLoader

    init(
        relays: [String] = NostrRelayProfileService.pikaProfileRelays,
        timeoutSeconds: Double = 5,
        eventLoader: @escaping EventLoader = { relay, filter, subscriptionPrefix, timeoutNanoseconds in
            try await NostrRelayProfileService.fetchEventsFromRelay(
                from: relay,
                filter: filter,
                subscriptionPrefix: subscriptionPrefix,
                timeoutNanoseconds: timeoutNanoseconds
            )
        }
    ) {
        self.relays = relays
        timeoutNanoseconds = UInt64(max(timeoutSeconds, 1) * 1_000_000_000)
        self.eventLoader = eventLoader
    }

    func fetchFollowProfiles(forAccountID accountID: String) async throws -> NostrFollowFetchResult {
        let contacts = await fetchContacts(forAccountID: accountID)
        let followed = contacts.values.sorted { left, right in
            left.pubkey < right.pubkey
        }
        guard !followed.isEmpty else {
            return NostrFollowFetchResult(
                profiles: [],
                relayCount: relays.count,
                followedPubkeyCount: 0
            )
        }
        let metadata = await fetchMetadata(forPubkeys: followed.map(\.pubkey))
        let profiles = followed.compactMap { contact -> NostrFollowProfile? in
            guard let npub = try? npubFromAccountId(accountId: contact.pubkey) else { return nil }
            let profile = metadata[contact.pubkey]
            return NostrFollowProfile(
                pubkey: contact.pubkey,
                npub: npub,
                name: profile?.displayName ?? profile?.name ?? contact.petname,
                username: profile?.name,
                about: profile?.about,
                pictureURL: profile?.pictureURL,
                relayHint: contact.relayHint
            )
        }
        .sorted { left, right in
            let leftNamed = left.hasProfileName
            let rightNamed = right.hasProfileName
            if leftNamed != rightNamed {
                return leftNamed
            }
            return left.displayName.localizedCaseInsensitiveCompare(right.displayName) == .orderedAscending
        }
        return NostrFollowFetchResult(
            profiles: profiles,
            relayCount: relays.count,
            followedPubkeyCount: followed.count
        )
    }

    private func fetchContacts(forAccountID accountID: String) async -> [String: NostrContact] {
        let normalizedAccountID = accountID.lowercased()
        let filter = NostrRelayFilter(kinds: [3], authors: [normalizedAccountID], limit: 1)
        let events = await fetchEvents(filter: filter, subscriptionPrefix: "finite-contacts")
        guard let latest = events
            .filter({ $0.kind == 3 && $0.pubkey.lowercased() == normalizedAccountID })
            .max(by: { $0.createdAt < $1.createdAt })
        else {
            return [:]
        }
        var contacts: [String: NostrContact] = [:]
        for tag in latest.tags {
            guard tag.count >= 2, tag[0] == "p", Self.isHexPubkey(tag[1]) else { continue }
            let pubkey = tag[1].lowercased()
            let relayHint = tag.count >= 3 ? tag[2].nostrNonEmptyTrimmed : nil
            let petname = tag.count >= 4 ? tag[3].nostrNonEmptyTrimmed : nil
            contacts[pubkey] = NostrContact(pubkey: pubkey, relayHint: relayHint, petname: petname)
        }
        return contacts
    }

    private func fetchMetadata(forPubkeys pubkeys: [String]) async -> [String: NostrProfileMetadata] {
        var metadata: [String: NostrProfileMetadata] = [:]
        for chunk in pubkeys.chunked(into: 80) {
            let filter = NostrRelayFilter(kinds: [0], authors: chunk, limit: chunk.count)
            let events = await fetchEvents(filter: filter, subscriptionPrefix: "finite-profiles")
            for event in events where event.kind == 0 {
                guard Self.isHexPubkey(event.pubkey) else { continue }
                let pubkey = event.pubkey.lowercased()
                guard metadata[pubkey]?.createdAt ?? 0 <= event.createdAt else { continue }
                metadata[pubkey] = NostrProfileMetadata(event: event)
            }
        }
        return metadata
    }

    private func fetchEvents(
        filter: NostrRelayFilter,
        subscriptionPrefix: String
    ) async -> [NostrRelayEvent] {
        await withTaskGroup(of: [NostrRelayEvent].self) { group in
            for relay in relays {
                group.addTask {
                    do {
                        return try await self.eventLoader(
                            relay,
                            filter,
                            subscriptionPrefix,
                            self.timeoutNanoseconds
                        )
                    } catch {
                        return []
                    }
                }
            }
            var events: [NostrRelayEvent] = []
            for await relayEvents in group {
                events.append(contentsOf: relayEvents)
            }
            return events
        }
    }

    private static func fetchEventsFromRelay(
        from relay: String,
        filter: NostrRelayFilter,
        subscriptionPrefix: String,
        timeoutNanoseconds: UInt64
    ) async throws -> [NostrRelayEvent] {
        guard let url = URL(string: relay) else { return [] }
        let task = URLSession.shared.webSocketTask(with: url)
        let subscriptionID = "\(subscriptionPrefix)-\(UUID().uuidString)"
        let filterData = try JSONEncoder().encode(filter)
        let filterObject = try JSONSerialization.jsonObject(with: filterData)
        let payload: [Any] = ["REQ", subscriptionID, filterObject]
        let data = try JSONSerialization.data(withJSONObject: payload)
        guard let message = String(data: data, encoding: .utf8) else { return [] }

        task.resume()
        defer {
            task.cancel(with: .goingAway, reason: nil)
        }
        try await task.send(.string(message))

        var events: [NostrRelayEvent] = []
        while !Task.isCancelled {
            let received: URLSessionWebSocketTask.Message
            do {
                received = try await receiveWithTimeout(task, timeoutNanoseconds: timeoutNanoseconds)
            } catch is NostrRelayTimeout {
                break
            }
            let text: String?
            switch received {
            case .string(let value):
                text = value
            case .data(let data):
                text = String(data: data, encoding: .utf8)
            @unknown default:
                text = nil
            }
            guard let text else { continue }
            let parsed = Self.parseRelayMessage(text)
            if parsed.eoseSubscriptionID == subscriptionID {
                break
            }
            if let event = parsed.event, parsed.subscriptionID == subscriptionID {
                events.append(event)
            }
        }
        return events
    }

    private static func receiveWithTimeout(
        _ task: URLSessionWebSocketTask,
        timeoutNanoseconds: UInt64
    ) async throws -> URLSessionWebSocketTask.Message {
        try await withThrowingTaskGroup(of: URLSessionWebSocketTask.Message.self) { group in
            group.addTask {
                try await task.receive()
            }
            group.addTask { [timeoutNanoseconds] in
                try await Task.sleep(nanoseconds: timeoutNanoseconds)
                throw NostrRelayTimeout()
            }
            guard let first = try await group.next() else {
                throw NostrRelayTimeout()
            }
            group.cancelAll()
            return first
        }
    }

    private static func parseRelayMessage(_ text: String) -> NostrRelayMessage {
        guard let root = try? JSONSerialization.jsonObject(with: Data(text.utf8)),
              let array = root as? [Any],
              let kind = array.first as? String
        else {
            return NostrRelayMessage()
        }
        if kind == "EOSE", array.count >= 2 {
            return NostrRelayMessage(eoseSubscriptionID: array[1] as? String)
        }
        guard kind == "EVENT",
              array.count >= 3,
              let subscriptionID = array[1] as? String,
              let eventObject = array[2] as? [String: Any]
        else {
            return NostrRelayMessage()
        }
        return NostrRelayMessage(
            subscriptionID: subscriptionID,
            event: NostrRelayEvent(object: eventObject)
        )
    }

    private static func isHexPubkey(_ value: String) -> Bool {
        let hexCharacters = Set("0123456789abcdefABCDEF")
        return value.count == 64 && value.allSatisfy { character in
            hexCharacters.contains(character)
        }
    }
}

struct NostrRelayFilter: Encodable, Sendable {
    let kinds: [Int]
    let authors: [String]
    let limit: Int?
}

private struct NostrRelayTimeout: Error, Sendable {}

private struct NostrRelayMessage: Sendable {
    var subscriptionID: String?
    var eoseSubscriptionID: String?
    var event: NostrRelayEvent?
}

struct NostrRelayEvent: Sendable {
    let pubkey: String
    let createdAt: Int
    let kind: Int
    let tags: [[String]]
    let content: String

    init(
        pubkey: String,
        createdAt: Int,
        kind: Int,
        tags: [[String]],
        content: String
    ) {
        self.pubkey = pubkey
        self.createdAt = createdAt
        self.kind = kind
        self.tags = tags
        self.content = content
    }

    init?(object: [String: Any]) {
        guard let pubkey = object["pubkey"] as? String,
              let createdAt = object["created_at"] as? Int,
              let kind = object["kind"] as? Int,
              let content = object["content"] as? String
        else {
            return nil
        }
        self.pubkey = pubkey
        self.createdAt = createdAt
        self.kind = kind
        self.content = content
        tags = (object["tags"] as? [[Any]])?.map { tag in
            tag.compactMap { $0 as? String }
        } ?? []
    }
}

private struct NostrContact: Sendable {
    let pubkey: String
    let relayHint: String?
    let petname: String?
}

private struct NostrProfileMetadata: Sendable {
    let createdAt: Int
    let name: String?
    let displayName: String?
    let about: String?
    let pictureURL: String?

    init(event: NostrRelayEvent) {
        createdAt = event.createdAt
        let object = (try? JSONSerialization.jsonObject(with: Data(event.content.utf8))) as? [String: Any]
        name = object?["name"] as? String
        displayName = (object?["display_name"] as? String) ?? (object?["displayName"] as? String)
        about = object?["about"] as? String
        pictureURL = (object?["picture"] as? String) ?? (object?["picture_url"] as? String)
    }
}

private extension Array {
    func chunked(into size: Int) -> [[Element]] {
        guard size > 0 else { return [self] }
        var chunks: [[Element]] = []
        var index = startIndex
        while index < endIndex {
            let next = Swift.min(index + size, endIndex)
            chunks.append(Array(self[index..<next]))
            index = next
        }
        return chunks
    }
}

private extension String {
    var nostrNonEmptyTrimmed: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
