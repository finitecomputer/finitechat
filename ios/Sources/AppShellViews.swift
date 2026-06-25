import SwiftUI
import UIKit

enum AppTab: Hashable {
    case chats
    case people
    case agents
    case home

    var title: String {
        switch self {
        case .chats:
            "Chats"
        case .people:
            "People"
        case .agents:
            "Agents"
        case .home:
            "New"
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
        case .home:
            "plus.circle.fill"
        }
    }

    var accessibilityIdentifier: String {
        switch self {
        case .chats:
            "ChatsTab"
        case .people:
            "PeopleTab"
        case .agents:
            "AgentsTab"
        case .home:
            "NewTab"
        }
    }
}

struct HomeView: View {
    @ObservedObject var model: AppModel
    let openChats: () -> Void
    let openPeople: () -> Void
    let openAgents: () -> Void
    let openRoom: (AppRoomSummary) -> Void
    let showScan: () -> Void
    let showSettings: () -> Void

    @State private var intentionText = ""
    @FocusState private var intentionFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(spacing: 28) {
                    Spacer(minLength: 96)

                    VStack(spacing: 16) {
                        FiniteLogoMark()
                            .fill(.tint)
                            .frame(width: 104, height: 104)
                            .accessibilityLabel("Finite logo")

                        Text("It's time to build")
                            .font(.title2.weight(.semibold))
                    }
                    .frame(maxWidth: .infinity)

                    Spacer(minLength: 220)
                }
                .padding(.horizontal)
                .padding(.top, 24)
                .padding(.bottom, 32)
            }
            .scrollDismissesKeyboard(.interactively)

            NoticeBar(text: model.actionNoticeText)

            HomeInputDock(
                text: $intentionText,
                isFocused: $intentionFocused,
                canSubmit: canSubmitIntention,
                openChats: openChats,
                openPeople: openPeople,
                openAgents: openAgents,
                showScan: showScan,
                submit: submitIntention
            )
        }
        .navigationTitle("Home")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(action: showSettings) {
                    Image(systemName: "person.crop.circle")
                }
                .accessibilityLabel("Profile")
                .accessibilityIdentifier("HomeProfileButton")
            }
        }
    }

    private var canSubmitIntention: Bool {
        !intentionText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func submitIntention() {
        let name = intentionText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        let existingRoomIDs = Set(model.rooms.map(\.roomId))
        model.roomDraft = name
        model.createRoom()
        intentionFocused = false
        if let room = model.rooms.first(where: { !existingRoomIDs.contains($0.roomId) }) {
            intentionText = ""
            openRoom(room)
        } else {
            openChats()
        }
    }
}

private struct FiniteLogoMark: Shape {
    private struct Bar {
        let x: CGFloat
        let y: CGFloat
        let width: CGFloat
        let height: CGFloat
        let radius: CGFloat
    }

    private static let bars: [Bar] = [
        Bar(x: 45.3336, y: 69.3335, width: 10.6667, height: 2.66668, radius: 1.33334),
        Bar(x: 15.9998, y: 69.3335, width: 10.6667, height: 2.66668, radius: 1.33334),
        Bar(x: 5.33289, y: 63.999, width: 21.3334, height: 2.66668, radius: 1.33334),
        Bar(x: 45.3336, y: 63.999, width: 21.3334, height: 2.66668, radius: 1.33334),
        Bar(x: 47.1108, y: 58.6675, width: 23.1112, height: 2.66668, radius: 1.33334),
        Bar(x: 1.77722, y: 58.6675, width: 22.2223, height: 2.66668, radius: 1.33334),
        Bar(x: 48.8887, y: 53.3335, width: 23.1112, height: 2.66668, radius: 1.33334),
        Bar(x: 0, y: 53.3335, width: 18.6667, height: 2.66668, radius: 1.33334),
        Bar(x: 49.7778, y: 47.9995, width: 22.2223, height: 2.66668, radius: 1.33334),
        Bar(x: 0, y: 47.9995, width: 13.3334, height: 2.66668, radius: 1.33334),
        Bar(x: 50.6665, y: 42.6655, width: 21.3334, height: 2.66668, radius: 1.33334),
        Bar(x: 0, y: 42.6655, width: 9.77782, height: 2.66668, radius: 1.33334),
        Bar(x: 18.6669, y: 42.6655, width: 6.22225, height: 2.66668, radius: 1.33334),
        Bar(x: 52.4449, y: 37.334, width: 19.5556, height: 2.66668, radius: 1.33334),
        Bar(x: 0, y: 37.334, width: 24.889, height: 2.66668, radius: 1.33334),
        Bar(x: 29.3331, y: 31.9995, width: 3.55557, height: 2.66668, radius: 1.33334),
        Bar(x: 38.2223, y: 31.9995, width: 2.66668, height: 2.66668, radius: 1.33334),
        Bar(x: 45.3336, y: 31.9995, width: 3.55557, height: 2.66668, radius: 1.33334),
        Bar(x: 54.2222, y: 31.9995, width: 17.7779, height: 2.66668, radius: 1.33334),
        Bar(x: 0, y: 31.9995, width: 24.0001, height: 2.66668, radius: 1.33334),
        Bar(x: 56.0006, y: 26.668, width: 16.0001, height: 2.66668, radius: 1.33334),
        Bar(x: 46.2222, y: 26.668, width: 5.33336, height: 2.66668, radius: 1.33334),
        Bar(x: 37.3337, y: 26.668, width: 4.44446, height: 2.66668, radius: 1.33334),
        Bar(x: 28.4452, y: 26.668, width: 3.55557, height: 2.66668, radius: 1.33334),
        Bar(x: 0, y: 26.668, width: 22.2223, height: 2.66668, radius: 1.33334),
        Bar(x: 0, y: 21.334, width: 21.3334, height: 2.66668, radius: 1.33334),
        Bar(x: 47.1115, y: 21.334, width: 24.889, height: 2.66668, radius: 1.33334),
        Bar(x: 26.6667, y: 21.334, width: 5.33336, height: 2.66668, radius: 1.33334),
        Bar(x: 37.3337, y: 21.334, width: 5.33336, height: 2.66668, radius: 1.33334),
        Bar(x: 0, y: 16.0005, width: 21.3334, height: 2.66668, radius: 1.33334),
        Bar(x: 25.3334, y: 16.0005, width: 7.11114, height: 2.66668, radius: 1.33334),
        Bar(x: 37.3337, y: 16.0005, width: 6.22225, height: 2.66668, radius: 1.33334),
        Bar(x: 48, y: 16.0005, width: 24.0001, height: 2.66668, radius: 1.33334),
        Bar(x: 37.3337, y: 10.6655, width: 32.889, height: 2.66668, radius: 1.33334),
        Bar(x: 1.77783, y: 10.6655, width: 30.889, height: 2.66668, radius: 1.33334),
        Bar(x: 5.33289, y: 5.33154, width: 61.3336, height: 2.66668, radius: 1.33334),
        Bar(x: 15.9998, y: 0, width: 40.0002, height: 2.66668, radius: 1.33334)
    ]

    func path(in rect: CGRect) -> Path {
        let side = min(rect.width, rect.height)
        let scale = side / 72
        let origin = CGPoint(
            x: rect.midX - side / 2,
            y: rect.midY - side / 2
        )
        var path = Path()

        for bar in Self.bars {
            path.addRoundedRect(
                in: CGRect(
                    x: origin.x + bar.x * scale,
                    y: origin.y + bar.y * scale,
                    width: bar.width * scale,
                    height: bar.height * scale
                ),
                cornerSize: CGSize(
                    width: bar.radius * scale,
                    height: bar.radius * scale
                )
            )
        }

        return path
    }
}

private struct HomeInputDock: View {
    @Binding var text: String
    let isFocused: FocusState<Bool>.Binding
    let canSubmit: Bool
    let openChats: () -> Void
    let openPeople: () -> Void
    let openAgents: () -> Void
    let showScan: () -> Void
    let submit: () -> Void

    var body: some View {
        glassContainer {
            VStack(spacing: 8) {
                HStack(spacing: 10) {
                    HomeSuggestionButton(
                        title: "Message someone",
                        systemImage: "person",
                        action: openPeople
                    )

                    HomeSuggestionButton(
                        title: "Chat with Agent",
                        systemImage: "sparkles",
                        action: openAgents
                    )
                }

                HomeIntentionComposer(
                    text: $text,
                    isFocused: isFocused,
                    canSubmit: canSubmit,
                    showScan: showScan,
                    submit: submit
                )
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
            .padding(.bottom, 10)
        }
    }

    @ViewBuilder
    private func glassContainer<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        if #available(iOS 26.0, *) {
            GlassEffectContainer(spacing: 10) {
                content()
            }
        } else {
            content()
        }
    }
}

private struct HomeSuggestionButton: View {
    let title: String
    let systemImage: String
    let action: () -> Void

    var body: some View {
        if #available(iOS 26.0, *) {
            button
                .buttonStyle(.glass)
                .controlSize(.small)
        } else {
            button
                .buttonStyle(.bordered)
                .buttonBorderShape(.capsule)
                .controlSize(.small)
        }
    }

    private var button: some View {
        Button(action: action) {
            Label(title, systemImage: systemImage)
                .font(.subheadline.weight(.medium))
                .lineLimit(1)
                .padding(.horizontal, 4)
        }
        .accessibilityIdentifier("HomeSuggestion\(title.replacingOccurrences(of: " ", with: ""))")
    }
}

private struct HomeIntentionComposer: View {
    @Binding var text: String
    let isFocused: FocusState<Bool>.Binding
    let canSubmit: Bool
    let showScan: () -> Void
    let submit: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ZStack(alignment: .topLeading) {
                if text.isEmpty {
                    Text("What's your intention?")
                        .font(.body)
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 8)
                }

                TextEditor(text: $text)
                    .font(.body)
                    .frame(height: editorHeight)
                    .scrollContentBackground(.hidden)
                    .background(Color.clear)
                    .focused(isFocused)
                    .accessibilityIdentifier("HomeIntentionField")
            }
            .padding(.horizontal, 12)
            .padding(.top, 8)

            HStack(spacing: 12) {
                Menu {
                    Button {
                        showScan()
                    } label: {
                        Label("Scan code", systemImage: "qrcode.viewfinder")
                    }
                } label: {
                    Image(systemName: "plus")
                        .font(.title3.weight(.regular))
                        .frame(width: 34, height: 34)
                        .contentShape(Circle())
                }
                .accessibilityLabel("Add attachment")

                Spacer()

                Button {
                } label: {
                    Image(systemName: "mic")
                        .font(.title3.weight(.regular))
                        .frame(width: 34, height: 34)
                        .contentShape(Circle())
                }
                .accessibilityLabel("Voice message")

                if canSubmit {
                    Button(action: submit) {
                        Image(systemName: "arrow.up")
                            .font(.body.weight(.bold))
                            .foregroundStyle(.white)
                            .frame(width: 34, height: 34)
                            .background(Circle().fill(Color.accentColor))
                    }
                    .accessibilityLabel("Send")
                    .transition(.scale.combined(with: .opacity))
                }
            }
            .foregroundStyle(.primary)
            .padding(.horizontal, 14)
            .padding(.bottom, 10)
        }
        .frame(maxWidth: .infinity, minHeight: 92, alignment: .topLeading)
        .modifier(HomeComposerSurface())
        .animation(.snappy(duration: 0.18), value: canSubmit)
        .animation(.snappy(duration: 0.18), value: editorHeight)
    }

    private var editorHeight: CGFloat {
        let explicitLineCount = text.split(separator: "\n", omittingEmptySubsequences: false).count
        let wrappedLineEstimate = Int(ceil(Double(text.count) / 34.0))
        let lineCount = min(5, max(1, explicitLineCount, wrappedLineEstimate))
        return CGFloat(lineCount) * 22 + 18
    }
}

private struct HomeComposerSurface: ViewModifier {
    func body(content: Content) -> some View {
        if #available(iOS 26.0, *) {
            content
                .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 28))
        } else {
            content
                .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 28, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 28, style: .continuous)
                        .strokeBorder(Color(.separator).opacity(0.18), lineWidth: 0.5)
                }
                .shadow(color: .black.opacity(0.08), radius: 18, x: 0, y: 8)
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

                if let error = model.developerErrorText {
                    Section {
                        Text(error)
                            .font(.footnote)
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("Finite Chat")
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
    let startProfileChat: (AppProfileSummary) -> Bool
    let showScan: () -> Void
    let showSettings: () -> Void

    @StateObject private var people = NostrPeopleModel()
    @State private var searchText = ""
    @State private var selectedFollow: NostrFollowProfile?
    @State private var showingLookupProfile = false

    private var knownProfiles: [AppProfileSummary] {
        let selfAccountID = model.nostrIdentity?.accountID
        let followIDs = Set(people.profiles.map(\.pubkey))
        return (model.state?.profiles ?? [])
            .filter { profile in
                profile.accountId != selfAccountID && !followIDs.contains(profile.accountId)
            }
            .sorted { left, right in
                left.displayName.localizedCaseInsensitiveCompare(right.displayName) == .orderedAscending
            }
    }

    private var filteredKnownProfiles: [AppProfileSummary] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !query.isEmpty else { return knownProfiles }
        return knownProfiles.filter { profile in
            profile.displayName.lowercased().contains(query)
                || profile.npub.lowercased().contains(query)
                || profile.accountId.lowercased().contains(query)
                || (profile.about?.lowercased().contains(query) ?? false)
        }
    }

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
            knownProfilesSection
            followsSection
        }
        .listStyle(.plain)
        .navigationTitle("People")
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button(action: showScan) {
                    Image(systemName: "qrcode.viewfinder")
                }
                .accessibilityLabel("Scan code")
                .accessibilityIdentifier("PeopleScanButton")

                Button(action: showSettings) {
                    Image(systemName: "gearshape")
                }
                .accessibilityLabel("Settings")
                .accessibilityIdentifier("TopSettingsButton")
            }
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
                onStartChat: {
                    startChat(with: profile.appProfileSummary)
                }
            )
        }
        .sheet(isPresented: $showingLookupProfile) {
            if let profile = model.activeProfile {
                AppProfileLookupSheet(profile: profile) {
                    startChat(with: profile)
                }
            } else {
                ContentUnavailableView("Profile unavailable", systemImage: "person.crop.circle.badge.questionmark")
            }
        }
    }

    @ViewBuilder
    private var knownProfilesSection: some View {
        if !filteredKnownProfiles.isEmpty {
            Section("Known in Finite Chat") {
                ForEach(filteredKnownProfiles, id: \.accountId) { profile in
                    Button {
                        startChat(with: profile)
                    } label: {
                        KnownProfileRow(profile: profile)
                            .padding(.vertical, 6)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }

    @ViewBuilder
    private var followsSection: some View {
        if people.isLoading && people.profiles.isEmpty && knownProfiles.isEmpty {
            HStack {
                Spacer()
                ProgressView("Loading people...")
                Spacer()
            }
            .padding(.vertical, 16)
            .listRowSeparator(.hidden)
        } else if people.profiles.isEmpty && knownProfiles.isEmpty {
            ContentUnavailableView(
                "No people yet",
                systemImage: "person.crop.circle.badge.questionmark",
                description: Text(people.statusText ?? "Pull to refresh.")
            )
                .padding(.vertical, 18)
                .listRowSeparator(.hidden)
        } else if filteredProfiles.isEmpty && filteredKnownProfiles.isEmpty {
            ContentUnavailableView("No matches", systemImage: "magnifyingglass")
                .padding(.vertical, 18)
                .listRowSeparator(.hidden)
        } else {
            if !filteredProfiles.isEmpty {
                Section("Nostr Follows") {
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

    @discardableResult
    private func startChat(with profile: AppProfileSummary) -> Bool {
        startProfileChat(profile)
    }
}

struct AgentsView: View {
    @ObservedObject var model: AppModel
    let openRoom: (AppRoomSummary) -> Void
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
                        AgentRoomRow(room: room)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .listStyle(.plain)
        .navigationTitle("Agents")
        .toolbar {
            ShellToolbarActions(showSettings: showSettings)
        }
        .searchable(
            text: $searchText,
            placement: .navigationBarDrawer(displayMode: .automatic),
            prompt: "Search agents"
        )
    }
}

struct ShellToolbarActions: ToolbarContent {
    let showSettings: () -> Void

    var body: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) {
            Button(action: showSettings) {
                Image(systemName: "gearshape")
            }
            .accessibilityLabel("Settings")
            .accessibilityIdentifier("TopSettingsButton")
        }
    }
}

private struct AgentRoomRow: View {
    let room: AppRoomSummary

    var body: some View {
        HStack(spacing: 12) {
            ZStack(alignment: .bottomTrailing) {
                RoundedRectangle(cornerRadius: 10)
                    .fill(Color(.tertiarySystemFill))
                Text(initial)
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(.secondary)

                Circle()
                    .fill(room.state.tint)
                    .frame(width: 10, height: 10)
                    .overlay(Circle().stroke(Color(.systemBackground), lineWidth: 2))
                    .offset(x: 1, y: 1)
            }
            .frame(width: 42, height: 42)
            .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 3) {
                Text(room.displayName)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                Text(room.lastMessagePreview.isEmpty ? room.userStatusText : room.lastMessagePreview)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

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
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }

    private var initial: String {
        let trimmed = room.displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let first = trimmed.first else { return "#" }
        return String(first).uppercased()
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
    let onStartChat: () -> Bool

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
                        if onStartChat() {
                            dismiss()
                        }
                    } label: {
                        Label("Message", systemImage: "bubble.left.and.bubble.right")
                    }
                    .buttonStyle(.borderedProminent)

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
            .navigationTitle(profile.displayName)
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
    let onStartChat: () -> Bool
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
                        if onStartChat() {
                            dismiss()
                        }
                    } label: {
                        Label("Start Chat", systemImage: "bubble.left.and.bubble.right")
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

            Text(profile.shortenedNpub)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .contentShape(Rectangle())
    }
}

private struct KnownProfileRow: View {
    let profile: AppProfileSummary

    var body: some View {
        HStack(spacing: 12) {
            NostrAvatar(name: profile.displayName, pictureURL: profile.picture, size: 42)

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

            Image(systemName: "bubble.left.and.bubble.right")
                .font(.subheadline)
                .foregroundStyle(.secondary)
        }
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityValue(profile.stale ? "Cached profile" : "")
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
                CachedRemoteImage(url: url) { image in
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
