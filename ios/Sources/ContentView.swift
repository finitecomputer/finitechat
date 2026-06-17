import CoreImage.CIFilterBuiltins
import PhotosUI
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
                    Text(model.roomListEmptyDescription)
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
            NoticeBar(text: model.userNoticeText)
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
        return room.userStatusText
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
    @State private var composerFocused = false
    @State private var imagePreviewSelection: ChatImagePreviewSelection?
    @State private var videoPreviewItem: ChatAttachmentPreviewItem?
    @State private var documentPreviewItem: ChatAttachmentPreviewItem?
    @State private var selectedPhotoItems: [PhotosPickerItem] = []
    @State private var stagedAttachments: [StagedComposerAttachment] = []
    @State private var showPhotoPicker = false
    @State private var pollComposerDraft: PollComposerDraft?
    @StateObject private var voiceRecorder = VoiceRecorder()
    @State private var voiceSendInFlight = false

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
                        composerFocused = true
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
            allowsMultipleSelection: true
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
        .sheet(item: $pollComposerDraft) { draft in
            PollComposerView { question, options in
                model.sendPoll(roomID: draft.roomID, question: question, options: options)
            }
        }
        .onDisappear {
            dismissFocusedMessage(animated: false)
            voiceRecorder.cancelRecording()
        }
        .onChange(of: selectedPhotoItems) { _, items in
            stagePhotoItems(items)
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
                onVotePoll: { message, option in
                    model.votePoll(message: message, option: option)
                },
                onLongPressMessage: { message, frame in
                    presentFocusedMessage(message, frame: frame)
                },
                accessoryContent: composerAccessory,
                isInputFocused: composerFocused,
                canLoadOlder: room.canLoadOlder,
                onLoadOlderMessages: { beforeMessageID in
                    model.loadOlderMessages(roomID: room.roomId, beforeMessageID: beforeMessageID)
                },
                followsBottom: $followsBottom
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(.systemGroupedBackground))
            .accessibilityLabel("Messages")
        case .waitingForApproval:
            PendingRoomView(room: room, model: model)
        case .joining:
            ProgressView(room.userStatusText)
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
            stageFileURLs(urls)
        case .failure(let error):
            model.errorText = String(describing: error)
        }
    }

    @ViewBuilder
    private var composerAccessory: some View {
        if let recording = voiceRecorder.state {
            VoiceRecordingComposerView(
                recording: recording,
                isSending: voiceSendInFlight,
                onSend: {
                    sendVoiceRecording()
                },
                onCancel: {
                    cancelVoiceRecording()
                },
                onTogglePause: {
                    toggleVoiceRecordingPause()
                }
            )
            .transition(.move(edge: .bottom).combined(with: .opacity))
        } else {
            Composer(
                model: model,
                replyTarget: replyDraftMessage,
                stagedAttachments: $stagedAttachments,
                isPhotoPickerPresented: $showPhotoPicker,
                selectedPhotoItems: $selectedPhotoItems,
                isInputFocused: $composerFocused,
                onCancelReply: {
                    replyDraftMessage = nil
                },
                onSend: {
                    sendComposerDraft()
                },
                onStartVoiceRecording: {
                    startVoiceRecording()
                }
            ) {
                importingAttachment = true
            } onCreatePoll: {
                pollComposerDraft = PollComposerDraft(roomID: roomID)
            }
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
        composerFocused = false
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

    private func sendComposerDraft() {
        if stagedAttachments.isEmpty {
            if model.send(replyTo: replyDraftMessage) {
                replyDraftMessage = nil
            }
            return
        }

        let outbound = stagedAttachments.map(\.outboundAttachment)
        model.sendAttachments(roomID: roomID, attachments: outbound, replyTo: replyDraftMessage) {
            stagedAttachments = []
            selectedPhotoItems = []
            replyDraftMessage = nil
        }
    }

    private func startVoiceRecording() {
        guard voiceRecorder.state == nil else { return }
        composerFocused = false
        Task {
            do {
                try await voiceRecorder.startRecording()
            } catch {
                model.errorText = String(describing: error)
            }
        }
    }

    private func sendVoiceRecording() {
        guard voiceRecorder.state != nil, !voiceSendInFlight else { return }
        voiceSendInFlight = true
        Task {
            do {
                let url = try await voiceRecorder.stopRecording()
                defer {
                    try? FileManager.default.removeItem(at: url)
                    voiceSendInFlight = false
                }
                let data = try await Task.detached(priority: .userInitiated) {
                    try Data(contentsOf: url)
                }.value
                let attachment = try VoiceRecordingAttachment.outboundAttachment(data: data)
                model.sendAttachments(
                    roomID: roomID,
                    attachments: [attachment],
                    replyTo: replyDraftMessage
                ) {
                    replyDraftMessage = nil
                }
            } catch {
                voiceRecorder.cancelRecording()
                voiceSendInFlight = false
                model.errorText = String(describing: error)
            }
        }
    }

    private func cancelVoiceRecording() {
        voiceRecorder.cancelRecording()
        voiceSendInFlight = false
    }

    private func toggleVoiceRecordingPause() {
        guard let recording = voiceRecorder.state else { return }
        do {
            switch recording.phase {
            case .recording:
                voiceRecorder.pauseRecording()
            case .paused:
                try voiceRecorder.resumeRecording()
            }
        } catch {
            model.errorText = String(describing: error)
        }
    }

    private func stageFileURLs(_ urls: [URL]) {
        guard !urls.isEmpty else { return }
        Task {
            do {
                let staged = try await Task.detached(priority: .userInitiated) {
                    try urls.map { try StagedComposerAttachment(fileURL: $0) }
                }.value
                appendStagedAttachments(staged)
            } catch {
                model.errorText = String(describing: error)
            }
        }
    }

    private func stagePhotoItems(_ items: [PhotosPickerItem]) {
        guard !items.isEmpty else { return }
        Task {
            do {
                var staged: [StagedComposerAttachment] = []
                staged.reserveCapacity(items.count)
                for item in items {
                    if let attachment = try await StagedComposerAttachment(photoItem: item) {
                        staged.append(attachment)
                    }
                }
                appendStagedAttachments(staged)
            } catch {
                model.errorText = String(describing: error)
            }
            selectedPhotoItems = []
        }
    }

    private func appendStagedAttachments(_ attachments: [StagedComposerAttachment]) {
        guard !attachments.isEmpty else { return }
        let remainingSlots = max(0, maxStagedComposerAttachments - stagedAttachments.count)
        guard remainingSlots > 0 else {
            model.errorText = "Attachment limit is \(maxStagedComposerAttachments) files."
            return
        }
        let accepted = Array(attachments.prefix(remainingSlots))
        stagedAttachments.append(contentsOf: accepted)
        if accepted.count < attachments.count {
            model.errorText = "Attachment limit is \(maxStagedComposerAttachments) files."
        }
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

private struct PendingRoomView: View {
    let room: AppRoomSummary
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "lock.open")
                .font(.largeTitle)
                .foregroundStyle(.secondary)
            Text(room.userStatusText)
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
            Text(room.userStatusText)
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
                    if let status = model.developerRuntimeStatus {
                        LabeledContent("Runtime Status", value: status)
                    }
                    if let notice = model.userNoticeText {
                        LabeledContent("Last Notice", value: notice)
                    }
                    if let errorText = model.developerErrorText {
                        VStack(alignment: .leading, spacing: 6) {
                            Text("Last Error")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Text(errorText)
                                .font(.caption)
                                .textSelection(.enabled)
                        }
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

private struct NoticeBar: View {
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

private extension AppRoomSummary {
    var userStatusText: String {
        switch state {
        case .connected:
            return "Connected"
        case .waitingForApproval:
            if status.localizedCaseInsensitiveContains("PIN") {
                return "Enter the invite PIN"
            }
            return "Waiting for approval"
        case .joining:
            return "Joining"
        case .needsAttention:
            return "Needs attention"
        case .offline:
            return "Offline"
        }
    }
}
