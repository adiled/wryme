import SwiftUI

@main
struct WrymeApp: App {
    init() {
        #if os(macOS)
        Keys.install()
        Core.shared.start()
        #else
        Folder.boot()
        #endif
    }

    var body: some Scene {
        #if os(macOS)
        WindowGroup {
            Root().frame(minWidth: 420, minHeight: 360)
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 860, height: 640)
        .commands { Menus(core: Core.shared) }
        #else
        WindowGroup {
            Root()
        }
        #endif
    }
}
