import SwiftUI

enum Theme {
    static let bg = Color(hex: 0x0b0b0f)
    static let panel = Color(hex: 0x15151b)
    static let fg = Color(hex: 0xd7d5db)
    static let faint = Color(hex: 0x7a7a88)

    static let named: [String: UInt32] = [
        "Black": 0x1c1c22, "Red": 0xa8555c, "Green": 0x6f9c6f, "Yellow": 0xb08c4a,
        "Blue": 0x6f8fb0, "Magenta": 0x9c7ba8, "Cyan": 0x5fa0a0, "Gray": 0xd0ced6,
        "DarkGray": 0x7a7a88, "LightRed": 0xc96a72, "LightGreen": 0x8fc08f,
        "LightYellow": 0xd0a95a, "LightBlue": 0x8fb0d0, "LightMagenta": 0xbc93ca,
        "LightCyan": 0x6fc0c0, "White": 0xf2f0f6,
    ]

    static func color(_ name: String?) -> Color? {
        guard let name else { return nil }
        if name.hasPrefix("#"), let hex = UInt32(name.dropFirst(), radix: 16) {
            return Color(hex: hex)
        }
        return named[name].map { Color(hex: $0) }
    }

    static func paint(_ runs: [Run], size: CGFloat, mono: Bool = false) -> AttributedString {
        var out = AttributedString()
        for run in runs {
            var part = AttributedString(run.t)
            var font = Font.system(size: size, design: mono ? .monospaced : .default)
            if run.b { font = font.bold() }
            if run.i { font = font.italic() }
            part.font = font
            let ink = color(run.fg) ?? fg
            part.foregroundColor = run.d ? ink.opacity(0.6) : ink
            if let fill = color(run.bg) { part.backgroundColor = fill }
            out += part
        }
        return out
    }

    static func paint(_ rows: [Row], size: CGFloat, mono: Bool = false) -> AttributedString {
        var out = AttributedString()
        for (i, row) in rows.enumerated() {
            if i > 0 { out += AttributedString("\n") }
            out += paint(row.runs, size: row.mono || mono ? size - 1 : size, mono: row.mono || mono)
        }
        return out
    }
}

extension Color {
    init(hex: UInt32) {
        self.init(
            red: Double((hex >> 16) & 0xff) / 255,
            green: Double((hex >> 8) & 0xff) / 255,
            blue: Double(hex & 0xff) / 255)
    }
}
