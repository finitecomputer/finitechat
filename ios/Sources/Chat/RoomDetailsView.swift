import SwiftUI

struct RoomDetailsView: View {
    let details: AppRoomDetailsState?
    let mediaItems: [ChatMediaGalleryItem]
    let onDownloadAttachment: (ChatMediaGalleryItem) -> Void
    let onCreateInvite: () -> Void
    let onRefreshDevices: () -> Void
    let onRevokeDevice: (AppDeviceSummary) -> Void

    var body: some View {
        Group {
            if let details {
                List {
                    Section {
                        RoomDetailsHeader(details: details)
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
                                onCreateInvite()
                            } label: {
                                Label("Invite", systemImage: "qrcode")
                            }
                            .accessibilityIdentifier("RoomDetailsInviteButton")
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
            ZStack {
                Circle()
                    .fill(details.state.detailsTint.opacity(0.16))
                Image(systemName: "bubble.left.and.bubble.right.fill")
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(details.state.detailsTint)
            }
            .frame(width: 52, height: 52)
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

private extension AppRoomState {
    var detailsTint: Color {
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

private extension AppDeviceSummary {
    var detailsListID: String {
        "\(accountId)/\(deviceId)"
    }
}
