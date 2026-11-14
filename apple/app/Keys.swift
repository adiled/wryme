import SwiftUI

#if os(macOS)
import AppKit
#else
import UIKit
#endif

enum Edit: String { case send, left, right, home, end, backspace, delete, kill_to_start, kill_to_end, kill_word }
enum Pane: String { case page_up, page_down, copy }

enum Act {
    case core(String)
    case edit(Edit)
    case pane(Pane)
}

struct Chord: Decodable, Hashable {
    let key: String
    let ctrl: Bool
    let shift: Bool
    let cmd: Bool

    func fits(_ pressed: Chord, relax: Bool) -> Bool {
        key == pressed.key && ctrl == pressed.ctrl && cmd == pressed.cmd
            && (shift == pressed.shift || (relax && !shift))
    }

    var equivalent: KeyEquivalent? {
        switch key {
        case "enter": .return
        case "esc": .escape
        case "tab": .tab
        case "up": .upArrow
        case "down": .downArrow
        case "left": .leftArrow
        case "right": .rightArrow
        case "home": .home
        case "end": .end
        case "page_up": .pageUp
        case "page_down": .pageDown
        case "backspace": .delete
        case "delete": .deleteForward
        default: key.count == 1 ? KeyEquivalent(Character(key)) : nil
        }
    }

    var modifiers: EventModifiers {
        var m: EventModifiers = []
        if ctrl { m.insert(.control) }
        if shift { m.insert(.shift) }
        if cmd { m.insert(.command) }
        return m
    }
}

struct Bind: Decodable, Identifiable {
    let scope: Scope
    let chord: Chord
    let name: String
    let act: Act
    let about: String
    let label: String

    var id: String { "\(scope.rawValue) \(label)" }

    var native: Bool {
        if case .edit(let e) = act { return [.left, .right, .backspace, .delete].contains(e) }
        return false
    }

    private enum Keys: String, CodingKey { case scope, chord, action, owner, about, label }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        scope = try c.decode(Scope.self, forKey: .scope)
        chord = try c.decode(Chord.self, forKey: .chord)
        about = try c.decode(String.self, forKey: .about)
        label = try c.decode(String.self, forKey: .label)
        name = try c.decode(String.self, forKey: .action)
        let owner = try c.decode(String.self, forKey: .owner)
        let unknown = DecodingError.dataCorruptedError(
            forKey: .action, in: c, debugDescription: "unknown \(owner) action \(name)")
        switch owner {
        case "core": act = .core(name)
        case "edit":
            guard let e = Edit(rawValue: name) else { throw unknown }
            act = .edit(e)
        case "pane":
            guard let p = Pane(rawValue: name) else { throw unknown }
            act = .pane(p)
        default: throw unknown
        }
    }
}

extension View {
    @ViewBuilder
    func shortcut(_ chord: Chord) -> some View {
        if let key = chord.equivalent {
            keyboardShortcut(key, modifiers: chord.modifiers)
        } else {
            self
        }
    }
}

enum Platform {
    #if os(macOS)
    static func allows(_ action: String) -> Bool { true }
    static func copy() { NSApp.sendAction(#selector(NSText.copy(_:)), to: nil, from: nil) }
    #else
    static func allows(_ action: String) -> Bool { action != "quit" }
    static func copy() {
        UIApplication.shared.sendAction(
            #selector(UIResponderStandardEditActions.copy(_:)), to: nil, from: nil, for: nil)
    }
    #endif
}

#if os(macOS)
enum Keys {
    private static let codes: [UInt16: String] = [
        36: "enter", 76: "enter", 53: "esc", 48: "tab", 126: "up", 125: "down", 123: "left",
        124: "right", 115: "home", 119: "end", 116: "page_up", 121: "page_down",
        51: "backspace", 117: "delete", 122: "f1",
    ]

    static func install() {
        NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            handle(event) ? nil : event
        }
    }

    private static func handle(_ event: NSEvent) -> Bool {
        let core = Core.shared
        let flags = event.modifierFlags
        guard let scope = core.stage?.scope, !flags.contains(.option) else { return false }
        if let editor = NSApp.keyWindow?.firstResponder as? NSTextView, editor.hasMarkedText() {
            return false
        }
        guard let key = codes[event.keyCode] ?? event.charactersIgnoringModifiers?.lowercased(),
            !key.isEmpty
        else { return false }
        let chord = Chord(
            key: key, ctrl: flags.contains(.control), shift: flags.contains(.shift),
            cmd: flags.contains(.command))
        if let binding = core.lookup(chord) { return core.perform(binding) }
        guard scope == .sheet, !chord.ctrl, !chord.cmd, let c = event.characters?.first,
            c.isLetter || c.isNumber || c.isPunctuation || c.isSymbol || c == " "
        else { return false }
        core.search(c)
        return true
    }
}

enum Editor {
    static func perform(_ e: Edit) -> Bool {
        switch e {
        case .send:
            Core.shared.sendField()
            return true
        case .left, .right, .backspace, .delete: return false
        case .home: return run(#selector(NSResponder.moveToBeginningOfParagraph(_:)))
        case .end: return run(#selector(NSResponder.moveToEndOfParagraph(_:)))
        case .kill_to_start: return run(#selector(NSResponder.deleteToBeginningOfParagraph(_:)))
        case .kill_to_end: return run(#selector(NSResponder.deleteToEndOfParagraph(_:)))
        case .kill_word: return run(#selector(NSResponder.deleteWordBackward(_:)))
        }
    }

    private static func run(_ action: Selector) -> Bool {
        NSApp.sendAction(action, to: nil, from: nil)
    }
}

struct Menus: Commands {
    @ObservedObject var core: Core

    var body: some Commands {
        CommandGroup(replacing: .appSettings) {
            Button("Settings…") { core.act("settings") }.keyboardShortcut(",")
        }
        CommandMenu("Chat") {
            ForEach(core.menu) { b in
                Button(b.about.prefix(1).uppercased() + b.about.dropFirst()) { core.perform(b) }
                    .shortcut(b.chord)
                    .disabled(!core.live(b.name))
            }
        }
    }
}
#else
enum Editor {
    static func perform(_ e: Edit) -> Bool {
        let core = Core.shared
        switch e {
        case .send:
            core.sendField()
            return true
        case .left, .right, .backspace, .delete: return false
        case .home, .end, .kill_to_start, .kill_to_end, .kill_word:
            if core.stage?.scope == .field {
                (core.fieldText, core.fieldCaret) = apply(e, core.fieldText, core.fieldCaret)
            } else {
                (core.draft, core.draftCaret) = apply(e, core.draft, core.draftCaret)
            }
            return true
        }
    }

    static func apply(_ e: Edit, _ text: String, _ caret: Int) -> (String, Int) {
        var units = Array(text.utf16)
        var at = min(max(caret, 0), units.count)
        let space = { (u: UInt16) in u == 32 || u == 9 || u == 10 }
        switch e {
        case .home: at = 0
        case .end: at = units.count
        case .kill_to_start:
            units.removeSubrange(0..<at)
            at = 0
        case .kill_to_end: units.removeSubrange(at..<units.count)
        case .kill_word:
            var start = at
            while start > 0, space(units[start - 1]) { start -= 1 }
            while start > 0, !space(units[start - 1]) { start -= 1 }
            units.removeSubrange(start..<at)
            at = start
        case .send, .left, .right, .backspace, .delete: break
        }
        return (String(decoding: units, as: UTF16.self), at)
    }
}

struct Shortcuts: View {
    @ObservedObject var core = Core.shared

    var body: some View {
        ZStack {
            ForEach(core.bindings.filter { core.reaches($0) && !$0.native && $0.chord.equivalent != nil }) { b in
                Button("") { core.perform(b) }.shortcut(b.chord)
            }
        }
        .frame(width: 0, height: 0)
        .opacity(0)
        .accessibilityHidden(true)
    }
}

struct ActionMenu: View {
    @ObservedObject var core = Core.shared

    var body: some View {
        Menu {
            ForEach(core.menu) { b in
                Button(b.about.prefix(1).uppercased() + b.about.dropFirst()) { core.perform(b) }
                    .disabled(!core.live(b.name))
            }
        } label: {
            Image(systemName: "ellipsis.circle")
        }
    }
}
#endif
