import CoreImage.CIFilterBuiltins
import SwiftUI
import UIKit
import UniformTypeIdentifiers

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
    @State private var lastAppliedSelectedRoomID: String?

    var body: some View {
        NavigationStack(path: $path) {
            RoomListView(
                model: model,
                present: { destination in
                    sheet = destination
                },
                open: { room in
                    model.openRoom(room)
                    path = [room.roomId]
                    lastAppliedSelectedRoomID = room.roomId
                }
            )
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
            routeSelectedRoomIfNeeded(model.state?.selectedRoomId)
        }
        .onChange(of: model.state?.selectedRoomId) { _, selectedRoomID in
            routeSelectedRoomIfNeeded(selectedRoomID)
        }
    }

    private func routeSelectedRoomIfNeeded(_ selectedRoomID: String?) {
        guard let selectedRoomID else {
            lastAppliedSelectedRoomID = nil
            return
        }
        guard selectedRoomID != lastAppliedSelectedRoomID else { return }
        path = [selectedRoomID]
        lastAppliedSelectedRoomID = selectedRoomID
    }
}

private struct RoomListView: View {
    @ObservedObject var model: AppModel
    let present: (AppSheet) -> Void
    let open: (AppRoomSummary) -> Void

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
                        open(room)
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
    @State private var followsBottom = true
    @State private var importingAttachment = false
    @State private var replyDraftMessage: ChatMessage?
    @State private var focusedMessage: ChatMessage?
    @State private var focusedMessageFrame: CGRect = .zero
    @State private var focusedActionsVisible = false
    @State private var imagePreviewSelection: ChatImagePreviewSelection?
    @State private var videoPreviewItem: ChatAttachmentPreviewItem?
    @State private var documentPreviewItem: ChatAttachmentPreviewItem?

    private var room: AppRoomSummary? {
        model.state?.rooms.first(where: { $0.roomId == roomID })
    }

    private var projection: ChatRoomProjection {
        model.projection(for: roomID)
    }

    private var latestMessageID: String? {
        projection.messages.last?.messageId
    }

    var body: some View {
        ZStack {
            VStack(spacing: 0) {
                if let room {
                    messageSurface(room: room)
                } else {
                    ContentUnavailableView("Room unavailable", systemImage: "exclamationmark.triangle")
                }
            }

            if let focusedMessage {
                FocusedMessageOverlay(
                    message: focusedMessage,
                    replyTarget: focusedReplyTarget(for: focusedMessage),
                    anchorFrame: focusedMessageFrame,
                    actionsVisible: focusedActionsVisible,
                    onDismiss: {
                        dismissFocusedMessage()
                    },
                    onReact: { emoji in
                        model.react(to: focusedMessage, emoji: emoji)
                        dismissFocusedMessage()
                    },
                    onReply: {
                        replyDraftMessage = focusedMessage
                        dismissFocusedMessage()
                    },
                    onCopy: {
                        UIPasteboard.general.string = messageClipboardText(focusedMessage)
                        dismissFocusedMessage()
                    },
                    canCopy: !messageClipboardText(focusedMessage).isEmpty
                )
                .transition(.opacity.combined(with: .scale(scale: 0.96)))
                .zIndex(10)
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
                model.markRoomRead(room)
            }
        }
        .onChange(of: latestMessageID) {
            if let room {
                model.markRoomRead(room)
            }
        }
        .fileImporter(
            isPresented: $importingAttachment,
            allowedContentTypes: [.item],
            allowsMultipleSelection: false
        ) { result in
            handleImportedAttachment(result)
        }
        .fullScreenCover(item: $imagePreviewSelection) { selection in
            ChatImagePreviewView(selection: selection) {
                imagePreviewSelection = nil
            }
        }
        .fullScreenCover(item: $videoPreviewItem) { item in
            ChatVideoPreviewView(item: item) {
                videoPreviewItem = nil
            }
        }
        .fullScreenCover(item: $documentPreviewItem) { item in
            ChatDocumentPreviewView(item: item) {
                documentPreviewItem = nil
            }
        }
        .onDisappear {
            dismissFocusedMessage(animated: false)
        }
    }

    @ViewBuilder
    private func messageSurface(room: AppRoomSummary) -> some View {
        switch room.state {
        case .connected:
            ChatTranscriptView(
                roomID: room.roomId,
                rows: projection.rows,
                messagesById: projection.messagesById,
                onReact: { message, emoji in
                    model.react(to: message, emoji: emoji)
                },
                onDownloadAttachment: { message, attachment in
                    model.downloadAttachment(roomID: room.roomId, message: message, attachment: attachment)
                },
                onOpenAttachment: { message, attachment in
                    handleAttachmentOpen(message: message, attachment: attachment)
                },
                onLongPressMessage: { message, frame in
                    presentFocusedMessage(message, frame: frame)
                },
                canLoadOlder: room.canLoadOlder,
                onLoadOlderMessages: { beforeMessageID in
                    model.loadOlderMessages(roomID: room.roomId, beforeMessageID: beforeMessageID)
                },
                followsBottom: $followsBottom
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(.systemGroupedBackground))
            .accessibilityLabel("Messages")
            Composer(
                model: model,
                replyTarget: replyDraftMessage,
                onCancelReply: {
                    replyDraftMessage = nil
                },
                onSend: {
                    if model.send(replyTo: replyDraftMessage) {
                        replyDraftMessage = nil
                    }
                }
            ) {
                importingAttachment = true
            }
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

    private func handleImportedAttachment(_ result: Result<[URL], Error>) {
        switch result {
        case .success(let urls):
            guard let url = urls.first else { return }
            model.sendAttachment(roomID: roomID, fileURL: url, replyTo: replyDraftMessage) {
                replyDraftMessage = nil
            }
        case .failure(let error):
            model.errorText = String(describing: error)
        }
    }

    private func handleAttachmentOpen(message: ChatMessage, attachment: ChatMediaAttachment) {
        guard let localURL = attachmentLocalURL(attachment) else {
            if attachmentCanDownload(attachment) {
                model.downloadAttachment(roomID: roomID, message: message, attachment: attachment)
            }
            return
        }

        switch attachment.kind {
        case .image:
            let imageAttachments = message.media.filter { media in
                media.kind == .image && attachmentLocalURL(media) != nil
            }
            imagePreviewSelection = ChatImagePreviewSelection(
                attachments: imageAttachments,
                selected: attachment
            )
        case .video:
            videoPreviewItem = ChatAttachmentPreviewItem(attachment: attachment, url: localURL)
        case .voiceNote, .file:
            documentPreviewItem = ChatAttachmentPreviewItem(attachment: attachment, url: localURL)
        }
    }

    private func presentFocusedMessage(_ message: ChatMessage, frame: CGRect) {
        focusedMessageFrame = frame
        withAnimation(.spring(response: 0.28, dampingFraction: 0.78)) {
            focusedMessage = message
            focusedActionsVisible = true
        }
    }

    private func dismissFocusedMessage(animated: Bool = true) {
        let updates = {
            focusedMessage = nil
            focusedActionsVisible = false
        }
        if animated {
            withAnimation(.easeOut(duration: 0.16), updates)
        } else {
            updates()
        }
    }

    private func focusedReplyTarget(for message: ChatMessage) -> ChatMessage? {
        guard let replyToMessageId = message.replyToMessageId else { return nil }
        return projection.messagesById[replyToMessageId]
    }
}

private struct FocusedMessageOverlay: View {
    let message: ChatMessage
    let replyTarget: ChatMessage?
    let anchorFrame: CGRect
    let actionsVisible: Bool
    let onDismiss: () -> Void
    let onReact: (String) -> Void
    let onReply: () -> Void
    let onCopy: () -> Void
    let canCopy: Bool

    var body: some View {
        GeometryReader { geometry in
            ZStack {
                Color.black.opacity(0.18)
                    .ignoresSafeArea()
                    .contentShape(Rectangle())
                    .onTapGesture(perform: onDismiss)

                VStack(alignment: message.isMine ? .trailing : .leading, spacing: 10) {
                    FocusedReactionBar(onReact: onReact)

                    FocusedChatMessageCard(
                        message: message,
                        replyTarget: replyTarget
                    )
                    .frame(maxWidth: min(geometry.size.width * 0.82, 360))

                    if actionsVisible {
                        FocusedMessageActionCard(
                            canCopy: canCopy,
                            onReply: onReply,
                            onCopy: onCopy
                        )
                        .transition(.opacity.combined(with: .move(edge: .top)))
                    }
                }
                .frame(
                    maxWidth: .infinity,
                    maxHeight: .infinity,
                    alignment: message.isMine ? .topTrailing : .topLeading
                )
                .padding(.top, overlayTop(in: geometry))
                .padding(.horizontal, 20)
                .animation(.easeOut(duration: 0.16), value: actionsVisible)
            }
        }
    }

    private func overlayTop(in geometry: GeometryProxy) -> CGFloat {
        let overlayOriginY = geometry.frame(in: .global).minY
        let localAnchorY = anchorFrame.minY - overlayOriginY
        let reactionBarSpace: CGFloat = 58
        let idealTop = localAnchorY - reactionBarSpace
        let maxTop = max(12, geometry.size.height * 0.58)
        return min(max(idealTop, 12), maxTop)
    }
}

private struct FocusedReactionBar: View {
    let onReact: (String) -> Void

    var body: some View {
        HStack(spacing: 2) {
            ForEach(focusedReactionEmojis, id: \.self) { emoji in
                Button {
                    UIImpactFeedbackGenerator(style: .light).impactOccurred()
                    onReact(emoji)
                } label: {
                    Text(emoji)
                        .font(.system(size: 24))
                        .frame(width: 42, height: 42)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("React \(emoji)")
            }
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 4)
        .background(.regularMaterial, in: Capsule())
        .shadow(color: .black.opacity(0.14), radius: 14, x: 0, y: 6)
    }
}

private struct FocusedMessageActionCard: View {
    let canCopy: Bool
    let onReply: () -> Void
    let onCopy: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            Button {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                onReply()
            } label: {
                Label("Reply", systemImage: "arrowshape.turn.up.left")
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
            }
            .buttonStyle(.plain)

            Divider()

            Button {
                onCopy()
            } label: {
                Label("Copy", systemImage: "doc.on.doc")
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
            }
            .buttonStyle(.plain)
            .disabled(!canCopy)
        }
        .frame(width: 176)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .shadow(color: .black.opacity(0.14), radius: 14, x: 0, y: 6)
    }
}

private let focusedReactionEmojis = ["❤️", "👍", "😂", "😮", "😢", "🙏"]

private func messageClipboardText(_ message: ChatMessage) -> String {
    let display = message.displayContent.trimmingCharacters(in: .whitespacesAndNewlines)
    if !display.isEmpty {
        return display
    }
    return message.text.trimmingCharacters(in: .whitespacesAndNewlines)
}

private struct Composer: View {
    @ObservedObject var model: AppModel
    let replyTarget: ChatMessage?
    let onCancelReply: () -> Void
    let onSend: () -> Void
    let onAttach: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            if let replyTarget {
                ComposerReplyPreview(
                    message: replyTarget,
                    onCancel: onCancelReply
                )
            }

            HStack(spacing: 10) {
                Button {
                    onAttach()
                } label: {
                    Image(systemName: "paperclip")
                        .font(.title3)
                }
                .accessibilityLabel("Attach")
                .accessibilityIdentifier("AttachButton")

                TextField("Message", text: $model.outboundText, axis: .vertical)
                    .lineLimit(1...4)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityLabel("Message")

                Button {
                    onSend()
                } label: {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.title2)
                }
                .disabled(!model.canSend)
                .accessibilityLabel("Send")
                .accessibilityIdentifier("SendButton")
            }
            .padding()
        }
        .background(.bar)
    }
}

private struct ComposerReplyPreview: View {
    let message: ChatMessage
    let onCancel: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Rectangle()
                .fill(Color.accentColor)
                .frame(width: 3, height: 36)
                .clipShape(Capsule())

            VStack(alignment: .leading, spacing: 2) {
                Text("Replying to \(senderLabel)")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Text(snippet)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer(minLength: 8)

            Button {
                onCancel()
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .font(.body)
                    .foregroundStyle(.tertiary)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Cancel reply")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.thinMaterial)
    }

    private var senderLabel: String {
        if message.isMine {
            return "You"
        }
        let name = message.senderDisplayName.trimmingCharacters(in: .whitespacesAndNewlines)
        return name.isEmpty ? message.senderDeviceId : name
    }

    private var snippet: String {
        let text = message.displayContent.isEmpty ? message.text : message.displayContent
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            return trimmed.split(separator: "\n").first.map(String.init) ?? trimmed
        }
        if let media = message.media.first {
            return media.filename.isEmpty ? composerMediaLabel(for: media.kind) : media.filename
        }
        return "Message"
    }
}

private func composerMediaLabel(for kind: ChatMediaKind) -> String {
    switch kind {
    case .image:
        return "Image"
    case .voiceNote:
        return "Voice note"
    case .video:
        return "Video"
    case .file:
        return "File"
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
