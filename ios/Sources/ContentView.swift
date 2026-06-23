import CoreImage.CIFilterBuiltins
import Photos
import PhotosUI
import SwiftUI
import UIKit
import UniformTypeIdentifiers

private enum AppSheet: Identifiable {
    case scan
    case invite
    case settings

    var id: String {
        switch self {
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
    @State private var selectedTab: AppTab = .home
    @State private var sheet: AppSheet?
    @State private var chatPath: [String] = []
    @State private var lastAppliedSelectedRoomID: String?
    @State private var scheduledRoomRouteID: String?

    var body: some View {
        Group {
            if model.requiresNostrLogin {
                NostrLoginView(model: model)
            } else {
                authenticatedShell
            }
        }
        .sheet(item: $sheet) { destination in
            switch destination {
            case .scan:
                ScanSheet(model: model)
            case .invite:
                InviteSheet(invite: model.state?.activeInvite)
            case .settings:
                SettingsSheet(model: model)
            }
        }
        .task {
            guard !model.requiresNostrLogin else { return }
            model.start()
            lastAppliedSelectedRoomID = model.state?.selectedRoomId
        }
        .onChange(of: model.requiresNostrLogin) { _, requiresLogin in
            if requiresLogin {
                lastAppliedSelectedRoomID = nil
                scheduledRoomRouteID = nil
                schedulePathUpdate([])
            } else {
                lastAppliedSelectedRoomID = model.state?.selectedRoomId
            }
        }
        .onChange(of: model.state?.selectedRoomId) { _, selectedRoomID in
            guard !model.requiresNostrLogin else { return }
            routeSelectedRoomIfNeeded(selectedRoomID)
        }
    }

    @ViewBuilder
    private var authenticatedShell: some View {
        switch selectedTab {
        case .home:
            homeStack
        case .chats, .people, .agents:
            tabbedShell
        }
    }

    private var tabbedShell: some View {
        TabView(selection: $selectedTab) {
            chatsStack
                .tabItem {
                    Label(AppTab.chats.title, systemImage: AppTab.chats.systemImage)
                }
                .tag(AppTab.chats)

            peopleStack
                .tabItem {
                    Label(AppTab.people.title, systemImage: AppTab.people.systemImage)
                }
                .tag(AppTab.people)

            agentsStack
                .tabItem {
                    Label(AppTab.agents.title, systemImage: AppTab.agents.systemImage)
                }
                .tag(AppTab.agents)

            Color.clear
                .accessibilityHidden(true)
                .tabItem {
                    Label(AppTab.home.title, systemImage: AppTab.home.systemImage)
                }
                .tag(AppTab.home)
        }
    }

    private var chatsStack: some View {
        NavigationStack(path: $chatPath) {
            RoomListView(
                model: model,
                present: { destination in
                    sheet = destination
                },
                open: { room in
                    model.openRoom(room)
                    routeSelectedRoom(room.roomId)
                }
            )
            .navigationDestination(for: String.self) { roomID in
                RoomThreadView(model: model, roomID: roomID) {
                    sheet = .invite
                }
            }
        }
    }

    private var peopleStack: some View {
        NavigationStack {
            PeopleView(
                model: model,
                openRoom: { room in
                    selectedTab = .chats
                    model.openRoom(room)
                    routeSelectedRoom(room.roomId)
                },
                showSettings: {
                    sheet = .settings
                }
            )
        }
    }

    private var agentsStack: some View {
        NavigationStack {
            AgentsView(
                model: model,
                openRoom: { room in
                    selectedTab = .chats
                    model.openRoom(room)
                    routeSelectedRoom(room.roomId)
                },
                showSettings: {
                    sheet = .settings
                }
            )
        }
    }

    private var homeStack: some View {
        NavigationStack {
            HomeView(
                model: model,
                openChats: {
                    selectedTab = .chats
                },
                openAgents: {
                    selectedTab = .agents
                },
                openRoom: { room in
                    model.openRoom(room)
                    routeSelectedRoom(room.roomId)
                },
                showScan: {
                    sheet = .scan
                },
                showSettings: {
                    sheet = .settings
                }
            )
        }
    }

    private func routeSelectedRoomIfNeeded(_ selectedRoomID: String?) {
        guard let selectedRoomID else {
            lastAppliedSelectedRoomID = nil
            scheduledRoomRouteID = nil
            schedulePathUpdate([])
            return
        }
        guard selectedRoomID != lastAppliedSelectedRoomID else { return }
        lastAppliedSelectedRoomID = selectedRoomID
        scheduledRoomRouteID = selectedRoomID
        selectedTab = .chats
        schedulePathUpdate([selectedRoomID])
    }

    private func routeSelectedRoom(_ selectedRoomID: String) {
        lastAppliedSelectedRoomID = selectedRoomID
        scheduledRoomRouteID = selectedRoomID
        selectedTab = .chats
        schedulePathUpdate([selectedRoomID])
    }

    private func schedulePathUpdate(_ nextPath: [String]) {
        Task { @MainActor in
            if let expectedRouteID = nextPath.last,
               scheduledRoomRouteID != expectedRouteID
            {
                return
            }
            if nextPath.isEmpty, scheduledRoomRouteID != nil {
                return
            }
            guard chatPath != nextPath else { return }
            chatPath = nextPath
        }
    }
}

private struct RoomListView: View {
    @ObservedObject var model: AppModel
    let present: (AppSheet) -> Void
    let open: (AppRoomSummary) -> Void
    @State private var searchText = ""

    private var filteredRooms: [AppRoomSummary] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !query.isEmpty else { return model.rooms }
        return model.rooms.filter { room in
            room.displayName.lowercased().contains(query)
                || room.lastMessagePreview.lowercased().contains(query)
                || room.userStatusText.lowercased().contains(query)
        }
    }

    var body: some View {
        List {
            if model.rooms.isEmpty {
                ContentUnavailableView("No chats yet", systemImage: "bubble.left.and.text.bubble")
                    .padding(.vertical, 28)
                    .frame(maxWidth: .infinity)
                    .listRowSeparator(.hidden)
            } else if filteredRooms.isEmpty {
                ContentUnavailableView("No matching chats", systemImage: "magnifyingglass")
                    .padding(.vertical, 28)
                    .frame(maxWidth: .infinity)
                    .listRowSeparator(.hidden)
            } else {
                ForEach(filteredRooms, id: \.roomId) { room in
                    Button {
                        open(room)
                    } label: {
                        RoomRow(room: room)
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("RoomRow-\(room.roomId)")
                }
            }
        }
        .listStyle(.plain)
        .navigationTitle("Chats")
        .toolbar {
            ShellToolbarActions(showSettings: { present(.settings) })
        }
        .searchable(
            text: $searchText,
            placement: .navigationBarDrawer(displayMode: .automatic),
            prompt: "Search chats"
        )
        .safeAreaInset(edge: .bottom) {
            NoticeBar(text: model.userNoticeText)
        }
    }
}

private struct RoomRow: View {
    let room: AppRoomSummary

    var body: some View {
        HStack(spacing: 12) {
            RoomAvatar(room: room)

            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text(room.displayName)
                        .font(.body.weight(.semibold))
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
        switch room.state {
        case .connected:
            return "No messages yet"
        case .waitingForApproval, .joining, .unavailableOnDevice:
            return room.userStatusText
        }
    }
}

private struct RoomAvatar: View {
    let room: AppRoomSummary

    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            Circle()
                .fill(Color(.tertiarySystemFill))
            Text(initial)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.secondary)

            Circle()
                .fill(room.state.tint)
                .frame(width: 10, height: 10)
                .overlay(Circle().stroke(Color(.systemBackground), lineWidth: 2))
        }
        .frame(width: 40, height: 40)
        .accessibilityHidden(true)
    }

    private var initial: String {
        let trimmed = room.displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let first = trimmed.first else { return "#" }
        return String(first).uppercased()
    }
}

private struct RoomOptionsSheet: View {
    @Environment(\.dismiss) private var dismiss
    let showRoomDetails: () -> Void
    let showMediaGallery: () -> Void

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Button {
                        dismiss()
                        showRoomDetails()
                    } label: {
                        SettingsRowLabel(
                            title: "Room info",
                            subtitle: nil,
                            systemImage: "info.circle"
                        )
                    }

                    Button {
                        dismiss()
                        showMediaGallery()
                    } label: {
                        SettingsRowLabel(
                            title: "Media gallery",
                            subtitle: nil,
                            systemImage: "photo.on.rectangle.angled"
                        )
                    }
                }
            }
            .navigationTitle("Room")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") {
                        dismiss()
                    }
                }
            }
        }
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
    @State private var reactionPickerContext: ReactionPickerContext?
    @State private var composerFocused = false
    @State private var imagePreviewSelection: ChatImagePreviewSelection?
    @State private var videoPreviewItem: ChatAttachmentPreviewItem?
    @State private var documentPreviewItem: ChatAttachmentPreviewItem?
    @State private var showMediaGallery = false
    @State private var showRoomDetails = false
    @State private var showRoomOptions = false
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

    private var mediaGalleryItems: [ChatMediaGalleryItem] {
        guard let gallery = model.state?.mediaGallery,
              gallery.roomId == roomID
        else {
            return []
        }
        return gallery.items
    }

    private var roomDetails: AppRoomDetailsState? {
        guard let details = model.state?.roomDetails,
              details.roomId == roomID
        else {
            return nil
        }
        return details
    }

    private var latestMessageID: String? {
        projection.messages.last?.messageId
    }

    private var transcriptRows: [ChatTimelineRow] {
        let members = typingMembers(for: roomID)
        guard !members.isEmpty else { return projection.rows }
        return projection.rows + [.typing(members)]
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
                    onMoreReaction: {
                        let message = focusedMessage
                        dismissFocusedMessage()
                        DispatchQueue.main.async {
                            reactionPickerContext = ReactionPickerContext(message: message)
                        }
                    },
                    onReply: {
                        replyDraftMessage = focusedMessage
                        composerFocused = true
                        dismissFocusedMessage()
                    },
                    onRetry: {
                        model.retry(focusedMessage)
                        dismissFocusedMessage()
                    },
                    onCopy: {
                        UIPasteboard.general.string = messageClipboardText(focusedMessage)
                        dismissFocusedMessage()
                    },
                    onSaveMedia: saveableImageAttachmentURLs(in: focusedMessage).isEmpty ? nil : {
                        saveImagesFromFocusedMessage(focusedMessage)
                        dismissFocusedMessage()
                    },
                    saveMediaTitle: saveMediaActionTitle(
                        imageCount: saveableImageAttachmentURLs(in: focusedMessage).count
                    ),
                    canReact: messageCanUseSentActions(focusedMessage),
                    canReply: messageCanUseSentActions(focusedMessage),
                    canRetry: messageCanRetry(focusedMessage),
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
                        showRoomOptions = true
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                    .accessibilityLabel("Room options")
                    .accessibilityIdentifier("RoomOptionsButton")
                }
            }
        }
        .sheet(isPresented: $showRoomOptions) {
            RoomOptionsSheet(
                showRoomDetails: {
                    showRoomOptions = false
                    Task { @MainActor in
                        showRoomDetails = true
                    }
                },
                showMediaGallery: {
                    showRoomOptions = false
                    Task { @MainActor in
                        showMediaGallery = true
                    }
                }
            )
            .presentationDetents([.medium])
        }
        .navigationDestination(isPresented: $showRoomDetails) {
            RoomDetailsView(
                details: roomDetails,
                mediaItems: mediaGalleryItems,
                onDownloadAttachment: { item in
                    model.downloadAttachment(
                        roomID: roomID,
                        messageID: item.messageId,
                        attachment: item.attachment
                    )
                },
                onCreateInvite: {
                    if let room {
                        _ = model.createInvite(for: room)
                        showInvite()
                    }
                },
                onRefreshDevices: {
                    model.refreshDevices()
                },
                onRevokeDevice: { device in
                    model.revokeDevice(device)
                }
            )
        }
        .navigationDestination(isPresented: $showMediaGallery) {
            ChatMediaGalleryView(
                roomTitle: room?.displayName ?? "this chat",
                items: mediaGalleryItems,
                onDownloadAttachment: { item in
                    model.downloadAttachment(
                        roomID: roomID,
                        messageID: item.messageId,
                        attachment: item.attachment
                    )
                }
            )
        }
        .onAppear {
            if let room {
                model.openRoom(room)
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
        .sheet(item: $reactionPickerContext) { context in
            ReactionEmojiPickerSheet { emoji in
                model.react(to: context.message, emoji: emoji)
            }
            .presentationDetents([.medium, .large])
        }
        .onDisappear {
            model.setTyping(roomID: roomID, isTyping: false)
            dismissFocusedMessage(animated: false)
            voiceRecorder.cancelRecording()
        }
        .onChange(of: selectedPhotoItems) { _, items in
            stagePhotoItems(items)
        }
        .onChange(of: model.outboundText) { _, text in
            updateTypingIntent(text)
        }
    }

    @ViewBuilder
    private func messageSurface(room: AppRoomSummary) -> some View {
        switch room.state {
        case .connected:
            transcriptView(room: room) {
                composerAccessory
            }
        case .waitingForApproval:
            PendingRoomView(room: room, model: model)
        case .joining:
            ProgressView(room.userStatusText)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .unavailableOnDevice:
            UnavailableOnDeviceView(room: room)
        }
    }

    private func transcriptView<AccessoryContent: View>(
        room: AppRoomSummary,
        @ViewBuilder accessoryContent: () -> AccessoryContent
    ) -> some View {
        ChatTranscriptView(
            roomID: room.roomId,
            rows: transcriptRows,
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
            onRetryMessage: { message in
                model.retry(message)
            },
            onLongPressMessage: { message, frame in
                presentFocusedMessage(message, frame: frame)
            },
            accessoryContent: accessoryContent(),
            isInputFocused: room.state == .connected && composerFocused,
            canLoadOlder: room.canLoadOlder,
            onLoadOlderMessages: { beforeMessageID in
                model.loadOlderMessages(roomID: room.roomId, beforeMessageID: beforeMessageID)
            },
            followsBottom: $followsBottom
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(.systemGroupedBackground))
        .accessibilityLabel("Messages")
    }

    private func messageCanRetry(_ message: ChatMessage) -> Bool {
        guard message.isMine, let outboundDelivery = message.outboundDelivery else { return false }
        if case .failed = outboundDelivery.serverDelivery {
            return true
        }
        return false
    }

    private func messageCanUseSentActions(_ message: ChatMessage) -> Bool {
        guard let outboundDelivery = message.outboundDelivery else { return true }
        if case .delivered = outboundDelivery.serverDelivery {
            return true
        }
        return false
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

    private func saveImagesFromFocusedMessage(_ message: ChatMessage) {
        let urls = saveableImageAttachmentURLs(in: message)
        guard !urls.isEmpty else {
            model.errorText = "No downloaded photos to save."
            return
        }

        Task {
            do {
                _ = try await PhotoLibraryImageSaver.saveImageFiles(urls)
                model.errorText = nil
            } catch {
                model.errorText = String(describing: error)
            }
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

    private func typingMembers(for roomID: String) -> [AppTypingMember] {
        model.state?.typingMembers.filter { $0.roomId == roomID } ?? []
    }

    private func updateTypingIntent(_ text: String) {
        guard room?.state == .connected else { return }
        let isTyping = !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        model.setTyping(roomID: roomID, isTyping: isTyping)
    }

    private func sendComposerDraft() {
        if stagedAttachments.isEmpty {
            if model.send(roomID: roomID, replyTo: replyDraftMessage) {
                model.setTyping(roomID: roomID, isTyping: false)
                replyDraftMessage = nil
            }
            return
        }

        let outbound = stagedAttachments.map(\.outboundAttachment)
        model.sendAttachments(roomID: roomID, attachments: outbound, replyTo: replyDraftMessage) {
            model.setTyping(roomID: roomID, isTyping: false)
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
        let caption = voiceRecordingCaption(voiceRecorder.state)
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
                    replyTo: replyDraftMessage,
                    captionOverride: caption
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
    let onMoreReaction: () -> Void
    let onReply: () -> Void
    let onRetry: () -> Void
    let onCopy: () -> Void
    let onSaveMedia: (() -> Void)?
    let saveMediaTitle: String?
    let canReact: Bool
    let canReply: Bool
    let canRetry: Bool
    let canCopy: Bool

    var body: some View {
        GeometryReader { geometry in
            ZStack {
                Color.black.opacity(0.18)
                    .ignoresSafeArea()
                    .contentShape(Rectangle())
                    .onTapGesture(perform: onDismiss)

                VStack(alignment: message.isMine ? .trailing : .leading, spacing: 10) {
                    if canReact {
                        FocusedReactionBar(onReact: onReact, onMore: onMoreReaction)
                    }

                    FocusedChatMessageCard(
                        message: message,
                        replyTarget: replyTarget
                    )
                    .frame(maxWidth: min(geometry.size.width * 0.82, 360))

                    if actionsVisible {
                        FocusedMessageActionCard(
                            canReply: canReply,
                            canRetry: canRetry,
                            canCopy: canCopy,
                            onReply: onReply,
                            onRetry: onRetry,
                            onCopy: onCopy,
                            onSaveMedia: onSaveMedia,
                            saveMediaTitle: saveMediaTitle
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
        let reactionBarSpace: CGFloat = canReact ? 58 : 0
        let idealTop = localAnchorY - reactionBarSpace
        let maxTop = max(12, geometry.size.height * 0.58)
        return min(max(idealTop, 12), maxTop)
    }
}

private struct FocusedReactionBar: View {
    let onReact: (String) -> Void
    let onMore: () -> Void

    var body: some View {
        HStack(spacing: 4) {
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
                .accessibilityIdentifier("ReactionQuickButton-\(reactionEmojiStableID(emoji))")
            }

            Button {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                onMore()
            } label: {
                Image(systemName: "plus")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .frame(width: 32, height: 32)
                    .background(Color(uiColor: .tertiarySystemGroupedBackground), in: Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("More reactions")
            .accessibilityIdentifier("ReactionMoreButton")
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 4)
        .background(.regularMaterial, in: Capsule())
        .shadow(color: .black.opacity(0.14), radius: 14, x: 0, y: 6)
    }
}

private struct FocusedMessageActionCard: View {
    let canReply: Bool
    let canRetry: Bool
    let canCopy: Bool
    let onReply: () -> Void
    let onRetry: () -> Void
    let onCopy: () -> Void
    let onSaveMedia: (() -> Void)?
    let saveMediaTitle: String?

    var body: some View {
        VStack(spacing: 0) {
            if canRetry {
                Button {
                    UIImpactFeedbackGenerator(style: .light).impactOccurred()
                    onRetry()
                } label: {
                    Label("Retry", systemImage: "arrow.clockwise")
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 10)
                }
                .buttonStyle(.plain)

                Divider()
            }

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
            .disabled(!canReply)

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

            if let onSaveMedia, let saveMediaTitle {
                Divider()

                Button {
                    UIImpactFeedbackGenerator(style: .light).impactOccurred()
                    onSaveMedia()
                } label: {
                    Label(saveMediaTitle, systemImage: "square.and.arrow.down")
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 10)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(saveMediaTitle)
            }
        }
        .frame(width: 176)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .shadow(color: .black.opacity(0.14), radius: 14, x: 0, y: 6)
    }
}

private let focusedReactionEmojis = ["❤️", "👍", "👎", "😂", "😮", "😢"]

private struct ReactionPickerContext: Identifiable {
    let message: ChatMessage

    var id: String {
        message.messageId
    }
}

struct ReactionEmojiSection: Equatable, Identifiable {
    let title: String
    let emojis: [ReactionEmojiChoice]

    var id: String {
        title
    }
}

struct ReactionEmojiChoice: Equatable, Identifiable {
    let emoji: String
    let name: String
    let keywords: [String]

    var id: String {
        emoji
    }

    func matches(_ query: String) -> Bool {
        let normalized = query
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
        guard !normalized.isEmpty else { return true }
        if name.lowercased().contains(normalized) {
            return true
        }
        return keywords.contains { keyword in
            keyword.lowercased().contains(normalized)
        }
    }
}

enum ReactionEmojiCatalog {
    static let recent = [
        choice("❤️", "Red heart", "love", "heart"),
        choice("👍", "Thumbs up", "yes", "agree", "like"),
        choice("👎", "Thumbs down", "no", "disagree"),
        choice("😂", "Face with tears of joy", "laugh", "funny"),
        choice("😮", "Surprised face", "wow", "shock"),
        choice("😢", "Crying face", "sad"),
        choice("🔥", "Fire", "hot", "lit"),
        choice("🎉", "Party popper", "celebrate", "party"),
        choice("👀", "Eyes", "looking", "watching"),
        choice("🙏", "Folded hands", "thanks", "please"),
        choice("💯", "Hundred points", "perfect", "agree"),
        choice("🤔", "Thinking face", "think", "hmm"),
    ]

    static let sections = [
        ReactionEmojiSection(title: "Recent", emojis: recent),
        ReactionEmojiSection(title: "Smileys", emojis: [
            choice("😀", "Grinning face", "smile"),
            choice("😃", "Smiling face", "happy"),
            choice("😄", "Smiling eyes", "happy"),
            choice("😁", "Beaming face", "grin"),
            choice("😆", "Squinting face", "laugh"),
            choice("😅", "Grinning sweat", "relief"),
            choice("🤣", "Rolling on the floor laughing", "laugh", "funny"),
            choice("😂", "Face with tears of joy", "laugh", "funny"),
            choice("🙂", "Slightly smiling face", "smile"),
            choice("🙃", "Upside-down face", "silly"),
            choice("😉", "Winking face", "wink"),
            choice("😊", "Smiling face with smiling eyes", "warm"),
            choice("😇", "Smiling face with halo", "angel"),
            choice("😍", "Heart eyes", "love"),
            choice("😘", "Face blowing a kiss", "kiss"),
            choice("😋", "Yum face", "tasty"),
            choice("😜", "Winking tongue", "joke"),
            choice("🤔", "Thinking face", "think", "hmm"),
            choice("🤨", "Raised eyebrow", "skeptical"),
            choice("😐", "Neutral face", "neutral"),
            choice("😑", "Expressionless face", "blank"),
            choice("😶", "Face without mouth", "quiet"),
            choice("😏", "Smirking face", "smirk"),
            choice("😒", "Unamused face", "unimpressed"),
            choice("🙄", "Face with rolling eyes", "eyeroll"),
            choice("😬", "Grimacing face", "grimace"),
            choice("😮", "Surprised face", "wow", "shock"),
            choice("😯", "Hushed face", "surprised"),
            choice("😲", "Astonished face", "amazed"),
            choice("😴", "Sleeping face", "sleep"),
            choice("🤤", "Drooling face", "want"),
            choice("😪", "Sleepy face", "tired"),
            choice("😵", "Dizzy face", "dizzy"),
            choice("🤯", "Exploding head", "mind blown"),
            choice("🥳", "Partying face", "party", "celebrate"),
            choice("🥺", "Pleading face", "please"),
            choice("😭", "Loudly crying face", "cry"),
            choice("😤", "Face with steam", "frustrated"),
            choice("😡", "Pouting face", "angry"),
        ]),
        ReactionEmojiSection(title: "Gestures", emojis: [
            choice("👋", "Waving hand", "hello", "bye"),
            choice("👌", "OK hand", "ok"),
            choice("✌️", "Victory hand", "peace"),
            choice("🤞", "Crossed fingers", "hope"),
            choice("🤟", "Love-you gesture", "love"),
            choice("🤘", "Sign of the horns", "rock"),
            choice("👍", "Thumbs up", "yes", "agree", "like"),
            choice("👎", "Thumbs down", "no", "disagree"),
            choice("👏", "Clapping hands", "applause"),
            choice("🙌", "Raising hands", "celebrate"),
            choice("🙏", "Folded hands", "thanks", "please"),
            choice("🤝", "Handshake", "deal", "agree"),
            choice("💪", "Flexed biceps", "strong"),
            choice("🫡", "Saluting face", "salute"),
        ]),
        ReactionEmojiSection(title: "Hearts", emojis: [
            choice("❤️", "Red heart", "love", "heart"),
            choice("🧡", "Orange heart", "heart"),
            choice("💛", "Yellow heart", "heart"),
            choice("💚", "Green heart", "heart"),
            choice("💙", "Blue heart", "heart"),
            choice("💜", "Purple heart", "heart"),
            choice("🖤", "Black heart", "heart"),
            choice("🤍", "White heart", "heart"),
            choice("💔", "Broken heart", "heartbreak"),
            choice("💕", "Two hearts", "love"),
            choice("💖", "Sparkling heart", "love"),
            choice("💝", "Heart with ribbon", "gift"),
        ]),
        ReactionEmojiSection(title: "Symbols", emojis: [
            choice("⭐️", "Star", "favorite"),
            choice("✨", "Sparkles", "sparkle"),
            choice("🔥", "Fire", "hot", "lit"),
            choice("💯", "Hundred points", "perfect", "agree"),
            choice("🎉", "Party popper", "celebrate", "party"),
            choice("✅", "Check mark", "done", "yes"),
            choice("❌", "Cross mark", "no", "cancel"),
            choice("⚠️", "Warning", "caution"),
            choice("🚀", "Rocket", "ship", "launch"),
            choice("💡", "Light bulb", "idea"),
            choice("👑", "Crown", "king", "queen"),
        ]),
    ]

    static func filteredSections(searchText: String) -> [ReactionEmojiSection] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return sections }

        var seen = Set<String>()
        let matches = sections
            .flatMap(\.emojis)
            .filter { choice in
                guard choice.matches(query), !seen.contains(choice.emoji) else { return false }
                seen.insert(choice.emoji)
                return true
            }
        return matches.isEmpty ? [] : [ReactionEmojiSection(title: "Results", emojis: matches)]
    }

    private static func choice(
        _ emoji: String,
        _ name: String,
        _ keywords: String...
    ) -> ReactionEmojiChoice {
        ReactionEmojiChoice(emoji: emoji, name: name, keywords: keywords)
    }
}

private struct ReactionEmojiPickerSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var searchText = ""
    let onSelect: (String) -> Void

    private var sections: [ReactionEmojiSection] {
        ReactionEmojiCatalog.filteredSections(searchText: searchText)
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 18) {
                    if sections.isEmpty {
                        ContentUnavailableView("No matching emoji", systemImage: "magnifyingglass")
                            .frame(maxWidth: .infinity)
                            .padding(.top, 44)
                    } else {
                        ForEach(sections) { section in
                            ReactionEmojiSectionView(section: section) { emoji in
                                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                                onSelect(emoji)
                                dismiss()
                            }
                        }
                    }
                }
                .padding(.horizontal, 18)
                .padding(.vertical, 16)
            }
            .navigationTitle("Reactions")
            .navigationBarTitleDisplayMode(.inline)
            .searchable(text: $searchText, prompt: "Search emoji")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        dismiss()
                    }
                }
            }
        }
    }
}

private struct ReactionEmojiSectionView: View {
    let section: ReactionEmojiSection
    let onSelect: (String) -> Void

    private let columns = Array(
        repeating: GridItem(.flexible(minimum: 40, maximum: 52), spacing: 8),
        count: 6
    )

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(section.title)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.secondary)

            LazyVGrid(columns: columns, spacing: 8) {
                ForEach(section.emojis) { choice in
                    Button {
                        onSelect(choice.emoji)
                    } label: {
                        Text(choice.emoji)
                            .font(.system(size: 30))
                            .frame(width: 44, height: 44)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(choice.name)
                    .accessibilityIdentifier("ReactionEmojiButton-\(reactionEmojiStableID(choice.emoji))")
                }
            }
        }
    }
}

private func reactionEmojiStableID(_ emoji: String) -> String {
    let scalars = emoji.unicodeScalars
        .map { String($0.value, radix: 16, uppercase: true) }
        .joined(separator: "-")
    return scalars.isEmpty ? "empty" : scalars
}

private func messageClipboardText(_ message: ChatMessage) -> String {
    let display = message.displayContent.trimmingCharacters(in: .whitespacesAndNewlines)
    if !display.isEmpty {
        return display
    }
    return message.text.trimmingCharacters(in: .whitespacesAndNewlines)
}

func saveableImageAttachmentURLs(in message: ChatMessage) -> [URL] {
    message.media
        .filter { $0.kind == .image }
        .compactMap(attachmentLocalURL)
}

func saveMediaActionTitle(imageCount: Int) -> String? {
    guard imageCount > 0 else { return nil }
    return imageCount == 1 ? "Save Photo" : "Save Photos"
}

enum PhotoLibraryImageSaveError: Error, CustomStringConvertible {
    case noImages
    case notAuthorized(PHAuthorizationStatus)
    case saveFailed

    var description: String {
        switch self {
        case .noImages:
            "No downloaded photos to save."
        case .notAuthorized:
            "Photo library access was not granted."
        case .saveFailed:
            "Photo library save did not complete."
        }
    }
}

enum PhotoLibraryImageSaver {
    static func saveImageFiles(_ urls: [URL]) async throws -> Int {
        let existingURLs = urls.filter { FileManager.default.fileExists(atPath: $0.path) }
        guard !existingURLs.isEmpty else {
            throw PhotoLibraryImageSaveError.noImages
        }

        let status = await requestAddOnlyAuthorization()
        guard status == .authorized || status == .limited else {
            throw PhotoLibraryImageSaveError.notAuthorized(status)
        }

        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            PHPhotoLibrary.shared().performChanges {
                for url in existingURLs {
                    PHAssetChangeRequest.creationRequestForAssetFromImage(atFileURL: url)
                }
            } completionHandler: { success, error in
                if let error {
                    continuation.resume(throwing: error)
                } else if success {
                    continuation.resume()
                } else {
                    continuation.resume(throwing: PhotoLibraryImageSaveError.saveFailed)
                }
            }
        }

        return existingURLs.count
    }

    private static func requestAddOnlyAuthorization() async -> PHAuthorizationStatus {
        let current = PHPhotoLibrary.authorizationStatus(for: .addOnly)
        guard current == .notDetermined else { return current }
        return await withCheckedContinuation { continuation in
            PHPhotoLibrary.requestAuthorization(for: .addOnly) { status in
                continuation.resume(returning: status)
            }
        }
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

private struct UnavailableOnDeviceView: View {
    let room: AppRoomSummary

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "exclamationmark.triangle")
                .font(.largeTitle)
                .foregroundStyle(.secondary)
            Text(room.userStatusText)
                .font(.body)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .padding()
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct ScanSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var model: AppModel
    @State private var showingCameraScanner = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    if QRCodeScannerSheet.canUseCamera {
                        Button {
                            showingCameraScanner = true
                        } label: {
                            Label("Scan with Camera", systemImage: "qrcode.viewfinder")
                        }
                    }

                    Button {
                        model.scanDraft = UIPasteboard.general.string ?? ""
                    } label: {
                        Label("Paste", systemImage: "doc.on.clipboard")
                    }

                    TextField("Invite URL or npub", text: $model.scanDraft, axis: .vertical)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .lineLimit(3...6)
                        .accessibilityLabel("Invite URL or npub")
                } header: {
                    Text("Invite or Profile Code")
                }

                Section {
                    Label("Create New Finite Agent", systemImage: "sparkles")
                        .foregroundStyle(.secondary)
                    Label("Clawi, Maple, Codex, and Claude sessions", systemImage: "ellipsis.bubble")
                        .foregroundStyle(.secondary)
                } header: {
                    Text("Agents")
                } footer: {
                    Text("Coming soon")
                }

                if let profile = model.activeProfile {
                    Section("Profile") {
                        ProfileRow(profile: profile)
                    }
                }
            }
            .navigationTitle("Scan")
            .sheet(isPresented: $showingCameraScanner) {
                QRCodeScannerSheet { value in
                    model.scanDraft = value
                    if model.scanTarget() {
                        dismiss()
                    }
                }
            }
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
    @State private var showingMyProfile = false
    @State private var showingScan = false
    @State private var confirmingSignOut = false

    var body: some View {
        NavigationStack {
            Form {
                Section("Profile") {
                    Button {
                        showingMyProfile = true
                    } label: {
                        HStack(spacing: 12) {
                            if let profile = model.activeProfile {
                                ProfileAvatar(profile: profile)
                            } else {
                                Image(systemName: "person.crop.circle")
                                    .font(.title2)
                                    .foregroundStyle(.secondary)
                                    .frame(width: 40, height: 40)
                            }

                            VStack(alignment: .leading, spacing: 3) {
                                Text(model.activeProfile?.displayName ?? "My Profile")
                                    .foregroundStyle(.primary)
                                Text(profileSubtitle)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                            }
                        }
                    }
                    .buttonStyle(.plain)
                }

                Section("Server") {
                    Button {
                        showingScan = true
                    } label: {
                        SettingsRowLabel(
                            title: "Scan code",
                            subtitle: "Invite, profile, or agent code",
                            systemImage: "qrcode.viewfinder"
                        )
                    }
                    .buttonStyle(.plain)

                    SettingsRowLabel(
                        title: "Configured server",
                        subtitle: model.serverURL,
                        systemImage: "server.rack"
                    )
                }

                Section {
                    DisclosureGroup {
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
                            LabeledContent("Persistence", value: model.developerPersistenceSummary)
                        }
                        if let status = model.developerRuntimeStatus {
                            LabeledContent("Runtime Status", value: status)
                        }
                        if let notice = model.userNoticeText {
                            LabeledContent("Last Notice", value: notice)
                        }
                        if let storePath = model.runtimeStorePath {
                            VStack(alignment: .leading, spacing: 6) {
                                Text("Client Store")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                Text(storePath)
                                    .font(.caption)
                                    .textSelection(.enabled)
                            }
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
                        if let state = model.state {
                            if !state.profiles.isEmpty {
                                Text("Profiles")
                                    .font(.caption.weight(.semibold))
                                    .foregroundStyle(.secondary)
                                ForEach(state.profiles, id: \.accountId) { profile in
                                    ProfileRow(profile: profile)
                                }
                            }

                            Text("Devices")
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(.secondary)
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
                                Label("Refresh Devices", systemImage: "arrow.clockwise")
                            }
                            .accessibilityIdentifier("RefreshDevicesButton")
                        }
                        if !model.developerDiagnostics.isEmpty {
                            LabeledContent(
                                "Debug Events",
                                value: "\(model.developerDiagnostics.count)"
                            )
                            HStack {
                                Button {
                                    UIPasteboard.general.string = model.developerDiagnosticsExport
                                } label: {
                                    Label("Copy Logs", systemImage: "doc.on.doc")
                                }
                                ShareLink(item: model.developerDiagnosticsExport) {
                                    Label("Share Logs", systemImage: "square.and.arrow.up")
                                }
                            }
                            ForEach(model.developerDiagnosticsPreview) { entry in
                                VStack(alignment: .leading, spacing: 4) {
                                    Text("\(entry.category) / \(entry.event)")
                                        .font(.caption.weight(.medium))
                                    if !entry.details.isEmpty {
                                        Text(developerDiagnosticDetails(entry.details))
                                            .font(.caption2)
                                            .foregroundStyle(.secondary)
                                            .textSelection(.enabled)
                                    }
                                }
                            }
                        }
                    } label: {
                        SettingsRowLabel(
                            title: "Developer diagnostics",
                            subtitle: "Redacted local copy and share only",
                            systemImage: "doc.text.magnifyingglass"
                        )
                    }

                    Button(role: .destructive) {
                        confirmingSignOut = true
                    } label: {
                        Label("Sign Out and Delete Local Data", systemImage: "rectangle.portrait.and.arrow.right")
                    }
                }
            }
            .navigationTitle("Settings")
            .task {
                model.refreshDevices()
            }
            .sheet(isPresented: $showingMyProfile) {
                MyNostrProfileSheet(identity: model.nostrIdentity, myNpub: model.myNpub)
            }
            .sheet(isPresented: $showingScan) {
                ScanSheet(model: model)
            }
            .confirmationDialog(
                "Delete this device's Finite Chat data?",
                isPresented: $confirmingSignOut,
                titleVisibility: .visible
            ) {
                Button("Delete Everything", role: .destructive) {
                    model.signOutAndDeleteEverything()
                    dismiss()
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("This removes local chats, config, and the saved nsec from this device.")
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

    private var profileSubtitle: String {
        if let npub = model.myNpub {
            return shortenedDisplayNpub(npub)
        }
        return "Signed in on this phone"
    }
}

private struct SettingsRowLabel: View {
    let title: String
    let subtitle: String?
    let systemImage: String

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .foregroundStyle(.primary)
                if let subtitle, !subtitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    Text(subtitle)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
        } icon: {
            Image(systemName: systemImage)
                .foregroundStyle(.secondary)
                .frame(width: 28)
        }
    }
}

private func shortenedDisplayNpub(_ npub: String) -> String {
    guard npub.count > 18 else { return npub }
    return "\(npub.prefix(10))...\(npub.suffix(4))"
}

private func developerDiagnosticDetails(_ details: [String: String]) -> String {
    details
        .sorted { $0.key < $1.key }
        .map { "\($0.key)=\($0.value)" }
        .joined(separator: " ")
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

struct QRCodeView: View {
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

struct NoticeBarPresentation: Equatable {
    let text: String?

    var visibleText: String? {
        guard let text = text?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else {
            return nil
        }
        return text
    }

    var accessibilityIdentifier: String {
        "NoticeBar"
    }
}

struct NoticeBar: View {
    let presentation: NoticeBarPresentation

    init(text: String?) {
        presentation = NoticeBarPresentation(text: text)
    }

    var body: some View {
        if let text = presentation.visibleText {
            Text(text)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal)
                .padding(.vertical, 8)
                .background(.bar)
                .accessibilityIdentifier(presentation.accessibilityIdentifier)
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
        case .unavailableOnDevice:
            .red
        }
    }
}
