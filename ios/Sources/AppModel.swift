import Foundation
import SwiftUI
import UniformTypeIdentifiers

struct RuntimeConfig: Codable, Equatable {
    let serverURL: String
    let deviceID: String

    private static let defaultServerURL = "http://127.0.0.1:8787"
    private static let defaultDeviceID = "ios"
    private static let dataRootDirectoryName = "FiniteChat"
    private static let clientStoreFileName = "client.sqlite3"
    private static let accountSecretFileName = "account-secret.hex"
    private static let transientConfigArgument = "--finitechat-transient-config"
    private static let transientConfigEnvironmentKey = "FINITECHAT_TRANSIENT_CONFIG"

    enum CodingKeys: String, CodingKey {
        case serverURL = "server_url"
        case deviceID = "device_id"
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
        let fallback = persisted ?? RuntimeConfig(
            serverURL: defaultServerURL,
            deviceID: existingSingleDeviceStoreID(storageURL: storageURL) ?? defaultDeviceID
        )
        let config = RuntimeConfig(
            serverURL: serverURL ?? fallback.serverURL,
            deviceID: deviceID ?? fallback.deviceID
        )
        let hasLaunchOverride = serverURL != nil || deviceID != nil
        let transientOverride = argumentFlag(transientConfigArgument, in: args)
            || truthyEnvironmentValue(transientConfigEnvironmentKey, in: environment)
        // Runtime identity is product state. A phone launched from Xcode with a
        // LAN server or device id must reopen that same SQLite store after a
        // manual force-close. Tests/debug harnesses can still opt into a
        // process-local override with the transient flag.
        if persisted == nil || (hasLaunchOverride && !transientOverride) {
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

    private static func loadPersisted(storageURL: URL?) -> RuntimeConfig? {
        guard let url = storageURL ?? (try? configURL()),
              let data = try? Data(contentsOf: url),
              let config = try? JSONDecoder().decode(RuntimeConfig.self, from: data)
        else {
            return nil
        }
        let serverURL = config.serverURL.trimmingCharacters(in: .whitespacesAndNewlines)
        let deviceID = config.deviceID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !serverURL.isEmpty, !deviceID.isEmpty else { return nil }
        return RuntimeConfig(serverURL: serverURL, deviceID: deviceID)
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

    private static func existingSingleDeviceStoreID(storageURL: URL?) -> String? {
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
            return nil
        }

        let dataRoot = supportURL.appendingPathComponent(dataRootDirectoryName, isDirectory: true)
        guard let entries = try? FileManager.default.contentsOfDirectory(
            at: dataRoot,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        ) else {
            return nil
        }

        let candidates = entries.filter { entry in
            let isDirectory = (try? entry.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) ?? false
            guard isDirectory else { return false }
            return FileManager.default.fileExists(
                atPath: entry.appendingPathComponent(clientStoreFileName).path
            ) || FileManager.default.fileExists(
                atPath: entry.appendingPathComponent(accountSecretFileName).path
            )
        }
        guard candidates.count == 1 else { return nil }
        let deviceID = candidates[0].lastPathComponent.trimmingCharacters(in: .whitespacesAndNewlines)
        return deviceID.isEmpty ? nil : deviceID
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

@MainActor
final class AppModel: ObservableObject {
    private static let initialConfig = RuntimeConfig.load()

    @Published var serverURL: String = AppModel.initialConfig.serverURL
    @Published var deviceID: String = AppModel.initialConfig.deviceID
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

    private var runtime: FiniteChatRuntime?
    private var openKey = ""
    private var updateTask: Task<Void, Never>?
    private var launchAutomationTask: Task<Void, Never>?
    private var attachmentDownloadsInFlight = Set<String>()
    private var didRunLaunchAutomation = false

    deinit {
        updateTask?.cancel()
        launchAutomationTask?.cancel()
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
        runLaunchAutomationIfRequested()
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
        startUpdateLoop()
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
                startUpdateLoop()
            } catch {
                errorText = String(describing: error)
            }
        }
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
                startUpdateLoop()
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
            try RuntimeConfig(serverURL: serverURL, deviceID: deviceID).save()
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
        startUpdateLoop()
        return succeeded
    }

    private func currentRuntime() throws -> FiniteChatRuntime {
        let key = "\(serverURL)|\(deviceID)"
        if let runtime, openKey == key {
            return runtime
        }
        let dataDir = try Self.dataDir(deviceID: deviceID)
        let opened = try FiniteChatRuntime.open(
            options: OpenOptions(
                dataDir: dataDir,
                serverUrl: serverURL,
                deviceId: deviceID,
                accountSecretHex: nil,
                nowUnixSeconds: nil
            )
        )
        runtime = opened
        openKey = key
        return opened
    }

    private func closeRuntime() {
        updateTask?.cancel()
        launchAutomationTask?.cancel()
        updateTask = nil
        launchAutomationTask = nil
        attachmentDownloadsInFlight.removeAll()
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
        let args = CommandLine.arguments
        guard let inviteURL = Self.argumentValue("--finitechat-auto-join", in: args) else {
            return
        }

        didRunLaunchAutomation = true
        scanDraft = inviteURL
        pinDraft = Self.argumentValue("--finitechat-pin", in: args) ?? pinDraft
        deviceID = Self.argumentValue("--finitechat-device", in: args) ?? deviceID
        serverURL = Self.argumentValue("--finitechat-server", in: args) ?? serverURL
        let requestedRoomID = Self.argumentValue("--finitechat-room", in: args)
        let outbound = Self.argumentValue("--finitechat-auto-send", in: args)

        launchAutomationTask = Task {
            self.scanTarget()
            let roomID = requestedRoomID ?? self.state?.selectedRoomId
            if let room = self.launchAutomationRoom(roomID: roomID) {
                self.submitPin(for: room)
            }
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

    private static func dataDir(deviceID: String) throws -> String {
        let safeDeviceID = deviceID
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "/", with: "-")
        let base = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let dir = base.appendingPathComponent("FiniteChat/\(safeDeviceID)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.path
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

private struct PreparedAttachment: Sendable {
    let data: Data
    let filename: String
    let mimeType: String
    let kind: ChatMediaKind
}
