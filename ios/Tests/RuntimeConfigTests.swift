import XCTest
import CoreGraphics
@testable import FiniteChat

final class RuntimeConfigTests: XCTestCase {
    func testExplicitSaveTrimsAndPersistsConfig() throws {
        let url = try temporaryConfigURL()

        try RuntimeConfig(
            serverURL: "  http://127.0.0.1:8787  ",
            deviceID: "  ios  "
        ).save(storageURL: url)

        let persisted = try persistedConfig(at: url)
        XCTAssertEqual(persisted.serverURL, "http://127.0.0.1:8787")
        XCTAssertEqual(persisted.deviceID, "ios")
    }

    func testLaunchOverridesUseStableStoreWithoutRewritingStableConfig() throws {
        let url = try temporaryConfigURL()
        try RuntimeConfig(
            serverURL: "http://persisted.example",
            deviceID: "persisted-device"
        ).save(storageURL: url)

        let loaded = RuntimeConfig.load(
            environment: ["FINITECHAT_SERVER_URL": "http://env.example"],
            args: [
                "FiniteChat",
                "--finitechat-server",
                "http://args.example",
                "--finitechat-device",
                "transient-device",
            ],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://args.example")
        XCTAssertEqual(loaded.deviceID, "transient-device")
        XCTAssertFalse(loaded.usesTransientStore)
        XCTAssertTrue(loaded.persistsRuntimeIdentityUpdates)

        let persisted = try persistedConfig(at: url)
        XCTAssertEqual(persisted.serverURL, "http://persisted.example")
        XCTAssertEqual(persisted.deviceID, "persisted-device")
        XCTAssertFalse(persisted.usesTransientStore)

        let relaunched = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(relaunched.serverURL, "http://persisted.example")
        XCTAssertEqual(relaunched.deviceID, "persisted-device")
        XCTAssertFalse(relaunched.usesTransientStore)
    }

    func testExplicitPersistentLaunchOverridesPersistForManualRelaunch() throws {
        let url = try temporaryConfigURL()
        try RuntimeConfig(
            serverURL: "http://persisted.example",
            deviceID: "persisted-device"
        ).save(storageURL: url)

        let loaded = RuntimeConfig.load(
            environment: ["FINITECHAT_SERVER_URL": "http://env.example"],
            args: [
                "FiniteChat",
                "--finitechat-server",
                "http://args.example",
                "--finitechat-device",
                "persisted-override-device",
                "--finitechat-persist-launch-config",
            ],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://args.example")
        XCTAssertEqual(loaded.deviceID, "persisted-override-device")
        XCTAssertFalse(loaded.usesTransientStore)

        let persisted = try persistedConfig(at: url)
        XCTAssertEqual(persisted.serverURL, "http://args.example")
        XCTAssertEqual(persisted.deviceID, "persisted-override-device")
        XCTAssertFalse(persisted.usesTransientStore)

        let relaunched = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(relaunched.serverURL, "http://args.example")
        XCTAssertEqual(relaunched.deviceID, "persisted-override-device")
        XCTAssertFalse(relaunched.usesTransientStore)
    }

    func testTransientLaunchOverridesDoNotRewritePersistedConfig() throws {
        let url = try temporaryConfigURL()
        try RuntimeConfig(
            serverURL: "http://persisted.example",
            deviceID: "persisted-device"
        ).save(storageURL: url)

        let loaded = RuntimeConfig.load(
            environment: ["FINITECHAT_SERVER_URL": "http://env.example"],
            args: [
                "FiniteChat",
                "--finitechat-server",
                "http://args.example",
                "--finitechat-device",
                "transient-device",
                "--finitechat-transient-config",
            ],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://args.example")
        XCTAssertEqual(loaded.deviceID, "transient-device")
        XCTAssertTrue(loaded.usesTransientStore)

        let persisted = try persistedConfig(at: url)
        XCTAssertEqual(persisted.serverURL, "http://persisted.example")
        XCTAssertEqual(persisted.deviceID, "persisted-device")
        XCTAssertFalse(persisted.usesTransientStore)
    }

    func testLaunchAutomationUsesStableStoreAndDoesNotRewritePersistedConfigByDefault() throws {
        let url = try temporaryConfigURL()
        try RuntimeConfig(
            serverURL: "http://persisted.example",
            deviceID: "persisted-device"
        ).save(storageURL: url)

        let loaded = RuntimeConfig.load(
            environment: [:],
            args: [
                "FiniteChat",
                "--finitechat-server",
                "http://127.0.0.1:1",
                "--finitechat-device",
                "codex-persist-check",
                "--finitechat-auto-join",
                "finite://join?v=1&s=http%3A%2F%2F127.0.0.1%3A1&r=room-main&i=invite-1&t=token&a=npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqgcpfl3",
                "--finitechat-auto-send",
                "probe",
            ],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://127.0.0.1:1")
        XCTAssertEqual(loaded.deviceID, "codex-persist-check")
        XCTAssertFalse(loaded.usesTransientStore)
        XCTAssertTrue(loaded.persistsRuntimeIdentityUpdates)

        let persisted = try persistedConfig(at: url)
        XCTAssertEqual(persisted.serverURL, "http://persisted.example")
        XCTAssertEqual(persisted.deviceID, "persisted-device")
        XCTAssertFalse(persisted.usesTransientStore)

        let relaunched = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(relaunched.serverURL, "http://persisted.example")
        XCTAssertEqual(relaunched.deviceID, "persisted-device")
        XCTAssertFalse(relaunched.usesTransientStore)
    }

    func testExplicitPersistentLaunchAutomationOverridesPersistForManualRelaunch() throws {
        let url = try temporaryConfigURL()
        try RuntimeConfig(
            serverURL: "http://persisted.example",
            deviceID: "persisted-device"
        ).save(storageURL: url)

        let loaded = RuntimeConfig.load(
            environment: [:],
            args: [
                "FiniteChat",
                "--finitechat-server",
                "http://127.0.0.1:1",
                "--finitechat-device",
                "codex-persist-check",
                "--finitechat-auto-create-room",
                "Probe",
                "--finitechat-auto-send",
                "probe",
                "--finitechat-persist-launch-config",
            ],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://127.0.0.1:1")
        XCTAssertEqual(loaded.deviceID, "codex-persist-check")
        XCTAssertFalse(loaded.usesTransientStore)

        let persisted = try persistedConfig(at: url)
        XCTAssertEqual(persisted.serverURL, "http://127.0.0.1:1")
        XCTAssertEqual(persisted.deviceID, "codex-persist-check")
        XCTAssertFalse(persisted.usesTransientStore)

        let relaunched = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(relaunched.serverURL, "http://127.0.0.1:1")
        XCTAssertEqual(relaunched.deviceID, "codex-persist-check")
        XCTAssertFalse(relaunched.usesTransientStore)
    }

    func testExplicitTransientLaunchAutomationDoesNotRewritePersistedConfig() throws {
        let url = try temporaryConfigURL()
        try RuntimeConfig(
            serverURL: "http://persisted.example",
            deviceID: "persisted-device"
        ).save(storageURL: url)

        let loaded = RuntimeConfig.load(
            environment: [:],
            args: [
                "FiniteChat",
                "--finitechat-server",
                "http://127.0.0.1:1",
                "--finitechat-device",
                "codex-persist-check",
                "--finitechat-auto-join",
                "finite://join?v=1&s=http%3A%2F%2F127.0.0.1%3A1&r=room-main&i=invite-1&t=token&a=npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqgcpfl3",
                "--finitechat-auto-send",
                "probe",
                "--finitechat-transient-config",
            ],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://127.0.0.1:1")
        XCTAssertEqual(loaded.deviceID, "codex-persist-check")
        XCTAssertTrue(loaded.usesTransientStore)

        let persisted = try persistedConfig(at: url)
        XCTAssertEqual(persisted.serverURL, "http://persisted.example")
        XCTAssertEqual(persisted.deviceID, "persisted-device")
        XCTAssertFalse(persisted.usesTransientStore)
    }

    func testFirstLaunchAutomationSeedsStableConfigForManualRelaunch() throws {
        let url = try temporaryConfigURL()

        let loaded = RuntimeConfig.load(
            environment: [:],
            args: [
                "FiniteChat",
                "--finitechat-server",
                "http://127.0.0.1:1",
                "--finitechat-device",
                "codex-persist-check",
                "--finitechat-auto-create-room",
                "Probe",
            ],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://127.0.0.1:1")
        XCTAssertEqual(loaded.deviceID, "codex-persist-check")
        XCTAssertFalse(loaded.usesTransientStore)
        XCTAssertTrue(loaded.persistsRuntimeIdentityUpdates)
        XCTAssertEqual(try persistedConfig(at: url), loaded)

        let relaunched = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(relaunched.serverURL, "http://127.0.0.1:1")
        XCTAssertEqual(relaunched.deviceID, "codex-persist-check")
        XCTAssertFalse(relaunched.usesTransientStore)
    }

    func testPersistedServerOnlyConfigCombinesWithDefaultDevice() throws {
        let url = try temporaryConfigURL()
        try Data(#"{"server_url":"http://192.168.1.226:8789"}"#.utf8).write(to: url)

        let loaded = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://192.168.1.226:8789")
        XCTAssertEqual(loaded.deviceID, "ios")
        XCTAssertFalse(loaded.usesTransientStore)

        let persisted = try persistedConfig(at: url)
        XCTAssertEqual(persisted.serverURL, "http://192.168.1.226:8789")
        XCTAssertEqual(persisted.deviceID, "ios")
    }

    func testFirstLaunchOverridesSeedStableConfigForManualRelaunch() throws {
        let url = try temporaryConfigURL()

        let firstLaunch = RuntimeConfig.load(
            environment: [:],
            args: [
                "FiniteChat",
                "--finitechat-server",
                "http://192.168.1.226:8789",
                "--finitechat-device",
                "qt433",
            ],
            storageURL: url
        )

        XCTAssertEqual(firstLaunch.serverURL, "http://192.168.1.226:8789")
        XCTAssertEqual(firstLaunch.deviceID, "qt433")
        XCTAssertFalse(firstLaunch.usesTransientStore)
        XCTAssertEqual(try persistedConfig(at: url), firstLaunch)

        let relaunched = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(relaunched.serverURL, "http://192.168.1.226:8789")
        XCTAssertEqual(relaunched.deviceID, "qt433")
        XCTAssertFalse(relaunched.usesTransientStore)
    }

    func testFirstLaunchOverridesWithPersistSeedPersistedConfigForRelaunch() throws {
        let url = try temporaryConfigURL()

        let firstLaunch = RuntimeConfig.load(
            environment: [:],
            args: [
                "FiniteChat",
                "--finitechat-server",
                "http://192.168.1.226:8789",
                "--finitechat-device",
                "qt433",
                "--finitechat-persist-launch-config",
            ],
            storageURL: url
        )

        XCTAssertEqual(firstLaunch.serverURL, "http://192.168.1.226:8789")
        XCTAssertEqual(firstLaunch.deviceID, "qt433")
        XCTAssertFalse(firstLaunch.usesTransientStore)
        XCTAssertEqual(try persistedConfig(at: url), firstLaunch)

        let relaunched = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(relaunched.serverURL, "http://192.168.1.226:8789")
        XCTAssertEqual(relaunched.deviceID, "qt433")
    }

    func testMissingConfigAdoptsSingleExistingDeviceStore() throws {
        let url = try temporaryConfigURL()
        let supportURL = url.deletingLastPathComponent()
        let deviceStoreURL = supportURL
            .appendingPathComponent("FiniteChat", isDirectory: true)
            .appendingPathComponent("qt433", isDirectory: true)
        try FileManager.default.createDirectory(
            at: deviceStoreURL,
            withIntermediateDirectories: true
        )
        try Data("secret".utf8).write(to: deviceStoreURL.appendingPathComponent("account-secret.hex"))

        let loaded = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://127.0.0.1:8787")
        XCTAssertEqual(loaded.deviceID, "qt433")
        XCTAssertEqual(try persistedConfig(at: url), loaded)
    }

    func testMissingConfigDoesNotAdoptEmptyPlaceholderStore() throws {
        let url = try temporaryConfigURL()
        let supportURL = url.deletingLastPathComponent()
        let deviceStoreURL = supportURL
            .appendingPathComponent("FiniteChat", isDirectory: true)
            .appendingPathComponent("qt433", isDirectory: true)
        try FileManager.default.createDirectory(
            at: deviceStoreURL,
            withIntermediateDirectories: true
        )
        try Data().write(to: deviceStoreURL.appendingPathComponent("client.sqlite3"))

        let loaded = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://127.0.0.1:8787")
        XCTAssertEqual(loaded.deviceID, "ios")
        XCTAssertEqual(try persistedConfig(at: url), loaded)
    }

    func testPersistedEmptyDeviceRecoversUniqueInitializedStore() throws {
        let url = try temporaryConfigURL()
        try RuntimeConfig(
            serverURL: "http://192.168.1.226:8789",
            deviceID: "ios"
        ).save(storageURL: url)

        let supportURL = url.deletingLastPathComponent()
        let dataRoot = supportURL.appendingPathComponent("FiniteChat", isDirectory: true)
        let emptyStoreURL = dataRoot.appendingPathComponent("ios", isDirectory: true)
        let initializedStoreURL = dataRoot.appendingPathComponent("qt433", isDirectory: true)
        try FileManager.default.createDirectory(
            at: emptyStoreURL,
            withIntermediateDirectories: true
        )
        try FileManager.default.createDirectory(
            at: initializedStoreURL,
            withIntermediateDirectories: true
        )
        try Data().write(to: emptyStoreURL.appendingPathComponent("client.sqlite3"))
        try Data("secret".utf8)
            .write(to: initializedStoreURL.appendingPathComponent("account-secret.hex"))

        let loaded = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://192.168.1.226:8789")
        XCTAssertEqual(loaded.deviceID, "qt433")
        XCTAssertEqual(try persistedConfig(at: url), loaded)
    }

    func testMissingConfigDoesNotGuessAmongMultipleInitializedStores() throws {
        let url = try temporaryConfigURL()
        let supportURL = url.deletingLastPathComponent()
        let dataRoot = supportURL.appendingPathComponent("FiniteChat", isDirectory: true)
        for deviceID in ["alice", "bob"] {
            let storeURL = dataRoot.appendingPathComponent(deviceID, isDirectory: true)
            try FileManager.default.createDirectory(
                at: storeURL,
                withIntermediateDirectories: true
            )
            try Data("secret".utf8).write(to: storeURL.appendingPathComponent("account-secret.hex"))
        }

        let loaded = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://127.0.0.1:8787")
        XCTAssertEqual(loaded.deviceID, "ios")
        XCTAssertEqual(try persistedConfig(at: url), loaded)
    }

    func testRuntimeDataStoreMigratesRequestedLegacyStoreToStableStore() throws {
        let supportURL = try temporarySupportURL()
        let legacyStoreURL = supportURL
            .appendingPathComponent("FiniteChat", isDirectory: true)
            .appendingPathComponent("qt433", isDirectory: true)
        try FileManager.default.createDirectory(
            at: legacyStoreURL,
            withIntermediateDirectories: true
        )
        try Data("secret".utf8)
            .write(to: legacyStoreURL.appendingPathComponent("account-secret.hex"))
        try Data("sqlite".utf8)
            .write(to: legacyStoreURL.appendingPathComponent("client.sqlite3"))

        let dataDir = try RuntimeDataStore.dataDir(
            deviceID: "qt433",
            applicationSupportURL: supportURL
        )
        let migratedURL = URL(fileURLWithPath: dataDir)

        XCTAssertEqual(migratedURL.lastPathComponent, "FiniteChatStore")
        XCTAssertTrue(FileManager.default.fileExists(
            atPath: migratedURL.appendingPathComponent("account-secret.hex").path
        ))
        XCTAssertEqual(
            try Data(contentsOf: migratedURL.appendingPathComponent("client.sqlite3")),
            Data("sqlite".utf8)
        )
    }

    func testRuntimeDataStoreKeepsExistingStableStore() throws {
        let supportURL = try temporarySupportURL()
        let stableStoreURL = supportURL.appendingPathComponent("FiniteChatStore", isDirectory: true)
        try FileManager.default.createDirectory(
            at: stableStoreURL,
            withIntermediateDirectories: true
        )
        try Data("stable".utf8)
            .write(to: stableStoreURL.appendingPathComponent("account-secret.hex"))

        let legacyStoreURL = supportURL
            .appendingPathComponent("FiniteChat", isDirectory: true)
            .appendingPathComponent("qt433", isDirectory: true)
        try FileManager.default.createDirectory(
            at: legacyStoreURL,
            withIntermediateDirectories: true
        )
        try Data("legacy".utf8)
            .write(to: legacyStoreURL.appendingPathComponent("account-secret.hex"))

        let dataDir = try RuntimeDataStore.dataDir(
            deviceID: "qt433",
            applicationSupportURL: supportURL
        )
        let selectedURL = URL(fileURLWithPath: dataDir)

        XCTAssertEqual(selectedURL, stableStoreURL)
        XCTAssertEqual(
            try Data(contentsOf: stableStoreURL.appendingPathComponent("account-secret.hex")),
            Data("stable".utf8)
        )
    }

    func testRuntimeDataStoreUsesIsolatedTransientStore() throws {
        let supportURL = try temporarySupportURL()
        let stableStoreURL = supportURL.appendingPathComponent("FiniteChatStore", isDirectory: true)
        try FileManager.default.createDirectory(
            at: stableStoreURL,
            withIntermediateDirectories: true
        )
        try Data("stable".utf8)
            .write(to: stableStoreURL.appendingPathComponent("account-secret.hex"))

        let dataDir = try RuntimeDataStore.dataDir(
            deviceID: "codex/persist-check",
            applicationSupportURL: supportURL,
            transient: true
        )
        let transientURL = URL(fileURLWithPath: dataDir)

        XCTAssertEqual(transientURL.lastPathComponent, "codex-persist-check")
        XCTAssertEqual(transientURL.deletingLastPathComponent().lastPathComponent, "FiniteChatTransient")
        XCTAssertTrue(FileManager.default.fileExists(atPath: transientURL.path))
        XCTAssertEqual(
            try Data(contentsOf: stableStoreURL.appendingPathComponent("account-secret.hex")),
            Data("stable".utf8)
        )
    }

    private func temporaryConfigURL() throws -> URL {
        try temporarySupportURL().appendingPathComponent("finitechat_config.json")
    }

    private func temporarySupportURL() throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        return directory
    }

    private func persistedConfig(at url: URL) throws -> RuntimeConfig {
        let data = try Data(contentsOf: url)
        return try JSONDecoder().decode(RuntimeConfig.self, from: data)
    }
}

final class ChatTimelineTypingTests: XCTestCase {
    func testTypingRowHasStableNonDurableIdentity() {
        let row = ChatTimelineRow.typing([
            AppTypingMember(
                roomId: "room-main",
                accountId: "alice-account",
                deviceId: "alice-ios",
                displayName: "Alice",
                npub: nil
            ),
        ])

        XCTAssertEqual(row.id, "typing-indicator")
        XCTAssertNil(row.oldestMessageID)
    }

    func testGroupedMessageRowCanBeFoundByAnyContainedMessageID() {
        let first = chatMessage(id: "message-1", seq: 1, text: "first")
        let second = chatMessage(id: "message-2", seq: 2, text: "second")

        let rows = ChatTimeline.rows(messages: [first, second])

        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(
            ChatTimeline.rowID(containingMessageID: "message-1", rows: rows),
            "group-message-1"
        )
        XCTAssertEqual(
            ChatTimeline.rowID(containingMessageID: "message-2", rows: rows),
            "group-message-1"
        )
        XCTAssertNil(ChatTimeline.rowID(containingMessageID: "missing-message", rows: rows))
    }

    private func chatMessage(id: String, seq: UInt64, text: String) -> ChatMessage {
        ChatMessage(
            roomId: "room-main",
            seq: seq,
            messageId: id,
            conversationId: nil,
            senderAccountId: "alice-account",
            senderDeviceId: "alice-ios",
            senderDisplayName: "Alice",
            senderNpub: nil,
            text: text,
            displayContent: text,
            payload: Data(text.utf8),
            replyToMessageId: nil,
            isMine: false,
            delivery: .sent,
            reactions: [],
            media: [],
            readReceipt: nil,
            poll: nil,
            timestampUnixSeconds: 1_700_000_000 + seq,
            displayTimestamp: "now"
        )
    }
}

@MainActor
final class AppModelPersistenceTests: XCTestCase {
    func testForceCloseStyleRelaunchUsesSameStableStoreAndKeepsSavedProjection() throws {
        let supportURL = try temporarySupportURL()
        let configURL = supportURL.appendingPathComponent("finitechat_config.json")
        let config = RuntimeConfig(
            serverURL: "http://192.168.1.226:8789",
            deviceID: "qt433"
        )
        try config.save(storageURL: configURL)

        let savedState = savedChatState()
        let offlineState = savedChatState(
            status: "offline",
            toast: "Showing saved chats. Connection will retry."
        )
        var openedOptions: [OpenOptions] = []

        let firstRuntime = FakeFiniteChatRuntime(
            initialState: savedState,
            startRuntimeState: offlineState
        )
        let firstLaunch = AppModel(
            config: config,
            applicationSupportURL: supportURL,
            configStorageURL: configURL,
            args: ["FiniteChat"],
            startsUpdateLoop: false
        ) { options in
            openedOptions.append(options)
            return firstRuntime
        }

        firstLaunch.start()

        XCTAssertEqual(firstLaunch.rooms.map(\.roomId), ["room-main"])
        XCTAssertEqual(firstLaunch.selectedRoom?.roomId, "room-main")
        XCTAssertEqual(firstLaunch.selectedRoomMessages.map(\.text), ["saved before force close"])
        XCTAssertEqual(firstLaunch.chatProjections["room-main"]?.messages.map(\.text), [
            "saved before force close",
        ])
        XCTAssertEqual(firstLaunch.runtimeStorePath, openedOptions[0].dataDir)
        XCTAssertEqual(firstLaunch.state?.status, "offline")
        XCTAssertEqual(firstLaunch.userNoticeText, "Showing saved chats. Connection will retry.")
        XCTAssertEqual(firstLaunch.developerRuntimeStatus, "offline")
        XCTAssertEqual(firstRuntime.dispatchedActions, [.startRuntime])

        let relaunchConfig = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: configURL
        )
        let secondRuntime = FakeFiniteChatRuntime(
            initialState: savedState,
            startRuntimeState: offlineState
        )
        let relaunch = AppModel(
            config: relaunchConfig,
            applicationSupportURL: supportURL,
            configStorageURL: configURL,
            args: ["FiniteChat"],
            startsUpdateLoop: false
        ) { options in
            openedOptions.append(options)
            return secondRuntime
        }

        relaunch.start()

        XCTAssertEqual(openedOptions.count, 2)
        XCTAssertEqual(openedOptions[0].serverUrl, "http://192.168.1.226:8789")
        XCTAssertEqual(openedOptions[0].deviceId, "qt433")
        XCTAssertEqual(openedOptions[0].dataDir, openedOptions[1].dataDir)
        XCTAssertEqual(
            URL(fileURLWithPath: openedOptions[1].dataDir).lastPathComponent,
            "FiniteChatStore"
        )
        XCTAssertEqual(relaunch.runtimeStorePath, openedOptions[1].dataDir)
        XCTAssertEqual(relaunch.rooms.map(\.roomId), ["room-main"])
        XCTAssertEqual(relaunch.selectedRoomMessages.map(\.text), ["saved before force close"])
        XCTAssertEqual(relaunch.state?.status, "offline")
        XCTAssertEqual(
            relaunch.state?.toast,
            "Showing saved chats. Connection will retry."
        )
        XCTAssertEqual(relaunch.userNoticeText, "Showing saved chats. Connection will retry.")
        XCTAssertEqual(relaunch.developerRuntimeStatus, "offline")
        XCTAssertNil(relaunch.errorText)
        XCTAssertEqual(secondRuntime.dispatchedActions, [.startRuntime])
    }

    func testRawRuntimeDiagnosticsStayOutOfNormalChatSurfaces() throws {
        let config = RuntimeConfig(
            serverURL: "http://127.0.0.1:1",
            deviceID: "qt433"
        )
        let model = AppModel(
            config: config,
            applicationSupportURL: try temporarySupportURL(),
            args: ["FiniteChat"],
            startsUpdateLoop: false
        ) { _ in
            throw RawDiagnosticError(
                description: "HTTP runtime transport failed: server returned 404 Not Found"
            )
        }

        model.start()

        XCTAssertNil(model.userNoticeText)
        XCTAssertEqual(model.roomListEmptyDescription, "Open Settings to check connection.")
        XCTAssertEqual(
            model.developerErrorText,
            "HTTP runtime transport failed: server returned 404 Not Found"
        )
    }

    func testAttachmentCaptionOverrideDispatchesCaptionWithoutClearingComposerDraft() async throws {
        let config = RuntimeConfig(
            serverURL: "http://127.0.0.1:1",
            deviceID: "qt433"
        )
        let runtime = FakeFiniteChatRuntime(
            initialState: savedChatState(),
            startRuntimeState: savedChatState()
        )
        let model = AppModel(
            config: config,
            applicationSupportURL: try temporarySupportURL(),
            args: ["FiniteChat"],
            startsUpdateLoop: false
        ) { _ in
            runtime
        }
        model.start()
        model.outboundText = "typed draft"

        let attachment = OutboundAttachment(
            filename: "voice_1725000123.m4a",
            mimeType: VoiceRecordingAttachment.mimeType,
            kind: .voiceNote,
            bytes: Data([0x00, 0x01])
        )
        model.sendAttachments(
            roomID: "room-main",
            attachments: [attachment],
            captionOverride: "  hello from transcript  "
        )

        try await waitUntil {
            runtime.dispatchedActions.count >= 2
        }

        guard case .sendAttachments(
            let roomID,
            let attachments,
            let caption,
            let replyToMessageID
        ) = runtime.dispatchedActions.last else {
            return XCTFail("expected sendAttachments action")
        }
        XCTAssertEqual(roomID, "room-main")
        XCTAssertEqual(attachments, [attachment])
        XCTAssertEqual(caption, "hello from transcript")
        XCTAssertNil(replyToMessageID)
        XCTAssertEqual(model.outboundText, "typed draft")
    }

    func testDownloadAttachmentDispatchesRustBeginBeforeBlockingDownload() async throws {
        let config = RuntimeConfig(
            serverURL: "http://127.0.0.1:1",
            deviceID: "qt433"
        )
        var savedState = savedChatState()
        let attachment = ChatMediaAttachment(
            attachmentId: "attachment-1",
            url: "http://blob.example/sha256",
            mimeType: "image/jpeg",
            filename: "photo.jpg",
            kind: .image,
            width: nil,
            height: nil,
            localPath: nil,
            uploadProgressPerMille: nil,
            downloadProgressPerMille: nil
        )
        var message = savedState.messages[0]
        message.media = [attachment]
        savedState.messages = [message]
        let runtime = FakeFiniteChatRuntime(
            initialState: savedState,
            startRuntimeState: savedState
        )
        let model = AppModel(
            config: config,
            applicationSupportURL: try temporarySupportURL(),
            args: ["FiniteChat"],
            startsUpdateLoop: false
        ) { _ in
            runtime
        }
        model.start()

        model.downloadAttachment(roomID: "room-main", message: message, attachment: attachment)

        try await waitUntil {
            runtime.dispatchedActions.count >= 3
        }

        guard case .beginDownloadAttachment(
            let beginRoomID,
            let beginMessageID,
            let beginAttachmentID
        ) = runtime.dispatchedActions[1] else {
            return XCTFail("expected beginDownloadAttachment before blocking download")
        }
        XCTAssertEqual(beginRoomID, "room-main")
        XCTAssertEqual(beginMessageID, "message-1")
        XCTAssertEqual(beginAttachmentID, "attachment-1")

        guard case .downloadAttachment(
            let downloadRoomID,
            let downloadMessageID,
            let downloadAttachmentID
        ) = runtime.dispatchedActions[2] else {
            return XCTFail("expected downloadAttachment after beginDownloadAttachment")
        }
        XCTAssertEqual(downloadRoomID, "room-main")
        XCTAssertEqual(downloadMessageID, "message-1")
        XCTAssertEqual(downloadAttachmentID, "attachment-1")
    }

    func testOfflineNoticeIsSuppressedWhenThereAreNoSavedChats() throws {
        let config = RuntimeConfig(
            serverURL: "http://127.0.0.1:1",
            deviceID: "qt433"
        )
        let offlineEmpty = emptyChatState(
            status: "offline",
            toast: "Showing saved chats. Connection will retry."
        )
        let runtime = FakeFiniteChatRuntime(
            initialState: offlineEmpty,
            startRuntimeState: offlineEmpty
        )
        let model = AppModel(
            config: config,
            applicationSupportURL: try temporarySupportURL(),
            args: ["FiniteChat"],
            startsUpdateLoop: false
        ) { _ in
            runtime
        }

        model.start()

        XCTAssertNil(model.userNoticeText)
        XCTAssertEqual(model.roomListEmptyDescription, "No chats yet")
        XCTAssertEqual(model.developerRuntimeStatus, "offline")
    }

    func testExplicitTransientDiagnosticLaunchKeepsStableRelaunchOnSavedIdentity() throws {
        let supportURL = try temporarySupportURL()
        let configURL = supportURL.appendingPathComponent("finitechat_config.json")
        try RuntimeConfig(
            serverURL: "http://persisted.example",
            deviceID: "qt433"
        ).save(storageURL: configURL)

        let diagnosticArgs = [
            "FiniteChat",
            "--finitechat-server",
            "http://127.0.0.1:1",
            "--finitechat-device",
            "diagnostics-visual",
            "--finitechat-transient-config",
        ]
        let diagnosticConfig = RuntimeConfig.load(
            environment: [:],
            args: diagnosticArgs,
            storageURL: configURL
        )
        var openedOptions: [OpenOptions] = []
        let diagnosticRuntime = FakeFiniteChatRuntime(
            initialState: emptyChatState(deviceID: "diagnostics-visual"),
            startRuntimeState: emptyChatState(deviceID: "diagnostics-visual")
        )
        let diagnosticLaunch = AppModel(
            config: diagnosticConfig,
            applicationSupportURL: supportURL,
            configStorageURL: configURL,
            args: diagnosticArgs,
            startsUpdateLoop: false
        ) { options in
            openedOptions.append(options)
            return diagnosticRuntime
        }

        diagnosticLaunch.start()

        XCTAssertEqual(openedOptions.count, 1)
        XCTAssertEqual(openedOptions[0].serverUrl, "http://127.0.0.1:1")
        XCTAssertEqual(openedOptions[0].deviceId, "diagnostics-visual")
        let diagnosticStore = URL(fileURLWithPath: openedOptions[0].dataDir)
        XCTAssertEqual(diagnosticStore.lastPathComponent, "diagnostics-visual")
        XCTAssertEqual(diagnosticStore.deletingLastPathComponent().lastPathComponent, "FiniteChatTransient")
        XCTAssertEqual(diagnosticLaunch.runtimeStorePath, openedOptions[0].dataDir)

        let relaunchConfig = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: configURL
        )
        let relaunchRuntime = FakeFiniteChatRuntime(
            initialState: savedChatState(),
            startRuntimeState: savedChatState()
        )
        let relaunch = AppModel(
            config: relaunchConfig,
            applicationSupportURL: supportURL,
            configStorageURL: configURL,
            args: ["FiniteChat"],
            startsUpdateLoop: false
        ) { options in
            openedOptions.append(options)
            return relaunchRuntime
        }

        relaunch.start()

        XCTAssertEqual(openedOptions.count, 2)
        XCTAssertEqual(openedOptions[1].serverUrl, "http://persisted.example")
        XCTAssertEqual(openedOptions[1].deviceId, "qt433")
        XCTAssertEqual(
            URL(fileURLWithPath: openedOptions[1].dataDir).lastPathComponent,
            "FiniteChatStore"
        )
        XCTAssertEqual(relaunch.runtimeStorePath, openedOptions[1].dataDir)
        XCTAssertEqual(relaunch.selectedRoomMessages.map(\.text), ["saved before force close"])
    }

    func testStableLaunchOverridePersistsResolvedChatIdentityForManualRelaunch() throws {
        let supportURL = try temporarySupportURL()
        let configURL = supportURL.appendingPathComponent("finitechat_config.json")
        try RuntimeConfig(
            serverURL: "http://persisted.example",
            deviceID: "ios"
        ).save(storageURL: configURL)

        let launchArgs = [
            "FiniteChat",
            "--finitechat-server",
            "http://192.168.1.226:8789",
            "--finitechat-device",
            "qt433",
        ]
        let launchConfig = RuntimeConfig.load(
            environment: [:],
            args: launchArgs,
            storageURL: configURL
        )
        var openedOptions: [OpenOptions] = []
        let launchRuntime = FakeFiniteChatRuntime(
            initialState: savedChatState(),
            startRuntimeState: savedChatState()
        )
        let firstLaunch = AppModel(
            config: launchConfig,
            applicationSupportURL: supportURL,
            configStorageURL: configURL,
            args: launchArgs,
            startsUpdateLoop: false
        ) { options in
            openedOptions.append(options)
            return launchRuntime
        }

        firstLaunch.start()

        XCTAssertEqual(openedOptions.count, 1)
        XCTAssertEqual(openedOptions[0].serverUrl, "http://192.168.1.226:8789")
        XCTAssertEqual(openedOptions[0].deviceId, "qt433")
        XCTAssertEqual(
            URL(fileURLWithPath: openedOptions[0].dataDir).lastPathComponent,
            "FiniteChatStore"
        )
        XCTAssertEqual(firstLaunch.deviceID, "qt433")
        XCTAssertEqual(firstLaunch.selectedRoomMessages.map(\.text), ["saved before force close"])

        let persistedAfterLaunch = try persistedConfig(at: configURL)
        XCTAssertEqual(persistedAfterLaunch.serverURL, "http://192.168.1.226:8789")
        XCTAssertEqual(persistedAfterLaunch.deviceID, "qt433")

        let relaunchConfig = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: configURL
        )
        let relaunchRuntime = FakeFiniteChatRuntime(
            initialState: savedChatState(),
            startRuntimeState: savedChatState()
        )
        let relaunch = AppModel(
            config: relaunchConfig,
            applicationSupportURL: supportURL,
            configStorageURL: configURL,
            args: ["FiniteChat"],
            startsUpdateLoop: false
        ) { options in
            openedOptions.append(options)
            return relaunchRuntime
        }

        relaunch.start()

        XCTAssertEqual(openedOptions.count, 2)
        XCTAssertEqual(openedOptions[1].serverUrl, "http://192.168.1.226:8789")
        XCTAssertEqual(openedOptions[1].deviceId, "qt433")
        XCTAssertEqual(
            URL(fileURLWithPath: openedOptions[1].dataDir).lastPathComponent,
            "FiniteChatStore"
        )
        XCTAssertEqual(relaunch.selectedRoomMessages.map(\.text), ["saved before force close"])
    }

    private func savedChatState(
        status: String = "ready",
        toast: String? = nil
    ) -> AppState {
        let identity = Identity(
            accountId: "alice-account",
            deviceId: "qt433",
            accountSecretHex: String(repeating: "0", count: 64)
        )
        let room = AppRoomSummary(
            roomId: "room-main",
            displayName: "Main Room",
            state: .connected,
            status: "connected",
            lastMessagePreview: "saved before force close",
            unreadCount: 0,
            canLoadOlder: false
        )
        let message = ChatMessage(
            roomId: "room-main",
            seq: 1,
            messageId: "message-1",
            conversationId: nil,
            senderAccountId: "alice-account",
            senderDeviceId: "qt433",
            senderDisplayName: "qt433",
            senderNpub: nil,
            text: "saved before force close",
            displayContent: "saved before force close",
            payload: Data("saved before force close".utf8),
            replyToMessageId: nil,
            isMine: true,
            delivery: .sent,
            reactions: [],
            media: [],
            readReceipt: nil,
            poll: nil,
            timestampUnixSeconds: 1_700_000_000,
            displayTimestamp: "now"
        )
        return AppState(
            rev: 1,
            identity: identity,
            rooms: [room],
            selectedRoomId: "room-main",
            activeInvite: nil,
            activeProfileId: nil,
            status: status,
            toast: toast,
            messages: [message],
            profiles: [],
            devices: [],
            typingMembers: []
        )
    }

    private func emptyChatState(
        deviceID: String = "qt433",
        status: String = "ready",
        toast: String? = nil
    ) -> AppState {
        AppState(
            rev: 1,
            identity: Identity(
                accountId: "alice-account",
                deviceId: deviceID,
                accountSecretHex: String(repeating: "0", count: 64)
            ),
            rooms: [],
            selectedRoomId: nil,
            activeInvite: nil,
            activeProfileId: nil,
            status: status,
            toast: toast,
            messages: [],
            profiles: [],
            devices: [],
            typingMembers: []
        )
    }

    private func temporarySupportURL() throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        return directory
    }

    private func persistedConfig(at url: URL) throws -> RuntimeConfig {
        let data = try Data(contentsOf: url)
        return try JSONDecoder().decode(RuntimeConfig.self, from: data)
    }
}

final class ReactionEmojiCatalogTests: XCTestCase {
    func testEmptySearchShowsRecentSectionFirst() {
        let sections = ReactionEmojiCatalog.filteredSections(searchText: "")

        XCTAssertEqual(sections.first?.title, "Recent")
        XCTAssertEqual(
            Array(sections.first?.emojis.map(\.emoji).prefix(6) ?? []),
            ["❤️", "👍", "👎", "😂", "😮", "😢"]
        )
    }

    func testSearchMatchesEmojiNameAndKeywordWithoutDuplicates() {
        let rocketMatches = ReactionEmojiCatalog.filteredSections(searchText: "rocket")
        XCTAssertEqual(rocketMatches.map(\.title), ["Results"])
        XCTAssertEqual(rocketMatches.first?.emojis.map(\.emoji), ["🚀"])

        let agreeMatches = ReactionEmojiCatalog.filteredSections(searchText: "agree")
            .flatMap(\.emojis)
            .map(\.emoji)
        XCTAssertEqual(agreeMatches.count, Set(agreeMatches).count)
        XCTAssertTrue(agreeMatches.contains("👍"))
        XCTAssertTrue(agreeMatches.contains("🤝"))
    }

    func testWhitespaceOnlySearchUsesDefaultSections() {
        XCTAssertEqual(
            ReactionEmojiCatalog.filteredSections(searchText: "   "),
            ReactionEmojiCatalog.sections
        )
    }

    func testUnknownSearchReturnsNoSections() {
        XCTAssertTrue(ReactionEmojiCatalog.filteredSections(searchText: "not-an-emoji").isEmpty)
    }
}

final class SaveMediaActionTests: XCTestCase {
    func testSaveableImageAttachmentURLsIncludesOnlyLocalImages() {
        let message = chatMessage(media: [
            mediaAttachment(id: "image-local", kind: .image, localPath: "/tmp/photo.jpg"),
            mediaAttachment(id: "video-local", kind: .video, localPath: "/tmp/video.mov"),
            mediaAttachment(id: "image-remote", kind: .image, localPath: nil),
            mediaAttachment(id: "image-blank", kind: .image, localPath: "   "),
            mediaAttachment(id: "file-local", kind: .file, localPath: "/tmp/file.pdf"),
        ])

        XCTAssertEqual(
            saveableImageAttachmentURLs(in: message).map(\.path),
            ["/tmp/photo.jpg"]
        )
    }

    func testSaveableImageAttachmentURLsPreservesMultipleImagesInMessageOrder() {
        let message = chatMessage(media: [
            mediaAttachment(id: "first", kind: .image, localPath: "/tmp/first.jpg"),
            mediaAttachment(id: "second", kind: .image, localPath: "/tmp/second.png"),
        ])

        XCTAssertEqual(
            saveableImageAttachmentURLs(in: message).map(\.path),
            ["/tmp/first.jpg", "/tmp/second.png"]
        )
    }

    func testSaveMediaActionTitleMatchesImageCount() {
        XCTAssertNil(saveMediaActionTitle(imageCount: 0))
        XCTAssertEqual(saveMediaActionTitle(imageCount: 1), "Save Photo")
        XCTAssertEqual(saveMediaActionTitle(imageCount: 2), "Save Photos")
    }

    func testPhotoLibraryAddUsageDescriptionIsPresent() {
        let value = Bundle.main.object(
            forInfoDictionaryKey: "NSPhotoLibraryAddUsageDescription"
        ) as? String

        XCTAssertEqual(
            value,
            "FiniteChat saves photos from chats to your photo library when you choose Save Photo."
        )
    }

    private func chatMessage(media: [ChatMediaAttachment]) -> ChatMessage {
        ChatMessage(
            roomId: "room-main",
            seq: 1,
            messageId: "message-1",
            conversationId: nil,
            senderAccountId: "alice-account",
            senderDeviceId: "alice-ios",
            senderDisplayName: "Alice",
            senderNpub: nil,
            text: "photo",
            displayContent: "photo",
            payload: Data("photo".utf8),
            replyToMessageId: nil,
            isMine: false,
            delivery: .sent,
            reactions: [],
            media: media,
            readReceipt: nil,
            poll: nil,
            timestampUnixSeconds: 1_700_000_000,
            displayTimestamp: "now"
        )
    }

    private func mediaAttachment(
        id: String,
        kind: ChatMediaKind,
        localPath: String?
    ) -> ChatMediaAttachment {
        ChatMediaAttachment(
            attachmentId: id,
            url: nil,
            mimeType: kind == .image ? "image/jpeg" : "application/octet-stream",
            filename: "\(id).dat",
            kind: kind,
            width: nil,
            height: nil,
            localPath: localPath,
            uploadProgressPerMille: nil,
            downloadProgressPerMille: nil
        )
    }
}

private struct RawDiagnosticError: Error, CustomStringConvertible {
    let description: String
}

private final class FakeFiniteChatRuntime: FiniteChatRuntimeProtocol, @unchecked Sendable {
    private var currentState: AppState
    private let startRuntimeState: AppState
    private(set) var dispatchedActions: [AppAction] = []

    init(initialState: AppState, startRuntimeState: AppState) {
        currentState = initialState
        self.startRuntimeState = startRuntimeState
    }

    func state() throws -> AppState {
        currentState
    }

    func dispatch(action: AppAction) throws -> AppState {
        dispatchedActions.append(action)
        if action == .startRuntime {
            currentState = startRuntimeState
        }
        return currentState
    }

    func waitForUpdate(timeoutMillis: UInt64) throws -> AppState {
        currentState
    }
}

final class MessageCollectionLayoutTests: XCTestCase {
    func testJumpButtonSpacingMatchesKeyboardChromeGap() {
        XCTAssertEqual(MessageCollectionLayout.jumpButtonSpacing, 12)
    }

    func testEffectiveContentInsetAccountsForAccessoryInset() {
        let inset = MessageCollectionLayout.effectiveContentInset(
            boundsHeight: 600,
            contentHeight: 180,
            topChromeInset: 44,
            bottomInset: 72
        )

        XCTAssertEqual(inset.top, 292)
        XCTAssertEqual(inset.bottom, 84)
    }

    func testNearBottomUsesVisibleViewportBottom() {
        XCTAssertTrue(
            MessageCollectionLayout.isNearBottom(
                contentOffsetY: 900,
                boundsHeight: 500,
                contentHeight: 1300,
                topAdjustedInset: 30,
                bottomInset: 106
            )
        )
        XCTAssertFalse(
            MessageCollectionLayout.isNearBottom(
                contentOffsetY: 700,
                boundsHeight: 500,
                contentHeight: 1300,
                topAdjustedInset: 30,
                bottomInset: 106
            )
        )
    }

    func testBottomContentOffsetUsesHostOwnedBottomInset() {
        let offset = MessageCollectionLayout.bottomContentOffset(
            contentHeight: 1300,
            boundsHeight: 500,
            topAdjustedInset: 30,
            bottomInset: 72
        )
        XCTAssertEqual(offset, CGPoint(x: 0, y: 872))
    }

    func testUpdateClassificationUsesTailMutationForAppendAndTrim() {
        XCTAssertEqual(
            MessageCollectionLayout.classifyUpdate(
                oldIDs: ["a", "b"],
                newIDs: ["a", "b", "c"]
            ),
            .tailMutation
        )
        XCTAssertEqual(
            MessageCollectionLayout.classifyUpdate(
                oldIDs: ["a", "b", "c"],
                newIDs: ["a", "b"]
            ),
            .tailMutation
        )
    }

    func testUpdateClassificationTreatsReshapesAsStructural() {
        XCTAssertEqual(
            MessageCollectionLayout.classifyUpdate(
                oldIDs: ["row-1", "row-2"],
                newIDs: ["row-0", "row-2"]
            ),
            .structural
        )
        XCTAssertEqual(
            MessageCollectionLayout.classifyUpdate(
                oldIDs: ["row-1", "row-2"],
                newIDs: ["row-1", "row-2"]
            ),
            .reconfigureOnly
        )
    }
}

final class StagedComposerAttachmentTests: XCTestCase {
    func testFileURLStagesOutboundAttachmentMetadataAndBytes() throws {
        let directory = try temporaryDirectory()
        let url = directory.appendingPathComponent("sample.png")
        let bytes = Data([0x89, 0x50, 0x4E, 0x47])
        try bytes.write(to: url)

        let staged = try StagedComposerAttachment(fileURL: url)
        let outbound = staged.outboundAttachment

        XCTAssertEqual(staged.filename, "sample.png")
        XCTAssertEqual(staged.mimeType, "image/png")
        XCTAssertEqual(staged.kind, .image)
        XCTAssertEqual(outbound.filename, "sample.png")
        XCTAssertEqual(outbound.mimeType, "image/png")
        XCTAssertEqual(outbound.kind, .image)
        XCTAssertEqual(outbound.bytes, bytes)
    }

    func testFileURLRejectsProtocolOversizedAttachment() throws {
        let directory = try temporaryDirectory()
        let url = directory.appendingPathComponent("too-large.bin")
        try Data(count: maxComposerAttachmentBytes + 1).write(to: url)

        XCTAssertThrowsError(try StagedComposerAttachment(fileURL: url)) { error in
            guard case ComposerAttachmentError.tooLarge(let filename) = error else {
                return XCTFail("Unexpected error: \(error)")
            }
            XCTAssertEqual(filename, "too-large.bin")
        }
    }

    func testPastedImageStagesAsOutboundAttachment() throws {
        let bytes = Data([0x47, 0x49, 0x46, 0x38])

        let staged = try StagedComposerAttachment(
            pastedData: bytes,
            mimeType: "image/gif"
        )
        let outbound = staged.outboundAttachment

        XCTAssertTrue(staged.filename.hasPrefix("pasted-"))
        XCTAssertTrue(staged.filename.hasSuffix(".gif"))
        XCTAssertEqual(staged.mimeType, "image/gif")
        XCTAssertEqual(staged.kind, .image)
        XCTAssertEqual(outbound.mimeType, "image/gif")
        XCTAssertEqual(outbound.kind, .image)
        XCTAssertEqual(outbound.bytes, bytes)
    }

    private func temporaryDirectory() throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        return directory
    }
}

final class VoiceMessageTests: XCTestCase {
    func testVoiceRecordingAttachmentUsesProtocolVoiceKind() throws {
        let bytes = Data([0x00, 0x01, 0x02])
        let now = Date(timeIntervalSince1970: 1_725_000_123)

        let attachment = try VoiceRecordingAttachment.outboundAttachment(data: bytes, now: now)

        XCTAssertEqual(attachment.filename, "voice_1725000123.m4a")
        XCTAssertEqual(attachment.mimeType, "audio/mp4")
        XCTAssertEqual(attachment.kind, .voiceNote)
        XCTAssertEqual(attachment.bytes, bytes)
    }

    func testVoiceRecordingAttachmentRejectsOversizeBeforeDispatch() {
        let now = Date(timeIntervalSince1970: 1_725_000_123)

        XCTAssertThrowsError(try VoiceRecordingAttachment.outboundAttachment(
            data: Data(count: maxComposerAttachmentBytes + 1),
            now: now
        )) { error in
            guard case ComposerAttachmentError.tooLarge(let filename) = error else {
                return XCTFail("Unexpected error: \(error)")
            }
            XCTAssertEqual(filename, "voice_1725000123.m4a")
        }
    }

    func testVoiceDurationFormattingUsesMonospacedClockShape() {
        XCTAssertEqual(formattedDuration(0), "0:00")
        XCTAssertEqual(formattedDuration(65.9), "1:05")
        XCTAssertEqual(formattedDuration(3_605), "60:05")
    }

    func testVoiceRecordingCaptionTrimsTranscript() {
        XCTAssertEqual(
            voiceRecordingCaption(VoiceRecordingState(
                phase: .recording,
                durationSecs: 1,
                levels: [],
                transcript: "  Hello from speech  \n"
            )),
            "Hello from speech"
        )
        XCTAssertEqual(
            voiceRecordingCaption(VoiceRecordingState(
                phase: .paused,
                durationSecs: 1,
                levels: [],
                transcript: " \n "
            )),
            ""
        )
        XCTAssertEqual(voiceRecordingCaption(nil), "")
    }
}

private func waitUntil(
    timeout: TimeInterval = 2,
    condition: @escaping @MainActor () -> Bool
) async throws {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if await condition() {
            return
        }
        try await Task.sleep(nanoseconds: 10_000_000)
    }
    XCTFail("timed out waiting for condition")
}
