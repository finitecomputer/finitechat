import XCTest
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

    func testLaunchOverridesPersistForManualRelaunch() throws {
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

        let persisted = try persistedConfig(at: url)
        XCTAssertEqual(persisted.serverURL, "http://args.example")
        XCTAssertEqual(persisted.deviceID, "transient-device")

        let relaunched = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(relaunched.serverURL, "http://args.example")
        XCTAssertEqual(relaunched.deviceID, "transient-device")
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

    func testLaunchAutomationOverridesPersistForManualRelaunch() throws {
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

    func testFirstLaunchOverridesSeedPersistedConfigForRelaunch() throws {
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
