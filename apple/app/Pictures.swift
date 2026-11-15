import SwiftUI

#if os(macOS)
import AppKit

typealias PlatformImage = NSImage

extension Image {
    init(picture: NSImage) { self.init(nsImage: picture) }
}
#else
import UIKit

typealias PlatformImage = UIImage

extension Image {
    init(picture: UIImage) { self.init(uiImage: picture) }
}
#endif

enum Pictures {
    static let kinds: Set<String> = ["png", "jpg", "jpeg", "gif", "webp"]
    private static let cache = NSCache<NSString, PlatformImage>()

    static func isPicture(_ path: String) -> Bool {
        kinds.contains((path as NSString).pathExtension.lowercased())
    }

    static func image(_ path: String) -> PlatformImage? {
        if let hit = cache.object(forKey: path as NSString) { return hit }
        guard let image = PlatformImage(contentsOfFile: path) else { return nil }
        cache.setObject(image, forKey: path as NSString)
        return image
    }

    static func store(_ data: Data, ext: String) -> String? {
        guard let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        else { return nil }
        let dir = base.appendingPathComponent("wryme/pictures", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let file = dir.appendingPathComponent("\(UUID().uuidString).\(ext)")
        return (try? data.write(to: file)) != nil ? file.path : nil
    }

    #if os(macOS)
    static func paste(into core: Core) -> Bool {
        let board = NSPasteboard.general
        let urls = (board.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
        let files = urls.map(\.path).filter(isPicture)
        if !files.isEmpty {
            files.forEach(core.attach)
            return true
        }
        guard board.string(forType: .string) == nil, let image = NSImage(pasteboard: board),
            let tiff = image.tiffRepresentation,
            let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]),
            let path = store(png, ext: "png")
        else { return false }
        core.attach(path)
        return true
    }

    static func open(_ path: String) {
        NSWorkspace.shared.open(URL(fileURLWithPath: path))
    }
    #else
    static func open(_ path: String) {}
    #endif
}

struct Thumbs: View {
    let paths: [String]
    let height: CGFloat
    var remove: ((Int) -> Void)?

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            ForEach(Array(paths.enumerated()), id: \.offset) { i, path in
                Thumb(path: path, height: height, remove: remove.map { drop in { drop(i) } })
            }
        }
    }
}

struct Thumb: View {
    let path: String
    let height: CGFloat
    let remove: (() -> Void)?

    var body: some View {
        Group {
            if let image = Pictures.image(path) {
                Image(picture: image)
                    .resizable()
                    .scaledToFit()
                    .frame(maxHeight: height)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Color.white.opacity(0.08), lineWidth: 1))
                    .onTapGesture { Pictures.open(path) }
            } else {
                Text("📷 \((path as NSString).lastPathComponent)")
                    .foregroundStyle(Theme.faint)
            }
        }
        .overlay(alignment: .topTrailing) {
            if let remove {
                Button(action: remove) {
                    Image(systemName: "xmark.circle.fill")
                        .symbolRenderingMode(.palette)
                        .foregroundStyle(Theme.fg, Color.black.opacity(0.6))
                }
                .buttonStyle(.plain)
                .padding(3)
            }
        }
    }
}
