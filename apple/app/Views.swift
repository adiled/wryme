import SwiftUI

struct Root: View {
    @ObservedObject var core = Core.shared

    var body: some View {
        ZStack {
            Theme.bg.ignoresSafeArea()
            if let stage = core.stage {
                Main(stage: stage)
            } else if let fatal = core.fatal {
                Text(fatal)
                    .foregroundStyle(Theme.color("Red") ?? .red)
                    .textSelection(.enabled)
                    .padding(40)
            } else {
                ProgressView()
            }
        }
        .preferredColorScheme(.dark)
    }
}

struct Main: View {
    let stage: Stage

    var accent: Color { Theme.color(stage.accent) ?? Theme.fg }

    var body: some View {
        VStack(spacing: 0) {
            Composer(bar: stage.bar, accent: accent, locked: stage.sheet != nil)
            Transcript(bubbles: stage.bubbles, accent: accent)
            Footer(status: stage.status)
        }
        .overlay(alignment: .bottomTrailing) {
            if let error = stage.error {
                Text(error)
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
        .overlay {
            if let sheet = stage.sheet {
                Panel(sheet: sheet, accent: accent)
            }
        }
    }
}

struct Composer: View {
    let bar: Bar
    let accent: Color
    let locked: Bool
    @State private var draft = ""
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("›").foregroundStyle(accent)
                TextField("say something", text: $draft, axis: .vertical)
                    .textFieldStyle(.plain)
                    .lineLimit(1...8)
                    .focused($focused)
                    .onSubmit(submit)
                    .onKeyPress(.escape) {
                        Core.shared.key("esc")
                        return .handled
                    }
                Button {
                    Core.shared.key("s", ctrl: true)
                } label: {
                    Image(systemName: "slider.horizontal.3")
                }
                .buttonStyle(.plain)
                .foregroundStyle(Theme.faint)
                .help("Settings")
            }
            .font(.system(size: 15))
            .padding(12)
            .background(RoundedRectangle(cornerRadius: 12).fill(Theme.panel))
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .strokeBorder(bar.busy ? Theme.faint.opacity(0.4) : accent.opacity(0.7), lineWidth: 1)
            )
            if !bar.state.isEmpty {
                Text("\(bar.state)  ·  \(bar.esc)")
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.faint)
                    .padding(.leading, 4)
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 34)
        .padding(.bottom, 8)
        .onAppear { focused = true }
        .onChange(of: locked) { if !locked { focused = true } }
    }

    private func submit() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        Core.shared.submit(text)
        draft = ""
        focused = true
    }
}

struct Transcript: View {
    let bubbles: [Bubble]
    let accent: Color

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 20) {
                if bubbles.isEmpty {
                    Text("no messages yet. type above and hit return")
                        .italic()
                        .foregroundStyle(Theme.faint)
                }
                ForEach(bubbles) { bubble in
                    BubbleView(bubble: bubble, accent: accent)
                }
            }
            .frame(maxWidth: 760, alignment: .leading)
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
            .frame(maxWidth: .infinity)
        }
    }
}

struct BubbleView: View {
    let bubble: Bubble
    let accent: Color

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text(bubble.name)
                    .fontWeight(.semibold)
                    .foregroundStyle(Theme.color(bubble.user ? "Green" : "Magenta") ?? Theme.fg)
                if let activity = bubble.activity {
                    Text(activity).foregroundStyle(Theme.faint)
                }
                if bubble.waiting {
                    Text("waiting…").foregroundStyle(Theme.faint)
                }
                Spacer()
                if let tool = bubble.tool {
                    Text(tool).fontWeight(.semibold).foregroundStyle(accent)
                }
                Text(bubble.time).monospacedDigit().foregroundStyle(Theme.faint)
            }
            .font(.system(size: 12))
            ForEach(bubble.images, id: \.self) { image in
                Text("📷 attached: \(image)")
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.faint)
            }
            if !bubble.reply.isEmpty {
                prose(bubble.reply)
            } else if bubble.live && bubble.brain.isEmpty && bubble.heart.isEmpty {
                Text("▌").foregroundStyle(Theme.faint)
            }
            thought("brain", bubble.brain)
            thought("heart", bubble.heart)
        }
        .padding(bubble.user ? 12 : 0)
        .background(RoundedRectangle(cornerRadius: 10).fill(bubble.user ? Theme.panel : .clear))
        .opacity(bubble.waiting ? 0.6 : 1)
    }

    private func prose(_ rows: [Row]) -> some View {
        Text(Theme.paint(rows, size: 14))
            .lineSpacing(3)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func thought(_ label: String, _ rows: [Row]) -> some View {
        if !rows.isEmpty {
            VStack(alignment: .leading, spacing: 2) {
                Text(label)
                    .font(.system(size: 11, weight: .semibold))
                    .italic()
                    .foregroundStyle(Theme.faint)
                Text(Theme.paint(rows, size: 13))
                    .lineSpacing(2)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.leading, 10)
            .overlay(alignment: .leading) {
                Rectangle().fill(Theme.faint.opacity(0.4)).frame(width: 2)
            }
        }
    }
}

struct Footer: View {
    let status: Status

    var body: some View {
        HStack(spacing: 12) {
            Text(Theme.paint(status.left, size: 11) + AttributedString(" • ") + Theme.paint([status.note], size: 11))
                .lineLimit(1)
            Spacer()
            Text(Theme.paint(status.right, size: 11))
                .monospacedDigit()
        }
        .foregroundStyle(Theme.faint)
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }
}

struct Panel: View {
    let sheet: Sheet
    let accent: Color
    @FocusState private var focused: Bool

    var body: some View {
        ZStack {
            Color.black.opacity(0.45)
                .ignoresSafeArea()
                .onTapGesture { Core.shared.key("esc") }
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 6) {
                    Spacer()
                    ForEach(sheet.tabs, id: \.label) { tab in
                        Text(tab.label)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(tab.on ? Theme.bg : Theme.faint)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 4)
                            .background(Capsule().fill(tab.on ? accent : .clear))
                    }
                }
                ScrollView {
                    Text(Theme.paint(sheet.lines, size: 13, mono: true))
                        .lineSpacing(5)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                Text(sheet.hint)
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.faint)
            }
            .padding(18)
            .frame(maxWidth: 580, maxHeight: 460)
            .background(RoundedRectangle(cornerRadius: 14).fill(Theme.panel))
            .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(accent.opacity(0.6), lineWidth: 1))
            .shadow(color: .black.opacity(0.5), radius: 30, y: 10)
            .padding(24)
        }
        .focusable()
        .focusEffectDisabled()
        .focused($focused)
        .onKeyPress(phases: [.down, .repeat]) { press in
            guard let name = Panel.name(press) else { return .ignored }
            Core.shared.key(name, ctrl: press.modifiers.contains(.control))
            return .handled
        }
        .onAppear { focused = true }
    }

    static func name(_ press: KeyPress) -> String? {
        switch press.key {
        case .escape: return "esc"
        case .return: return "enter"
        case .tab: return "tab"
        case .upArrow: return "up"
        case .downArrow: return "down"
        case .leftArrow: return "left"
        case .rightArrow: return "right"
        case .home: return "home"
        case .end: return "end"
        case .pageUp: return "pageup"
        case .pageDown: return "pagedown"
        case .delete: return "backspace"
        case .deleteForward: return "delete"
        default:
            if press.modifiers.contains(.command) { return nil }
            guard let scalar = press.key.character.unicodeScalars.first else { return nil }
            if scalar.value < 0x20, let letter = Unicode.Scalar(scalar.value + 0x60) {
                return String(Character(letter))
            }
            return String(press.key.character)
        }
    }
}
