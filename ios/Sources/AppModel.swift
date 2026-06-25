import Foundation
import SwiftUI
import UIKit
import UniformTypeIdentifiers

struct RuntimeConfig: Codable, Equatable {
    let serverURL: String
    let deviceID: String
    let usesTransientStore: Bool
    let persistsRuntimeIdentityUpdates: Bool

    private static let defaultServerURL = "https://chat.finite.computer"
    private static let defaultDeviceID = "ios"
    private static let transientConfigArgument = "--finitechat-transient-config"
    private static let transientConfigEnvironmentKey = "FINITECHAT_TRANSIENT_CONFIG"
    private static let persistLaunchConfigArgument = "--finitechat-persist-launch-config"
    private static let persistLaunchConfigEnvironmentKey = "FINITECHAT_PERSIST_LAUNCH_CONFIG"

    enum CodingKeys: String, CodingKey {
        case serverURL = "server_url"
        case deviceID = "device_id"
    }

    init(
        serverURL: String,
        deviceID: String,
        usesTransientStore: Bool = false,
        persistsRuntimeIdentityUpdates: Bool = true
    ) {
        self.serverURL = serverURL
        self.deviceID = deviceID
        self.usesTransientStore = usesTransientStore
        self.persistsRuntimeIdentityUpdates = persistsRuntimeIdentityUpdates
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        serverURL = try container.decode(String.self, forKey: .serverURL)
        deviceID = try container.decode(String.self, forKey: .deviceID)
        usesTransientStore = false
        persistsRuntimeIdentityUpdates = true
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(serverURL, forKey: .serverURL)
        try container.encode(deviceID, forKey: .deviceID)
    }

    static func load(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        args: [String] = CommandLine.arguments,
        storageURL: URL? = nil
    ) -> RuntimeConfig {
        let serverURL = argumentValue("--finitechat-server", in: args)
            ?? environmentValue("FINITECHAT_SERVER_URL", in: environment)
        let deviceID = argumentValue("--finitechat-device", in: args)
            ?? environmentValue("FINITECHAT_DEVICE_ID", in: environment)
        let persisted = loadPersisted(storageURL: storageURL)
        let fallback = RuntimeConfig(
            serverURL: persisted.serverURL ?? defaultServerURL,
            deviceID: persisted.deviceID ?? defaultDeviceID
        )
        let hostedUnitTest = storageURL == nil && environment["XCTestConfigurationFilePath"] != nil
        let persistLaunchOverride = argumentFlag(persistLaunchConfigArgument, in: args)
            || truthyEnvironmentValue(persistLaunchConfigEnvironmentKey, in: environment)
        let hasLaunchOverride = serverURL != nil || deviceID != nil
        let hasPersistentLaunchState = persisted.serverURL != nil
            || persisted.deviceID != nil
        let transientOverride = argumentFlag(transientConfigArgument, in: args)
            || truthyEnvironmentValue(transientConfigEnvironmentKey, in: environment)
            || hostedUnitTest
        let shouldPersistFirstLaunchOverride = hasLaunchOverride
            && !hasPersistentLaunchState
        let shouldPersistResolvedIdentity = !transientOverride
        let config = RuntimeConfig(
            serverURL: serverURL ?? fallback.serverURL,
            deviceID: deviceID ?? fallback.deviceID,
            usesTransientStore: transientOverride,
            persistsRuntimeIdentityUpdates: shouldPersistResolvedIdentity
        )
        let shouldPersistFallbackRepair = !hasLaunchOverride
            && (
                persisted.serverURL != config.serverURL
                    || persisted.deviceID != config.deviceID
            )
        // Runtime identity is product state. First-run launch values can seed a
        // stable client store. Existing saved identities are not rewritten by
        // one-off launch overrides unless the caller explicitly persists them.
        if !transientOverride
            && (
                shouldPersistFallbackRepair
                    || shouldPersistFirstLaunchOverride
                    || persistLaunchOverride
            )
        {
            try? config.save(storageURL: storageURL)
        }
        return config
    }

    func save(storageURL: URL? = nil) throws {
        let config = RuntimeConfig(
            serverURL: serverURL.trimmingCharacters(in: .whitespacesAndNewlines),
            deviceID: deviceID.trimmingCharacters(in: .whitespacesAndNewlines)
        )
        guard !config.serverURL.isEmpty, !config.deviceID.isEmpty else {
            throw ConfigError.emptyValue
        }
        let data = try JSONEncoder().encode(config)
        let url = try storageURL ?? Self.configURL()
        try data.write(to: url, options: .atomic)
    }

    private static func loadPersisted(storageURL: URL?) -> PersistedRuntimeConfig {
        guard let url = storageURL ?? (try? configURL()),
              let data = try? Data(contentsOf: url),
              let config = try? JSONDecoder().decode(PersistedRuntimeConfig.self, from: data)
        else {
            return PersistedRuntimeConfig()
        }
        return config.normalized()
    }

    private static func configURL() throws -> URL {
        let support = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        return support.appendingPathComponent("finitechat_config.json")
    }

    enum ConfigError: Error {
        case emptyValue
    }

    private static func environmentValue(
        _ key: String,
        in environment: [String: String]
    ) -> String? {
        let value = environment[key]?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let value, !value.isEmpty else { return nil }
        return value
    }

    private static func argumentValue(_ name: String, in args: [String]) -> String? {
        guard let index = args.firstIndex(of: name) else {
            return nil
        }
        let valueIndex = args.index(after: index)
        guard valueIndex < args.endIndex else {
            return nil
        }
        let value = args[valueIndex].trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty ? nil : value
    }

    private static func argumentFlag(_ name: String, in args: [String]) -> Bool {
        args.contains(name)
    }

    private static func truthyEnvironmentValue(
        _ key: String,
        in environment: [String: String]
    ) -> Bool {
        guard let value = environment[key]?.trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased(),
            !value.isEmpty
        else {
            return false
        }
        return !["0", "false", "no", "off"].contains(value)
    }
}

private struct PersistedRuntimeConfig: Codable, Equatable {
    var serverURL: String?
    var deviceID: String?

    enum CodingKeys: String, CodingKey {
        case serverURL = "server_url"
        case deviceID = "device_id"
    }

    init(serverURL: String? = nil, deviceID: String? = nil) {
        self.serverURL = serverURL
        self.deviceID = deviceID
    }

    func normalized() -> PersistedRuntimeConfig {
        PersistedRuntimeConfig(
            serverURL: normalizedNonEmpty(serverURL),
            deviceID: normalizedNonEmpty(deviceID)
        )
    }

    private func normalizedNonEmpty(_ value: String?) -> String? {
        let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let trimmed, !trimmed.isEmpty else { return nil }
        return trimmed
    }
}

typealias AppRuntimeFactory = (OpenOptions) throws -> any FiniteChatRuntimeProtocol

struct RuntimeDataStore {
    private static let currentDataDirectoryName = "FiniteChatStore"
    private static let transientDataRootDirectoryName = "FiniteChatTransient"

    static func dataDir(
        deviceID: String,
        applicationSupportURL: URL? = nil,
        transient: Bool = false
    ) throws -> String {
        let supportURL: URL
        if let applicationSupportURL {
            supportURL = applicationSupportURL
        } else {
            supportURL = try defaultApplicationSupportURL(create: true)
        }
        if transient {
            let transientStoreURL = supportURL
                .appendingPathComponent(transientDataRootDirectoryName, isDirectory: true)
                .appendingPathComponent(safeDeviceDirectoryName(deviceID), isDirectory: true)
            try FileManager.default.createDirectory(
                at: transientStoreURL,
                withIntermediateDirectories: true
            )
            return transientStoreURL.path
        }
        let currentStoreURL = supportURL.appendingPathComponent(
            currentDataDirectoryName,
            isDirectory: true
        )
        try FileManager.default.createDirectory(
            at: currentStoreURL,
            withIntermediateDirectories: true
        )
        return currentStoreURL.path
    }

    static func deleteDataDir(
        deviceID: String,
        applicationSupportURL: URL? = nil,
        transient: Bool = false
    ) throws {
        let path = try dataDir(
            deviceID: deviceID,
            applicationSupportURL: applicationSupportURL,
            transient: transient
        )
        let url = URL(fileURLWithPath: path, isDirectory: true)
        if FileManager.default.fileExists(atPath: url.path) {
            try FileManager.default.removeItem(at: url)
        }
    }

    static func hasRecoverableStableStore(applicationSupportURL: URL? = nil) -> Bool {
        let supportURL: URL
        if let applicationSupportURL {
            supportURL = applicationSupportURL
        } else if let defaultURL = try? defaultApplicationSupportURL(create: false) {
            supportURL = defaultURL
        } else {
            return false
        }
        let storeURL = supportURL.appendingPathComponent(
            currentDataDirectoryName,
            isDirectory: true
        )
        return FileManager.default.fileExists(
            atPath: storeURL.appendingPathComponent("account-secret.hex").path
        ) && FileManager.default.fileExists(
            atPath: storeURL.appendingPathComponent("client.sqlite3").path
        )
    }

    private static func safeDeviceDirectoryName(_ deviceID: String) -> String {
        deviceID
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "/", with: "-")
    }

    private static func defaultApplicationSupportURL(create: Bool) throws -> URL {
        try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: create
        )
    }
}

enum AppScanTargetResult {
    case empty
    case profile(AppProfileSummary)
    case room(AppRoomSummary)
    case unavailable
}

private struct ProductHarnessSupportResolution {
    let url: URL?
    let error: String?
}

private struct AppLaunchConfigurationError: Error, CustomStringConvertible {
    let message: String

    var description: String {
        message
    }
}

struct DeveloperDiagnosticEntry: Identifiable, Equatable {
    let id: Int
    let timestampUnixSeconds: Int64
    let category: String
    let event: String
    let details: [String: String]

    static func exportText(_ entries: [DeveloperDiagnosticEntry]) -> String {
        var lines = [
            "Finite Chat diagnostics",
            "redaction=urls,paths,long-hex",
            "event_count=\(entries.count)",
        ]
        for entry in entries {
            let details = entry.details
                .sorted { $0.key < $1.key }
                .map { "\($0.key)=\($0.value)" }
                .joined(separator: " ")
            if details.isEmpty {
                lines.append(
                    "seq=\(entry.id) ts=\(entry.timestampUnixSeconds) category=\(entry.category) event=\(entry.event)"
                )
            } else {
                lines.append(
                    "seq=\(entry.id) ts=\(entry.timestampUnixSeconds) category=\(entry.category) event=\(entry.event) \(details)"
                )
            }
        }
        return lines.joined(separator: "\n")
    }
}

private struct DiagnosticActionSummary {
    let category: String
    let name: String
    let details: [String: String]
}

@MainActor
final class AppModel: ObservableObject {
    private static let developerDiagnosticsLimit = 200

    @Published var serverURL: String
    @Published var deviceID: String
    @Published private(set) var state: AppState? {
        didSet {
            rebuildChatProjections()
            if let state {
                appendStateDiagnostic(state, event: "state.projected")
            }
        }
    }
    private(set) var chatProjections: [String: ChatRoomProjection] = [:]
    @Published var errorText: String?
    @Published var roomDraft: String = ""
    @Published var scanDraft: String = ""
    @Published var pinDraft: String = ""
    @Published var outboundText: String = ""
    @Published private(set) var runtimeStorePath: String?
    @Published private(set) var developerDiagnostics: [DeveloperDiagnosticEntry] = []
    @Published private(set) var nostrIdentity: AppNostrIdentity?
    @Published private(set) var requiresNostrLogin: Bool

    private var runtime: (any FiniteChatRuntimeProtocol)?
    private var openKey = ""
    private let usesTransientStore: Bool
    private let persistsRuntimeIdentityUpdates: Bool
    private let applicationSupportURL: URL?
    private let configStorageURL: URL?
    private let args: [String]
    private let runtimeFactory: AppRuntimeFactory
    private let startsUpdateLoop: Bool
    private let nostrIdentityStore: AppNostrIdentityStoring
    private var updateTask: Task<Void, Never>?
    private var launchAutomationTask: Task<Void, Never>?
    private var postSendCatchUpTask: Task<Void, Never>?
    private var attachmentDownloadsInFlight = Set<String>()
    private var messageRetriesInFlight = Set<String>()
    private var lastTypingIntentByRoom: [String: Bool] = [:]
    private var pendingPushToken: String?
    private var didRunLaunchAutomation = false
    private let launchConfigurationError: String?

    deinit {
        updateTask?.cancel()
        launchAutomationTask?.cancel()
        postSendCatchUpTask?.cancel()
    }

    init(
        config: RuntimeConfig? = nil,
        applicationSupportURL: URL? = nil,
        configStorageURL: URL? = nil,
        args: [String] = CommandLine.arguments,
        requiresNostrLogin: Bool = false,
        nostrIdentityStore: AppNostrIdentityStoring = KeychainNostrIdentityStore(),
        startsUpdateLoop: Bool = true,
        runtimeFactory: @escaping AppRuntimeFactory = { options in
            try FiniteChatRuntime.open(options: options)
        }
    ) {
        let productHarnessSupport = Self.productHarnessApplicationSupportURL(args: args)
        let resolvedApplicationSupportURL = applicationSupportURL ?? productHarnessSupport.url
        let resolvedConfigStorageURL = configStorageURL
            ?? resolvedApplicationSupportURL?.appendingPathComponent("finitechat_config.json")
        let resolvedConfig = config ?? RuntimeConfig.load(storageURL: resolvedConfigStorageURL)
        serverURL = resolvedConfig.serverURL
        deviceID = resolvedConfig.deviceID
        usesTransientStore = resolvedConfig.usesTransientStore
        persistsRuntimeIdentityUpdates = resolvedConfig.persistsRuntimeIdentityUpdates
        self.applicationSupportURL = resolvedApplicationSupportURL
        self.configStorageURL = configStorageURL
            ?? resolvedConfigStorageURL
        self.args = args
        self.runtimeFactory = runtimeFactory
        self.startsUpdateLoop = startsUpdateLoop
        self.nostrIdentityStore = nostrIdentityStore
        let storedNostrIdentity = nostrIdentityStore.load()
        let hasRecoverableRuntimeIdentity = !usesTransientStore
            && storedNostrIdentity == nil
            && RuntimeDataStore.hasRecoverableStableStore(
                applicationSupportURL: resolvedApplicationSupportURL
            )
        nostrIdentity = storedNostrIdentity
        self.requiresNostrLogin = requiresNostrLogin
            && storedNostrIdentity == nil
            && !hasRecoverableRuntimeIdentity
            && !Self.hasLaunchAutomation(args: args)
        launchConfigurationError = productHarnessSupport.error
        appendDiagnostic(
            category: "persistence",
            event: "app.configured",
            details: [
                "store_mode": usesTransientStore ? "transient" : "stable",
                "has_explicit_support_root": resolvedApplicationSupportURL == nil ? "false" : "true",
                "has_recoverable_runtime_identity": hasRecoverableRuntimeIdentity ? "true" : "false",
                "has_launch_configuration_error": launchConfigurationError == nil ? "false" : "true",
                "requires_nostr_login": self.requiresNostrLogin ? "true" : "false",
            ]
        )
    }

    var rooms: [AppRoomSummary] {
        state?.rooms ?? []
    }

    var selectedRoom: AppRoomSummary? {
        guard let state else { return nil }
        if let selected = state.selectedRoomId,
           let room = state.rooms.first(where: { $0.roomId == selected })
        {
            return room
        }
        return state.rooms.first
    }

    var selectedRoomMessages: [ChatMessage] {
        guard let roomId = selectedRoom?.roomId else { return [] }
        return projection(for: roomId).messages
    }

    var roomListEmptyDescription: String {
        if developerErrorText != nil {
            return "Open Settings to check connection."
        }
        if state == nil {
            return "Opening chats..."
        }
        return "No chats yet"
    }

    var userNoticeText: String? {
        let toast = state?.toast?.nonEmptyTrimmed
        if toast == "Showing saved chats. Connection will retry." {
            return nil
        }
        return toast
    }

    var actionNoticeText: String? {
        userNoticeText ?? developerErrorText
    }

    var developerErrorText: String? {
        errorText?.nonEmptyTrimmed
    }

    var developerRuntimeStatus: String? {
        state?.status.nonEmptyTrimmed
    }

    var developerPersistenceSummary: String {
        let roomCount = rooms.count
        let selectedRoomID = state?.selectedRoomId?.nonEmptyTrimmed ?? "none"
        let selectedMessages = selectedRoomMessages.count
        let projectedMessages = state?.messages.count ?? 0
        return "\(roomCount) room(s), selected \(selectedRoomID), \(selectedMessages) selected message(s), \(projectedMessages) projected message(s)"
    }

    var developerDiagnosticsExport: String {
        DeveloperDiagnosticEntry.exportText(developerDiagnostics)
    }

    var developerDiagnosticsPreview: [DeveloperDiagnosticEntry] {
        Array(developerDiagnostics.suffix(8))
    }

    var activeProfile: AppProfileSummary? {
        guard let state, let activeProfileId = state.activeProfileId else { return nil }
        return state.profiles.first { $0.accountId == activeProfileId }
    }

    var myNpub: String? {
        if let npub = nostrIdentity?.npub {
            return npub
        }
        guard let accountID = state?.identity.accountId.nonEmptyTrimmed else {
            return nil
        }
        return try? npubFromAccountId(accountId: accountID)
    }

    var activeAccountID: String? {
        nostrIdentity?.accountID.nonEmptyTrimmed
            ?? state?.identity.accountId.nonEmptyTrimmed
    }

    @discardableResult
    func createAndSignInNostrIdentity() -> Bool {
        do {
            let material = try createNostrIdentity()
            try applyNostrIdentity(AppNostrIdentity(material: material), resetStore: true)
            return true
        } catch {
            errorText = String(describing: error)
            return false
        }
    }

    @discardableResult
    func signInWithNsec(_ nsec: String) -> Bool {
        do {
            let material = try nostrIdentityFromNsec(nsec: nsec)
            try applyNostrIdentity(AppNostrIdentity(material: material), resetStore: true)
            return true
        } catch {
            errorText = String(describing: error)
            return false
        }
    }

    func signOutAndDeleteEverything() {
        appendDiagnostic(category: "persistence", event: "signout.delete_all.requested")
        removePushTokenIfPossible()
        pendingPushToken = nil
        nostrIdentityStore.clear()
        closeRuntime()
        try? RuntimeDataStore.deleteDataDir(
            deviceID: deviceID,
            applicationSupportURL: applicationSupportURL,
            transient: usesTransientStore
        )
        if let configStorageURL {
            try? FileManager.default.removeItem(at: configStorageURL)
        }
        let resetConfig = RuntimeConfig.load(args: args, storageURL: configStorageURL)
        serverURL = resetConfig.serverURL
        deviceID = resetConfig.deviceID
        nostrIdentity = nil
        requiresNostrLogin = true
        errorText = nil
    }

    var canSend: Bool {
        guard let selectedRoom else { return false }
        return selectedRoom.state == .connected
            && !outboundText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func roomAllowsComposition(_ roomID: String) -> Bool {
        state?.rooms.first(where: { $0.roomId == roomID })?.state == .connected
    }

    private static func messageAllowsRetry(_ message: ChatMessage) -> Bool {
        guard message.isMine,
              let serverDelivery = message.outboundDelivery?.serverDelivery,
              case .failed = serverDelivery
        else {
            return false
        }
        return true
    }

    func start() {
        appendDiagnostic(category: "runtime", event: "start.requested")
        do {
            let runtime = try currentRuntime()
            state = try runtime.state()
            do {
                state = try runtime.dispatch(action: .startRuntime)
                appendDiagnostic(category: "runtime", event: "start.succeeded")
                errorText = nil
            } catch {
                appendDiagnostic(
                    category: "runtime",
                    event: "start.failed",
                    details: diagnosticErrorDetails(error)
                )
                errorText = String(describing: error)
            }
        } catch {
            appendDiagnostic(
                category: "runtime",
                event: "open.failed",
                details: diagnosticErrorDetails(error)
            )
            errorText = String(describing: error)
        }
        restartUpdateLoopIfEnabled()
        if runtime != nil {
            flushPendingPushTokenIfPossible()
            runLaunchAutomationIfRequested()
        }
    }

    func registerPushToken(_ token: String) {
        let token = token.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty else { return }
        pendingPushToken = token
        appendDiagnostic(
            category: "push",
            event: "token.received",
            details: ["token_bytes": "\(token.count / 2)"]
        )
        flushPendingPushTokenIfPossible()
    }

    func notePushRegistrationFailed(_ error: Error) {
        appendDiagnostic(
            category: "push",
            event: "apns.registration.failed",
            details: diagnosticErrorDetails(error)
        )
    }

    func handleRemotePushWake(
        userInfo: [AnyHashable: Any],
        completion: @escaping (UIBackgroundFetchResult) -> Void
    ) {
        appendDiagnostic(
            category: "push",
            event: "wake.received",
            details: pushWakeDiagnosticDetails(userInfo)
        )
        Task { [weak self] in
            guard let self else {
                completion(.noData)
                return
            }
            do {
                let runtime = try currentRuntime()
                let runtimeKey = openKey
                let nextState = try await Task.detached(priority: .utility) {
                    try runtime.dispatch(action: .startRuntime)
                }.value
                guard openKey == runtimeKey else {
                    completion(.noData)
                    return
                }
                state = nextState
                errorText = nil
                appendDiagnostic(category: "push", event: "wake.sync.succeeded")
                restartUpdateLoopIfEnabled()
                completion(.newData)
            } catch {
                appendDiagnostic(
                    category: "push",
                    event: "wake.sync.failed",
                    details: diagnosticErrorDetails(error)
                )
                completion(.failed)
            }
        }
    }

    func openRoom(_ room: AppRoomSummary) {
        dispatch(.openRoom(roomId: room.roomId))
    }

    func projection(for roomID: String) -> ChatRoomProjection {
        chatProjections[roomID] ?? .empty(roomID: roomID)
    }

    func createRoom() {
        let name = roomDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        roomDraft = ""
        dispatch(.createRoom(displayName: name))
    }

    func createInvite(for room: AppRoomSummary) -> Bool {
        guard room.state == .connected else { return false }
        dispatch(.createInvite(roomId: room.roomId))
        return state?.activeInvite?.roomId == room.roomId
    }

    func startProfileChat(for profile: AppProfileSummary) -> Bool {
        let existingRoomIDs = Set(rooms.map(\.roomId))
        let displayName = profile.displayName.nonEmptyTrimmed ?? profile.npub
        dispatch(.startProfileChat(
            accountId: profile.accountId,
            displayName: "Chat with \(displayName)"
        ))
        if let room = rooms.first(where: { !existingRoomIDs.contains($0.roomId) }) {
            return room.state == .connected
        }
        let status = state?.status.nonEmptyTrimmed
        if let room = selectedRoom,
           room.state == .connected,
           status == "chat opened" || status == "chat created"
        {
            return true
        }
        if userNoticeText == nil {
            errorText = "Chat could not be created."
        }
        return false
    }

    func startGroupChat(named rawName: String, with profiles: [AppProfileSummary]) -> Bool {
        let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
        let accountIDs = profiles
            .map(\.accountId)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        guard !name.isEmpty, !accountIDs.isEmpty else { return false }
        let existingRoomIDs = Set(rooms.map(\.roomId))
        dispatch(.startGroupChat(accountIds: accountIDs, displayName: name))
        guard let room = rooms.first(where: { !existingRoomIDs.contains($0.roomId) }) else {
            if userNoticeText == nil {
                errorText = "Group chat could not be created."
            }
            return false
        }
        return room.state == .connected
    }

    func startNewChat(named rawName: String, with profiles: [AppProfileSummary]) -> Bool {
        let candidates = profiles.filter {
            !$0.accountId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
        guard !candidates.isEmpty else { return false }
        if candidates.count == 1, let profile = candidates.first {
            return startProfileChat(for: profile)
        }
        return startGroupChat(named: rawName, with: candidates)
    }

    func addMembers(to room: AppRoomSummary, profiles: [AppProfileSummary]) -> Bool {
        guard room.state == .connected else { return false }
        let accountIDs = profiles
            .map(\.accountId)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        guard !accountIDs.isEmpty else { return false }
        dispatch(.addRoomMembers(roomId: room.roomId, accountIds: accountIDs))
        if state?.status == "people added" {
            return true
        }
        if userNoticeText == nil {
            errorText = "People could not be added to this chat."
        }
        return false
    }

    @discardableResult
    func scanTarget() -> Bool {
        if case .profile = scanTargetResult() {
            return false
        }
        return true
    }

    @discardableResult
    func scanTargetResult() -> AppScanTargetResult {
        let value = scanDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return .empty }

        let previousRoomIDs = Set(rooms.map(\.roomId))
        let previousSelectedRoomID = state?.selectedRoomId
        let dispatched = dispatch(.scanTarget(value: value))
        guard dispatched else { return .unavailable }

        if let profile = activeProfile {
            scanDraft = ""
            return .profile(profile)
        }

        if let room = selectedRoom,
           state?.status == "invite scanned"
            || room.roomId != previousSelectedRoomID
            || !previousRoomIDs.contains(room.roomId)
        {
            scanDraft = ""
            return .room(room)
        }

        return .unavailable
    }

    @discardableResult
    func submitPin(for room: AppRoomSummary) -> Bool {
        guard room.state == .waitingForApproval else { return false }
        let pin = pinDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !pin.isEmpty else { return false }
        pinDraft = ""
        dispatch(.submitInvitePin(pendingRoomId: room.roomId, pin: pin))
        return true
    }

    @discardableResult
    func retry(_ message: ChatMessage) -> Bool {
        guard Self.messageAllowsRetry(message) else { return false }
        let key = "\(message.roomId)|\(message.messageId)"
        guard !messageRetriesInFlight.contains(key) else { return false }
        messageRetriesInFlight.insert(key)

        Task { [weak self] in
            guard let self else { return }
            defer {
                messageRetriesInFlight.remove(key)
            }
            do {
                let runtime = try currentRuntime()
                let runtimeKey = openKey
                let action = AppAction.retryMessage(
                    roomId: message.roomId,
                    messageId: message.messageId
                )
                let nextState = try await Task.detached(priority: .userInitiated) {
                    try runtime.dispatch(action: action)
                }.value
                guard openKey == runtimeKey else { return }
                state = nextState
                errorText = nil
                restartUpdateLoopIfEnabled()
            } catch {
                errorText = String(describing: error)
            }
        }
        return true
    }

    func refreshDevices() {
        dispatch(.refreshDevices)
    }

    func revokeDevice(_ device: AppDeviceSummary) {
        guard !device.currentDevice, !device.revoked else { return }
        dispatch(.revokeDevice(accountId: device.accountId, deviceId: device.deviceId))
    }

    @discardableResult
    func send(roomID: String, replyTo message: ChatMessage? = nil) -> Bool {
        guard roomAllowsComposition(roomID) else { return false }
        let text = outboundText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return false }
        let action: AppAction
        if let message {
            action = .sendReply(
                roomId: roomID,
                text: text,
                replyToMessageId: message.messageId
            )
        } else {
            action = .sendMessage(roomId: roomID, text: text)
        }
        let sent = dispatch(action)
        if sent {
            outboundText = ""
            schedulePostSendCatchUp()
        }
        return sent
    }

    @discardableResult
    func send(replyTo message: ChatMessage? = nil) -> Bool {
        guard let roomID = selectedRoom?.roomId else { return false }
        return send(roomID: roomID, replyTo: message)
    }

    @discardableResult
    func sendAttachment(
        roomID: String,
        fileURL: URL,
        replyTo message: ChatMessage? = nil,
        onSuccess: (@MainActor () -> Void)? = nil
    ) -> Bool {
        guard roomAllowsComposition(roomID) else { return false }
        let caption = outboundText.trimmingCharacters(in: .whitespacesAndNewlines)
        outboundText = ""
        Task { [weak self] in
            guard let self else { return }
            do {
                let attachment = try await Task.detached(priority: .userInitiated) {
                    try Self.loadAttachment(from: fileURL)
                }.value
                let runtime = try currentRuntime()
                let runtimeKey = openKey
                let action = AppAction.sendAttachment(
                    roomId: roomID,
                    filename: attachment.filename,
                    mimeType: attachment.mimeType,
                    kind: attachment.kind,
                    bytes: attachment.data,
                    caption: caption,
                    replyToMessageId: message?.messageId
                )
                let nextState = try await Task.detached(priority: .userInitiated) {
                    try runtime.dispatch(action: action)
                }.value
                guard openKey == runtimeKey else { return }
                state = nextState
                errorText = nil
                onSuccess?()
                restartUpdateLoopIfEnabled()
                schedulePostSendCatchUp()
            } catch {
                errorText = String(describing: error)
            }
        }
        return true
    }

    @discardableResult
    func sendAttachments(
        roomID: String,
        attachments: [OutboundAttachment],
        replyTo message: ChatMessage? = nil,
        captionOverride: String? = nil,
        onSuccess: (@MainActor () -> Void)? = nil
    ) -> Bool {
        guard roomAllowsComposition(roomID) else { return false }
        guard !attachments.isEmpty else { return false }
        let caption = (captionOverride ?? outboundText)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        Task { [weak self] in
            guard let self else { return }
            do {
                let runtime = try currentRuntime()
                let runtimeKey = openKey
                let action = AppAction.sendAttachments(
                    roomId: roomID,
                    attachments: attachments,
                    caption: caption,
                    replyToMessageId: message?.messageId
                )
                let nextState = try await Task.detached(priority: .userInitiated) {
                    try runtime.dispatch(action: action)
                }.value
                guard openKey == runtimeKey else { return }
                state = nextState
                if captionOverride == nil {
                    outboundText = ""
                }
                errorText = nil
                onSuccess?()
                restartUpdateLoopIfEnabled()
                schedulePostSendCatchUp()
            } catch {
                errorText = String(describing: error)
            }
        }
        return true
    }

    @discardableResult
    func sendPoll(roomID: String, question: String, options: [String]) -> Bool {
        guard roomAllowsComposition(roomID) else { return false }
        let trimmedQuestion = question.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedOptions = options
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        guard !trimmedQuestion.isEmpty, trimmedOptions.count >= 2 else { return false }
        return dispatch(.sendPoll(
            roomId: roomID,
            question: trimmedQuestion,
            options: trimmedOptions
        ))
    }

    func votePoll(message: ChatMessage, option: ChatPollOption) {
        dispatch(.votePoll(
            roomId: message.roomId,
            messageId: message.messageId,
            optionId: option.optionId
        ))
    }

    func downloadAttachment(roomID: String, message: ChatMessage, attachment: ChatMediaAttachment) {
        downloadAttachment(roomID: roomID, messageID: message.messageId, attachment: attachment)
    }

    func downloadAttachment(roomID: String, messageID: String, attachment: ChatMediaAttachment) {
        guard attachmentCanDownload(attachment) else { return }

        let key = "\(roomID)|\(messageID)|\(attachment.attachmentId)"
        guard !attachmentDownloadsInFlight.contains(key) else { return }
        attachmentDownloadsInFlight.insert(key)

        let runtime: any FiniteChatRuntimeProtocol
        let runtimeKey: String
        do {
            runtime = try currentRuntime()
            runtimeKey = openKey
            state = try runtime.dispatch(action: .beginDownloadAttachment(
                roomId: roomID,
                messageId: messageID,
                attachmentId: attachment.attachmentId
            ))
            errorText = nil
            restartUpdateLoopIfEnabled()
        } catch {
            attachmentDownloadsInFlight.remove(key)
            errorText = String(describing: error)
            return
        }

        Task { [weak self] in
            guard let self else { return }
            defer {
                attachmentDownloadsInFlight.remove(key)
            }
            do {
                let action = AppAction.downloadAttachment(
                    roomId: roomID,
                    messageId: messageID,
                    attachmentId: attachment.attachmentId
                )
                let nextState = try await Task.detached(priority: .utility) {
                    try runtime.dispatch(action: action)
                }.value
                guard openKey == runtimeKey else { return }
                state = nextState
                errorText = nil
                restartUpdateLoopIfEnabled()
            } catch {
                errorText = String(describing: error)
            }
        }
    }

    func loadOlderMessages(roomID: String, beforeMessageID: String) {
        dispatch(.loadOlderMessages(
            roomId: roomID,
            beforeMessageId: beforeMessageID,
            limit: 50
        ))
    }

    func react(to message: ChatMessage, emoji: String) {
        dispatch(.reactToMessage(
            roomId: message.roomId,
            messageId: message.messageId,
            emoji: emoji
        ))
    }

    func markRoomRead(_ room: AppRoomSummary) {
        dispatch(.markRoomRead(roomId: room.roomId))
    }

    func setTyping(roomID: String, isTyping: Bool) {
        guard lastTypingIntentByRoom[roomID] != isTyping else { return }
        lastTypingIntentByRoom[roomID] = isTyping
        dispatch(.setTyping(roomId: roomID, isTyping: isTyping))
    }

    private func applyNostrIdentity(
        _ identity: AppNostrIdentity,
        resetStore: Bool
    ) throws {
        closeRuntime()
        if resetStore {
            try? RuntimeDataStore.deleteDataDir(
                deviceID: deviceID,
                applicationSupportURL: applicationSupportURL,
                transient: usesTransientStore
            )
        }
        nostrIdentityStore.save(identity)
        nostrIdentity = identity
        requiresNostrLogin = false
        appendDiagnostic(category: "persistence", event: "nostr_identity.applied")
        start()
    }

    private func flushPendingPushTokenIfPossible() {
        guard let token = pendingPushToken else { return }
        appendDiagnostic(category: "push", event: "token.register.requested")
        do {
            let runtime = try currentRuntime()
            state = try runtime.dispatch(action: .setPushToken(token: token))
            pendingPushToken = nil
            appendDiagnostic(category: "push", event: "token.register.succeeded")
            restartUpdateLoopIfEnabled()
        } catch {
            appendDiagnostic(
                category: "push",
                event: "token.register.failed",
                details: diagnosticErrorDetails(error)
            )
        }
    }

    private func removePushTokenIfPossible() {
        guard runtime != nil else { return }
        appendDiagnostic(category: "push", event: "token.remove.requested")
        do {
            let runtime = try currentRuntime()
            state = try runtime.dispatch(action: .removePushToken)
            appendDiagnostic(category: "push", event: "token.remove.succeeded")
            restartUpdateLoopIfEnabled()
        } catch {
            appendDiagnostic(
                category: "push",
                event: "token.remove.failed",
                details: diagnosticErrorDetails(error)
            )
        }
    }

    private func startUpdateLoop() {
        updateTask?.cancel()
        guard let runtime else { return }
        let runtimeKey = openKey
        updateTask = Task { [weak self, runtime, runtimeKey] in
            while !Task.isCancelled {
                do {
                    try? await Task.sleep(nanoseconds: 2_000_000_000)
                    guard !Task.isCancelled else { return }
                    let nextState = try await Task.detached(priority: .background) {
                        try runtime.dispatch(action: .startRuntime)
                    }.value
                    guard !Task.isCancelled else { return }
                    guard let self, self.openKey == runtimeKey else { return }
                    self.state = nextState
                    self.appendDiagnostic(category: "runtime", event: "update.polled")
                    self.errorText = nil
                } catch {
                    guard !Task.isCancelled else { return }
                    guard let self, self.openKey == runtimeKey else { return }
                    self.appendDiagnostic(
                        category: "runtime",
                        event: "update.failed",
                        details: self.diagnosticErrorDetails(error)
                    )
                    self.errorText = String(describing: error)
                    try? await Task.sleep(nanoseconds: 1_000_000_000)
                }
            }
        }
    }

    @discardableResult
    private func dispatch(_ action: AppAction) -> Bool {
        var succeeded = false
        let diagnostic = diagnosticAction(action)
        appendDiagnostic(
            category: diagnostic.category,
            event: "\(diagnostic.name).requested",
            details: diagnostic.details
        )
        run {
            let runtime = try currentRuntime()
            self.state = try runtime.dispatch(action: action)
            succeeded = true
        }
        if succeeded {
            appendDiagnostic(
                category: diagnostic.category,
                event: "\(diagnostic.name).succeeded",
                details: diagnostic.details
            )
        } else {
            appendDiagnostic(
                category: diagnostic.category,
                event: "\(diagnostic.name).failed",
                details: diagnostic.details.merging(diagnosticErrorDetails(errorText)) { current, _ in
                    current
                }
            )
        }
        restartUpdateLoopIfEnabled()
        return succeeded
    }

    private func restartUpdateLoopIfEnabled() {
        if startsUpdateLoop {
            startUpdateLoop()
        }
    }

    private func schedulePostSendCatchUp() {
        postSendCatchUpTask?.cancel()
        guard runtime != nil else { return }
        let runtimeKey = openKey
        postSendCatchUpTask = Task { [weak self, runtimeKey] in
            for delay in [1_000_000_000, 3_000_000_000, 6_000_000_000, 12_000_000_000] as [UInt64] {
                try? await Task.sleep(nanoseconds: delay)
                guard !Task.isCancelled else { return }
                guard let self, self.openKey == runtimeKey else { return }
                do {
                    let runtime = try currentRuntime()
                    let nextState = try await Task.detached(priority: .utility) {
                        try runtime.dispatch(action: .startRuntime)
                    }.value
                    guard !Task.isCancelled, openKey == runtimeKey else { return }
                    state = nextState
                    errorText = nil
                    appendDiagnostic(category: "runtime", event: "post_send_catchup.succeeded")
                    restartUpdateLoopIfEnabled()
                } catch {
                    appendDiagnostic(
                        category: "runtime",
                        event: "post_send_catchup.failed",
                        details: diagnosticErrorDetails(error)
                    )
                }
            }
        }
    }

    private func currentRuntime() throws -> any FiniteChatRuntimeProtocol {
        if let launchConfigurationError {
            throw AppLaunchConfigurationError(message: launchConfigurationError)
        }
        if requiresNostrLogin {
            throw AppLaunchConfigurationError(message: "Create or sign in to a Nostr account first.")
        }
        let accountSecretHex = nostrIdentity?.accountSecretHex
        let key = "\(serverURL)|\(deviceID)|\(accountSecretHex ?? "stored")"
        if let runtime, openKey == key {
            return runtime
        }
        let dataDir = try RuntimeDataStore.dataDir(
            deviceID: deviceID,
            applicationSupportURL: applicationSupportURL,
            transient: usesTransientStore
        )
        runtimeStorePath = dataDir
        appendDiagnostic(
            category: "persistence",
            event: "store.resolved",
            details: [
                "store": redactedPathSummary(dataDir),
                "mode": usesTransientStore ? "transient" : "stable",
            ]
        )
        let opened = try runtimeFactory(
            OpenOptions(
                dataDir: dataDir,
                serverUrl: serverURL,
                deviceId: deviceID,
                accountSecretHex: accountSecretHex,
                nowUnixSeconds: nil
            )
        )
        let openedState = try opened.state()
        syncNostrIdentityFromRuntime(openedState.identity)
        let resolvedDeviceID = openedState.identity.deviceId
        if resolvedDeviceID != deviceID {
            appendDiagnostic(
                category: "runtime",
                event: "identity.resolved",
                details: ["device_changed": "true"]
            )
            deviceID = resolvedDeviceID
        }
        if !usesTransientStore && persistsRuntimeIdentityUpdates {
            try? RuntimeConfig(serverURL: serverURL, deviceID: resolvedDeviceID).save(
                storageURL: configStorageURL
            )
        }
        runtime = opened
        let resolvedAccountSecretHex = nostrIdentity?.accountSecretHex ?? accountSecretHex
        openKey = "\(serverURL)|\(deviceID)|\(resolvedAccountSecretHex ?? "stored")"
        appendDiagnostic(category: "runtime", event: "open.succeeded")
        return opened
    }

    private func syncNostrIdentityFromRuntime(_ identity: Identity) {
        guard nostrIdentity == nil else { return }
        guard let material = try? nostrIdentityFromAccountSecretHex(
            accountSecretHex: identity.accountSecretHex
        ) else {
            return
        }
        let appIdentity = AppNostrIdentity(material: material)
        nostrIdentityStore.save(appIdentity)
        nostrIdentity = appIdentity
    }

    private func closeRuntime() {
        updateTask?.cancel()
        launchAutomationTask?.cancel()
        postSendCatchUpTask?.cancel()
        updateTask = nil
        launchAutomationTask = nil
        postSendCatchUpTask = nil
        attachmentDownloadsInFlight.removeAll()
        messageRetriesInFlight.removeAll()
        lastTypingIntentByRoom.removeAll()
        runtime = nil
        openKey = ""
        state = nil
        runtimeStorePath = nil
    }

    private func rebuildChatProjections() {
        guard let state else {
            chatProjections = [:]
            return
        }
        chatProjections = ChatTimeline.roomProjections(messages: state.messages)
    }

    private func run(_ operation: () throws -> Void) {
        do {
            try operation()
            errorText = nil
        } catch {
            appendDiagnostic(
                category: "runtime",
                event: "operation.failed",
                details: diagnosticErrorDetails(error)
            )
            errorText = String(describing: error)
        }
    }

    private func appendStateDiagnostic(_ state: AppState, event: String) {
        let outboundMessages = state.messages.compactMap(\.outboundDelivery)
        var undelivered = 0
        var delivered = 0
        var failed = 0
        for delivery in outboundMessages {
            switch delivery.serverDelivery {
            case .undelivered:
                undelivered += 1
            case .delivered:
                delivered += 1
            case .failed:
                failed += 1
            }
        }
        let roomStates = Dictionary(grouping: state.rooms, by: \.state)
            .mapValues(\.count)
        appendDiagnostic(
            category: "runtime",
            event: event,
            details: [
                "rev": "\(state.rev)",
                "status": Self.redactedDiagnosticValue(state.status),
                "rooms": "\(state.rooms.count)",
                "connected_rooms": "\(roomStates[.connected] ?? 0)",
                "unavailable_rooms": "\(roomStates[.unavailableOnDevice] ?? 0)",
                "selected_room": state.selectedRoomId.map(Self.redactedDiagnosticValue) ?? "none",
                "messages": "\(state.messages.count)",
                "outbound": "\(outboundMessages.count)",
                "undelivered": "\(undelivered)",
                "delivered": "\(delivered)",
                "failed": "\(failed)",
                "profiles": "\(state.profiles.count)",
                "devices": "\(state.devices.count)",
            ]
        )
    }

    private func appendDiagnostic(
        category: String,
        event: String,
        details: [String: String] = [:]
    ) {
        let sanitizedDetails = details.reduce(into: [String: String]()) { output, item in
            output[item.key] = Self.redactedDiagnosticValue(item.value)
        }
        developerDiagnostics.append(DeveloperDiagnosticEntry(
            id: (developerDiagnostics.last?.id ?? 0) + 1,
            timestampUnixSeconds: Int64(Date().timeIntervalSince1970),
            category: Self.redactedDiagnosticValue(category),
            event: Self.redactedDiagnosticValue(event),
            details: sanitizedDetails
        ))
#if DEBUG
        if let entry = developerDiagnostics.last {
            persistDebugDiagnostic(entry)
        }
#endif
        if developerDiagnostics.count > Self.developerDiagnosticsLimit {
            developerDiagnostics.removeFirst(
                developerDiagnostics.count - Self.developerDiagnosticsLimit
            )
        }
    }

#if DEBUG
    private func persistDebugDiagnostic(_ entry: DeveloperDiagnosticEntry) {
        let supportURL: URL
        if let applicationSupportURL {
            supportURL = applicationSupportURL
        } else if let defaultURL = try? FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        ) {
            supportURL = defaultURL
        } else {
            return
        }
        let details = entry.details
            .sorted { $0.key < $1.key }
            .map { "\($0.key)=\($0.value)" }
            .joined(separator: " ")
        let line: String
        if details.isEmpty {
            line = "seq=\(entry.id) ts=\(entry.timestampUnixSeconds) category=\(entry.category) event=\(entry.event)\n"
        } else {
            line = "seq=\(entry.id) ts=\(entry.timestampUnixSeconds) category=\(entry.category) event=\(entry.event) \(details)\n"
        }
        let url = supportURL.appendingPathComponent("finitechat_debug_diagnostics.log")
        guard let data = line.data(using: .utf8) else { return }
        if FileManager.default.fileExists(atPath: url.path),
           let handle = try? FileHandle(forWritingTo: url)
        {
            _ = try? handle.seekToEnd()
            try? handle.write(contentsOf: data)
            try? handle.close()
        } else {
            try? data.write(to: url, options: .atomic)
        }
    }
#endif

    private func diagnosticErrorDetails(_ error: Error) -> [String: String] {
        diagnosticErrorDetails(String(describing: error))
    }

    private func diagnosticErrorDetails(_ errorText: String?) -> [String: String] {
        guard let errorText = errorText?.trimmingCharacters(in: .whitespacesAndNewlines),
              !errorText.isEmpty
        else {
            return [:]
        }
        return ["error": Self.redactedDiagnosticValue(errorText)]
    }

    private func diagnosticAction(_ action: AppAction) -> DiagnosticActionSummary {
        switch action {
        case .startRuntime:
            return DiagnosticActionSummary(category: "runtime", name: "start_runtime", details: [:])
        case .stopRuntime:
            return DiagnosticActionSummary(category: "runtime", name: "stop_runtime", details: [:])
        case .openRoom(let roomId):
            return DiagnosticActionSummary(
                category: "runtime",
                name: "open_room",
                details: ["room": roomId]
            )
        case .createRoom:
            return DiagnosticActionSummary(
                category: "transport",
                name: "create_room",
                details: [:]
            )
        case .startProfileChat(let accountId, _):
            return DiagnosticActionSummary(
                category: "transport",
                name: "start_profile_chat",
                details: ["account": accountId]
            )
        case .startGroupChat(let accountIds, _):
            return DiagnosticActionSummary(
                category: "transport",
                name: "start_group_chat",
                details: ["members": "\(accountIds.count)"]
            )
        case .addRoomMembers(let roomId, let accountIds):
            return DiagnosticActionSummary(
                category: "transport",
                name: "add_room_members",
                details: ["room": roomId, "members": "\(accountIds.count)"]
            )
        case .createInvite(let roomId):
            return DiagnosticActionSummary(
                category: "transport",
                name: "create_invite",
                details: ["room": roomId]
            )
        case .scanTarget:
            return DiagnosticActionSummary(
                category: "transport",
                name: "scan_target",
                details: [:]
            )
        case .submitInvitePin(let pendingRoomId, _):
            return DiagnosticActionSummary(
                category: "transport",
                name: "submit_invite_pin",
                details: ["room": pendingRoomId]
            )
        case .sendMessage(let roomId, _):
            return DiagnosticActionSummary(
                category: "transport",
                name: "send_message",
                details: ["room": roomId]
            )
        case .sendReply(let roomId, _, let replyToMessageId):
            return DiagnosticActionSummary(
                category: "transport",
                name: "send_reply",
                details: ["room": roomId, "reply_to": replyToMessageId]
            )
        case .sendAttachment(let roomId, _, _, _, _, let caption, let replyToMessageId):
            var details = [
                "room": roomId,
                "attachment_count": "1",
                "has_caption": caption.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    ? "false" : "true",
            ]
            if let replyToMessageId {
                details["reply_to"] = replyToMessageId
            }
            return DiagnosticActionSummary(
                category: "transport",
                name: "send_attachment",
                details: details
            )
        case .sendAttachments(let roomId, let attachments, let caption, let replyToMessageId):
            var details = [
                "room": roomId,
                "attachment_count": "\(attachments.count)",
                "has_caption": caption.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    ? "false" : "true",
            ]
            if let replyToMessageId {
                details["reply_to"] = replyToMessageId
            }
            return DiagnosticActionSummary(
                category: "transport",
                name: "send_attachments",
                details: details
            )
        case .sendPoll(let roomId, _, let options):
            return DiagnosticActionSummary(
                category: "transport",
                name: "send_poll",
                details: ["room": roomId, "option_count": "\(options.count)"]
            )
        case .votePoll(let roomId, let messageId, let optionId):
            return DiagnosticActionSummary(
                category: "transport",
                name: "vote_poll",
                details: ["room": roomId, "message": messageId, "option": optionId]
            )
        case .downloadAttachment(let roomId, let messageId, let attachmentId):
            return DiagnosticActionSummary(
                category: "transport",
                name: "download_attachment",
                details: ["room": roomId, "message": messageId, "attachment": attachmentId]
            )
        case .beginDownloadAttachment(let roomId, let messageId, let attachmentId):
            return DiagnosticActionSummary(
                category: "transport",
                name: "begin_download_attachment",
                details: ["room": roomId, "message": messageId, "attachment": attachmentId]
            )
        case .loadOlderMessages(let roomId, let beforeMessageId, let limit):
            return DiagnosticActionSummary(
                category: "runtime",
                name: "load_older_messages",
                details: [
                    "room": roomId,
                    "before": beforeMessageId,
                    "limit": "\(limit)",
                ]
            )
        case .reactToMessage(let roomId, let messageId, _):
            return DiagnosticActionSummary(
                category: "transport",
                name: "react_to_message",
                details: ["room": roomId, "message": messageId]
            )
        case .markRoomRead(let roomId):
            return DiagnosticActionSummary(
                category: "runtime",
                name: "mark_room_read",
                details: ["room": roomId]
            )
        case .retryMessage(let roomId, let messageId):
            return DiagnosticActionSummary(
                category: "transport",
                name: "retry_message",
                details: ["room": roomId, "message": messageId]
            )
        case .setTyping(let roomId, let isTyping):
            return DiagnosticActionSummary(
                category: "transport",
                name: "set_typing",
                details: ["room": roomId, "typing": isTyping ? "true" : "false"]
            )
        case .refreshDevices:
            return DiagnosticActionSummary(
                category: "transport",
                name: "refresh_devices",
                details: [:]
            )
        case .revokeDevice(let accountId, let deviceId):
            return DiagnosticActionSummary(
                category: "transport",
                name: "revoke_device",
                details: ["account": accountId, "device": deviceId]
            )
        case .setPushToken:
            return DiagnosticActionSummary(
                category: "push",
                name: "set_push_token",
                details: [:]
            )
        case .removePushToken:
            return DiagnosticActionSummary(
                category: "push",
                name: "remove_push_token",
                details: [:]
            )
        }
    }

    private func pushWakeDiagnosticDetails(_ userInfo: [AnyHashable: Any]) -> [String: String] {
        var details = [String: String]()
        if let roomID = userInfo["room_id"] as? String {
            details["room"] = roomID
        }
        if let seq = userInfo["seq"] {
            details["seq"] = "\(seq)"
        }
        return details
    }

    private func redactedPathSummary(_ path: String) -> String {
        let components = URL(fileURLWithPath: path).standardizedFileURL.pathComponents
        let suffix = components.suffix(2).joined(separator: "/")
        return suffix.isEmpty ? "[path]" : "[path:\(suffix)]"
    }

    private static func redactedDiagnosticValue(_ value: String) -> String {
        var output = value.trimmingCharacters(in: .whitespacesAndNewlines)
        output = replacingMatches(
            in: output,
            pattern: #"https?://[^\s\)"]+"#,
            replacement: "[url]"
        )
        output = replacingMatches(
            in: output,
            pattern: #"file://[^\s\)"]+"#,
            replacement: "[url]"
        )
        output = replacingMatches(
            in: output,
            pattern: #"/(?:Users|private|var|tmp|Volumes)/[^\s]+"#,
            replacement: "[path]"
        )
        output = replacingMatches(
            in: output,
            pattern: #"\b[0-9a-fA-F]{32,}\b"#,
            replacement: "[hex]"
        )
        if output.count > 240 {
            output = String(output.prefix(237)) + "..."
        }
        return output
    }

    private static func replacingMatches(
        in value: String,
        pattern: String,
        replacement: String
    ) -> String {
        guard let regex = try? NSRegularExpression(pattern: pattern) else {
            return value
        }
        let range = NSRange(value.startIndex..<value.endIndex, in: value)
        return regex.stringByReplacingMatches(
            in: value,
            options: [],
            range: range,
            withTemplate: replacement
        )
    }

    private func runLaunchAutomationIfRequested() {
        guard !didRunLaunchAutomation else { return }
        let inviteURL = Self.argumentValue("--finitechat-auto-join", in: args)
        let createRoomName = Self.argumentValue("--finitechat-auto-create-room", in: args)
        let outbound = Self.argumentValue("--finitechat-auto-send", in: args)
        let attachmentText = Self.argumentValue(
            "--finitechat-auto-send-attachment-text",
            in: args
        )
        let attachmentFile = Self.argumentValue(
            "--finitechat-auto-send-attachment-file",
            in: args
        )
        let attachmentBase64 = Self.argumentValue(
            "--finitechat-auto-send-attachment-base64",
            in: args
        )
        let attachmentFilename = Self.argumentValue(
            "--finitechat-auto-send-attachment-filename",
            in: args
        ) ?? "launch-automation.bin"
        let attachmentMimeType = Self.argumentValue(
            "--finitechat-auto-send-attachment-mime-type",
            in: args
        ) ?? "application/octet-stream"
        let attachmentCaption = Self.argumentValue(
            "--finitechat-auto-send-attachment-caption",
            in: args
        )
        guard inviteURL != nil
            || createRoomName != nil
            || outbound != nil
            || attachmentText != nil
            || attachmentFile != nil
            || attachmentBase64 != nil
        else {
            return
        }

        didRunLaunchAutomation = true
        pinDraft = Self.argumentValue("--finitechat-pin", in: args) ?? pinDraft
        deviceID = Self.argumentValue("--finitechat-device", in: args) ?? deviceID
        serverURL = Self.argumentValue("--finitechat-server", in: args) ?? serverURL
        let requestedRoomID = Self.argumentValue("--finitechat-room", in: args)

        launchAutomationTask = Task {
            if let createRoomName,
               !createRoomName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            {
                self.roomDraft = createRoomName
                self.createRoom()
            }
            if let inviteURL {
                self.scanDraft = inviteURL
                self.scanTarget()
                let roomID = requestedRoomID ?? self.state?.selectedRoomId
                if let room = self.launchAutomationRoom(roomID: roomID) {
                    self.submitPin(for: room)
                }
            }
            let roomID = requestedRoomID ?? self.state?.selectedRoomId
            if let outbound, !outbound.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                await self.sendLaunchAutomationMessage(roomID: roomID, text: outbound)
            }
            if let attachmentText,
               !attachmentText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            {
                await self.sendLaunchAutomationAttachment(roomID: roomID, text: attachmentText)
            }
            if let attachmentFile,
               !attachmentFile.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            {
                await self.sendLaunchAutomationAttachmentFile(
                    roomID: roomID,
                    path: attachmentFile,
                    caption: attachmentCaption ?? ""
                )
            }
            if let attachmentBase64,
               !attachmentBase64.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            {
                await self.sendLaunchAutomationAttachmentBase64(
                    roomID: roomID,
                    base64: attachmentBase64,
                    filename: attachmentFilename,
                    mimeType: attachmentMimeType,
                    caption: attachmentCaption ?? ""
                )
            }
        }
    }

    private func launchAutomationRoom(roomID: String?) -> AppRoomSummary? {
        guard let state else { return nil }
        if let roomID {
            return state.rooms.first { $0.roomId == roomID }
        }
        return selectedRoom
    }

    private func sendLaunchAutomationMessage(roomID: String?, text: String) async {
        let deadline = Date().addingTimeInterval(90)
        while !Task.isCancelled, Date() < deadline {
            if let room = launchAutomationRoom(roomID: roomID), room.state == .connected {
                dispatch(.openRoom(roomId: room.roomId))
                outboundText = text
                send()
                return
            }
            try? await Task.sleep(nanoseconds: 500_000_000)
        }
        outboundText = text
        errorText = "Launch automation timed out waiting for the room to connect"
    }

    private func sendLaunchAutomationAttachmentBase64(
        roomID: String?,
        base64: String,
        filename: String,
        mimeType: String,
        caption: String
    ) async {
        let deadline = Date().addingTimeInterval(90)
        while !Task.isCancelled, Date() < deadline {
            if let room = launchAutomationRoom(roomID: roomID), room.state == .connected {
                dispatch(.openRoom(roomId: room.roomId))
                let normalized = base64.trimmingCharacters(in: .whitespacesAndNewlines)
                guard let data = Data(base64Encoded: normalized) else {
                    errorText = "Launch automation attachment base64 was invalid"
                    return
                }
                let cleanedFilename = filename.trimmingCharacters(in: .whitespacesAndNewlines)
                let finalFilename = cleanedFilename.isEmpty ? "launch-automation.bin" : cleanedFilename
                let cleanedMimeType = mimeType.trimmingCharacters(in: .whitespacesAndNewlines)
                let finalMimeType = cleanedMimeType.isEmpty
                    ? "application/octet-stream"
                    : cleanedMimeType
                let type = UTType(filenameExtension: URL(fileURLWithPath: finalFilename).pathExtension)
                let attachment = OutboundAttachment(
                    filename: finalFilename,
                    mimeType: finalMimeType,
                    kind: Self.chatMediaKind(for: type),
                    bytes: data
                )
                sendAttachments(
                    roomID: room.roomId,
                    attachments: [attachment],
                    captionOverride: caption
                )
                return
            }
            try? await Task.sleep(nanoseconds: 500_000_000)
        }
        errorText = "Launch automation timed out waiting for the room to connect"
    }

    private func sendLaunchAutomationAttachmentFile(
        roomID: String?,
        path: String,
        caption: String
    ) async {
        let fileURL = URL(fileURLWithPath: path).standardizedFileURL
        let deadline = Date().addingTimeInterval(90)
        while !Task.isCancelled, Date() < deadline {
            if let room = launchAutomationRoom(roomID: roomID), room.state == .connected {
                dispatch(.openRoom(roomId: room.roomId))
                do {
                    let prepared = try await Task.detached(priority: .userInitiated) {
                        try Self.loadAttachment(from: fileURL)
                    }.value
                    let attachment = OutboundAttachment(
                        filename: prepared.filename,
                        mimeType: prepared.mimeType,
                        kind: prepared.kind,
                        bytes: prepared.data
                    )
                    sendAttachments(
                        roomID: room.roomId,
                        attachments: [attachment],
                        captionOverride: caption
                    )
                } catch {
                    errorText = String(describing: error)
                }
                return
            }
            try? await Task.sleep(nanoseconds: 500_000_000)
        }
        errorText = "Launch automation timed out waiting for the room to connect"
    }

    private func sendLaunchAutomationAttachment(roomID: String?, text: String) async {
        let deadline = Date().addingTimeInterval(90)
        while !Task.isCancelled, Date() < deadline {
            if let room = launchAutomationRoom(roomID: roomID), room.state == .connected {
                dispatch(.openRoom(roomId: room.roomId))
                let attachment = OutboundAttachment(
                    filename: "launch-automation.txt",
                    mimeType: "text/plain",
                    kind: .file,
                    bytes: Data(text.utf8)
                )
                sendAttachments(
                    roomID: room.roomId,
                    attachments: [attachment],
                    captionOverride: ""
                )
                return
            }
            try? await Task.sleep(nanoseconds: 500_000_000)
        }
        errorText = "Launch automation timed out waiting for the room to connect"
    }

    private static func argumentValue(_ name: String, in args: [String]) -> String? {
        guard let index = args.firstIndex(of: name) else {
            return nil
        }
        let valueIndex = args.index(after: index)
        guard valueIndex < args.endIndex else {
            return nil
        }
        return args[valueIndex]
    }

    private static func hasLaunchAutomation(args: [String]) -> Bool {
        [
            "--finitechat-auto-join",
            "--finitechat-auto-create-room",
            "--finitechat-auto-send",
            "--finitechat-auto-send-attachment-text",
            "--finitechat-auto-send-attachment-file",
            "--finitechat-auto-send-attachment-base64",
        ].contains { args.contains($0) }
    }

    private static func productHarnessApplicationSupportURL(
        args: [String]
    ) -> ProductHarnessSupportResolution {
        let argument = "--finitechat-product-harness-root"
        guard let rawValue = argumentValue(argument, in: args) else {
            return ProductHarnessSupportResolution(url: nil, error: nil)
        }
        let url = URL(fileURLWithPath: rawValue).standardizedFileURL
        guard url.path == rawValue || rawValue.hasPrefix("/") else {
            return ProductHarnessSupportResolution(
                url: nil,
                error: "\(argument) must be an absolute path"
            )
        }
        guard url.isFileURL, url.path.hasPrefix("/") else {
            return ProductHarnessSupportResolution(
                url: nil,
                error: "\(argument) must be an absolute file path"
            )
        }
        if let defaultSupport = try? FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: false
        ).standardizedFileURL,
            url == defaultSupport
        {
            return ProductHarnessSupportResolution(
                url: nil,
                error: "\(argument) must not be the default Application Support path"
            )
        }
        return ProductHarnessSupportResolution(url: url, error: nil)
    }

    nonisolated private static func loadAttachment(from url: URL) throws -> PreparedAttachment {
        let didStartAccessing = url.startAccessingSecurityScopedResource()
        defer {
            if didStartAccessing {
                url.stopAccessingSecurityScopedResource()
            }
        }

        let data = try Data(contentsOf: url)
        let filename = url.lastPathComponent.isEmpty ? "attachment" : url.lastPathComponent
        let type = UTType(filenameExtension: url.pathExtension)
        return PreparedAttachment(
            data: data,
            filename: filename,
            mimeType: type?.preferredMIMEType ?? "application/octet-stream",
            kind: chatMediaKind(for: type)
        )
    }

    nonisolated private static func chatMediaKind(for type: UTType?) -> ChatMediaKind {
        guard let type else { return .file }
        if type.conforms(to: .image) {
            return .image
        }
        if type.conforms(to: .movie) {
            return .video
        }
        if type.conforms(to: .audio) {
            return .voiceNote
        }
        return .file
    }
}

private extension String {
    var nonEmptyTrimmed: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}

private struct PreparedAttachment: Sendable {
    let data: Data
    let filename: String
    let mimeType: String
    let kind: ChatMediaKind
}
