import XCTest
@testable import FiniteChat

final class NostrPeopleTests: XCTestCase {
    func testInviteAvailabilityServiceChunksAndMergesResponses() async throws {
        let ids = [
            String(repeating: "a", count: 64),
            String(repeating: "b", count: 64),
            String(repeating: "c", count: 64),
        ]
        let recorder = InviteAvailabilityChunkRecorder()
        let service = FiniteInviteAvailabilityService(
            chunkSize: 2,
            availabilityLoader: { serverURL, accountIDs in
                XCTAssertEqual(serverURL, "https://chat.example")
                await recorder.record(accountIDs)
                return Dictionary(uniqueKeysWithValues: accountIDs.map { accountID in
                    (accountID, accountID == ids[1] || accountID == ids[2])
                })
            }
        )

        let availability = try await service.fetchAvailability(
            serverURL: "https://chat.example",
            accountIDs: ids
        )

        let chunks = await recorder.recordedChunks()
        XCTAssertEqual(chunks, [[ids[0], ids[1]], [ids[2]]])
        XCTAssertEqual(availability[ids[0]], false)
        XCTAssertEqual(availability[ids[1]], true)
        XCTAssertEqual(availability[ids[2]], true)
    }

    func testPeopleModelAppliesInviteAvailabilityWithoutResortingProfiles() async throws {
        let material = try createNostrIdentity()
        let owner = material.accountId
        let amy = String(repeating: "a", count: 64)
        let zed = String(repeating: "b", count: 64)
        let relayService = NostrRelayProfileService(
            relays: ["wss://relay.example"],
            eventLoader: { _, filter, _, _ in
                if filter.kinds == [3] {
                    return [
                        NostrRelayEvent(
                            pubkey: owner,
                            createdAt: 1,
                            kind: 3,
                            tags: [["p", zed], ["p", amy]],
                            content: ""
                        ),
                    ]
                }
                if filter.kinds == [0] {
                    return [
                        NostrRelayEvent(
                            pubkey: zed,
                            createdAt: 1,
                            kind: 0,
                            tags: [],
                            content: #"{"display_name":"Zed"}"#
                        ),
                        NostrRelayEvent(
                            pubkey: amy,
                            createdAt: 1,
                            kind: 0,
                            tags: [],
                            content: #"{"display_name":"Amy"}"#
                        ),
                    ]
                }
                return []
            }
        )
        let availabilityService = FiniteInviteAvailabilityService(
            availabilityLoader: { _, accountIDs in
                Dictionary(uniqueKeysWithValues: accountIDs.map { accountID in
                    (accountID, accountID == zed)
                })
            }
        )
        let model = NostrPeopleModel(
            service: relayService,
            inviteAvailabilityService: availabilityService
        )

        await model.refresh(
            identity: AppNostrIdentity(material: material),
            serverURL: "https://chat.example"
        )

        XCTAssertEqual(model.profiles.map(\.displayName), ["Amy", "Zed"])
        XCTAssertEqual(model.profiles.map(\.inviteAvailability), [.unavailable, .available])
    }

    func testPeopleModelRechecksInviteAvailabilityForOneProfile() async throws {
        let material = try createNostrIdentity()
        let owner = material.accountId
        let bob = String(repeating: "b", count: 64)
        let relayService = NostrRelayProfileService(
            relays: ["wss://relay.example"],
            eventLoader: { _, filter, _, _ in
                if filter.kinds == [3] {
                    return [
                        NostrRelayEvent(
                            pubkey: owner,
                            createdAt: 1,
                            kind: 3,
                            tags: [["p", bob]],
                            content: ""
                        ),
                    ]
                }
                return []
            }
        )
        let availability = InviteAvailabilitySequence(accountID: bob)
        let availabilityService = FiniteInviteAvailabilityService(
            availabilityLoader: { _, accountIDs in
                await availability.next(for: accountIDs)
            }
        )
        let model = NostrPeopleModel(
            service: relayService,
            inviteAvailabilityService: availabilityService
        )

        await model.refresh(
            identity: AppNostrIdentity(material: material),
            serverURL: "https://chat.example"
        )
        XCTAssertEqual(model.profiles[0].inviteAvailability, .unavailable)

        let updated = try await model.recheckInviteAvailability(
            for: model.profiles[0],
            serverURL: "https://chat.example"
        )

        XCTAssertEqual(updated.pubkey, bob)
        XCTAssertEqual(updated.inviteAvailability, .available)
        XCTAssertEqual(model.profiles[0].pubkey, bob)
        XCTAssertEqual(model.profiles[0].inviteAvailability, .available)
    }

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

    func testFetchFollowProfilesUsesNip65WriteRelaysForContactList() async throws {
        let owner = String(repeating: "d", count: 64)
        let followed = String(repeating: "e", count: 64)

        let service = NostrRelayProfileService(
            relays: ["wss://bootstrap.example"],
            eventLoader: { relay, filter, _, _ in
                if filter.kinds == [10_002],
                   relay == "wss://bootstrap.example"
                {
                    return [
                        NostrRelayEvent(
                            pubkey: owner,
                            createdAt: 1_800_000_010,
                            kind: 10_002,
                            tags: [
                                ["r", "wss://read-only.example", "read"],
                                ["r", "wss://write.example", "write"],
                                ["r", "wss://implicit-read-write.example"],
                            ],
                            content: ""
                        ),
                    ]
                }

                if filter.kinds == [3],
                   relay == "wss://write.example"
                {
                    return [
                        NostrRelayEvent(
                            pubkey: owner,
                            createdAt: 1_800_000_011,
                            kind: 3,
                            tags: [["p", followed, "", "Relay Follow"]],
                            content: ""
                        ),
                    ]
                }

                return []
            }
        )

        let result = try await service.fetchFollowProfiles(forAccountID: owner)

        XCTAssertEqual(result.relayCount, 3)
        XCTAssertEqual(result.followedPubkeyCount, 1)
        XCTAssertEqual(result.profiles[0].displayName, "Relay Follow")
    }

    func testFetchFollowProfilesUsesContactRelayHintForMetadata() async throws {
        let owner = String(repeating: "4", count: 64)
        let followed = String(repeating: "5", count: 64)

        let service = NostrRelayProfileService(
            relays: ["wss://bootstrap.example"],
            eventLoader: { relay, filter, _, _ in
                if filter.kinds == [3],
                   relay == "wss://bootstrap.example"
                {
                    return [
                        NostrRelayEvent(
                            pubkey: owner,
                            createdAt: 1_800_000_020,
                            kind: 3,
                            tags: [["p", followed, "wss://profile.example", "Fallback Name"]],
                            content: ""
                        ),
                    ]
                }

                if filter.kinds == [0],
                   relay == "wss://profile.example"
                {
                    return [
                        NostrRelayEvent(
                            pubkey: followed,
                            createdAt: 1_800_000_021,
                            kind: 0,
                            tags: [],
                            content: #"{"display_name":"Relay Hint Profile","name":"relayhint"}"#
                        ),
                    ]
                }

                return []
            }
        )

        let result = try await service.fetchFollowProfiles(forAccountID: owner)

        XCTAssertEqual(result.followedPubkeyCount, 1)
        XCTAssertEqual(result.profiles[0].displayName, "Relay Hint Profile")
        XCTAssertEqual(result.profiles[0].username, "relayhint")
    }

    func testLiveFollowProfilesFixtureLoadsFromConfiguredRelays() async throws {
#if FINITECHAT_LIVE_NOSTR_TESTS
        let accountID = "4dcfa4f7ab49fb1484623c5f4c271fd0a079691c6d3ea3b1da0221a418638e8e"
        let relays = [
            "wss://relay.primal.net",
            "wss://nos.lol",
            "wss://relay.damus.io",
            "wss://us-east.nostr.pikachat.org",
            "wss://eu.nostr.pikachat.org",
        ]
        let service = NostrRelayProfileService(
            relays: relays,
            timeoutSeconds: 10
        )

        let result = try await service.fetchFollowProfiles(forAccountID: accountID)

        XCTAssertEqual(result.relayCount, relays.count)
        XCTAssertGreaterThanOrEqual(result.followedPubkeyCount, 4)
        XCTAssertGreaterThanOrEqual(result.profiles.count, 4)
        XCTAssertTrue(
            result.profiles.contains { $0.displayName.localizedCaseInsensitiveContains("jack") },
            "expected the live fixture to include Jack"
        )
        XCTAssertTrue(
            result.profiles.contains { $0.displayName.localizedCaseInsensitiveContains("fiatjaf") },
            "expected the live fixture to include fiatjaf"
        )
#else
        throw XCTSkip("Pass OTHER_SWIFT_FLAGS='$(inherited) -D FINITECHAT_LIVE_NOSTR_TESTS' to run the live Nostr relay fixture test.")
#endif
    }
}

private actor InviteAvailabilityChunkRecorder {
    private var chunks: [[String]] = []

    func record(_ chunk: [String]) {
        chunks.append(chunk)
    }

    func recordedChunks() -> [[String]] {
        chunks
    }
}

private actor InviteAvailabilitySequence {
    private let accountID: String
    private var calls = 0

    init(accountID: String) {
        self.accountID = accountID
    }

    func next(for accountIDs: [String]) -> [String: Bool] {
        calls += 1
        return Dictionary(uniqueKeysWithValues: accountIDs.map { accountID in
            (accountID, accountID == self.accountID && calls > 1)
        })
    }
}
