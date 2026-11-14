import Combine
#if os(macOS)
import AppKit
#endif
import Foundation
import SwiftUI

struct Run: Decodable {
    let t: String
    let fg: String?
    let bg: String?
    let b: Bool
    let i: Bool
    let d: Bool
    let u: Bool
    let x: Bool
}

enum Kind: String, Decodable { case prose, head, code, table }

struct Row: Decodable {
    let mono: Bool
    let kind: Kind
    let runs: [Run]
}

enum Nametags: String, Decodable { case standout, inlined }
enum ViewMode: String, Decodable { case page, scroll }
enum Scope: String, Decodable { case any, chat, sheet, field }
enum Typeface: String, Decodable { case system, serif, rounded, mono }

struct SyncPlace: Decodable {
    let place: String
    let pick: Int
}

struct Marks: Decodable {
    let brain: String
    let heart: String
}

struct Bar: Decodable {
    let state: String
    let esc: String
    let busy: Bool
    let tool: String?
}

struct Bubble: Decodable, Identifiable {
    let id: String
    let user: Bool
    let name: String
    let guest: String?
    let ink: String?
    let doing: String?
    let mark: String
    let blinks: Bool
    let thinkingLive: Bool
    let tool: String?
    let waiting: Bool
    let live: Bool
    let time: String
    let images: [String]
    let orphan: Bool
    let reply: [Row]
    let brain: [Row]
    let heart: [Row]
}

struct Status: Decodable {
    let left: [Run]
    let note: Run
    let right: [Run]
}

struct Pill: Decodable {
    let label: String
    let on: Bool
}

struct Field: Decodable {
    let label: String
    let text: String
}

struct Sheet: Decodable {
    let tabs: [Pill]
    let lines: [Row]
    let picks: [Int]
    let selected: Int?
    let hint: String
    let field: Field?
}

struct Stage: Decodable {
    let window: String
    let accent: String?
    let nametags: Nametags
    let font: Typeface
    let view: ViewMode
    let scope: Scope
    let turn: Int
    let marks: Marks
    let sync: SyncPlace
    let bar: Bar
    let status: Status
    let error: String?
    let sheet: Sheet?
}

struct Envelope: Decodable {
    let bindings: [Binding]?
    let stage: Stage?
    let order: [String]?
    let bubbles: [String: Bubble]?
    let fatal: String?
}

enum Move {
    case top
    case by(CGFloat)
}

struct Geo: Equatable {
    var offset: CGFloat = 0
    var viewport: CGFloat = 1
    var content: CGFloat = 0

    init() {}

    init(_ g: ScrollGeometry) {
        offset = g.contentOffset.y + g.contentInsets.top
        viewport = max(g.containerSize.height, 1)
        content = g.contentSize.height
    }

    func target(_ dir: CGFloat) -> CGFloat {
        min(max(0, offset + dir * viewport), max(0, content - viewport))
    }
}

final class GeoBox {
    var value = Geo()
}

final class Readout: ObservableObject {
    @Published private(set) var page = 1
    @Published private(set) var pages = 1
    @Published private(set) var percent = 0

    func update(_ g: Geo) {
        let pages = max(1, Int(ceil(g.content / g.viewport - 0.01)))
        let page = min(pages, Int((g.offset / g.viewport).rounded()) + 1)
        let span = max(g.content - g.viewport, 1)
        let percent = Int((min(max(g.offset / span, 0), 1) * 100).rounded())
        if pages != self.pages { self.pages = pages }
        if page != self.page { self.page = page }
        if percent != self.percent { self.percent = percent }
    }

    func label(_ view: ViewMode) -> (text: String, lit: Bool) {
        switch view {
        case .page: ("page \(page)/\(pages)", pages > 1)
        case .scroll: (percent == 0 ? "scroll (top)" : "scroll \(percent)%", true)
        }
    }
}

protocol Link: AnyObject {
    func start(_ receive: @escaping (Data) -> Void, ended: @escaping () -> Void)
    func send(_ text: String)
    func stop()
}

func fatalLine(_ text: String) -> Data {
    (try? JSONSerialization.data(withJSONObject: ["fatal": text])) ?? Data()
}

#if os(macOS)
final class Child: Link {
    private let process = Process()
    private let input = Pipe()
    private let output = Pipe()
    private var buffer = Data()

    func start(_ receive: @escaping (Data) -> Void, ended: @escaping () -> Void) {
        guard let url = Bundle.main.url(forAuxiliaryExecutable: "wme") else {
            receive(fatalLine("wme is missing from the app bundle"))
            return
        }
        process.executableURL = url
        process.arguments = ["--serve"]
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        output.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let chunk = handle.availableData
            guard let self, !chunk.isEmpty else {
                handle.readabilityHandler = nil
                return
            }
            self.buffer.append(chunk)
            while let end = self.buffer.firstIndex(of: 0x0A) {
                let line = Data(self.buffer[self.buffer.startIndex..<end])
                self.buffer.removeSubrange(self.buffer.startIndex...end)
                if line.first == UInt8(ascii: "{") { receive(line) }
            }
        }
        process.terminationHandler = { _ in DispatchQueue.main.async { ended() } }
        do {
            try process.run()
        } catch {
            receive(fatalLine("could not start wme: \(error.localizedDescription)"))
        }
    }

    func send(_ text: String) {
        try? input.fileHandleForWriting.write(contentsOf: Data((text + "\n").utf8))
    }

    func stop() {
        if process.isRunning { process.terminate() }
    }
}
#else
final class Inline: Link {
    private static var receive: ((Data) -> Void)?

    func start(_ receive: @escaping (Data) -> Void, ended: @escaping () -> Void) {
        Inline.receive = receive
        wryme_start { text in
            guard let text else { return }
            Inline.receive?(Data(bytes: text, count: strlen(text)))
        }
    }

    func send(_ text: String) { wryme_send(text) }

    func stop() {}
}
#endif

final class Core: ObservableObject {
    #if os(macOS)
    private static let open = NSHashTable<Core>.weakObjects()
    weak var window: NSWindow?

    static var focused: Core? {
        open.allObjects.first { $0.window != nil && $0.window === NSApp.keyWindow }
    }
    #else
    static let phone = Core(link: Inline())
    #endif

    @Published var stage: Stage?
    @Published var bubbles: [Bubble] = []
    @Published var bindings: [Binding] = []
    @Published var fatal: String?
    @Published var needsFolder = false
    @Published var draft = ""
    @Published var fieldText = ""
    @Published var attachments: [String] = []
    @Published var ended = false
    #if os(iOS)
    @Published var draftSelection: TextSelection?
    @Published var fieldSelection: TextSelection?
    #endif

    let chatMoves = PassthroughSubject<Move, Never>()
    let sheetMoves = PassthroughSubject<Move, Never>()
    let readout = Readout()

    private let link: Link
    private var cache: [String: Bubble] = [:]
    private(set) var started = false

    init(link: Link) {
        self.link = link
        #if os(macOS)
        Core.open.add(self)
        #endif
    }

    func start() {
        guard !started else { return }
        started = true
        link.start({ [weak self] data in
            let decoder = JSONDecoder()
            decoder.keyDecodingStrategy = .convertFromSnakeCase
            let result = Result { try decoder.decode(Envelope.self, from: data) }
            DispatchQueue.main.async {
                guard let self else { return }
                switch result {
                case .success(let envelope): self.apply(envelope)
                case .failure(let error): self.fatal = "This app can't read what wryme sent: \(error)"
                }
            }
        }, ended: { [weak self] in self?.ended = true })
    }

    func stop() { link.stop() }

    private func apply(_ e: Envelope) {
        if let bindings = e.bindings { self.bindings = bindings }
        if let fatal = e.fatal { self.fatal = fatal }
        guard let stage = e.stage, let order = e.order else { return }
        for (id, bubble) in e.bubbles ?? [:] { cache[id] = bubble }
        let keep = Set(order)
        cache = cache.filter { keep.contains($0.key) }
        bubbles = order.compactMap { cache[$0] }
        let old = self.stage
        if stage.scope == .field, old?.scope != .field {
            fieldText = stage.sheet?.field?.text ?? ""
        }
        #if os(macOS)
        if let window, window.title != stage.window { window.title = stage.window }
        #endif
        if old?.turn != stage.turn || old?.view != stage.view {
            chatMoves.send(.top)
        }
        self.stage = stage
    }

    private func send(_ msg: Any) {
        guard let data = try? JSONSerialization.data(withJSONObject: msg, options: [.fragmentsAllowed]),
            let text = String(data: data, encoding: .utf8)
        else { return }
        link.send(text)
    }

    func act(_ name: String) { send(["do": name]) }
    func submit(_ text: String) { send(["submit": text]) }
    func search(_ c: Character) { send(["search": String(c)]) }
    func find(_ text: String) { send(["find": text]) }
    func tab(_ i: Int) { send(["tab": i]) }
    func pick(_ i: Int) { send(["pick": i]) }
    func reload() { send("reload") }

    func sendField() {
        if stage?.scope == .field {
            submit(fieldText)
            return
        }
        let (text, images) = (draft, attachments)
        draft = ""
        attachments = []
        send(["send": ["text": text, "images": images]])
    }

    func attach(_ path: String) {
        if !attachments.contains(path) { attachments.append(path) }
    }

    func detach(_ index: Int) {
        if attachments.indices.contains(index) { attachments.remove(at: index) }
    }

    func reaches(_ b: Binding) -> Bool {
        b.scope == .any || b.scope == stage?.scope
    }

    func live(_ name: String) -> Bool {
        bindings.contains { $0.name == name && reaches($0) }
    }

    func lookup(_ chord: Chord) -> Binding? {
        let near = bindings.filter(reaches)
        return near.first { $0.chord.fits(chord, relax: false) }
            ?? near.first { $0.chord.fits(chord, relax: true) }
    }

    var menu: [Binding] {
        var seen = Set<String>()
        return bindings.filter { b in
            guard case .core = b.act, Platform.allows(b.name), !seen.contains(b.name) else { return false }
            seen.insert(b.name)
            return true
        }
    }

    @discardableResult
    func perform(_ b: Binding) -> Bool {
        switch b.act {
        case .core(let name):
            if Platform.allows(name) { act(name) }
            return true
        case .edit(let e):
            return Editor.perform(e, self)
        case .pane(let p):
            pane(p)
            return true
        }
    }

    private func pane(_ p: Pane) {
        let moves = stage?.sheet == nil ? chatMoves : sheetMoves
        switch p {
        case .copy: Platform.copy()
        case .page_up: moves.send(.by(-1))
        case .page_down: moves.send(.by(1))
        }
    }
}
