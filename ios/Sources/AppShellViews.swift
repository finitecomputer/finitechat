import SwiftUI
import UIKit

enum AppTab: Hashable {
    case chats
    case people
    case agents

    var title: String {
        switch self {
        case .chats:
            "Chats"
        case .people:
            "People"
        case .agents:
            "Agents"
        }
    }

    var systemImage: String {
        switch self {
        case .chats:
            "bubble.left.and.bubble.right"
        case .people:
            "person.2"
        case .agents:
            "sparkles"
        }
    }
}

struct NostrLoginView: View {
    @ObservedObject var model: AppModel
    @State private var nsec = ""

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    SecureField("nsec1...", text: $nsec)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("NostrNsecField")

                    Button {
                        signIn()
                    } label: {
                        Label("Sign In", systemImage: "key")
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .disabled(trimmedNsec.isEmpty)
                    .accessibilityIdentifier("NostrSignInButton")

                    Button {
                        _ = model.createAndSignInNostrIdentity()
                    } label: {
                        Label("Create New Account", systemImage: "plus.circle")
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .accessibilityIdentifier("NostrCreateAccountButton")
                } header: {
                    Text("Nostr Account")
                }

                Section("Server") {
                    TextField("Server", text: $model.serverURL)
                        .textInputAutocapitalization(.never)
                        .keyboardType(.URL)
                    TextField("Device", text: $model.deviceID)
                        .textInputAutocapitalization(.never)
                }

                if let error = model.developerErrorText {
                    Section {
                        Text(error)
                            .font(.footnote)
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("FiniteChat")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Apply") {
                        model.applyDevSettings()
                    }
                }
            }
        }
    }

    private var trimmedNsec: String {
        nsec.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func signIn() {
        if model.signInWithNsec(trimmedNsec) {
            nsec = ""
        }
    }
}

struct PeopleView: View {
    @ObservedObject var model: AppModel
    let openRoom: (AppRoomSummary) -> Void
    let showScan: () -> Void
    let showSettings: () -> Void

    @StateObject private var people = NostrPeopleModel()
    @State private var searchText = ""
    @State private var selectedFollow: NostrFollowProfile?
    @State private var showingMyProfile = false
    @State private var showingManualLookup = false
    @State private var showingLookupProfile = false
    @State private var confirmingSignOut = false

    private var filteredProfiles: [NostrFollowProfile] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !query.isEmpty else { return people.profiles }
        return people.profiles.filter { profile in
            profile.displayName.lowercased().contains(query)
                || profile.npub.lowercased().contains(query)
                || profile.pubkey.lowercased().contains(query)
                || (profile.about?.lowercased().contains(query) ?? false)
        }
    }

    var body: some View {
        List {
            identitySection
            quickActionsSection
            followsSection
            accountSection
        }
        .listStyle(.insetGrouped)
        .navigationTitle("People")
        .searchable(text: $searchText, prompt: "Search follows")
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button {
                    Task { await people.refresh(identity: model.nostrIdentity) }
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .disabled(people.isLoading || model.nostrIdentity == nil)
                .accessibilityLabel("Refresh follows")

                Button {
                    showSettings()
                } label: {
                    Image(systemName: "gearshape")
                }
                .accessibilityLabel("Settings")
            }
        }
        .task(id: model.nostrIdentity?.accountID) {
            await people.loadIfNeeded(identity: model.nostrIdentity)
        }
        .refreshable {
            await people.refresh(identity: model.nostrIdentity)
        }
        .sheet(isPresented: $showingMyProfile) {
            MyNostrProfileSheet(identity: model.nostrIdentity, myNpub: model.myNpub)
        }
        .sheet(isPresented: $showingManualLookup) {
            ManualNpubLookupSheet { value in
                lookupProfile(value)
            }
        }
        .sheet(item: $selectedFollow) { profile in
            NostrFollowProfileSheet(
                profile: profile,
                onLookup: {
                    lookupProfile(profile.npub)
                },
                onCreateRoom: {
                    createRoom(named: "Chat with \(profile.displayName)")
                }
            )
        }
        .sheet(isPresented: $showingLookupProfile) {
            if let profile = model.activeProfile {
                AppProfileLookupSheet(profile: profile) {
                    createRoom(named: "Chat with \(profile.displayName)")
                }
            } else {
                ContentUnavailableView("Profile unavailable", systemImage: "person.crop.circle.badge.questionmark")
            }
        }
        .confirmationDialog(
            "Delete this device's FiniteChat data?",
            isPresented: $confirmingSignOut,
            titleVisibility: .visible
        ) {
            Button("Delete Everything", role: .destructive) {
                model.signOutAndDeleteEverything()
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This removes local chats, config, and the saved nsec from this device.")
        }
    }

    private var identitySection: some View {
        Section {
            HStack(spacing: 12) {
                NostrAvatar(
                    name: model.nostrIdentity?.npub ?? model.myNpub,
                    pictureURL: nil,
                    size: 48
                )

                VStack(alignment: .leading, spacing: 3) {
                    Text("My Profile")
                        .font(.body.weight(.semibold))
                    Text(model.myNpub.map(shortenedNpub) ?? "No profile code")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

                Spacer(minLength: 8)

                Button {
                    showingMyProfile = true
                } label: {
                    Image(systemName: "qrcode")
                        .frame(width: 34, height: 34)
                }
                .buttonStyle(.borderless)
                .accessibilityLabel("Show my profile code")
            }
            .padding(.vertical, 4)
        }
    }

    private var quickActionsSection: some View {
        Section {
            HStack(spacing: 8) {
                ShellActionButton(title: "Enter Code", systemImage: "keyboard", primary: true) {
                    showingManualLookup = true
                }

                ShellActionButton(title: "Paste", systemImage: "doc.on.clipboard") {
                    lookupProfile(UIPasteboard.general.string ?? "")
                }

                ShellActionButton(title: "Scan", systemImage: "qrcode.viewfinder") {
                    showScan()
                }
            }
            .padding(.vertical, 8)
        }
    }

    @ViewBuilder
    private var followsSection: some View {
        Section {
            if people.isLoading && people.profiles.isEmpty {
                HStack {
                    Spacer()
                    ProgressView("Loading follows...")
                    Spacer()
                }
                .padding(.vertical, 16)
            } else if people.profiles.isEmpty {
                ContentUnavailableView("No follows found", systemImage: "person.crop.circle.badge.questionmark")
                    .padding(.vertical, 18)
            } else if filteredProfiles.isEmpty {
                ContentUnavailableView("No matches", systemImage: "magnifyingglass")
                    .padding(.vertical, 18)
            } else {
                ForEach(filteredProfiles) { profile in
                    Button {
                        selectedFollow = profile
                    } label: {
                        NostrProfileRow(profile: profile)
                            .padding(.vertical, 6)
                    }
                    .buttonStyle(.plain)
                }
            }
        } header: {
            HStack(spacing: 6) {
                Text("Follows")
                if people.isLoading {
                    ProgressView()
                        .controlSize(.small)
                }
            }
        } footer: {
            if let status = people.statusText {
                Text(status)
            }
        }
    }

    private var accountSection: some View {
        Section {
            Button(role: .destructive) {
                confirmingSignOut = true
            } label: {
                Label("Sign Out and Delete Everything", systemImage: "trash")
            }
        }
    }

    private func lookupProfile(_ value: String) {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        model.scanDraft = trimmed
        _ = model.scanTarget()
        showingLookupProfile = model.activeProfile != nil
    }

    private func createRoom(named rawName: String) {
        let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
        model.roomDraft = name.isEmpty ? "New Chat" : name
        model.createRoom()
        if let room = model.selectedRoom {
            openRoom(room)
        }
    }
}

struct AgentsView: View {
    @ObservedObject var model: AppModel
    let openRoom: (AppRoomSummary) -> Void
    let showScan: () -> Void

    private var agentRooms: [AppRoomSummary] {
        model.rooms.filter { room in
            let name = room.displayName.lowercased()
            return name.contains("agent") || name.contains("hermes")
        }
    }

    var body: some View {
        List {
            Section {
                Button {
                    createFiniteAgentRoom()
                } label: {
                    Label("Create New Finite Agent", systemImage: "sparkles")
                }
                .accessibilityIdentifier("CreateFiniteAgentButton")

                Button {
                    showScan()
                } label: {
                    Label("Scan Hermes Invite", systemImage: "qrcode.viewfinder")
                }
            }

            Section("Agent Chats") {
                if agentRooms.isEmpty {
                    ContentUnavailableView("No agent chats yet", systemImage: "sparkles")
                        .padding(.vertical, 18)
                } else {
                    ForEach(agentRooms, id: \.roomId) { room in
                        Button {
                            openRoom(room)
                        } label: {
                            HStack(spacing: 12) {
                                Image(systemName: "sparkles")
                                    .frame(width: 40, height: 40)
                                    .background(Color(.tertiarySystemFill), in: Circle())
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(room.displayName)
                                        .foregroundStyle(.primary)
                                    Text(room.lastMessagePreview.isEmpty ? room.userStatusText : room.lastMessagePreview)
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                }
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }
            }

            Section("Coming Soon") {
                AgentComingSoonRow(title: "Clawi Agent", systemImage: "terminal")
                AgentComingSoonRow(title: "Maple Agent", systemImage: "leaf")
                AgentComingSoonRow(title: "Codex Session", systemImage: "laptopcomputer")
                AgentComingSoonRow(title: "Claude Session", systemImage: "text.bubble")
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Agents")
    }

    private func createFiniteAgentRoom() {
        model.roomDraft = "Finite Agent"
        model.createRoom()
        if let room = model.selectedRoom {
            openRoom(room)
        }
    }
}

private struct MyNostrProfileSheet: View {
    @Environment(\.dismiss) private var dismiss
    let identity: AppNostrIdentity?
    let myNpub: String?
    @State private var showingSecret = false
    @State private var copiedField: String?

    var body: some View {
        NavigationStack {
            Form {
                if let npub = myNpub {
                    Section {
                        QRCodeView(value: npub)
                            .frame(width: 220, height: 220)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 8)

                        CopyableValueRow(title: "npub", value: npub, copiedField: $copiedField)
                        ShareLink(item: npub) {
                            Label("Share Profile Code", systemImage: "square.and.arrow.up")
                        }
                    } header: {
                        Text("Profile Code")
                    }
                }

                if let identity {
                    Section {
                        if showingSecret {
                            CopyableValueRow(title: "nsec", value: identity.nsec, copiedField: $copiedField)
                        }

                        Button {
                            showingSecret.toggle()
                        } label: {
                            Label(showingSecret ? "Hide Secret Key" : "Show Secret Key", systemImage: showingSecret ? "eye.slash" : "eye")
                        }
                    } header: {
                        Text("Secret Key")
                    } footer: {
                        Text("The nsec signs in to this account. Anyone with it controls this identity.")
                    }
                }
            }
            .navigationTitle("My Profile")
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

private struct ManualNpubLookupSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var value = ""
    let onLookup: (String) -> Void

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("npub1...", text: $value, axis: .vertical)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .lineLimit(3...6)

                    Button {
                        value = UIPasteboard.general.string ?? ""
                    } label: {
                        Label("Paste", systemImage: "doc.on.clipboard")
                    }
                }
            }
            .navigationTitle("Profile Code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        dismiss()
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Lookup") {
                        onLookup(value)
                        dismiss()
                    }
                    .disabled(value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
    }
}

private struct NostrFollowProfileSheet: View {
    @Environment(\.dismiss) private var dismiss
    let profile: NostrFollowProfile
    let onLookup: () -> Void
    let onCreateRoom: () -> Void

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    NostrProfileHeader(
                        displayName: profile.displayName,
                        npub: profile.npub,
                        about: profile.about,
                        pictureURL: profile.pictureURL
                    )
                }
                .listRowBackground(Color.clear)

                Section {
                    Button {
                        onCreateRoom()
                        dismiss()
                    } label: {
                        Label("Create Chat Room", systemImage: "bubble.left.and.bubble.right")
                    }

                    Button {
                        dismiss()
                        Task { @MainActor in
                            onLookup()
                        }
                    } label: {
                        Label("Lookup Server Profile", systemImage: "person.text.rectangle")
                    }
                }

                Section("Profile Code") {
                    QRCodeView(value: profile.npub)
                        .frame(width: 220, height: 220)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 8)
                    CopyableValueRow(title: "npub", value: profile.npub, copiedField: .constant(nil))
                    ShareLink(item: profile.npub) {
                        Label("Share Profile Code", systemImage: "square.and.arrow.up")
                    }
                }
            }
            .navigationTitle("Profile")
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

private struct AppProfileLookupSheet: View {
    @Environment(\.dismiss) private var dismiss
    let profile: AppProfileSummary
    let onCreateRoom: () -> Void
    @State private var copiedField: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    NostrProfileHeader(
                        displayName: profile.displayName,
                        npub: profile.npub,
                        about: profile.about,
                        pictureURL: profile.picture
                    )
                }
                .listRowBackground(Color.clear)

                Section {
                    Button {
                        onCreateRoom()
                        dismiss()
                    } label: {
                        Label("Create Chat Room", systemImage: "bubble.left.and.bubble.right")
                    }
                }

                Section("Profile Code") {
                    QRCodeView(value: profile.npub)
                        .frame(width: 220, height: 220)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 8)
                    CopyableValueRow(title: "npub", value: profile.npub, copiedField: $copiedField)
                    ShareLink(item: profile.npub) {
                        Label("Share Profile Code", systemImage: "square.and.arrow.up")
                    }
                }
            }
            .navigationTitle("Profile")
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

private struct NostrProfileHeader: View {
    let displayName: String
    let npub: String
    let about: String?
    let pictureURL: String?

    var body: some View {
        VStack(spacing: 10) {
            NostrAvatar(name: displayName, pictureURL: pictureURL, size: 96)
                .frame(maxWidth: .infinity)

            Text(displayName)
                .font(.title3.weight(.semibold))
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity)

            Text(shortenedNpub(npub))
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity)

            if let about, !about.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                Text(about)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
                    .padding(.horizontal, 18)
            }
        }
        .padding(.vertical, 8)
    }
}

private struct NostrProfileRow: View {
    let profile: NostrFollowProfile

    var body: some View {
        HStack(spacing: 12) {
            NostrAvatar(name: profile.displayName, pictureURL: profile.pictureURL, size: 42)

            VStack(alignment: .leading, spacing: 3) {
                Text(profile.displayName)
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                Text(profile.about ?? shortenedNpub(profile.npub))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer(minLength: 8)
        }
        .contentShape(Rectangle())
    }
}

private struct NostrAvatar: View {
    let name: String?
    let pictureURL: String?
    let size: CGFloat

    var body: some View {
        ZStack {
            Circle()
                .fill(Color(.tertiarySystemFill))

            if let url = pictureURL.flatMap(URL.init(string:)) {
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
        .frame(width: size, height: size)
        .clipShape(Circle())
    }

    private var initials: some View {
        Text(initialText)
            .font(.system(size: max(13, size * 0.36), weight: .semibold))
            .foregroundStyle(.secondary)
    }

    private var initialText: String {
        let trimmed = name?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard let first = trimmed.first else { return "#" }
        return String(first).uppercased()
    }
}

private struct ShellActionButton: View {
    let title: String
    let systemImage: String
    var primary = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 6) {
                Image(systemName: systemImage)
                    .font(.body.weight(.semibold))
                Text(title)
                    .font(.caption.weight(.medium))
                    .lineLimit(1)
                    .minimumScaleFactor(0.75)
            }
            .frame(maxWidth: .infinity, minHeight: 58)
        }
        .buttonStyle(.plain)
        .foregroundStyle(primary ? .white : .primary)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(primary ? Color.accentColor : Color(.tertiarySystemGroupedBackground))
        )
    }
}

private struct CopyableValueRow: View {
    let title: String
    let value: String
    @Binding var copiedField: String?

    var body: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(value)
                    .font(.system(.footnote, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            Spacer(minLength: 8)

            Button {
                UIPasteboard.general.string = value
                copiedField = title
                Task { @MainActor in
                    try? await Task.sleep(nanoseconds: 1_200_000_000)
                    if copiedField == title {
                        copiedField = nil
                    }
                }
            } label: {
                Image(systemName: copiedField == title ? "checkmark.circle.fill" : "doc.on.doc")
                    .frame(width: 34, height: 34)
            }
            .buttonStyle(.borderless)
            .accessibilityLabel(copiedField == title ? "Copied \(title)" : "Copy \(title)")
        }
    }
}

private struct AgentComingSoonRow: View {
    let title: String
    let systemImage: String

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: systemImage)
                .frame(width: 32, height: 32)
                .foregroundStyle(.secondary)
            Text(title)
            Spacer()
            Text("Soon")
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
        }
    }
}

private func shortenedNpub(_ npub: String) -> String {
    guard npub.count > 18 else { return npub }
    return "\(npub.prefix(10))...\(npub.suffix(4))"
}
