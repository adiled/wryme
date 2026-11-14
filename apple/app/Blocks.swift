import SwiftUI

#if os(macOS)
import AppKit
#else
import UIKit
#endif

enum Chunk {
    case prose([Row])
    case code(String, [Row])
    case table([Row])
}

extension Row {
    var blank: Bool { runs.allSatisfy { $0.t.trimmingCharacters(in: .whitespaces).isEmpty } }
    var body: Row { Row(mono: true, kind: kind, runs: Array(runs.dropFirst())) }
}

func chunks(_ rows: [Row]) -> [Chunk] {
    var out: [Chunk] = []
    for row in rows {
        switch (row.kind, out.last) {
        case (.head, _):
            out.append(.code(row.body.runs.map(\.t).joined(), []))
        case (.code, .code(let lang, let body)?):
            out[out.count - 1] = .code(lang, body + [row])
        case (.code, _):
            out.append(.code("", [row]))
        case (.table, .table(let body)?):
            out[out.count - 1] = .table(body + [row])
        case (.table, _):
            out.append(.table([row]))
        case (.prose, .prose(let body)?):
            out[out.count - 1] = .prose(body + [row])
        case (.prose, _):
            out.append(.prose([row]))
        }
    }
    return out.compactMap { chunk in
        guard case .prose(var body) = chunk else { return chunk }
        while body.first?.blank == true { body.removeFirst() }
        while body.last?.blank == true { body.removeLast() }
        return body.isEmpty ? nil : .prose(body)
    }
}

struct Blocks: View {
    let lead: AttributedString
    let rows: [Row]
    let face: Typeface

    var body: some View {
        let parts = chunks(rows)
        VStack(alignment: .leading, spacing: Theme.size * 0.8) {
            if case .prose? = parts.first {
            } else if !lead.characters.isEmpty {
                Text(lead).frame(maxWidth: .infinity, alignment: .leading)
            }
            ForEach(Array(parts.enumerated()), id: \.offset) { i, part in
                switch part {
                case .prose(let body):
                    Text((i == 0 ? lead : AttributedString()) + Theme.paint(body, face: face))
                        .frame(maxWidth: .infinity, alignment: .leading)
                case .code(let lang, let body):
                    CodeCard(lang: lang, rows: body, face: face)
                case .table(let body):
                    Text(Theme.paint(body, face: face))
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
    }
}

struct CodeCard: View {
    let lang: String
    let rows: [Row]
    let face: Typeface
    @State private var hover = false
    @State private var copied = false

    private var code: String {
        rows.map { $0.body.runs.map(\.t).joined() }
            .joined(separator: "\n")
            .replacingOccurrences(of: "▌", with: "")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(lang.isEmpty ? "code" : lang)
                Spacer()
                Button(copied ? "copied" : "copy") {
                    Platform.copy(code)
                    copied = true
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { copied = false }
                }
                .buttonStyle(.plain)
                .opacity(Platform.hovers ? (hover || copied ? 1 : 0) : 1)
            }
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(Theme.faint)
            ScrollView(.horizontal, showsIndicators: false) {
                Text(Theme.paint(rows.map(\.body), face: face))
                    .fixedSize(horizontal: true, vertical: false)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 8).fill(Color.white.opacity(0.05)))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Color.white.opacity(0.07), lineWidth: 1))
        .onHover { hover = $0 }
    }
}

struct Backdrop: View {
    var body: some View {
        #if os(macOS)
        ZStack {
            Blur().ignoresSafeArea()
            Theme.bg.opacity(0.78).ignoresSafeArea()
        }
        #else
        Theme.bg.ignoresSafeArea()
        #endif
    }
}

#if os(macOS)
struct Blur: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .hudWindow
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {}
}
#endif
