#if os(iOS)
import SwiftUI
import UniformTypeIdentifiers

enum Folder {
    private static let mark = "wryme.folder"
    private static let demo = "wryme.demo"
    private static let files = ["shops.toml", "stations.toml", "prefs.json"]
    private static var chosen: URL?

    static func boot() {
        let defaults = UserDefaults.standard
        if defaults.bool(forKey: demo) {
            Core.phone.start()
            return
        }
        var stale = false
        guard let data = defaults.data(forKey: mark),
            let url = try? URL(resolvingBookmarkData: data, bookmarkDataIsStale: &stale),
            url.startAccessingSecurityScopedResource()
        else {
            Core.phone.needsFolder = true
            return
        }
        open(url)
    }

    static func choose(_ url: URL) {
        guard url.startAccessingSecurityScopedResource() else { return }
        if let data = try? url.bookmarkData() {
            UserDefaults.standard.set(data, forKey: mark)
        }
        open(url)
    }

    static func useDemo() {
        UserDefaults.standard.set(true, forKey: demo)
        Core.phone.needsFolder = false
        Core.phone.start()
    }

    static func refresh() {
        guard let chosen, Core.phone.stage != nil else { return }
        DispatchQueue.global().async {
            fetch(chosen)
            DispatchQueue.main.async { Core.phone.reload() }
        }
    }

    private static func open(_ url: URL) {
        chosen?.stopAccessingSecurityScopedResource()
        chosen = url
        UserDefaults.standard.set(false, forKey: demo)
        Core.phone.needsFolder = false
        setenv("WRYME_CONFIG_DIR", url.path, 1)
        DispatchQueue.global().async {
            fetch(url)
            DispatchQueue.main.async {
                let core = Core.phone
                if core.started { core.reload() } else { core.start() }
            }
        }
    }

    private static func fetch(_ url: URL) {
        let coordinator = NSFileCoordinator()
        for name in files {
            let file = url.appendingPathComponent(name)
            try? FileManager.default.startDownloadingUbiquitousItem(at: file)
            coordinator.coordinate(readingItemAt: file, options: [], error: nil) { _ in }
        }
    }
}

struct FolderSetup: View {
    @State private var picking = false

    var body: some View {
        VStack(spacing: 18) {
            Text("wryme").font(.largeTitle.bold()).foregroundStyle(Theme.fg)
            Text("Choose the wryme folder in iCloud Drive. It holds shops.toml, stations.toml and prefs.json, the same files your Mac uses.")
                .multilineTextAlignment(.center)
                .foregroundStyle(Theme.faint)
            Button("Choose folder") { picking = true }
                .buttonStyle(.borderedProminent)
            Button("Try the demo instead") { Folder.useDemo() }
                .foregroundStyle(Theme.faint)
        }
        .padding(32)
        .fileImporter(isPresented: $picking, allowedContentTypes: [.folder]) { result in
            if case .success(let url) = result { Folder.choose(url) }
        }
    }
}
#endif
