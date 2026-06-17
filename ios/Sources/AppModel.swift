import Foundation
import SwiftUI
import UniformTypeIdentifiers

struct RuntimeConfig: Codable, Equatable {
    let serverURL: String
    let deviceID: String
    let usesTransientStore: Bool

    private static let defaultServerURL = "http://127.0.0.1:8787"
    private static let defaultDeviceID = "ios"
    private static let transientConfigArgument = "--finitechat-transient-config"
    private static let transientConfigEnvironmentKey = "FINITECHAT_TRANSIENT_CONFIG"
    private static let persistLaunchConfigArgument = "--finitechat-persist-launch-config"
    private static let persistLaunchConfigEnvironmentKey = "FINITECHAT_PERSIST_LAUNCH_CONFIG"
    private static let launchAutomationArguments = [
        "--finitechat-auto-join",
        "--finitechat-auto-create-room",
        "--finitechat-auto-send",
    ]

    enum CodingKeys: String, CodingKey {
        case serverURL = "server_url"
        case deviceID = "device_id"
    }

    init(serverURL: String, deviceID: String, usesTransientStore: Bool = false) {
        self.serverURL = serverURL
        self.deviceID = deviceID
        self.usesTransientStore = usesTransientStore
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        serverURL = try container.decode(String.self, forKey: .serverURL)
        deviceID = try container.decode(String.self, forKey: .deviceID)
        usesTransientStore = false
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
        let recoveredDeviceID = existingSingleRecoverableDeviceStoreID(storageURL: storageURL)
        let persistedDeviceIsRecoverable = persisted.deviceID
            .map { recoverableDeviceStoreExists($0, storageURL: storageURL) } ?? false
        let fallbackDeviceID: String
        if let persistedDeviceID = persisted.deviceID,
           persistedDeviceIsRecoverable || recoveredDeviceID == nil
        {
            fallbackDeviceID = persistedDeviceID
        } else {
            fallbackDeviceID = recoveredDeviceID ?? defaultDeviceID
        }
        let fallback = RuntimeConfig(
            serverURL: persisted.serverURL ?? defaultServerURL,
            deviceID: fallbackDeviceID
        )
        let hasLaunchOverride = serverURL != nil || deviceID != nil
        let hostedUnitTest = storageURL == nil && environment["XCTestConfigurationFilePath"] != nil
        let persistLaunchOverride = argumentFlag(persistLaunchConfigArgument, in: args)
            || truthyEnvironmentValue(persistLaunchConfigEnvironmentKey, in: environment)
        let hasLaunchAutomation = launchAutomationArguments.contains {
            argumentValue($0, in: args) != nil
        }
        let transientOverride = argumentFlag(transientConfigArgument, in: args)
            || truthyEnvironmentValue(transientConfigEnvironmentKey, in: environment)
            || hostedUnitTest
            || (hasLaunchAutomation && !persistLaunchOverride)
            || (hasLaunchOverride && !persistLaunchOverride)
        let config = RuntimeConfig(
            serverURL: serverURL ?? fallback.serverURL,
            deviceID: deviceID ?? fallback.deviceID,
            usesTransientStore: transientOverride
        )
        // Runtime identity is product state. Launch-provided server/device
        // values are one-shot unless explicitly persisted, so diagnostics and
        // automation cannot strand a home-screen relaunch on the wrong store.
        if !transientOverride
            && (
                persisted.serverURL != config.serverURL
                    || persisted.deviceID != config.deviceID
                    || hasLaunchOverride
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

    private static func existingSingleRecoverableDeviceStoreID(storageURL: URL?) -> String? {
        let candidates = recoverableDeviceStores(storageURL: storageURL)
        guard candidates.count == 1 else { return nil }
        return candidates[0]
    }

    private static func recoverableDeviceStoreExists(
        _ deviceID: String,
        storageURL: URL?
    ) -> Bool {
        recoverableDeviceStores(storageURL: storageURL).contains(deviceID)
    }

    private static func recoverableDeviceStores(storageURL: URL?) -> [String] {
        let supportURL: URL
        if let storageURL {
            supportURL = storageURL.deletingLastPathComponent()
        } else if let applicationSupport = try? FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: false
        ) {
            supportURL = applicationSupport
        } else {
            return []
        }

        return RuntimeDataStore.recoverableLegacyDeviceStoreIDs(applicationSupportURL: supportURL)
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
    private static let legacyDataRootDirectoryName = "FiniteChat"
    private static let currentDataDirectoryName = "FiniteChatStore"
    private static let transientDataRootDirectoryName = "FiniteChatTransient"
    private static let clientStoreFileName = "client.sqlite3"
    private static let accountSecretFileName = "account-secret.hex"

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
        if !deviceStoreIsRecoverable(currentStoreURL) {
            try migrateLegacyStoreIfNeeded(
                to: currentStoreURL,
                requestedDeviceID: deviceID,
                applicationSupportURL: supportURL
            )
        }
        try FileManager.default.createDirectory(
            at: currentStoreURL,
            withIntermediateDirectories: true
        )
        return currentStoreURL.path
    }

    static func recoverableLegacyDeviceStoreIDs(applicationSupportURL: URL) -> [String] {
        recoverableLegacyDeviceStores(applicationSupportURL: applicationSupportURL)
            .map(\.deviceID)
    }

    private static func migrateLegacyStoreIfNeeded(
        to currentStoreURL: URL,
        requestedDeviceID: String,
        applicationSupportURL: URL
    ) throws {
        let candidates = recoverableLegacyDeviceStores(applicationSupportURL: applicationSupportURL)
        guard !candidates.isEmpty else { return }
        let requestedID = safeDeviceDirectoryName(requestedDeviceID)
        let selected = candidates.first { $0.deviceID == requestedID }
            ?? candidates.sorted { lhs, rhs in
                if lhs.modifiedAt == rhs.modifiedAt {
                    return lhs.deviceID < rhs.deviceID
                }
                return lhs.modifiedAt > rhs.modifiedAt
            }.first
        guard let selected else { return }

        let fileManager = FileManager.default
        if fileManager.fileExists(atPath: currentStoreURL.path) {
            try fileManager.removeItem(at: currentStoreURL)
        }
        try fileManager.copyItem(at: selected.url, to: currentStoreURL)
    }

    private static func recoverableLegacyDeviceStores(
        applicationSupportURL: URL
    ) -> [LegacyDeviceStore] {
        let dataRoot = applicationSupportURL.appendingPathComponent(
            legacyDataRootDirectoryName,
            isDirectory: true
        )
        guard let entries = try? FileManager.default.contentsOfDirectory(
            at: dataRoot,
            includingPropertiesForKeys: [.isDirectoryKey, .contentModificationDateKey],
            options: [.skipsHiddenFiles]
        ) else {
            return []
        }

        return entries.compactMap { entry in
            let values = try? entry.resourceValues(
                forKeys: [.isDirectoryKey, .contentModificationDateKey]
            )
            guard values?.isDirectory == true else { return nil }
            guard deviceStoreIsRecoverable(entry) else { return nil }
            let deviceID = entry.lastPathComponent.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !deviceID.isEmpty else { return nil }
            return LegacyDeviceStore(
                deviceID: deviceID,
                url: entry,
                modifiedAt: latestModificationDate(for: entry)
                    ?? values?.contentModificationDate
                    ?? .distantPast
            )
        }
    }

    private static func deviceStoreIsRecoverable(_ url: URL) -> Bool {
        let accountSecret = url.appendingPathComponent(accountSecretFileName)
        if FileManager.default.fileExists(atPath: accountSecret.path) {
            return true
        }

        let clientStore = url.appendingPathComponent(clientStoreFileName)
        guard FileManager.default.fileExists(atPath: clientStore.path) else {
            return false
        }
        let size = (try? clientStore.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
        return size > 0
    }

    private static func latestModificationDate(for storeURL: URL) -> Date? {
        let candidates = [
            storeURL,
            storeURL.appendingPathComponent(accountSecretFileName),
            storeURL.appendingPathComponent(clientStoreFileName),
            storeURL.appendingPathComponent("\(clientStoreFileName)-wal"),
            storeURL.appendingPathComponent("\(clientStoreFileName)-shm"),
        ]
        return candidates.compactMap { url in
            try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
        }.max()
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

private struct LegacyDeviceStore {
    let deviceID: String
    let url: URL
    let modifiedAt: Date
}

@MainActor
final class AppModel: ObservableObject {
    @Published var serverURL: String
    @Published var deviceID: String
    @Published private(set) var state: AppState? {
        didSet {
            rebuildChatProjections()
        }
    }
    private(set) var chatProjections: [String: ChatRoomProjection] = [:]
    @Published var errorText: String?
    @Published var roomDraft: String = ""
    @Published var scanDraft: String = ""
    @Published var pinDraft: String = ""
    @Published var outboundText: String = ""

    private var runtime: (any FiniteChatRuntimeProtocol)?
    private var openKey = ""
    private let usesTransientStore: Bool
    private let applicationSupportURL: URL?
    private let configStorageURL: URL?
    private let args: [String]
    private let runtimeFactory: AppRuntimeFactory
    private let startsUpdateLoop: Bool
    private var updateTask: Task<Void, Never>?
    private var launchAutomationTask: Task<Void, Never>?
    private var attachmentDownloadsInFlight = Set<String>()
    private var messageRetriesInFlight = Set<String>()
    private var didRunLaunchAutomation = false

    deinit {
        updateTask?.cancel()
        launchAutomationTask?.cancel()
    }

    init(
        config: RuntimeConfig = RuntimeConfig.load(),
        applicationSupportURL: URL? = nil,
        configStorageURL: URL? = nil,
        args: [String] = CommandLine.arguments,
        startsUpdateLoop: Bool = true,
        runtimeFactory: @escaping AppRuntimeFactory = { options in
            try FiniteChatRuntime.open(options: options)
        }
    ) {
        serverURL = config.serverURL
        deviceID = config.deviceID
        usesTransientStore = config.usesTransientStore
        self.applicationSupportURL = applicationSupportURL
        self.configStorageURL = configStorageURL
        self.args = args
        self.runtimeFactory = runtimeFactory
        self.startsUpdateLoop = startsUpdateLoop
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
        if rooms.isEmpty {
            return nil
        }
        return state?.toast?.nonEmptyTrimmed
    }

    var developerErrorText: String? {
        errorText?.nonEmptyTrimmed
    }

    var developerRuntimeStatus: String? {
        state?.status.nonEmptyTrimmed
    }

    var activeProfile: AppProfileSummary? {
        guard let state, let activeProfileId = state.activeProfileId else { return nil }
        return state.profiles.first { $0.accountId == activeProfileId }
    }

    var canSend: Bool {
        guard let selectedRoom else { return false }
        return selectedRoom.state == .connected
            && !outboundText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    func start() {
        do {
            let runtime = try currentRuntime()
            state = try runtime.state()
            do {
                state = try runtime.dispatch(action: .startRuntime)
                errorText = nil
            } catch {
                errorText = String(describing: error)
            }
        } catch {
            errorText = String(describing: error)
        }
        restartUpdateLoopIfEnabled()
        if runtime != nil {
            runLaunchAutomationIfRequested()
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
        dispatch(.createInvite(roomId: room.roomId))
        return state?.activeInvite?.roomId == room.roomId
    }

    @discardableResult
    func scanTarget() -> Bool {
        let value = scanDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return true }
        scanDraft = ""
        dispatch(.scanTarget(value: value))
        return activeProfile == nil
    }

    func submitPin(for room: AppRoomSummary) {
        let pin = pinDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !pin.isEmpty else { return }
        pinDraft = ""
        dispatch(.submitInvitePin(pendingRoomId: room.roomId, pin: pin))
    }

    func retry(_ room: AppRoomSummary) {
        dispatch(.retryRoom(roomId: room.roomId))
    }

    func retry(_ message: ChatMessage) {
        let key = "\(message.roomId)|\(message.messageId)"
        guard !messageRetriesInFlight.contains(key) else { return }
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
    }

    func refreshDevices() {
        dispatch(.refreshDevices)
    }

    func revokeDevice(_ device: AppDeviceSummary) {
        guard !device.currentDevice, !device.revoked else { return }
        dispatch(.revokeDevice(accountId: device.accountId, deviceId: device.deviceId))
    }

    @discardableResult
    func send(replyTo message: ChatMessage? = nil) -> Bool {
        guard let room = selectedRoom else { return false }
        let text = outboundText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return false }
        let action: AppAction
        if let message {
            action = .sendReply(
                roomId: room.roomId,
                text: text,
                replyToMessageId: message.messageId
            )
        } else {
            action = .sendMessage(roomId: room.roomId, text: text)
        }
        let sent = dispatch(action)
        if sent {
            outboundText = ""
        }
        return sent
    }

    func sendAttachment(
        roomID: String,
        fileURL: URL,
        replyTo message: ChatMessage? = nil,
        onSuccess: (@MainActor () -> Void)? = nil
    ) {
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
            } catch {
                errorText = String(describing: error)
            }
        }
    }

    func sendAttachments(
        roomID: String,
        attachments: [OutboundAttachment],
        replyTo message: ChatMessage? = nil,
        onSuccess: (@MainActor () -> Void)? = nil
    ) {
        guard !attachments.isEmpty else { return }
        let caption = outboundText.trimmingCharacters(in: .whitespacesAndNewlines)
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
                outboundText = ""
                errorText = nil
                onSuccess?()
                restartUpdateLoopIfEnabled()
            } catch {
                errorText = String(describing: error)
            }
        }
    }

    @discardableResult
    func sendPoll(roomID: String, question: String, options: [String]) -> Bool {
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
        if let localPath = attachment.localPath?.trimmingCharacters(in: .whitespacesAndNewlines),
           !localPath.isEmpty
        {
            return
        }
        guard let url = attachment.url?.trimmingCharacters(in: .whitespacesAndNewlines),
              !url.isEmpty
        else {
            return
        }

        let key = "\(roomID)|\(message.messageId)|\(attachment.attachmentId)"
        guard !attachmentDownloadsInFlight.contains(key) else { return }
        attachmentDownloadsInFlight.insert(key)

        Task { [weak self] in
            guard let self else { return }
            defer {
                attachmentDownloadsInFlight.remove(key)
            }
            do {
                let runtime = try currentRuntime()
                let runtimeKey = openKey
                let action = AppAction.downloadAttachment(
                    roomId: roomID,
                    messageId: message.messageId,
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

    func applyDevSettings() {
        do {
            try RuntimeConfig(serverURL: serverURL, deviceID: deviceID).save(
                storageURL: configStorageURL
            )
        } catch {
            errorText = String(describing: error)
            return
        }
        closeRuntime()
        start()
    }

    private func startUpdateLoop() {
        updateTask?.cancel()
        guard let runtime else { return }
        let runtimeKey = openKey
        updateTask = Task { [weak self, runtime, runtimeKey] in
            while !Task.isCancelled {
                do {
                    let nextState = try await Task.detached(priority: .background) {
                        try runtime.waitForUpdate(timeoutMillis: 30_000)
                    }.value
                    guard !Task.isCancelled else { return }
                    guard let self, self.openKey == runtimeKey else { return }
                    self.state = nextState
                    self.errorText = nil
                } catch {
                    guard !Task.isCancelled else { return }
                    guard let self, self.openKey == runtimeKey else { return }
                    self.errorText = String(describing: error)
                    try? await Task.sleep(nanoseconds: 1_000_000_000)
                }
            }
        }
    }

    @discardableResult
    private func dispatch(_ action: AppAction) -> Bool {
        var succeeded = false
        run {
            let runtime = try currentRuntime()
            self.state = try runtime.dispatch(action: action)
            succeeded = true
        }
        restartUpdateLoopIfEnabled()
        return succeeded
    }

    private func restartUpdateLoopIfEnabled() {
        if startsUpdateLoop {
            startUpdateLoop()
        }
    }

    private func currentRuntime() throws -> any FiniteChatRuntimeProtocol {
        let key = "\(serverURL)|\(deviceID)"
        if let runtime, openKey == key {
            return runtime
        }
        let dataDir = try RuntimeDataStore.dataDir(
            deviceID: deviceID,
            applicationSupportURL: applicationSupportURL,
            transient: usesTransientStore
        )
        let opened = try runtimeFactory(
            OpenOptions(
                dataDir: dataDir,
                serverUrl: serverURL,
                deviceId: deviceID,
                accountSecretHex: nil,
                nowUnixSeconds: nil
            )
        )
        let openedState = try opened.state()
        if openedState.identity.deviceId != deviceID {
            deviceID = openedState.identity.deviceId
            if !usesTransientStore {
                try? RuntimeConfig(serverURL: serverURL, deviceID: deviceID).save(
                    storageURL: configStorageURL
                )
            }
        }
        runtime = opened
        openKey = "\(serverURL)|\(deviceID)"
        return opened
    }

    private func closeRuntime() {
        updateTask?.cancel()
        launchAutomationTask?.cancel()
        updateTask = nil
        launchAutomationTask = nil
        attachmentDownloadsInFlight.removeAll()
        messageRetriesInFlight.removeAll()
        runtime = nil
        openKey = ""
        state = nil
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
            errorText = String(describing: error)
        }
    }

    private func runLaunchAutomationIfRequested() {
        guard !didRunLaunchAutomation else { return }
        let inviteURL = Self.argumentValue("--finitechat-auto-join", in: args)
        let createRoomName = Self.argumentValue("--finitechat-auto-create-room", in: args)
        let outbound = Self.argumentValue("--finitechat-auto-send", in: args)
        guard inviteURL != nil || createRoomName != nil || outbound != nil else {
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
