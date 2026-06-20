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
    @State private var unavailableInviteProfile: NostrFollowProfile?
    @State private var checkingInviteProfileID: String?
    @State private var inviteAvailabilityCheckFailed = false
    @State private var showingLookupProfile = false

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
            followsSection
        }
        .listStyle(.plain)
        .navigationTitle("People")
        .toolbar {
            ShellToolbarActions(showScan: showScan, showSettings: showSettings)
        }
        .searchable(
            text: $searchText,
            placement: .navigationBarDrawer(displayMode: .automatic),
            prompt: "Search people"
        )
        .task(id: "\(model.nostrIdentity?.accountID ?? "")|\(model.serverURL)") {
            await people.loadIfNeeded(identity: model.nostrIdentity, serverURL: model.serverURL)
        }
        .refreshable {
            await people.refresh(identity: model.nostrIdentity, serverURL: model.serverURL)
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
        .sheet(item: $unavailableInviteProfile) { profile in
            InviteUnavailableSheet(profile: profile)
        }
        .alert("Could not check invite availability", isPresented: $inviteAvailabilityCheckFailed) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("Try again when the FiniteChat server is reachable.")
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
    }

    @ViewBuilder
    private var followsSection: some View {
        if people.isLoading && people.profiles.isEmpty {
            HStack {
                Spacer()
                ProgressView("Loading people...")
                Spacer()
            }
            .padding(.vertical, 16)
            .listRowSeparator(.hidden)
        } else if people.profiles.isEmpty {
            ContentUnavailableView(
                "No people yet",
                systemImage: "person.crop.circle.badge.questionmark",
                description: Text(people.statusText ?? "Pull to refresh.")
            )
                .padding(.vertical, 18)
                .listRowSeparator(.hidden)
        } else if filteredProfiles.isEmpty {
            ContentUnavailableView("No matches", systemImage: "magnifyingglass")
                .padding(.vertical, 18)
                .listRowSeparator(.hidden)
        } else {
            ForEach(filteredProfiles) { profile in
                Button {
                    selectFollow(profile)
                } label: {
                    NostrProfileRow(
                        profile: profile,
                        isChecking: checkingInviteProfileID == profile.id
                    )
                        .padding(.vertical, 6)
                }
                .buttonStyle(.plain)
            }
        }
    }

    private func selectFollow(_ profile: NostrFollowProfile) {
        guard profile.inviteAvailability == .unavailable else {
            selectedFollow = profile
            return
        }
        checkingInviteProfileID = profile.id
        Task { @MainActor in
            defer { checkingInviteProfileID = nil }
            do {
                let updated = try await people.recheckInviteAvailability(
                    for: profile,
                    serverURL: model.serverURL
                )
                if updated.inviteAvailability == .available {
                    selectedFollow = updated
                } else {
                    unavailableInviteProfile = updated
                }
            } catch {
                inviteAvailabilityCheckFailed = true
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
    let showSettings: () -> Void
    @State private var searchText = ""

    private var agentRooms: [AppRoomSummary] {
        model.rooms.filter { room in
            let name = room.displayName.lowercased()
            return name.contains("agent") || name.contains("hermes")
        }
    }

    private var filteredAgentRooms: [AppRoomSummary] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !query.isEmpty else { return agentRooms }
        return agentRooms.filter { room in
            room.displayName.lowercased().contains(query)
                || room.lastMessagePreview.lowercased().contains(query)
                || room.userStatusText.lowercased().contains(query)
        }
    }

    var body: some View {
        List {
            if agentRooms.isEmpty {
                ContentUnavailableView("No agents yet", systemImage: "sparkles")
                    .padding(.vertical, 18)
                    .listRowSeparator(.hidden)
            } else if filteredAgentRooms.isEmpty {
                ContentUnavailableView("No matching agents", systemImage: "magnifyingglass")
                    .padding(.vertical, 18)
                    .listRowSeparator(.hidden)
            } else {
                ForEach(filteredAgentRooms, id: \.roomId) { room in
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
        .listStyle(.plain)
        .navigationTitle("Agents")
        .toolbar {
            ShellToolbarActions(showScan: showScan, showSettings: showSettings)
        }
        .searchable(
            text: $searchText,
            placement: .navigationBarDrawer(displayMode: .automatic),
            prompt: "Search agents"
        )
    }
}

struct ShellToolbarActions: ToolbarContent {
    let showScan: () -> Void
    let showSettings: () -> Void

    var body: some ToolbarContent {
        ToolbarItemGroup(placement: .topBarTrailing) {
            Button(action: showScan) {
                Image(systemName: "qrcode.viewfinder")
            }
            .accessibilityLabel("Scan")
            .accessibilityIdentifier("TopScanButton")

            Button(action: showSettings) {
                Image(systemName: "gearshape")
            }
            .accessibilityLabel("Settings")
            .accessibilityIdentifier("TopSettingsButton")
        }
    }
}

struct MyNostrProfileSheet: View {
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

private struct InviteUnavailableSheet: View {
    @Environment(\.dismiss) private var dismiss
    let profile: NostrFollowProfile

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
                    Text("This person doesn't have FiniteChat yet.")
                        .foregroundStyle(.secondary)

                    ShareLink(item: finiteChatInstallInviteURL(for: profile)) {
                        Label("Send Invite Link", systemImage: "square.and.arrow.up")
                    }
                }
            }
            .navigationTitle("Invite")
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
    let isChecking: Bool

    var body: some View {
        let isUnavailable = profile.inviteAvailability == .unavailable

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

            if isChecking {
                ProgressView()
                    .controlSize(.small)
            }
        }
        .opacity(isUnavailable ? 0.45 : 1)
        .saturation(isUnavailable ? 0 : 1)
        .contentShape(Rectangle())
        .accessibilityValue(isUnavailable ? "Invite unavailable" : "")
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

private func shortenedNpub(_ npub: String) -> String {
    guard npub.count > 18 else { return npub }
    return "\(npub.prefix(10))...\(npub.suffix(4))"
}

private func finiteChatInstallInviteURL(for profile: NostrFollowProfile) -> URL {
    var components = URLComponents(string: "https://chat.finite.computer/invite")!
    components.queryItems = [
        URLQueryItem(name: "npub", value: profile.npub),
    ]
    return components.url!
}
