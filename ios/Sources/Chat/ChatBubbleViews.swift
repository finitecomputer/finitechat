import SwiftUI

enum ChatBubblePosition {
    case single
    case first
    case middle
    case last
}

struct ChatTimelineRowView: View {
    let row: ChatTimelineRow
    let messagesById: [String: ChatMessage]

    var body: some View {
        switch row {
        case .messageGroup(let group):
            ChatMessageGroupRow(group: group, messagesById: messagesById)
                .padding(.horizontal, 12)
                .padding(.vertical, 4)
        }
    }
}

private struct ChatMessageGroupRow: View {
    let group: ChatTimelineMessageGroup
    let messagesById: [String: ChatMessage]

    private let avatarSize: CGFloat = 28

    var body: some View {
        Group {
            if group.isMine {
                outgoingRow
            } else {
                incomingRow
            }
        }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .contain)
    }

    private var incomingRow: some View {
        HStack(alignment: .bottom, spacing: 8) {
            ChatAvatar(
                title: group.senderDisplayName,
                subtitle: group.senderNpub ?? group.senderAccountId,
                size: avatarSize
            )
            .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 4) {
                Text(senderLabel)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)

                ChatBubbleStack(
                    messages: group.messages,
                    messagesById: messagesById,
                    alignment: .leading
                )
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Spacer(minLength: 44)
        }
    }

    private var outgoingRow: some View {
        HStack(alignment: .bottom, spacing: 8) {
            Spacer(minLength: 52)

            VStack(alignment: .trailing, spacing: 4) {
                ChatBubbleStack(
                    messages: group.messages,
                    messagesById: messagesById,
                    alignment: .trailing
                )

                if let last = group.messages.last {
                    MessageStatusLine(message: last)
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
        }
    }

    private var senderLabel: String {
        let trimmed = group.senderDisplayName.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            return trimmed
        }
        if let npub = group.senderNpub, npub.count > 12 {
            return "\(npub.prefix(12))..."
        }
        return group.senderDeviceId
    }
}

private struct ChatBubbleStack: View {
    let messages: [ChatMessage]
    let messagesById: [String: ChatMessage]
    let alignment: HorizontalAlignment

    var body: some View {
        VStack(alignment: alignment, spacing: 2) {
            ForEach(Array(messages.enumerated()), id: \.element.messageId) { index, message in
                ChatMessageBubble(
                    message: message,
                    replyTarget: replyTarget(for: message),
                    position: bubblePosition(at: index, count: messages.count)
                )
            }
        }
    }

    private func replyTarget(for message: ChatMessage) -> ChatMessage? {
        guard let replyToMessageId = message.replyToMessageId else { return nil }
        return messagesById[replyToMessageId]
    }

    private func bubblePosition(at index: Int, count: Int) -> ChatBubblePosition {
        guard count > 1 else { return .single }
        if index == 0 { return .first }
        if index == count - 1 { return .last }
        return .middle
    }
}

private struct ChatMessageBubble: View {
    let message: ChatMessage
    let replyTarget: ChatMessage?
    let position: ChatBubblePosition

    @State private var isPressed = false

    private var bubbleColor: Color {
        message.isMine ? .accentColor : Color(uiColor: .secondarySystemGroupedBackground)
    }

    private var foregroundColor: Color {
        message.isMine ? .white : .primary
    }

    private var secondaryForegroundColor: Color {
        message.isMine ? .white.opacity(0.78) : .secondary
    }

    private var bodyText: String {
        let display = message.displayContent.trimmingCharacters(in: .whitespacesAndNewlines)
        if !display.isEmpty {
            return display
        }
        return message.text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var body: some View {
        VStack(alignment: message.isMine ? .trailing : .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                if message.replyToMessageId != nil {
                    ReplyPreview(message: message, target: replyTarget, isMine: message.isMine)
                        .padding(.horizontal, 8)
                        .padding(.top, 8)
                }

                if !message.media.isEmpty {
                    ChatMediaGrid(attachments: message.media, isMine: message.isMine)
                        .padding(.top, message.replyToMessageId == nil ? 0 : 6)
                }

                if !bodyText.isEmpty {
                    Text(bodyText)
                        .font(.body)
                        .foregroundStyle(foregroundColor)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.horizontal, 12)
                        .padding(.top, message.media.isEmpty ? 8 : 7)
                        .padding(.bottom, statusText == nil ? 8 : 3)
                }

                if let statusText {
                    Text(statusText)
                        .font(.caption2)
                        .foregroundStyle(secondaryForegroundColor)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                        .padding(.horizontal, 12)
                        .padding(.bottom, 6)
                }
            }
            .frame(maxWidth: 326, alignment: message.isMine ? .trailing : .leading)
            .background(bubbleColor)
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
            .scaleEffect(isPressed ? 0.97 : 1)
            .animation(.spring(response: 0.24, dampingFraction: 0.76), value: isPressed)
            .onLongPressGesture(minimumDuration: 0.3, maximumDistance: 44) {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
            } onPressingChanged: { pressing in
                isPressed = pressing
            }

            if !message.reactions.isEmpty {
                ReactionChips(reactions: message.reactions)
                    .offset(y: -7)
                    .padding(.horizontal, 5)
                    .padding(.bottom, -2)
            }
        }
        .accessibilityElement(children: .combine)
    }

    private var statusText: String? {
        if !message.displayTimestamp.isEmpty {
            return message.displayTimestamp
        }
        return nil
    }

    private var cornerRadius: CGFloat {
        switch position {
        case .single:
            return 18
        case .first, .last:
            return 16
        case .middle:
            return 12
        }
    }
}

private struct ReactionChips: View {
    let reactions: [ChatReactionSummary]

    var body: some View {
        HStack(spacing: 4) {
            ForEach(reactions, id: \.emoji) { reaction in
                HStack(spacing: 3) {
                    Text(reaction.emoji)
                        .font(.system(size: 13))
                    if reaction.count > 1 {
                        Text("\(reaction.count)")
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(reaction.reactedByMe ? .white : .secondary)
                    }
                }
                .padding(.horizontal, 6)
                .padding(.vertical, 3)
                .background(
                    Capsule()
                        .fill(reaction.reactedByMe ? Color.accentColor : Color(uiColor: .tertiarySystemGroupedBackground))
                )
                .overlay(
                    Capsule()
                        .strokeBorder(Color(uiColor: .systemBackground), lineWidth: 1.5)
                )
            }
        }
    }
}

private struct ReplyPreview: View {
    let message: ChatMessage
    let target: ChatMessage?
    let isMine: Bool

    var body: some View {
        HStack(spacing: 8) {
            Capsule()
                .fill(isMine ? .white.opacity(0.72) : Color.accentColor)
                .frame(width: 3)

            VStack(alignment: .leading, spacing: 2) {
                Text(replyTitle)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(isMine ? .white.opacity(0.88) : .secondary)
                    .lineLimit(1)
                Text(replySnippet)
                    .font(.caption)
                    .foregroundStyle(isMine ? .white.opacity(0.78) : .secondary)
                    .lineLimit(2)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 6)
        .padding(.horizontal, 8)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(isMine ? .white.opacity(0.12) : Color(uiColor: .tertiarySystemGroupedBackground))
        )
    }

    private var replyTitle: String {
        guard let target else { return "Reply" }
        let name = target.senderDisplayName.trimmingCharacters(in: .whitespacesAndNewlines)
        return name.isEmpty ? target.senderDeviceId : name
    }

    private var replySnippet: String {
        guard let target else {
            return message.replyToMessageId ?? "Message unavailable"
        }
        let text = target.displayContent.isEmpty ? target.text : target.displayContent
        if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return text
        }
        if let firstMedia = target.media.first {
            return mediaLabel(for: firstMedia.kind)
        }
        return "Message"
    }
}

private struct ChatMediaGrid: View {
    let attachments: [ChatMediaAttachment]
    let isMine: Bool

    var body: some View {
        let visual = attachments.filter { $0.kind == .image || $0.kind == .video }
        let files = attachments.filter { $0.kind != .image && $0.kind != .video }

        VStack(alignment: .leading, spacing: 2) {
            if !visual.isEmpty {
                visualGrid(attachments: visual)
                    .frame(height: gridHeight(count: visual.count))
                    .clipped()
            }

            ForEach(files, id: \.attachmentId) { attachment in
                FileAttachmentRow(attachment: attachment, isMine: isMine)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 6)
            }
        }
    }

    @ViewBuilder
    private func visualGrid(attachments: [ChatMediaAttachment]) -> some View {
        let spacing: CGFloat = 2
        GeometryReader { geometry in
            let width = geometry.size.width
            let halfWidth = max(0, (width - spacing) / 2)

            switch attachments.count {
            case 1:
                MediaTile(attachment: attachments[0], isMine: isMine)
                    .frame(width: width, height: geometry.size.height)

            case 2:
                HStack(spacing: spacing) {
                    MediaTile(attachment: attachments[0], isMine: isMine)
                        .frame(width: halfWidth)
                    MediaTile(attachment: attachments[1], isMine: isMine)
                        .frame(width: halfWidth)
                }

            case 3:
                HStack(spacing: spacing) {
                    MediaTile(attachment: attachments[0], isMine: isMine)
                        .frame(width: halfWidth)
                    VStack(spacing: spacing) {
                        MediaTile(attachment: attachments[1], isMine: isMine)
                        MediaTile(attachment: attachments[2], isMine: isMine)
                    }
                    .frame(width: halfWidth)
                }

            default:
                VStack(spacing: spacing) {
                    HStack(spacing: spacing) {
                        MediaTile(attachment: attachments[0], isMine: isMine)
                            .frame(width: halfWidth)
                        MediaTile(attachment: attachments[1], isMine: isMine)
                            .frame(width: halfWidth)
                    }
                    HStack(spacing: spacing) {
                        MediaTile(attachment: attachments[2], isMine: isMine)
                            .frame(width: halfWidth)
                        MediaTile(attachment: attachments[3], isMine: isMine)
                            .frame(width: halfWidth)
                            .overlay {
                                let remaining = attachments.count - 4
                                if remaining > 0 {
                                    Color.black.opacity(0.48)
                                    Text("+\(remaining)")
                                        .font(.title2.bold())
                                        .foregroundStyle(.white)
                                }
                            }
                    }
                }
            }
        }
        .background(Color(uiColor: .systemGray5))
    }

    private func gridHeight(count: Int) -> CGFloat {
        switch count {
        case 0:
            return 0
        case 1, 2:
            return 202
        default:
            return 280
        }
    }
}

private struct MediaTile: View {
    let attachment: ChatMediaAttachment
    let isMine: Bool

    var body: some View {
        ZStack {
            if attachment.kind == .image,
               let urlString = attachment.url,
               let url = URL(string: urlString)
            {
                AsyncImage(url: url) { phase in
                    switch phase {
                    case .success(let image):
                        image
                            .resizable()
                            .scaledToFill()
                    case .failure:
                        mediaPlaceholder
                    case .empty:
                        mediaPlaceholder
                            .redacted(reason: .placeholder)
                    @unknown default:
                        mediaPlaceholder
                    }
                }
            } else {
                mediaPlaceholder
            }

            if attachment.kind == .video {
                Image(systemName: "play.fill")
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(.white)
                    .padding(12)
                    .background(.black.opacity(0.42), in: Circle())
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .clipped()
        .accessibilityElement(children: .combine)
        .accessibilityLabel(mediaLabel(for: attachment.kind))
    }

    private var mediaPlaceholder: some View {
        Rectangle()
            .fill(isMine ? Color.white.opacity(0.16) : Color(uiColor: .tertiarySystemGroupedBackground))
            .overlay {
                VStack(spacing: 6) {
                    Image(systemName: iconName(for: attachment.kind))
                        .font(.title2)
                    Text(attachment.filename.isEmpty ? mediaLabel(for: attachment.kind) : attachment.filename)
                        .font(.caption)
                        .lineLimit(1)
                }
                .foregroundStyle(isMine ? .white.opacity(0.82) : .secondary)
                .padding(10)
            }
    }
}

private struct FileAttachmentRow: View {
    let attachment: ChatMediaAttachment
    let isMine: Bool

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: iconName(for: attachment.kind))
                .font(.body.weight(.semibold))
                .frame(width: 30, height: 30)
                .background(
                    Circle()
                        .fill(isMine ? .white.opacity(0.16) : Color(uiColor: .systemGroupedBackground))
                )

            VStack(alignment: .leading, spacing: 2) {
                Text(attachment.filename.isEmpty ? mediaLabel(for: attachment.kind) : attachment.filename)
                    .font(.subheadline.weight(.medium))
                    .lineLimit(1)
                Text(attachment.mimeType.isEmpty ? mediaLabel(for: attachment.kind) : attachment.mimeType)
                    .font(.caption)
                    .foregroundStyle(isMine ? .white.opacity(0.72) : .secondary)
                    .lineLimit(1)
            }
        }
        .foregroundStyle(isMine ? .white : .primary)
    }
}

private struct MessageStatusLine: View {
    let message: ChatMessage

    var body: some View {
        if let text = readReceiptText ?? deliveryText {
            Text(text)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
    }

    private var readReceiptText: String? {
        guard let readReceipt = message.readReceipt else { return nil }
        let text = readReceipt.displayText.trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty ? nil : text
    }

    private var deliveryText: String? {
        switch message.delivery {
        case .pending:
            return "Sending"
        case .sent:
            return nil
        case .failed(let reason):
            let trimmed = reason.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? "Failed" : "Failed: \(trimmed)"
        }
    }
}

private struct ChatAvatar: View {
    let title: String
    let subtitle: String
    let size: CGFloat

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: size, height: size)
            .overlay {
                Text(initials)
                    .font(.system(size: size * 0.38, weight: .semibold))
                    .foregroundStyle(.white)
            }
    }

    private var initials: String {
        let source = title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? subtitle : title
        let pieces = source
            .split(separator: " ")
            .prefix(2)
            .compactMap(\.first)
        let value = String(pieces).uppercased()
        return value.isEmpty ? "#" : value
    }

    private var color: Color {
        let palette: [Color] = [.blue, .green, .indigo, .mint, .pink, .teal, .cyan, .orange]
        let scalarSum = subtitle.unicodeScalars.reduce(0) { $0 + Int($1.value) }
        return palette[scalarSum % palette.count]
    }
}

private func mediaLabel(for kind: ChatMediaKind) -> String {
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

private func iconName(for kind: ChatMediaKind) -> String {
    switch kind {
    case .image:
        return "photo"
    case .voiceNote:
        return "waveform"
    case .video:
        return "video"
    case .file:
        return "doc"
    }
}
