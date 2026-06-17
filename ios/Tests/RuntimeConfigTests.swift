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

    func testLaunchOverridesDoNotRewritePersistedConfig() throws {
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
        XCTAssertEqual(persisted.serverURL, "http://persisted.example")
        XCTAssertEqual(persisted.deviceID, "persisted-device")
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
        try Data().write(to: deviceStoreURL.appendingPathComponent("client.sqlite3"))

        let loaded = RuntimeConfig.load(
            environment: [:],
            args: ["FiniteChat"],
            storageURL: url
        )

        XCTAssertEqual(loaded.serverURL, "http://127.0.0.1:8787")
        XCTAssertEqual(loaded.deviceID, "qt433")
        XCTAssertEqual(try persistedConfig(at: url), loaded)
    }

    private func temporaryConfigURL() throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        return directory.appendingPathComponent("finitechat_config.json")
    }

    private func persistedConfig(at url: URL) throws -> RuntimeConfig {
        let data = try Data(contentsOf: url)
        return try JSONDecoder().decode(RuntimeConfig.self, from: data)
    }
}
