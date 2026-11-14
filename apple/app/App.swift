import SwiftUI

@main
struct WrymeApp: App {
    init() {
        #if os(macOS)
        NSWindow.allowsAutomaticWindowTabbing = false
        Keys.install()
        #else
        Folder.boot()
        #endif
    }

    var body: some Scene {
        #if os(macOS)
        WindowGroup {
            WindowRoot().frame(minWidth: 420, minHeight: 360)
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 860, height: 640)
        .commands { Menus() }
        #else
        WindowGroup {
            Root().environmentObject(Core.phone)
        }
        #endif
    }
}

#if os(macOS)
struct WindowRoot: View {
    @StateObject private var core = Core(link: Child())

    var body: some View {
        Root()
            .environmentObject(core)
            .focusedSceneObject(core)
            .background(
                WindowReader { window in
                    core.window = window
                    window?.isOpaque = false
                    window?.backgroundColor = .clear
                })
            .onAppear { core.start() }
            .onDisappear { core.stop() }
            .onChange(of: core.ended) { _, ended in
                if ended { core.window?.close() }
            }
    }
}

struct WindowReader: NSViewRepresentable {
    let found: (NSWindow?) -> Void

    final class Probe: NSView {
        var found: ((NSWindow?) -> Void)?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            found?(window)
        }
    }

    func makeNSView(context: Context) -> Probe {
        let probe = Probe()
        probe.found = found
        return probe
    }

    func updateNSView(_ probe: Probe, context: Context) {}
}
#endif
