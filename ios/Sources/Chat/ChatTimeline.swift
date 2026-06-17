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

struct ChatRoomProjection: Equatable {
    let roomID: String
    let messages: [ChatMessage]
    let rows: [ChatTimelineRow]
    let messagesById: [String: ChatMessage]

    static func empty(roomID: String) -> ChatRoomProjection {
        ChatRoomProjection(
            roomID: roomID,
            messages: [],
            rows: [],
            messagesById: [:]
        )
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

    var oldestMessageID: String? {
        switch self {
        case .messageGroup(let group):
            group.messages.first?.messageId
        }
    }
}

enum ChatTimeline {
    static func roomProjections(messages: [ChatMessage]) -> [String: ChatRoomProjection] {
        guard !messages.isEmpty else { return [:] }

        var messagesByRoom: [String: [ChatMessage]] = [:]
        messagesByRoom.reserveCapacity(8)
        for message in messages {
            messagesByRoom[message.roomId, default: []].append(message)
        }

        var projections: [String: ChatRoomProjection] = [:]
        projections.reserveCapacity(messagesByRoom.count)
        for (roomID, roomMessages) in messagesByRoom {
            let ordered = orderedMessages(roomMessages)
            projections[roomID] = ChatRoomProjection(
                roomID: roomID,
                messages: ordered,
                rows: rows(orderedMessages: ordered),
                messagesById: messagesById(ordered)
            )
        }
        return projections
    }

    static func rows(messages: [ChatMessage]) -> [ChatTimelineRow] {
        rows(orderedMessages: orderedMessages(messages))
    }

    static func messagesById(_ messages: [ChatMessage]) -> [String: ChatMessage] {
        Dictionary(uniqueKeysWithValues: messages.map { ($0.messageId, $0) })
    }

    private static func orderedMessages(_ messages: [ChatMessage]) -> [ChatMessage] {
        messages.sorted {
            if $0.seq == $1.seq {
                return $0.messageId < $1.messageId
            }
            return $0.seq < $1.seq
        }
    }

    private static func rows(orderedMessages ordered: [ChatMessage]) -> [ChatTimelineRow] {
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
}
