import SwiftUI

#if os(macOS)
import AppKit
#endif

#if os(iOS)
import PhotosUI
import UniformTypeIdentifiers
#endif

struct Root: View {
    @EnvironmentObject var core: Core
    @Environment(\.scenePhase) private var phase

    var body: some View {
        ZStack {
            Backdrop()
            if let fatal = core.fatal {
                Text(fatal)
                    .foregroundStyle(Theme.color("Red") ?? .red)
                    .textSelection(.enabled)
                    .padding(40)
            } else if let stage = core.stage {
                Main(stage: stage)
            } else if core.needsFolder {
                #if os(iOS)
                FolderSetup()
                #endif
            } else {
                ProgressView()
            }
        }
        .preferredColorScheme(.dark)
        #if os(iOS)
        .onChange(of: phase) { _, now in
            if now == .active { Folder.refresh() }
        }
        #else
        .overlay(alignment: .top) { TitleStrip() }
        #endif
    }
}

#if os(macOS)
struct TitleStrip: View {
    var body: some View {
        Color.clear
            .frame(height: 28)
            .frame(maxWidth: .infinity)
            .contentShape(Rectangle())
            .onTapGesture(count: 2) { TitleStrip.doubleClick() }
            .gesture(WindowDragGesture())
            .ignoresSafeArea(edges: .top)
    }

    static func doubleClick() {
        guard let window = NSApp.keyWindow else { return }
        switch UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") {
        case "Minimize": window.miniaturize(nil)
        case "None": break
        default: window.zoom(nil)
        }
    }
}
#endif

struct Main: View {
    let stage: Stage
    #if os(iOS)
    @State private var picking = false
    #endif

    var accent: Color { Theme.color(stage.accent) ?? Theme.fg }

    var body: some View {
        VStack(spacing: 0) {
            Composer(stage: stage, accent: accent)
            Transcript(stage: stage, accent: accent)
            Footer(stage: stage, accent: accent)
        }
        .overlay(alignment: .bottomTrailing) {
            if let error = stage.error {
                ErrorBox(text: error)
            }
        }
        .overlay {
            if let sheet = stage.sheet {
                Panel(sheet: sheet, stage: stage, accent: accent)
            }
        }
        #if os(iOS)
        .background(Shortcuts())
        .onChange(of: stage.sync.pick) { _, _ in picking = true }
        .fileImporter(isPresented: $picking, allowedContentTypes: [.folder]) { result in
            if case .success(let url) = result { Folder.choose(url) }
        }
        #endif
    }
}

struct Composer: View {
    let stage: Stage
    let accent: Color
    @EnvironmentObject var core: Core
    @FocusState private var focused: Bool
    #if os(iOS)
    @State private var photo: PhotosPickerItem?
    #endif

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            VStack(alignment: .leading, spacing: 10) {
                if !core.attachments.isEmpty {
                    Thumbs(paths: core.attachments, height: 56) { core.detach($0) }
                }
                HStack(alignment: .center, spacing: 8) {
                    Text("›").foregroundStyle(accent)
                    field
                        .textFieldStyle(.plain)
                        .lineLimit(1...8)
                        .focused($focused)
                        .onChange(of: core.draft) { _, text in settle(text) }
                    #if os(iOS)
                    PhotosPicker(selection: $photo, matching: .images) {
                        Image(systemName: "photo")
                    }
                    .onChange(of: photo) { _, item in attach(item) }
                    ActionMenu()
                    #else
                    Button {
                        core.act("settings")
                    } label: {
                        Image(systemName: "slider.horizontal.3")
                    }
                    .buttonStyle(.plain)
                    .help("Settings")
                    #endif
                }
            }
            .foregroundStyle(Theme.fg)
            .font(Theme.font(stage.font, size: 15))
            .padding(12)
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .strokeBorder(stage.bar.busy ? Theme.faint.opacity(0.4) : accent.opacity(0.7), lineWidth: 1)
            )
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .padding(.bottom, 4)
        .dropDestination(for: URL.self) { urls, _ in
            let paths = urls.map(\.path)
            let pictures = paths.filter(Pictures.isPicture)
            pictures.forEach(core.attach)
            if let other = paths.first(where: { !Pictures.isPicture($0) }) { core.draft = other }
            return !paths.isEmpty
        }
        .onAppear { focused = true }
        .onChange(of: stage.scope) { _, scope in
            if scope == .chat { focused = true }
        }
    }

    @ViewBuilder
    private var field: some View {
        #if os(iOS)
        TextField("", text: $core.draft, selection: $core.draftSelection, axis: .vertical)
        #else
        TextField("", text: $core.draft, axis: .vertical)
        #endif
    }

    private func settle(_ text: String) {
        guard text.contains("\n") else { return }
        if text.hasSuffix("\n"), text.dropLast().contains("\n") == false {
            core.draft = String(text.dropLast())
            core.sendField()
        } else {
            core.draft = text.replacingOccurrences(of: "\n", with: " ")
        }
    }

    #if os(iOS)
    private func attach(_ item: PhotosPickerItem?) {
        guard let item else { return }
        Task {
            guard let data = try? await item.loadTransferable(type: Data.self),
                let jpeg = UIImage(data: data)?.jpegData(compressionQuality: 0.9)
            else { return }
            guard let path = Pictures.store(jpeg, ext: "jpg") else { return }
            await MainActor.run {
                core.attach(path)
                photo = nil
            }
        }
    }
    #endif
}

struct Transcript: View {
    let stage: Stage
    let accent: Color
    @EnvironmentObject var core: Core
    @State private var position = ScrollPosition(edge: .top)
    @State private var geo = GeoBox()

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: Theme.size * 1.3) {
                if core.bubbles.isEmpty {
                    Text("no messages yet. type above and hit Return")
                        .italic()
                        .foregroundStyle(Theme.faint)
                }
                ForEach(core.bubbles) { bubble in
                    BubbleView(bubble: bubble, stage: stage, accent: accent)
                }
            }
            .font(Theme.font(stage.font))
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
        .scrollPosition($position)
        .scrollTargetBehavior(Snap(paging: stage.view == .page))
        .onScrollGeometryChange(for: Geo.self, of: Geo.init) { _, now in
            geo.value = now
            core.readout.update(now)
        }
        .onReceive(core.chatMoves) { move in
            switch move {
            case .top: position.scrollTo(edge: .top)
            case .by(let dir): withAnimation { position.scrollTo(y: geo.value.target(dir)) }
            }
        }
    }
}

struct Snap: ScrollTargetBehavior {
    let paging: Bool

    func updateTarget(_ target: inout ScrollTarget, context: TargetContext) {
        guard paging else { return }
        let pages: PagingScrollTargetBehavior = .paging
        pages.updateTarget(&target, context: context)
    }
}

struct BubbleView: View {
    let bubble: Bubble
    let stage: Stage
    let accent: Color

    private var ink: Color { Theme.color(bubble.ink) ?? Theme.fg }
    private var face: Typeface { stage.font }
    private var gap: CGFloat { Theme.size * 1.2 }

    var body: some View {
        Group {
            switch stage.nametags {
            case .standout: standout
            case .inlined: inlined
            }
        }
        .opacity(bubble.waiting ? 0.55 : 1)
        .textSelection(.enabled)
    }

    private var standout: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 0) {
                Text(bubble.name).fontWeight(.bold).foregroundStyle(ink)
                if let doing = bubble.doing {
                    Text("  \(doing)").foregroundStyle(Theme.faint)
                }
                if bubble.waiting {
                    Text("  waiting…").foregroundStyle(Theme.faint)
                }
                Spacer(minLength: 12)
                if let tool = bubble.tool {
                    Text(tool).fontWeight(.bold).foregroundStyle(accent)
                    Text("  ")
                }
                Text(bubble.time).foregroundStyle(Theme.faint).monospacedDigit()
            }
            if hasHead {
                Blocks(lead: lead, rows: bubble.reply, face: face, images: bubble.images)
            }
            thought("brain", bubble.brain, after: hasHead)
            thought("heart", bubble.heart, after: hasHead || !bubble.brain.isEmpty)
        }
    }

    @ViewBuilder
    private func thought(_ label: String, _ rows: [Row], after: Bool) -> some View {
        if !rows.isEmpty {
            VStack(alignment: .leading, spacing: 2) {
                Text(label).italic().fontWeight(.bold).foregroundStyle(Theme.faint)
                Text(Theme.paint(rows, face: face)).frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.top, after ? gap : 0)
        }
    }

    private var hasHead: Bool {
        !lead.characters.isEmpty || !bubble.reply.isEmpty || !bubble.images.isEmpty
    }

    private var lead: AttributedString {
        var out = AttributedString()
        if bubble.orphan { out += Theme.plain("▌", Theme.faint, face: face) }
        if !bubble.reply.isEmpty {
            if bubble.orphan { out += AttributedString("\n") }
            if stage.nametags == .inlined, let guest = bubble.guest {
                out += Theme.plain("@\(guest) ", ink, face: face)
            }
        }
        return out
    }

    private struct Part {
        let mark: String
        let ink: Color
        let blinks: Bool
        let lead: AttributedString
        let rows: [Row]
        var images: [String] = []
    }

    private var parts: [Part] {
        var out: [Part] = []
        if hasHead {
            out.append(Part(mark: bubble.mark, ink: ink, blinks: bubble.blinks, lead: lead, rows: bubble.reply, images: bubble.images))
        }
        for (rows, mark) in [(bubble.brain, stage.marks.brain), (bubble.heart, stage.marks.heart)] where !rows.isEmpty {
            out.append(Part(mark: mark, ink: Theme.faint, blinks: bubble.thinkingLive, lead: AttributedString(), rows: rows))
        }
        if out.isEmpty {
            out.append(Part(mark: bubble.mark, ink: ink, blinks: bubble.blinks, lead: AttributedString(), rows: []))
        }
        return out
    }

    private var inlined: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(parts.enumerated()), id: \.offset) { i, part in
                HStack(alignment: .firstTextBaseline, spacing: 0) {
                    Mark(text: part.mark, ink: part.ink, blinks: part.blinks)
                        .frame(width: Theme.size * 1.6, alignment: .leading)
                    Blocks(lead: part.lead, rows: part.rows, face: face, images: part.images)
                    if i == 0, let tool = bubble.tool {
                        Text(tool).fontWeight(.bold).foregroundStyle(accent).padding(.leading, 12)
                    }
                    if i == 0 {
                        Text(bubble.waiting ? "waiting…" : bubble.time)
                            .foregroundStyle(Theme.faint)
                            .monospacedDigit()
                            .padding(.leading, 12)
                    }
                }
                .padding(.top, i > 0 ? gap : 0)
            }
        }
    }
}

struct Mark: View {
    let text: String
    let ink: Color
    let blinks: Bool

    var body: some View {
        if blinks {
            TimelineView(.periodic(from: .now, by: 0.5)) { context in
                glyph.opacity(Int(context.date.timeIntervalSince1970 * 2) % 2 == 0 ? 1 : 0)
            }
        } else {
            glyph
        }
    }

    private var glyph: some View {
        Text(text).fontWeight(.bold).foregroundStyle(ink)
    }
}

struct Footer: View {
    let stage: Stage
    let accent: Color
    @EnvironmentObject var core: Core

    var body: some View {
        FooterLine(stage: stage, accent: accent, readout: core.readout, any: core.bubbles.contains { !$0.waiting })
    }
}

struct FooterLine: View {
    let stage: Stage
    let accent: Color
    @ObservedObject var readout: Readout
    let any: Bool

    var body: some View {
        HStack(spacing: 12) {
            Text(left).lineLimit(1)
            Spacer()
            Text(Theme.paint(stage.status.right, face: stage.font, size: 11)).monospacedDigit()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }

    private var left: AttributedString {
        let dot = Theme.plain(" • ", Theme.faint, face: stage.font, size: 11)
        var out = Theme.paint(stage.status.left, face: stage.font, size: 11)
        if any {
            let (text, lit) = readout.label(stage.view)
            out += dot
            out += Theme.plain(text, lit ? accent : Theme.faint, face: stage.font, size: 11)
        }
        out += dot
        out += Theme.paint([stage.status.note], face: stage.font, size: 11)
        return out
    }
}

struct ErrorBox: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundStyle(Theme.color("Red") ?? .red)
            .textSelection(.enabled)
            .padding(12)
            .frame(maxWidth: 360, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 10).fill(Theme.panel))
            .overlay(
                RoundedRectangle(cornerRadius: 10)
                    .strokeBorder(Theme.color("Red") ?? .red, lineWidth: 1)
            )
            .padding(.trailing, 16)
            .padding(.bottom, 36)
    }
}
