import CoreImage.CIFilterBuiltins
import SwiftUI

private enum AppSheet: Identifiable {
    case newRoom
    case scan
    case invite
    case settings

    var id: String {
        switch self {
        case .newRoom:
            "new-room"
        case .scan:
            "scan"
        case .invite:
            "invite"
        case .settings:
            "settings"
        }
    }
}

struct ContentView: View {
    @ObservedObject var model: AppModel
    @State private var sheet: AppSheet?
    @State private var path: [String] = []

    var body: some View {
        NavigationStack(path: $path) {
            RoomListView(
                model: model,
                openRoom: { room in
                    model.openRoom(room)
                    if path.last != room.roomId {
                        path.append(room.roomId)
                    }
                }
            ) { destination in
                sheet = destination
            }
            .navigationDestination(for: String.self) { roomID in
                RoomThreadView(model: model, roomID: roomID) {
                    sheet = .invite
                }
            }
            .sheet(item: $sheet) { destination in
                switch destination {
                case .newRoom:
                    NewRoomSheet(model: model)
                case .scan:
                    ScanSheet(model: model)
                case .invite:
                    InviteSheet(invite: model.state?.activeInvite)
                case .settings:
                    SettingsSheet(model: model)
                }
            }
        }
        .task {
            model.start()
        }
    }
}

private struct RoomListView: View {
    @ObservedObject var model: AppModel
    let openRoom: (AppRoomSummary) -> Void
    let present: (AppSheet) -> Void

    var body: some View {
        Group {
            if model.rooms.isEmpty {
                ContentUnavailableView {
                    Label("FiniteChat", systemImage: "bubble.left.and.bubble.right")
                } description: {
                    Text(model.errorText ?? model.state?.status ?? "Ready")
                } actions: {
                    HStack {
                        Button {
                            present(.newRoom)
                        } label: {
                            Label("New Room", systemImage: "square.and.pencil")
                        }
                        Button {
                            present(.scan)
                        } label: {
                            Label("Scan", systemImage: "qrcode.viewfinder")
                        }
                    }
                    .buttonStyle(.borderedProminent)
                }
            } else {
                List(model.rooms, id: \.roomId) { room in
                    Button {
                        openRoom(room)
                    } label: {
                        RoomRow(room: room)
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("RoomRow-\(room.roomId)")
                }
                .listStyle(.plain)
            }
        }
        .navigationTitle("Chats")
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button {
                    present(.scan)
                } label: {
                    Image(systemName: "qrcode.viewfinder")
                }
                .accessibilityLabel("Scan")
                .accessibilityIdentifier("ScanButton")

                Button {
                    present(.newRoom)
                } label: {
                    Image(systemName: "square.and.pencil")
                }
                .accessibilityLabel("New Room")
                .accessibilityIdentifier("NewRoomButton")

                Button {
                    present(.settings)
                } label: {
                    Image(systemName: "gearshape")
                }
                .accessibilityLabel("Settings")
                .accessibilityIdentifier("SettingsButton")
            }
        }
        .safeAreaInset(edge: .bottom) {
            StatusBar(text: model.errorText ?? model.state?.toast ?? model.state?.status)
        }
    }
}

private struct RoomRow: View {
    let room: AppRoomSummary

    var body: some View {
        HStack(spacing: 12) {
            Circle()
                .fill(room.state.tint)
                .frame(width: 12, height: 12)

            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text(room.displayName)
                        .font(.body)
                        .lineLimit(1)
                    Spacer(minLength: 8)
                    if room.unreadCount > 0 {
                        Text("\(room.unreadCount)")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.white)
                            .padding(.horizontal, 7)
                            .padding(.vertical, 3)
                            .background(Capsule().fill(Color.accentColor))
                    }
                }

                Text(rowSubtitle)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .padding(.vertical, 8)
        .accessibilityElement(children: .combine)
    }

    private var rowSubtitle: String {
        if !room.lastMessagePreview.isEmpty {
            return room.lastMessagePreview
        }
        return room.status
    }
}

private struct RoomThreadView: View {
    @ObservedObject var model: AppModel
    let roomID: String
    let showInvite: () -> Void

    private var room: AppRoomSummary? {
        model.state?.rooms.first(where: { $0.roomId == roomID })
    }

    private var messages: [ChatMessage] {
        model.state?.messages.filter { $0.roomId == roomID } ?? []
    }

    var body: some View {
        VStack(spacing: 0) {
            if let room {
                messageSurface(room: room)
            } else {
                ContentUnavailableView("Room unavailable", systemImage: "exclamationmark.triangle")
            }
        }
        .navigationTitle(room?.displayName ?? "Chat")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if let room, room.state == .connected {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        if model.createInvite(for: room) {
                            showInvite()
                        }
                    } label: {
                        Image(systemName: "qrcode")
                    }
                    .accessibilityLabel("Invite")
                    .accessibilityIdentifier("InviteButton")
                }
            }
        }
        .onAppear {
            if let room {
                model.openRoom(room)
            }
        }
    }

    @ViewBuilder
    private func messageSurface(room: AppRoomSummary) -> some View {
        switch room.state {
        case .connected:
            ThreadActionBar(room: room) {
                if model.createInvite(for: room) {
                    showInvite()
                }
            }
            MessageList(messages: messages)
            Composer(model: model)
        case .waitingForApproval:
            PendingRoomView(room: room, model: model)
        case .joining:
            ProgressView(room.status)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .needsAttention, .offline:
            NeedsAttentionView(room: room) {
                model.retry(room)
            }
        }
    }
}

private struct ThreadActionBar: View {
    let room: AppRoomSummary
    let invite: () -> Void

    var body: some View {
        HStack {
            Text(room.status)
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer()
            Button {
                invite()
            } label: {
                Label("Invite", systemImage: "qrcode")
            }
            .buttonStyle(.bordered)
            .accessibilityIdentifier("ThreadInviteButton")
        }
        .padding(.horizontal)
        .padding(.vertical, 8)
        .background(.bar)
    }
}

private struct MessageList: View {
    let messages: [ChatMessage]

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 10) {
                ForEach(messages, id: \.messageId) { message in
                    MessageBubble(message: message)
                }
            }
            .padding(.horizontal)
            .padding(.vertical, 12)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(.systemGroupedBackground))
        .accessibilityLabel("Messages")
    }
}

private struct MessageBubble: View {
    let message: ChatMessage

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(message.senderDeviceId)
                .font(.caption)
                .foregroundStyle(.secondary)
            Text(message.text)
                .font(.body)
                .textSelection(.enabled)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(RoundedRectangle(cornerRadius: 8).fill(Color(.secondarySystemGroupedBackground)))
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }
}

private struct Composer: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(spacing: 10) {
            TextField("Message", text: $model.outboundText, axis: .vertical)
                .lineLimit(1...4)
                .textFieldStyle(.roundedBorder)
                .accessibilityLabel("Message")

            Button {
                model.send()
            } label: {
                Image(systemName: "arrow.up.circle.fill")
                    .font(.title2)
            }
            .disabled(!model.canSend)
            .accessibilityLabel("Send")
            .accessibilityIdentifier("SendButton")
        }
        .padding()
        .background(.bar)
    }
}

private struct PendingRoomView: View {
    let room: AppRoomSummary
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "lock.open")
                .font(.largeTitle)
                .foregroundStyle(.secondary)
            Text(room.status)
                .font(.body)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)

            TextField("PIN", text: $model.pinDraft)
                .keyboardType(.numberPad)
                .textFieldStyle(.roundedBorder)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 180)
                .accessibilityLabel("PIN")

            Button {
                model.submitPin(for: room)
            } label: {
                Label("Join", systemImage: "arrow.right.circle.fill")
            }
            .buttonStyle(.borderedProminent)
        }
        .padding()
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct NeedsAttentionView: View {
    let room: AppRoomSummary
    let retry: () -> Void

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "exclamationmark.triangle")
                .font(.largeTitle)
                .foregroundStyle(.secondary)
            Text(room.status)
                .font(.body)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            Button {
                retry()
            } label: {
                Label("Retry", systemImage: "arrow.clockwise")
            }
            .buttonStyle(.borderedProminent)
        }
        .padding()
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct NewRoomSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var model: AppModel

    var body: some View {
        NavigationStack {
            Form {
                TextField("Room name", text: $model.roomDraft)
                    .textInputAutocapitalization(.words)
                    .accessibilityLabel("Room name")
            }
            .navigationTitle("New Room")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        dismiss()
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Create") {
                        model.createRoom()
                        dismiss()
                    }
                    .disabled(model.roomDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
    }
}

private struct ScanSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var model: AppModel

    var body: some View {
        NavigationStack {
            Form {
                TextField("Invite URL or npub", text: $model.scanDraft, axis: .vertical)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .lineLimit(3...6)
                    .accessibilityLabel("Invite URL or npub")

                if let profile = model.activeProfile {
                    Section("Profile") {
                        ProfileRow(profile: profile)
                    }
                }
            }
            .navigationTitle("Scan")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        dismiss()
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Continue") {
                        if model.scanTarget() {
                            dismiss()
                        }
                    }
                    .disabled(model.scanDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
    }
}

private struct InviteSheet: View {
    let invite: AppInviteState?

    var body: some View {
        NavigationStack {
            VStack(spacing: 18) {
                if let invite {
                    QRCodeView(value: invite.inviteUrl)
                        .frame(width: 220, height: 220)
                        .accessibilityLabel("Invite QR")

                    VStack(spacing: 6) {
                        Text(invite.pin)
                            .font(.system(size: 36, weight: .semibold, design: .rounded))
                            .monospacedDigit()
                        Text(invite.inviteUrl)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                            .textSelection(.enabled)
                            .lineLimit(4)
                    }

                    ShareLink(item: invite.inviteUrl) {
                        Label("Share", systemImage: "square.and.arrow.up")
                    }
                    .buttonStyle(.borderedProminent)
                } else {
                    ContentUnavailableView("Invite unavailable", systemImage: "qrcode")
                }
            }
            .padding()
            .navigationTitle("Invite")
            .navigationBarTitleDisplayMode(.inline)
        }
    }
}

private struct SettingsSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var model: AppModel

    var body: some View {
        NavigationStack {
            Form {
                if let state = model.state {
                    Section("Profiles") {
                        if state.profiles.isEmpty {
                            Text("No profiles cached")
                                .foregroundStyle(.secondary)
                        } else {
                            ForEach(state.profiles, id: \.accountId) { profile in
                                ProfileRow(profile: profile)
                            }
                        }
                    }

                    Section("Devices") {
                        if state.devices.isEmpty {
                            Text("No devices found")
                                .foregroundStyle(.secondary)
                        } else {
                            ForEach(state.devices, id: \.listID) { device in
                                DeviceRow(device: device) {
                                    model.revokeDevice(device)
                                }
                            }
                        }

                        Button {
                            model.refreshDevices()
                        } label: {
                            Label("Refresh", systemImage: "arrow.clockwise")
                        }
                        .accessibilityIdentifier("RefreshDevicesButton")
                    }
                }

                if let errorText = model.errorText {
                    Section("Last Error") {
                        Text(errorText)
                            .font(.caption)
                            .textSelection(.enabled)
                    }
                }

                DisclosureGroup("Developer") {
                    TextField("Server", text: $model.serverURL)
                        .textInputAutocapitalization(.never)
                        .keyboardType(.URL)
                        .accessibilityLabel("Server")
                    TextField("Device", text: $model.deviceID)
                        .textInputAutocapitalization(.never)
                        .accessibilityLabel("Device")

                    if let state = model.state {
                        LabeledContent("Account", value: state.identity.accountId)
                        LabeledContent("Runtime Device", value: state.identity.deviceId)
                        LabeledContent("Revision", value: "\(state.rev)")
                    }
                }
            }
            .navigationTitle("Settings")
            .task {
                model.refreshDevices()
            }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") {
                        dismiss()
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Apply") {
                        model.applyDevSettings()
                        dismiss()
                    }
                }
            }
        }
    }
}

private struct ProfileRow: View {
    let profile: AppProfileSummary

    var body: some View {
        HStack(spacing: 12) {
            ProfileAvatar(profile: profile)

            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text(profile.displayName)
                        .font(.body)
                        .lineLimit(1)
                    if profile.stale {
                        Text("Stale")
                            .font(.caption.weight(.medium))
                            .foregroundStyle(.secondary)
                    }
                }

                Text(profile.about ?? profile.npub)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }

            Spacer(minLength: 8)
        }
        .accessibilityElement(children: .combine)
    }
}

private struct ProfileAvatar: View {
    let profile: AppProfileSummary

    var body: some View {
        ZStack {
            Circle()
                .fill(Color(.tertiarySystemFill))

            if let url = profile.picture.flatMap(URL.init(string:)) {
                AsyncImage(url: url) { image in
                    image
                        .resizable()
                        .scaledToFill()
                } placeholder: {
                    initials
                }
            } else {
                initials
            }
        }
        .frame(width: 40, height: 40)
        .clipShape(Circle())
        .accessibilityHidden(true)
    }

    private var initials: some View {
        Text(profile.displayName.prefix(1).uppercased())
            .font(.headline)
            .foregroundStyle(.secondary)
    }
}

private struct DeviceRow: View {
    let device: AppDeviceSummary
    let revoke: () -> Void

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text(device.deviceId)
                        .font(.body)
                        .lineLimit(1)
                    if device.currentDevice {
                        Text("This device")
                            .font(.caption.weight(.medium))
                            .foregroundStyle(.secondary)
                    }
                }

                Text(statusText)
                    .font(.caption)
                    .foregroundStyle(device.revoked ? .red : .secondary)
            }

            Spacer(minLength: 12)

            if device.currentDevice {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(.green)
                    .accessibilityLabel("Current device")
            } else if device.revoked {
                Image(systemName: "xmark.circle.fill")
                    .foregroundStyle(.red)
                    .accessibilityLabel("Revoked")
            } else {
                Button(role: .destructive) {
                    revoke()
                } label: {
                    Label("Revoke", systemImage: "xmark.circle")
                }
                .buttonStyle(.borderless)
                .accessibilityIdentifier("RevokeDeviceButton")
            }
        }
        .accessibilityElement(children: .combine)
    }

    private var statusText: String {
        let rooms = "\(device.roomCount) room\(device.roomCount == 1 ? "" : "s")"
        if device.revoked {
            return "Revoked - \(rooms)"
        }
        if device.active {
            return "Active - \(rooms)"
        }
        return "Inactive - \(rooms)"
    }
}

private extension AppDeviceSummary {
    var listID: String {
        "\(accountId)/\(deviceId)"
    }
}

private struct QRCodeView: View {
    let value: String
    private let context = CIContext()
    private let filter = CIFilter.qrCodeGenerator()

    var body: some View {
        if let image = makeImage() {
            Image(uiImage: image)
                .interpolation(.none)
                .resizable()
                .scaledToFit()
        } else {
            Image(systemName: "qrcode")
                .resizable()
                .scaledToFit()
                .foregroundStyle(.secondary)
        }
    }

    private func makeImage() -> UIImage? {
        filter.message = Data(value.utf8)
        guard let output = filter.outputImage else { return nil }
        let scaled = output.transformed(by: CGAffineTransform(scaleX: 10, y: 10))
        guard let cgImage = context.createCGImage(scaled, from: scaled.extent) else { return nil }
        return UIImage(cgImage: cgImage)
    }
}

private struct StatusBar: View {
    let text: String?

    var body: some View {
        if let text, !text.isEmpty {
            Text(text)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal)
                .padding(.vertical, 8)
                .background(.bar)
        }
    }
}

private extension AppRoomState {
    var tint: Color {
        switch self {
        case .connected:
            .green
        case .waitingForApproval, .joining:
            .orange
        case .needsAttention:
            .red
        case .offline:
            .gray
        }
    }
}
