import SwiftUI

@main
struct WrymeApp: App {
    init() {
        Core.shared.start()
    }

    var body: some Scene {
        #if os(macOS)
        WindowGroup {
            Root().frame(minWidth: 420, minHeight: 360)
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 860, height: 640)
        .commands {
            CommandGroup(replacing: .appSettings) {
                Button("Settings…") { Core.shared.key("s", ctrl: true) }
                    .keyboardShortcut(",")
            }
        }
        #else
        WindowGroup {
            Root()
        }
        #endif
    }
}
