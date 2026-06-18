import XCTest
@testable import FiniteChat

final class NostrPeopleTests: XCTestCase {
    func testFetchFollowProfilesUsesContactListAndToleratesRelayFailure() async throws {
        let owner = String(repeating: "a", count: 64)
        let bob = String(repeating: "b", count: 64)
        let anonymous = String(repeating: "c", count: 64)

        let service = NostrRelayProfileService(
            relays: ["wss://bad.example", "wss://good.example"],
            eventLoader: { relay, filter, _, _ in
                if relay == "wss://bad.example" {
                    throw URLError(.cannotConnectToHost)
                }

                if filter.kinds == [3],
                   filter.authors == [owner],
                   filter.limit == 1
                {
                    return [
                        NostrRelayEvent(
                            pubkey: owner,
                            createdAt: 1_800_000_000,
                            kind: 3,
                            tags: [
                                ["p", anonymous, "", ""],
                                ["p", bob, "wss://relay.example", "Bobby"],
                                ["e", bob],
                            ],
                            content: ""
                        ),
                    ]
                }

                if filter.kinds == [0],
                   Set(filter.authors) == Set([bob, anonymous]),
                   filter.limit == 2
                {
                    return [
                        NostrRelayEvent(
                            pubkey: bob,
                            createdAt: 1_800_000_001,
                            kind: 0,
                            tags: [],
                            content: #"{"name":"bob","display_name":"Bob Miller","about":"hi","picture":"https://example.com/bob.jpg"}"#
                        ),
                    ]
                }

                return []
            }
        )

        let result = try await service.fetchFollowProfiles(forAccountID: owner.uppercased())

        XCTAssertEqual(result.relayCount, 2)
        XCTAssertEqual(result.followedPubkeyCount, 2)
        XCTAssertEqual(result.profiles.map(\.pubkey), [bob, anonymous])
        XCTAssertEqual(result.profiles[0].displayName, "Bob Miller")
        XCTAssertEqual(result.profiles[0].username, "bob")
        XCTAssertEqual(result.profiles[0].about, "hi")
        XCTAssertEqual(result.profiles[0].pictureURL, "https://example.com/bob.jpg")
        XCTAssertEqual(result.profiles[0].relayHint, "wss://relay.example")
        XCTAssertEqual(result.profiles[1].displayName, result.profiles[1].shortenedNpub)
    }

    func testFetchFollowProfilesUsesNewestContactListEvent() async throws {
        let owner = String(repeating: "1", count: 64)
        let olderFollow = String(repeating: "2", count: 64)
        let newerFollow = String(repeating: "3", count: 64)

        let service = NostrRelayProfileService(
            relays: ["wss://relay.example"],
            eventLoader: { _, filter, _, _ in
                if filter.kinds == [3] {
                    return [
                        NostrRelayEvent(
                            pubkey: owner,
                            createdAt: 10,
                            kind: 3,
                            tags: [["p", olderFollow]],
                            content: ""
                        ),
                        NostrRelayEvent(
                            pubkey: owner,
                            createdAt: 20,
                            kind: 3,
                            tags: [["p", newerFollow, "", "Newer"]],
                            content: ""
                        ),
                    ]
                }
                return []
            }
        )

        let result = try await service.fetchFollowProfiles(forAccountID: owner)

        XCTAssertEqual(result.followedPubkeyCount, 1)
        XCTAssertEqual(result.profiles.map(\.pubkey), [newerFollow])
        XCTAssertEqual(result.profiles[0].displayName, "Newer")
    }
}
