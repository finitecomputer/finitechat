import Foundation
import SwiftUI

struct RuntimeConfig: Codable {
    let serverURL: String
    let deviceID: String

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
        let config = RuntimeConfig(
            serverURL: serverURL ?? persisted?.serverURL ?? "http://127.0.0.1:8787",
            deviceID: deviceID ?? persisted?.deviceID ?? "ios"
        )
        // Launch args and environment are process-local test/dev overrides.
        // Persisting them can strand real chats under a different device store.
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

    func send() {
        guard let room = selectedRoom else { return }
        let text = outboundText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        outboundText = ""
        dispatch(.sendMessage(roomId: room.roomId, text: text))
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

    private func dispatch(_ action: AppAction) {
        run {
            let runtime = try currentRuntime()
            self.state = try runtime.dispatch(action: action)
        }
        startUpdateLoop()
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
}
