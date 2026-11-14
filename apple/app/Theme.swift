import SwiftUI

enum Theme {
    static let bg = Color(hex: 0x0b0b0f)
    static let panel = Color(hex: 0x15151b)
    static let fg = Color(hex: 0xd7d5db)
    static let faint = Color(hex: 0x7a7a88)
    static let size: CGFloat = 14

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

    static func design(_ face: Typeface) -> Font.Design {
        switch face {
        case .system: .default
        case .serif: .serif
        case .rounded: .rounded
        case .mono: .monospaced
        }
    }

    static func font(_ face: Typeface, size: CGFloat = size, mono: Bool = false) -> Font {
        .system(size: size, design: mono ? .monospaced : design(face))
    }

    static func paint(_ runs: [Run], face: Typeface, size: CGFloat = size, mono: Bool = false) -> AttributedString {
        var out = AttributedString()
        for run in runs {
            var part = AttributedString(run.t)
            var font = Theme.font(face, size: size, mono: mono)
            if run.b { font = font.bold() }
            if run.i { font = font.italic() }
            part.font = font
            let ink = color(run.fg) ?? fg
            part.foregroundColor = run.d ? ink.opacity(0.6) : ink
            if let fill = color(run.bg) { part.backgroundColor = fill }
            if run.u { part.underlineStyle = .single }
            if run.x { part.strikethroughStyle = .single }
            out += part
        }
        return out
    }

    static func paint(_ rows: [Row], face: Typeface, size: CGFloat = size, mono: Bool = false) -> AttributedString {
        var out = AttributedString()
        for (i, row) in rows.enumerated() {
            if i > 0 { out += AttributedString("\n") }
            let fixed = mono || row.mono
            out += paint(row.runs, face: face, size: fixed && !mono ? size - 1 : size, mono: fixed)
        }
        return out
    }

    static func plain(_ text: String, _ ink: Color, face: Typeface, size: CGFloat = size, bold: Bool = false) -> AttributedString {
        var out = AttributedString(text)
        var font = Theme.font(face, size: size)
        if bold { font = font.bold() }
        out.font = font
        out.foregroundColor = ink
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
