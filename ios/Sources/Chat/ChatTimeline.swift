import Foundation

struct ChatTimelineMessageGroup: Identifiable, Equatable {
    let senderAccountId: String
    let senderDeviceId: String
    let senderDisplayName: String
    let senderNpub: String?
    let isMine: Bool
    var messages: [ChatMessage]

    var id: String {
        messages.first?.messageId ?? "\(senderAccountId)/\(senderDeviceId)"
    }
}

enum ChatTimelineRow: Identifiable, Equatable {
    case messageGroup(ChatTimelineMessageGroup)

    var id: String {
        switch self {
        case .messageGroup(let group):
            "group-\(group.id)"
        }
    }
}

enum ChatTimeline {
    static func rows(messages: [ChatMessage]) -> [ChatTimelineRow] {
        let ordered = messages.sorted {
            if $0.seq == $1.seq {
                return $0.messageId < $1.messageId
            }
            return $0.seq < $1.seq
        }

        var rows: [ChatTimelineRow] = []
        rows.reserveCapacity(ordered.count)

        for message in ordered {
            if let lastIndex = rows.indices.last,
               case .messageGroup(var group) = rows[lastIndex],
               group.senderAccountId == message.senderAccountId,
               group.senderDeviceId == message.senderDeviceId,
               group.isMine == message.isMine,
               message.replyToMessageId == nil
            {
                group.messages.append(message)
                rows[lastIndex] = .messageGroup(group)
                continue
            }

            rows.append(
                .messageGroup(
                    ChatTimelineMessageGroup(
                        senderAccountId: message.senderAccountId,
                        senderDeviceId: message.senderDeviceId,
                        senderDisplayName: message.senderDisplayName,
                        senderNpub: message.senderNpub,
                        isMine: message.isMine,
                        messages: [message]
                    )
                )
            )
        }

        return rows
    }

    static func messagesById(_ messages: [ChatMessage]) -> [String: ChatMessage] {
        Dictionary(uniqueKeysWithValues: messages.map { ($0.messageId, $0) })
    }
}
