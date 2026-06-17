import Foundation
import PhotosUI
import SwiftUI
import UIKit
import UniformTypeIdentifiers

struct Composer: View {
    @ObservedObject var model: AppModel
    let replyTarget: ChatMessage?
    @Binding var stagedAttachments: [StagedComposerAttachment]
    @Binding var isPhotoPickerPresented: Bool
    @Binding var selectedPhotoItems: [PhotosPickerItem]
    @Binding var isInputFocused: Bool
    let onCancelReply: () -> Void
    let onSend: () -> Void
    let onAttach: () -> Void
    @FocusState private var textFieldFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            if let replyTarget {
                ComposerReplyPreview(
                    message: replyTarget,
                    onCancel: onCancelReply
                )
            }

            if !stagedAttachments.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(stagedAttachments) { item in
                            StagedAttachmentThumbnail(item: item) {
                                withAnimation(.easeOut(duration: 0.16)) {
                                    stagedAttachments.removeAll { $0.id == item.id }
                                }
                            }
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                }
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }

            HStack(spacing: 10) {
                Menu {
                    Button {
                        isPhotoPickerPresented = true
                    } label: {
                        Label("Photos & Videos", systemImage: "photo.on.rectangle")
                    }

                    Button {
                        onAttach()
                    } label: {
                        Label("Files", systemImage: "doc")
                    }
                } label: {
                    Image(systemName: "plus")
                        .font(.title3)
                        .frame(width: 30, height: 30)
                }
                .accessibilityLabel("Attach")
                .accessibilityIdentifier("AttachButton")
                .photosPicker(
                    isPresented: $isPhotoPickerPresented,
                    selection: $selectedPhotoItems,
                    maxSelectionCount: remainingPhotoSelectionCount,
                    matching: .any(of: [.images, .videos])
                )

                TextField("Message", text: $model.outboundText, axis: .vertical)
                    .lineLimit(1...4)
                    .textFieldStyle(.roundedBorder)
                    .focused($textFieldFocused)
                    .accessibilityLabel("Message")

                Button {
                    onSend()
                } label: {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.title2)
                }
                .disabled(sendDisabled)
                .accessibilityLabel("Send")
                .accessibilityIdentifier("SendButton")
            }
            .padding()
        }
        .background(.bar)
        .animation(.easeInOut(duration: 0.16), value: stagedAttachments.isEmpty)
        .onChange(of: textFieldFocused) { _, focused in
            isInputFocused = focused
        }
        .onChange(of: isInputFocused) { _, focused in
            guard textFieldFocused != focused else { return }
            textFieldFocused = focused
        }
    }

    private var sendDisabled: Bool {
        stagedAttachments.isEmpty && !model.canSend
    }

    private var remainingPhotoSelectionCount: Int {
        max(1, maxStagedComposerAttachments - stagedAttachments.count)
    }
}

let maxStagedComposerAttachments = 32
let maxComposerAttachmentBytes = 32 * 1024 * 1024

struct StagedComposerAttachment: Identifiable {
    let id: String
    let data: Data
    let filename: String
    let mimeType: String
    let kind: ChatMediaKind
    let thumbnail: UIImage?

    var outboundAttachment: OutboundAttachment {
        OutboundAttachment(
            filename: filename,
            mimeType: mimeType,
            kind: kind,
            bytes: data
        )
    }

    init(fileURL: URL) throws {
        let didStartAccessing = fileURL.startAccessingSecurityScopedResource()
        defer {
            if didStartAccessing {
                fileURL.stopAccessingSecurityScopedResource()
            }
        }

        let data = try Data(contentsOf: fileURL)
        let type = UTType(filenameExtension: fileURL.pathExtension)
        try self.init(
            data: data,
            filename: fileURL.lastPathComponent.isEmpty ? "attachment" : fileURL.lastPathComponent,
            mimeType: type?.preferredMIMEType ?? "application/octet-stream",
            kind: chatMediaKind(for: type)
        )
    }

    init?(photoItem: PhotosPickerItem) async throws {
        guard let data = try await photoItem.loadTransferable(type: Data.self) else {
            return nil
        }
        let type = photoItem.supportedContentTypes.first
        let filename = "attachment-\(UUID().uuidString).\(defaultFilenameExtension(for: type))"
        self = try await Task.detached(priority: .userInitiated) {
            try StagedComposerAttachment(
                data: data,
                filename: filename,
                mimeType: type?.preferredMIMEType ?? "application/octet-stream",
                kind: chatMediaKind(for: type)
            )
        }.value
    }

    private init(
        data: Data,
        filename: String,
        mimeType: String,
        kind: ChatMediaKind
    ) throws {
        guard data.count <= maxComposerAttachmentBytes else {
            throw ComposerAttachmentError.tooLarge(filename: filename)
        }
        self.id = UUID().uuidString
        self.data = data
        self.filename = filename
        self.mimeType = mimeType
        self.kind = kind
        self.thumbnail = Self.makeThumbnail(data: data, kind: kind)
    }

    private static func makeThumbnail(data: Data, kind: ChatMediaKind) -> UIImage? {
        guard kind == .image, let image = UIImage(data: data) else { return nil }
        let maxSide: CGFloat = 160
        let scale = min(maxSide / max(image.size.width, image.size.height), 1)
        let size = CGSize(width: image.size.width * scale, height: image.size.height * scale)
        let renderer = UIGraphicsImageRenderer(size: size)
        return renderer.image { _ in
            image.draw(in: CGRect(origin: .zero, size: size))
        }
    }
}

enum ComposerAttachmentError: LocalizedError {
    case tooLarge(filename: String)

    var errorDescription: String? {
        switch self {
        case .tooLarge(let filename):
            "\(filename) is larger than the 32 MiB attachment limit."
        }
    }
}

private struct StagedAttachmentThumbnail: View {
    let item: StagedComposerAttachment
    let onRemove: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ZStack(alignment: .topTrailing) {
                thumbnail
                    .frame(width: 72, height: 72)
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))

                Button(action: onRemove) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.body)
                        .symbolRenderingMode(.palette)
                        .foregroundStyle(.white, .black.opacity(0.65))
                }
                .buttonStyle(.plain)
                .offset(x: 6, y: -6)
                .accessibilityLabel("Remove \(item.filename)")
            }

            Text(item.filename)
                .font(.caption2)
                .lineLimit(1)
                .frame(width: 72, alignment: .leading)
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var thumbnail: some View {
        if let image = item.thumbnail {
            Image(uiImage: image)
                .resizable()
                .scaledToFill()
        } else {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(Color(.tertiarySystemFill))
                .overlay {
                    VStack(spacing: 4) {
                        Image(systemName: stagedAttachmentIcon(for: item.kind))
                            .font(.title3)
                        Text(composerMediaLabel(for: item.kind))
                            .font(.caption2.weight(.medium))
                            .lineLimit(1)
                    }
                    .foregroundStyle(.secondary)
                    .padding(6)
                }
        }
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

private func chatMediaKind(for type: UTType?) -> ChatMediaKind {
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

private func defaultFilenameExtension(for type: UTType?) -> String {
    if let ext = type?.preferredFilenameExtension {
        return ext
    }
    switch chatMediaKind(for: type) {
    case .image:
        return "jpg"
    case .video:
        return "mov"
    case .voiceNote:
        return "m4a"
    case .file:
        return "bin"
    }
}

private func stagedAttachmentIcon(for kind: ChatMediaKind) -> String {
    switch kind {
    case .image:
        return "photo"
    case .voiceNote:
        return "waveform"
    case .video:
        return "play.rectangle"
    case .file:
        return "doc"
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
