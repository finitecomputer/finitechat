import PhotosUI
import SwiftUI

struct RoomDetailsView: View {
    let details: AppRoomDetailsState?
    let mediaItems: [ChatMediaGalleryItem]
    let onDownloadAttachment: (ChatMediaGalleryItem) -> Void
    let onCreateInvite: () -> Void
    let onAddPeople: () -> Void
    let onRefreshDevices: () -> Void
    let onRevokeDevice: (AppDeviceSummary) -> Void
    let onUploadImage: @MainActor (Data, String) async -> String?
    let onSaveMetadata: @MainActor (String, String, String?) async -> Bool

    var body: some View {
        Group {
            if let details {
                List {
                    Section {
                        RoomDetailsHeader(details: details)
                    }

                    Section("Room") {
                        RoomMetadataEditor(
                            details: details,
                            onUploadImage: onUploadImage,
                            onSaveMetadata: onSaveMetadata
                        )
                    }

                    Section {
                        NavigationLink {
                            ChatMediaGalleryView(
                                roomTitle: details.displayName,
                                items: mediaItems,
                                onDownloadAttachment: onDownloadAttachment
                            )
                        } label: {
                            LabeledContent {
                                Text("\(details.mediaItemCount)")
                                    .foregroundStyle(.secondary)
                            } label: {
                                Label("Photos & Videos", systemImage: "photo.on.rectangle.angled")
                            }
                        }
                        .accessibilityIdentifier("RoomDetailsMediaGalleryLink")

                        if details.canCreateInvite {
                            Button {
                                onAddPeople()
                            } label: {
                                Label("Add People", systemImage: "person.badge.plus")
                            }
                            .accessibilityIdentifier("RoomDetailsAddPeopleButton")

                            Button {
                                onCreateInvite()
                            } label: {
                                Label("Invite", systemImage: "qrcode")
                            }
                            .accessibilityIdentifier("RoomDetailsInviteButton")
                        }
                    }

                    Section("People") {
                        if details.members.isEmpty {
                            Text("No people found")
                                .foregroundStyle(.secondary)
                        } else {
                            ForEach(details.members, id: \.detailsListID) { member in
                                RoomDetailsMemberRow(member: member)
                            }
                        }
                    }

                    Section("Your Devices") {
                        if details.devices.isEmpty {
                            Text("No devices found")
                                .foregroundStyle(.secondary)
                        } else {
                            ForEach(details.devices, id: \.detailsListID) { device in
                                RoomDetailsDeviceRow(device: device) {
                                    onRevokeDevice(device)
                                }
                            }
                        }

                        Button {
                            onRefreshDevices()
                        } label: {
                            Label("Refresh", systemImage: "arrow.clockwise")
                        }
                        .accessibilityIdentifier("RoomDetailsRefreshDevicesButton")
                    }
                }
            } else {
                ContentUnavailableView("Room unavailable", systemImage: "exclamationmark.triangle")
            }
        }
        .navigationTitle("Details")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            onRefreshDevices()
        }
    }
}

private struct RoomDetailsHeader: View {
    let details: AppRoomDetailsState

    var body: some View {
        HStack(spacing: 14) {
            ProfileAvatar(displayName: details.displayName, pictureURL: details.picture, size: 52)
            .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 4) {
                Text(details.displayName)
                    .font(.headline)
                    .lineLimit(2)
                Text(details.userStatusText)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                if !details.status.isEmpty, details.status != details.userStatusText.lowercased() {
                    Text(details.status)
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                        .lineLimit(2)
                }
            }
        }
        .padding(.vertical, 8)
        .accessibilityElement(children: .combine)
    }
}

private struct RoomMetadataEditor: View {
    let details: AppRoomDetailsState
    let onUploadImage: @MainActor (Data, String) async -> String?
    let onSaveMetadata: @MainActor (String, String, String?) async -> Bool

    @State private var draftDisplayName: String
    @State private var draftPictureURL: String
    @State private var selectedPhotoItem: PhotosPickerItem?
    @State private var photoPickerPresented = false
    @State private var draftRoomID: String
    @State private var imageUploadInFlight = false
    @State private var saveInFlight = false
    @State private var statusText: String?

    init(
        details: AppRoomDetailsState,
        onUploadImage: @escaping @MainActor (Data, String) async -> String?,
        onSaveMetadata: @escaping @MainActor (String, String, String?) async -> Bool
    ) {
        self.details = details
        self.onUploadImage = onUploadImage
        self.onSaveMetadata = onSaveMetadata
        _draftDisplayName = State(initialValue: details.displayName)
        _draftPictureURL = State(initialValue: details.picture ?? "")
        _draftRoomID = State(initialValue: details.roomId)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 12) {
                ProfileAvatar(displayName: previewDisplayName, pictureURL: normalizedDraftPictureURL, size: 46)
                    .accessibilityHidden(true)

                TextField("Name", text: $draftDisplayName)
                    .textInputAutocapitalization(.words)
                    .autocorrectionDisabled(false)
                    .disabled(saveInFlight)
            }

            HStack(spacing: 12) {
                Button {
                    photoPickerPresented = true
                } label: {
                    if imageUploadInFlight {
                        Label("Uploading Image", systemImage: "hourglass")
                    } else {
                        Label("Choose Image", systemImage: "photo")
                    }
                }
                .disabled(imageUploadInFlight || saveInFlight)
                .photosPicker(
                    isPresented: $photoPickerPresented,
                    selection: $selectedPhotoItem,
                    matching: .images,
                    photoLibrary: .shared()
                )

                if normalizedDraftPictureURL != nil {
                    Button(role: .destructive) {
                        draftPictureURL = ""
                        statusText = nil
                    } label: {
                        Label("Remove Image", systemImage: "trash")
                    }
                    .disabled(imageUploadInFlight || saveInFlight)
                }
            }

            Button {
                saveMetadata()
            } label: {
                if saveInFlight {
                    Label("Saving", systemImage: "hourglass")
                } else {
                    Label("Save Room", systemImage: "checkmark.circle")
                }
            }
            .disabled(!canSave)
            .accessibilityIdentifier("RoomDetailsSaveMetadataButton")

            if let statusText {
                Text(statusText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .onChange(of: selectedPhotoItem) { _, item in
            uploadSelectedPhoto(item)
        }
        .onChange(of: detailsDraftID) { _, _ in
            if draftRoomID != details.roomId {
                resetDrafts(force: true)
            } else if !hasChanges {
                resetDrafts(force: false)
            }
        }
    }

    private var detailsDraftID: String {
        [
            details.roomId,
            details.displayName,
            details.picture ?? "",
        ].joined(separator: "|")
    }

    private var previewDisplayName: String {
        let trimmed = draftDisplayName.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? details.displayName : trimmed
    }

    private var normalizedDraftPictureURL: String? {
        let trimmed = draftPictureURL.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private var hasChanges: Bool {
        draftDisplayName.trimmingCharacters(in: .whitespacesAndNewlines) != details.displayName
            || (normalizedDraftPictureURL ?? "") != (details.picture ?? "")
    }

    private var canSave: Bool {
        !saveInFlight
            && !imageUploadInFlight
            && !draftDisplayName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && hasChanges
    }

    private func resetDrafts(force: Bool) {
        guard force || (!saveInFlight && !imageUploadInFlight) else { return }
        draftRoomID = details.roomId
        draftDisplayName = details.displayName
        draftPictureURL = details.picture ?? ""
        statusText = nil
    }

    private func uploadSelectedPhoto(_ item: PhotosPickerItem?) {
        guard let item else { return }
        photoPickerPresented = false
        imageUploadInFlight = true
        statusText = nil
        Task {
            do {
                guard let data = try await item.loadTransferable(type: Data.self) else {
                    throw ImageUploadError.unreadableImage
                }
                let payload = try await Task.detached(priority: .userInitiated) {
                    try ImageUploadPayload(sourceData: data)
                }.value
                let url = await onUploadImage(payload.data, payload.mimeType)
                await MainActor.run {
                    selectedPhotoItem = nil
                    imageUploadInFlight = false
                    if let url {
                        draftPictureURL = url
                        statusText = "Image uploaded"
                    } else {
                        statusText = "Image upload failed"
                    }
                }
            } catch {
                await MainActor.run {
                    selectedPhotoItem = nil
                    imageUploadInFlight = false
                    statusText = String(describing: error)
                }
            }
        }
    }

    private func saveMetadata() {
        guard canSave else { return }
        photoPickerPresented = false
        selectedPhotoItem = nil
        saveInFlight = true
        statusText = nil
        let displayName = draftDisplayName.trimmingCharacters(in: .whitespacesAndNewlines)
        let picture = normalizedDraftPictureURL
        Task {
            let saved = await onSaveMetadata(details.roomId, displayName, picture)
            await MainActor.run {
                saveInFlight = false
                if saved {
                    draftDisplayName = displayName
                    draftPictureURL = picture ?? ""
                }
                statusText = saved ? "Saved" : "Could not save room"
            }
        }
    }
}

private struct RoomDetailsMemberRow: View {
    let member: AppRoomMemberSummary

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            ProfileAvatar(displayName: member.displayName, pictureURL: member.picture, size: 38)
            .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text(member.displayName)
                        .font(.body)
                        .lineLimit(1)
                    if member.currentDevice {
                        Text("You")
                            .font(.caption.weight(.medium))
                            .foregroundStyle(.secondary)
                    }
                }

                Text(memberSubtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer(minLength: 8)
        }
        .accessibilityElement(children: .combine)
    }

    private var memberSubtitle: String {
        let npub = shortenedRoomDetailsNpub(member.npub)
        guard !member.deviceId.isEmpty else { return npub }
        return "\(npub) - \(member.deviceId)"
    }
}

private struct RoomDetailsDeviceRow: View {
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
                .accessibilityIdentifier("RoomDetailsRevokeDeviceButton")
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
    var detailsListID: String {
        "\(accountId)/\(deviceId)"
    }
}

private extension AppRoomMemberSummary {
    var detailsListID: String {
        "\(accountId)/\(deviceId)"
    }
}

private func shortenedRoomDetailsNpub(_ npub: String) -> String {
    guard npub.count > 18 else { return npub }
    return "\(npub.prefix(10))...\(npub.suffix(4))"
}
