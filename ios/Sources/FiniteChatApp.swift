import SwiftUI

@main
struct FiniteChatApp: App {
    @StateObject private var model = AppModel(requiresNostrLogin: true)

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
        }
    }
}
