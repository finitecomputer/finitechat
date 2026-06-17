import ImageIO
import SwiftUI
import UIKit

enum ChatBubblePosition {
    case single
    case first
    case middle
    case last
}

struct ChatTimelineRowView: View {
    let row: ChatTimelineRow
    let messagesById: [String: ChatMessage]
    let messageFrameRegistry: ChatMessageFrameRegistry?
    let highlightedMessageID: String?
    let onReact: (ChatMessage, String) -> Void
    let onDownloadAttachment: (ChatMessage, ChatMediaAttachment) -> Void
    let onOpenAttachment: (ChatMessage, ChatMediaAttachment) -> Void
    let onVotePoll: (ChatMessage, ChatPollOption) -> Void
    let onRetryMessage: (ChatMessage) -> Void
    let onJumpToMessage: (String) -> Void
    let onLongPressMessage: (ChatMessage, CGRect) -> Void

    var body: some View {
        switch row {
        case .messageGroup(let group):
            ChatMessageGroupRow(
                group: group,
                messagesById: messagesById,
                messageFrameRegistry: messageFrameRegistry,
                highlightedMessageID: highlightedMessageID,
                onReact: onReact,
                onDownloadAttachment: onDownloadAttachment,
                onOpenAttachment: onOpenAttachment,
                onVotePoll: onVotePoll,
                onRetryMessage: onRetryMessage,
                onJumpToMessage: onJumpToMessage,
                onLongPressMessage: onLongPressMessage
            )
                .padding(.horizontal, 12)
                .padding(.vertical, 4)
        case .typing(let members):
            ChatTypingIndicatorRow(members: members)
                .padding(.horizontal, 12)
                .padding(.vertical, 4)
        }
    }
}

struct FocusedChatMessageCard: View {
    let message: ChatMessage
    let replyTarget: ChatMessage?

    var body: some View {
        ChatMessageBubble(
            message: message,
            replyTarget: replyTarget,
            position: .single,
            messageFrameRegistry: nil,
            isHighlighted: false,
            onReact: { _, _ in },
            onDownloadAttachment: { _, _ in },
            onOpenAttachment: { _, _ in },
            onVotePoll: { _, _ in },
            onRetryMessage: { _ in },
            onJumpToMessage: { _ in },
            onLongPressMessage: nil
        )
        .allowsHitTesting(false)
    }
}

private struct ChatMessageGroupRow: View {
    let group: ChatTimelineMessageGroup
    let messagesById: [String: ChatMessage]
    let messageFrameRegistry: ChatMessageFrameRegistry?
    let highlightedMessageID: String?
    let onReact: (ChatMessage, String) -> Void
    let onDownloadAttachment: (ChatMessage, ChatMediaAttachment) -> Void
    let onOpenAttachment: (ChatMessage, ChatMediaAttachment) -> Void
    let onVotePoll: (ChatMessage, ChatPollOption) -> Void
    let onRetryMessage: (ChatMessage) -> Void
    let onJumpToMessage: (String) -> Void
    let onLongPressMessage: (ChatMessage, CGRect) -> Void

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
                    messageFrameRegistry: messageFrameRegistry,
                    highlightedMessageID: highlightedMessageID,
                    onReact: onReact,
                    onDownloadAttachment: onDownloadAttachment,
                    onOpenAttachment: onOpenAttachment,
                    onVotePoll: onVotePoll,
                    onRetryMessage: onRetryMessage,
                    onJumpToMessage: onJumpToMessage,
                    onLongPressMessage: onLongPressMessage,
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
                    messageFrameRegistry: messageFrameRegistry,
                    highlightedMessageID: highlightedMessageID,
                    onReact: onReact,
                    onDownloadAttachment: onDownloadAttachment,
                    onOpenAttachment: onOpenAttachment,
                    onVotePoll: onVotePoll,
                    onRetryMessage: onRetryMessage,
                    onJumpToMessage: onJumpToMessage,
                    onLongPressMessage: onLongPressMessage,
                    alignment: .trailing
                )

                if let last = group.messages.last {
                    MessageStatusLine(message: last, onRetryMessage: onRetryMessage)
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
    let messageFrameRegistry: ChatMessageFrameRegistry?
    let highlightedMessageID: String?
    let onReact: (ChatMessage, String) -> Void
    let onDownloadAttachment: (ChatMessage, ChatMediaAttachment) -> Void
    let onOpenAttachment: (ChatMessage, ChatMediaAttachment) -> Void
    let onVotePoll: (ChatMessage, ChatPollOption) -> Void
    let onRetryMessage: (ChatMessage) -> Void
    let onJumpToMessage: (String) -> Void
    let onLongPressMessage: (ChatMessage, CGRect) -> Void
    let alignment: HorizontalAlignment

    var body: some View {
        VStack(alignment: alignment, spacing: 2) {
            ForEach(Array(messages.enumerated()), id: \.element.messageId) { index, message in
                ChatMessageBubble(
                    message: message,
                    replyTarget: replyTarget(for: message),
                    position: bubblePosition(at: index, count: messages.count),
                    messageFrameRegistry: messageFrameRegistry,
                    isHighlighted: highlightedMessageID == message.messageId,
                    onReact: onReact,
                    onDownloadAttachment: onDownloadAttachment,
                    onOpenAttachment: onOpenAttachment,
                    onVotePoll: onVotePoll,
                    onRetryMessage: onRetryMessage,
                    onJumpToMessage: onJumpToMessage,
                    onLongPressMessage: onLongPressMessage
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
    let messageFrameRegistry: ChatMessageFrameRegistry?
    let isHighlighted: Bool
    let onReact: (ChatMessage, String) -> Void
    let onDownloadAttachment: (ChatMessage, ChatMediaAttachment) -> Void
    let onOpenAttachment: (ChatMessage, ChatMediaAttachment) -> Void
    let onVotePoll: (ChatMessage, ChatPollOption) -> Void
    let onRetryMessage: (ChatMessage) -> Void
    let onJumpToMessage: (String) -> Void
    let onLongPressMessage: ((ChatMessage, CGRect) -> Void)?

    @State private var isPressed = false
    @State private var frameRef = BubbleFrameRef()

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
                    ReplyPreview(
                        message: message,
                        target: replyTarget,
                        isMine: message.isMine,
                        onJumpToMessage: onJumpToMessage
                    )
                        .padding(.horizontal, 8)
                        .padding(.top, 8)
                }

                if !message.media.isEmpty {
                    ChatMediaGrid(
                        attachments: message.media,
                        isMine: message.isMine,
                        onDownloadAttachment: { attachment in
                            onDownloadAttachment(message, attachment)
                        },
                        onOpenAttachment: { attachment in
                            onOpenAttachment(message, attachment)
                        }
                    )
                        .padding(.top, message.replyToMessageId == nil ? 0 : 6)
                }

                if let poll = message.poll {
                    PollContentView(
                        poll: poll,
                        isMine: message.isMine,
                        onVote: { option in
                            onVotePoll(message, option)
                        }
                    )
                    .padding(.horizontal, 10)
                    .padding(.top, message.media.isEmpty ? 8 : 7)
                    .padding(.bottom, statusText == nil ? 8 : 3)
                } else if !bodyText.isEmpty {
                    Text(bodyText)
                        .font(.body)
                        .foregroundStyle(foregroundColor)
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
            .overlay {
                if isHighlighted {
                    RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                        .strokeBorder(Color.yellow.opacity(0.95), lineWidth: 2)
                        .shadow(color: .yellow.opacity(0.38), radius: 9)
                        .allowsHitTesting(false)
                }
            }
            .background(
                GeometryReader { proxy in
                    let frame = proxy.frame(in: .global)
                    Color.clear
                        .onAppear {
                            updateFrame(frame)
                        }
                        .onChange(of: frame) { _, newFrame in
                            updateFrame(newFrame)
                        }
                }
            )
            .contentShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
            .scaleEffect(isPressed ? 0.97 : 1)
            .animation(.spring(response: 0.24, dampingFraction: 0.76), value: isPressed)
            .animation(.easeInOut(duration: 0.18), value: isHighlighted)
            .onLongPressGesture(minimumDuration: 0.3, maximumDistance: 44) {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                onLongPressMessage?(message, frameRef.frame)
            } onPressingChanged: { pressing in
                isPressed = pressing
            }

            if !message.reactions.isEmpty {
                ReactionChips(
                    message: message,
                    reactions: message.reactions,
                    onReact: onReact
                )
                    .offset(y: -7)
                    .padding(.horizontal, 5)
                    .padding(.bottom, -2)
            }
        }
        .accessibilityElement(children: .combine)
        .onDisappear {
            messageFrameRegistry?.removeFrame(for: message.messageId)
        }
    }

    private func updateFrame(_ frame: CGRect) {
        frameRef.frame = frame
        messageFrameRegistry?.setFrame(frame, for: message)
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

private final class BubbleFrameRef {
    var frame: CGRect = .zero
}

final class ChatMessageFrameRegistry {
    private struct Entry {
        let message: ChatMessage
        let frame: CGRect
    }

    private var entries: [String: Entry] = [:]

    func setFrame(_ frame: CGRect, for message: ChatMessage) {
        guard frame.width > 0, frame.height > 0 else { return }
        entries[message.messageId] = Entry(message: message, frame: frame)
    }

    func removeFrame(for messageID: String) {
        entries.removeValue(forKey: messageID)
    }

    func hitMessage(at point: CGPoint) -> (message: ChatMessage, frame: CGRect)? {
        var best: (message: ChatMessage, frame: CGRect, area: CGFloat)?
        for entry in entries.values {
            let frame = entry.frame
            guard frame.contains(point) else { continue }
            let area = frame.width * frame.height
            if best == nil || area < best!.area {
                best = (entry.message, frame, area)
            }
        }
        guard let best else { return nil }
        return (best.message, best.frame)
    }
}

private struct ReactionChips: View {
    let message: ChatMessage
    let reactions: [ChatReactionSummary]
    let onReact: (ChatMessage, String) -> Void

    var body: some View {
        HStack(spacing: 4) {
            ForEach(reactions, id: \.emoji) { reaction in
                Button {
                    UIImpactFeedbackGenerator(style: .light).impactOccurred()
                    onReact(message, reaction.emoji)
                } label: {
                    HStack(spacing: 3) {
                        Text(reaction.emoji)
                            .font(.system(size: 13))
                        if reaction.count > 1 {
                            Text("\(reaction.count)")
                                .font(.system(size: 10, weight: .semibold))
                                .foregroundStyle(reaction.reactedByMe ? .white : .secondary)
                        }
                    }
                }
                .buttonStyle(.plain)
                .disabled(reaction.reactedByMe)
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

private struct PollContentView: View {
    let poll: ChatPoll
    let isMine: Bool
    let onVote: (ChatPollOption) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(poll.question)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(isMine ? .white : .primary)
                .fixedSize(horizontal: false, vertical: true)

            VStack(spacing: 6) {
                ForEach(poll.options, id: \.optionId) { option in
                    PollOptionButton(
                        option: option,
                        totalVotes: poll.totalVotes,
                        isMine: isMine,
                        onVote: {
                            onVote(option)
                        }
                    )
                }
            }

            if poll.totalVotes > 0 {
                Text(voteCountText)
                    .font(.caption2)
                    .foregroundStyle(isMine ? .white.opacity(0.74) : .secondary)
            }
        }
    }

    private var voteCountText: String {
        poll.totalVotes == 1 ? "1 vote" : "\(poll.totalVotes) votes"
    }
}

private struct PollOptionButton: View {
    let option: ChatPollOption
    let totalVotes: UInt32
    let isMine: Bool
    let onVote: () -> Void

    var body: some View {
        Button {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            onVote()
        } label: {
            HStack(spacing: 8) {
                if option.votedByMe {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.caption.weight(.semibold))
                }

                Text(option.text)
                    .font(.subheadline)
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)

                Spacer(minLength: 8)

                if totalVotes > 0 {
                    Text("\(percent)%")
                        .font(.caption.weight(.semibold))
                        .monospacedDigit()
                }
            }
            .foregroundStyle(foregroundColor)
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, minHeight: 38, alignment: .leading)
            .background {
                GeometryReader { geometry in
                    ZStack(alignment: .leading) {
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .fill(rowBackground)

                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .fill(progressColor)
                            .frame(width: geometry.size.width * progressFraction)
                    }
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay {
                if option.votedByMe {
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .strokeBorder(foregroundColor.opacity(0.34), lineWidth: 1)
                }
            }
        }
        .buttonStyle(.plain)
        .disabled(option.votedByMe)
        .accessibilityLabel(option.text)
        .accessibilityValue(accessibilityValue)
    }

    private var progressFraction: CGFloat {
        guard totalVotes > 0 else { return 0 }
        return CGFloat(option.voteCount) / CGFloat(totalVotes)
    }

    private var percent: Int {
        guard totalVotes > 0 else { return 0 }
        return Int((Double(option.voteCount) / Double(totalVotes) * 100).rounded())
    }

    private var rowBackground: Color {
        isMine ? .white.opacity(0.14) : Color(uiColor: .tertiarySystemGroupedBackground)
    }

    private var progressColor: Color {
        isMine ? .white.opacity(0.22) : Color.accentColor.opacity(0.18)
    }

    private var foregroundColor: Color {
        isMine ? .white : .primary
    }

    private var accessibilityValue: String {
        if totalVotes == 0 {
            return option.votedByMe ? "Selected" : "No votes"
        }
        let votes = option.voteCount == 1 ? "1 vote" : "\(option.voteCount) votes"
        return option.votedByMe ? "Selected, \(votes), \(percent) percent" : "\(votes), \(percent) percent"
    }
}

private struct ReplyPreview: View {
    let message: ChatMessage
    let target: ChatMessage?
    let isMine: Bool
    let onJumpToMessage: (String) -> Void

    var body: some View {
        Group {
            if let replyToMessageID = message.replyToMessageId, target != nil {
                Button {
                    onJumpToMessage(replyToMessageID)
                } label: {
                    content
                }
                .buttonStyle(.plain)
            } else {
                content
            }
        }
        .accessibilityLabel(accessibilityLabel)
    }

    private var content: some View {
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
                    .lineLimit(1)
            }

            Spacer(minLength: 0)

            mediaThumbnail
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

    private var firstVisualMedia: ChatMediaAttachment? {
        target?.media.first { $0.kind == .image || $0.kind == .video }
    }

    @ViewBuilder
    private var mediaThumbnail: some View {
        if let media = firstVisualMedia {
            let size: CGFloat = 34
            Group {
                if media.kind == .image,
                   let path = attachmentLocalURL(media)?.path {
                    VerifiedLocalImageView(path: path) {
                        replyMediaPlaceholder(media)
                    }
                } else if media.kind == .video,
                          let path = attachmentLocalURL(media)?.path {
                    LocalVideoThumbnailView(path: path) {
                        replyMediaPlaceholder(media)
                    }
                } else {
                    replyMediaPlaceholder(media)
                }
            }
            .frame(width: size, height: size)
            .clipShape(RoundedRectangle(cornerRadius: 5, style: .continuous))
            .accessibilityHidden(true)
        }
    }

    private func replyMediaPlaceholder(_ media: ChatMediaAttachment) -> some View {
        RoundedRectangle(cornerRadius: 5, style: .continuous)
            .fill(isMine ? .white.opacity(0.18) : Color(uiColor: .systemGroupedBackground))
            .overlay {
                Image(systemName: iconName(for: media.kind))
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(isMine ? .white.opacity(0.82) : .secondary)
            }
    }

    private var accessibilityLabel: String {
        if target == nil {
            return "Reply preview, original message unavailable"
        }
        return "Reply preview, jump to original message"
    }
}

private struct ChatMediaGrid: View {
    let attachments: [ChatMediaAttachment]
    let isMine: Bool
    let onDownloadAttachment: (ChatMediaAttachment) -> Void
    let onOpenAttachment: (ChatMediaAttachment) -> Void

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
                if attachment.kind == .voiceNote {
                    VoiceAttachmentRow(
                        attachment: attachment,
                        isMine: isMine,
                        onDownload: {
                            onDownloadAttachment(attachment)
                        }
                    )
                        .padding(.horizontal, 8)
                        .padding(.vertical, 6)
                } else {
                    FileAttachmentRow(
                        attachment: attachment,
                        isMine: isMine,
                        onDownload: {
                            onDownloadAttachment(attachment)
                        },
                        onOpen: {
                            onOpenAttachment(attachment)
                        }
                    )
                        .padding(.horizontal, 8)
                        .padding(.vertical, 6)
                }
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
                mediaTile(attachments[0])
                    .frame(width: width, height: geometry.size.height)

            case 2:
                HStack(spacing: spacing) {
                    mediaTile(attachments[0])
                        .frame(width: halfWidth)
                    mediaTile(attachments[1])
                        .frame(width: halfWidth)
                }

            case 3:
                HStack(spacing: spacing) {
                    mediaTile(attachments[0])
                        .frame(width: halfWidth)
                    VStack(spacing: spacing) {
                        mediaTile(attachments[1])
                        mediaTile(attachments[2])
                    }
                    .frame(width: halfWidth)
                }

            default:
                VStack(spacing: spacing) {
                    HStack(spacing: spacing) {
                        mediaTile(attachments[0])
                            .frame(width: halfWidth)
                        mediaTile(attachments[1])
                            .frame(width: halfWidth)
                    }
                    HStack(spacing: spacing) {
                        mediaTile(attachments[2])
                            .frame(width: halfWidth)
                        mediaTile(attachments[3])
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

    private func mediaTile(_ attachment: ChatMediaAttachment) -> MediaTile {
        MediaTile(
            attachment: attachment,
            isMine: isMine,
            onDownload: {
                onDownloadAttachment(attachment)
            },
            onOpen: {
                onOpenAttachment(attachment)
            }
        )
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
    let onDownload: () -> Void
    let onOpen: () -> Void

    var body: some View {
        ZStack {
            if attachment.kind == .image, let path = localPath {
                VerifiedLocalImageView(path: path) {
                    mediaPlaceholder
                }
            } else if attachment.kind == .video, let path = localPath {
                LocalVideoThumbnailView(path: path) {
                    mediaPlaceholder
                }
            } else {
                mediaPlaceholder
            }

            if attachment.kind == .video, localPath != nil {
                Image(systemName: "play.fill")
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(.white)
                    .padding(12)
                    .background(.black.opacity(0.42), in: Circle())
            }

            if let uploadProgress {
                AttachmentProgressOverlay(
                    progress: uploadProgress,
                    accessibilityLabel: "Uploading attachment"
                )
            } else if isDownloading {
                AttachmentProgressOverlay(
                    progress: downloadProgress,
                    accessibilityLabel: "Downloading attachment"
                )
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .clipped()
        .contentShape(Rectangle())
        .onTapGesture {
            if localPath != nil {
                onOpen()
            } else if canDownload {
                onDownload()
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(mediaLabel(for: attachment.kind))
        .task(id: attachment.attachmentId) {
            if canDownload {
                onDownload()
            }
        }
    }

    private var localPath: String? {
        guard let path = attachment.localPath?.trimmingCharacters(in: .whitespacesAndNewlines),
              !path.isEmpty
        else {
            return nil
        }
        return path
    }

    private var canDownload: Bool {
        guard !isDownloading,
              localPath == nil,
              let url = attachment.url?.trimmingCharacters(in: .whitespacesAndNewlines)
        else {
            return false
        }
        return !url.isEmpty
    }

    private var uploadProgress: Double? {
        guard let progress = attachment.uploadProgressPerMille else { return nil }
        return Double(min(progress, 1_000)) / 1_000
    }

    private var isDownloading: Bool {
        attachment.downloadProgressPerMille != nil
    }

    private var downloadProgress: Double? {
        guard let progress = attachment.downloadProgressPerMille else { return nil }
        if progress == 0 {
            return nil
        }
        return Double(min(progress, 1_000)) / 1_000
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

struct VerifiedLocalImageView<Placeholder: View>: View {
    let path: String
    let placeholder: () -> Placeholder

    @Environment(\.displayScale) private var displayScale
    @State private var image: CGImage?

    var body: some View {
        GeometryReader { geometry in
            let maxPixelSize = max(64, Int(max(geometry.size.width, geometry.size.height) * displayScale))
            ZStack {
                if let image {
                    Image(decorative: image, scale: displayScale)
                        .resizable()
                        .scaledToFill()
                } else {
                    placeholder()
                }
            }
            .frame(width: geometry.size.width, height: geometry.size.height)
            .clipped()
            .task(id: "\(path)|\(maxPixelSize)") {
                image = await LocalImageLoader.thumbnail(path: path, maxPixelSize: maxPixelSize)
            }
        }
    }
}

private enum LocalImageLoader {
    nonisolated static func thumbnail(path: String, maxPixelSize: Int) async -> CGImage? {
        await Task.detached(priority: .utility) {
            let url = URL(fileURLWithPath: path) as CFURL
            guard let source = CGImageSourceCreateWithURL(url, nil) else {
                return nil
            }
            let options: [CFString: Any] = [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: max(1, maxPixelSize)
            ]
            return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
        }.value
    }
}

private struct FileAttachmentRow: View {
    let attachment: ChatMediaAttachment
    let isMine: Bool
    let onDownload: () -> Void
    let onOpen: () -> Void

    var body: some View {
        Button {
            if hasLocalPath {
                onOpen()
            } else if canDownload {
                onDownload()
            }
        } label: {
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
                    Text(detailText)
                        .font(.caption)
                        .foregroundStyle(isMine ? .white.opacity(0.72) : .secondary)
                        .lineLimit(1)
                    if let uploadProgress {
                        ProgressView(value: uploadProgress)
                            .tint(isMine ? .white : .accentColor)
                    } else if isDownloading {
                        ProgressView()
                            .tint(isMine ? .white : .accentColor)
                    }
                }

                Spacer(minLength: 4)

                Image(systemName: accessoryIconName)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(isMine ? .white.opacity(0.82) : .accentColor)
            }
            .foregroundStyle(isMine ? .white : .primary)
        }
        .buttonStyle(.plain)
    }

    private var hasLocalPath: Bool {
        attachmentLocalURL(attachment) != nil
    }

    private var canDownload: Bool {
        attachmentCanDownload(attachment)
    }

    private var uploadProgress: Double? {
        guard let progress = attachment.uploadProgressPerMille else { return nil }
        return Double(min(progress, 1_000)) / 1_000
    }

    private var isDownloading: Bool {
        attachment.downloadProgressPerMille != nil
    }

    private var detailText: String {
        if let uploadProgress {
            return "Uploading \(Int(uploadProgress * 100))%"
        }
        if isDownloading {
            return "Downloading..."
        }
        if hasLocalPath {
            return "Tap to open"
        }
        if canDownload {
            return "Tap to download"
        }
        return attachment.mimeType.isEmpty ? mediaLabel(for: attachment.kind) : attachment.mimeType
    }

    private var accessoryIconName: String {
        if uploadProgress != nil {
            return "arrow.up.circle"
        }
        if isDownloading {
            return "arrow.down.circle"
        }
        if hasLocalPath {
            return "eye"
        }
        if canDownload {
            return "arrow.down.circle"
        }
        return "info.circle"
    }
}

private struct AttachmentProgressOverlay: View {
    let progress: Double?
    let accessibilityLabel: String

    var body: some View {
        ZStack {
            Color.black.opacity(0.36)
            if let progress {
                ProgressView(value: progress)
                    .progressViewStyle(.circular)
                    .tint(.white)
                    .scaleEffect(1.25)
            } else {
                ProgressView()
                    .progressViewStyle(.circular)
                    .tint(.white)
                    .scaleEffect(1.25)
            }
        }
        .accessibilityLabel(accessibilityLabel)
    }
}

private struct MessageStatusLine: View {
    let message: ChatMessage
    let onRetryMessage: (ChatMessage) -> Void

    var body: some View {
        if isFailed {
            HStack(spacing: 6) {
                Button {
                    onRetryMessage(message)
                } label: {
                    Image(systemName: "arrow.clockwise.circle.fill")
                        .font(.caption.weight(.semibold))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Retry message")

                Text("Not sent")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        } else if let text = readReceiptText ?? deliveryText {
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
        case .failed:
            return "Not sent"
        }
    }

    private var isFailed: Bool {
        if case .failed = message.delivery {
            return true
        }
        return false
    }
}

private struct ChatTypingIndicatorRow: View {
    let members: [AppTypingMember]

    private let avatarSize: CGFloat = 28

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            ChatAvatar(
                title: primaryName,
                subtitle: primaryMember?.npub ?? primaryMember?.accountId ?? "typing",
                size: avatarSize
            )
            .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 4) {
                TypingDotsBubble()

                Text(label)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Spacer(minLength: 44)
        }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
    }

    private var primaryMember: AppTypingMember? {
        members.first
    }

    private var primaryName: String {
        let trimmed = primaryMember?.displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        if let trimmed, !trimmed.isEmpty {
            return trimmed
        }
        return primaryMember?.deviceId ?? "Someone"
    }

    private var label: String {
        if members.count <= 1 {
            return "\(primaryName) is typing"
        }
        return "\(primaryName) and \(members.count - 1) others are typing"
    }
}

private struct TypingDotsBubble: View {
    var body: some View {
        TimelineView(.animation) { context in
            HStack(spacing: 4) {
                ForEach(0..<3, id: \.self) { index in
                    Circle()
                        .fill(Color.secondary)
                        .frame(width: 5, height: 5)
                        .opacity(dotOpacity(index: index, date: context.date))
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .background(Color(uiColor: .secondarySystemGroupedBackground), in: Capsule())
        }
    }

    private func dotOpacity(index: Int, date: Date) -> Double {
        let beat = (date.timeIntervalSinceReferenceDate * 2.4 + Double(index) * 0.42)
            .truncatingRemainder(dividingBy: 1)
        return beat < 0.5 ? 1 : 0.34
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

func mediaLabel(for kind: ChatMediaKind) -> String {
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

func iconName(for kind: ChatMediaKind) -> String {
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
