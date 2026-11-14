import SwiftUI

#if os(macOS)
import AppKit

typealias PlatformFont = NSFont
#else
import UIKit

typealias PlatformFont = UIFont
#endif

extension Theme {
    static func platformFont(_ face: Typeface, size: CGFloat, mono: Bool = false) -> PlatformFont {
        let base = PlatformFont.systemFont(ofSize: size)
        #if os(macOS)
        let design: NSFontDescriptor.SystemDesign
        #else
        let design: UIFontDescriptor.SystemDesign
        #endif
        switch mono ? .mono : face {
        case .system: design = .default
        case .serif: design = .serif
        case .rounded: design = .rounded
        case .mono: design = .monospaced
        }
        guard let described = base.fontDescriptor.withDesign(design) else { return base }
        #if os(macOS)
        return NSFont(descriptor: described, size: size) ?? base
        #else
        return UIFont(descriptor: described, size: size)
        #endif
    }

    static func underscore(_ font: PlatformFont) -> CGFloat {
        max(("_" as NSString).size(withAttributes: [.font: font]).width, 6)
    }
}

#if os(macOS)
final class TyperView: NSTextView {
    private let bar = NSView()
    var ink: NSColor = .white {
        didSet { bar.layer?.backgroundColor = ink.cgColor }
    }

    func setUp() {
        insertionPointColor = .clear
        bar.wantsLayer = true
        bar.layer?.backgroundColor = ink.cgColor
        bar.isHidden = true
        addSubview(bar)
    }

    func placeCaret() {
        guard let window, window.firstResponder === self, selectedRange().length == 0, let font else {
            bar.isHidden = true
            return
        }
        let screen = firstRect(forCharacterRange: selectedRange(), actualRange: nil)
        let line = convert(window.convertFromScreen(screen), from: nil)
        bar.frame = NSRect(x: line.minX, y: line.maxY - 3, width: Theme.underscore(font), height: 2)
        bar.isHidden = false
        let blink = CAKeyframeAnimation(keyPath: "opacity")
        blink.values = [1, 0]
        blink.keyTimes = [0, 0.5, 1]
        blink.calculationMode = .discrete
        blink.duration = 1
        blink.repeatCount = .infinity
        bar.layer?.removeAnimation(forKey: "blink")
        bar.layer?.add(blink, forKey: "blink")
    }

    private func later() {
        DispatchQueue.main.async { [weak self] in self?.placeCaret() }
    }

    override func setSelectedRanges(_ ranges: [NSValue], affinity: NSSelectionAffinity, stillSelecting: Bool) {
        super.setSelectedRanges(ranges, affinity: affinity, stillSelecting: stillSelecting)
        later()
    }

    override func didChangeText() {
        super.didChangeText()
        later()
    }

    override func becomeFirstResponder() -> Bool {
        defer { later() }
        return super.becomeFirstResponder()
    }

    override func resignFirstResponder() -> Bool {
        defer { later() }
        return super.resignFirstResponder()
    }

    override func setFrameSize(_ size: NSSize) {
        super.setFrameSize(size)
        later()
    }
}

struct Typer: NSViewRepresentable {
    @Binding var text: String
    @Binding var caret: Int
    let font: NSFont
    let ink: Color
    let lines: Int
    let focus: Int

    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: Typer
        var focused = -1

        init(_ parent: Typer) { self.parent = parent }

        func textDidChange(_ note: Notification) {
            guard let view = note.object as? NSTextView else { return }
            parent.text = view.string
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSScrollView {
        let view = TyperView(usingTextLayoutManager: false)
        view.setUp()
        view.delegate = context.coordinator
        view.drawsBackground = false
        view.isRichText = false
        view.allowsUndo = true
        view.isAutomaticQuoteSubstitutionEnabled = false
        view.isAutomaticDashSubstitutionEnabled = false
        view.isAutomaticTextReplacementEnabled = false
        view.isAutomaticSpellingCorrectionEnabled = false
        view.textContainerInset = .zero
        view.textContainer?.lineFragmentPadding = 0
        view.textContainer?.widthTracksTextView = true
        view.isVerticallyResizable = true
        view.isHorizontallyResizable = false
        view.autoresizingMask = [.width]
        view.minSize = .zero
        view.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasVerticalScroller = false
        scroll.documentView = view
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.parent = self
        guard let view = scroll.documentView as? TyperView else { return }
        view.font = font
        view.textColor = NSColor(ink)
        view.ink = NSColor(ink)
        if view.string != text {
            view.string = text
            view.setSelectedRange(NSRange(location: (text as NSString).length, length: 0))
        }
        if context.coordinator.focused != focus {
            context.coordinator.focused = focus
            DispatchQueue.main.async { view.window?.makeFirstResponder(view) }
        }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView scroll: NSScrollView, context: Context) -> CGSize? {
        guard let view = scroll.documentView as? NSTextView, let layout = view.layoutManager,
            let box = view.textContainer
        else { return nil }
        let width = proposal.width ?? 300
        box.containerSize = NSSize(width: width, height: .greatestFiniteMagnitude)
        layout.ensureLayout(for: box)
        let line = layout.defaultLineHeight(for: font)
        let used = layout.usedRect(for: box).height
        return CGSize(width: width, height: min(max(used, line), line * CGFloat(lines)))
    }
}
#else
final class TyperView: UITextView {
    override func caretRect(for position: UITextPosition) -> CGRect {
        let line = super.caretRect(for: position)
        guard let font else { return line }
        return CGRect(x: line.minX, y: line.maxY - 3, width: Theme.underscore(font), height: 2)
    }
}

struct Typer: UIViewRepresentable {
    @Binding var text: String
    @Binding var caret: Int
    let font: UIFont
    let ink: Color
    let lines: Int
    let focus: Int

    final class Coordinator: NSObject, UITextViewDelegate {
        var parent: Typer
        var focused = -1

        init(_ parent: Typer) { self.parent = parent }

        func textViewDidChange(_ view: UITextView) {
            parent.text = view.text
        }

        func textViewDidChangeSelection(_ view: UITextView) {
            let at = view.selectedRange.location
            if parent.caret != at { parent.caret = at }
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> TyperView {
        let view = TyperView()
        view.delegate = context.coordinator
        view.backgroundColor = .clear
        view.textContainerInset = .zero
        view.textContainer.lineFragmentPadding = 0
        view.autocorrectionType = .no
        view.smartQuotesType = .no
        view.smartDashesType = .no
        view.autocapitalizationType = .none
        view.setContentHuggingPriority(.defaultHigh, for: .vertical)
        return view
    }

    func updateUIView(_ view: TyperView, context: Context) {
        context.coordinator.parent = self
        view.font = font
        view.textColor = UIColor(ink)
        view.tintColor = UIColor(ink)
        if view.text != text { view.text = text }
        let length = (view.text as NSString).length
        let at = min(caret, length)
        if view.selectedRange != NSRange(location: at, length: 0), view.selectedRange.length == 0 {
            view.selectedRange = NSRange(location: at, length: 0)
        }
        if context.coordinator.focused != focus {
            context.coordinator.focused = focus
            DispatchQueue.main.async { view.becomeFirstResponder() }
        }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView view: TyperView, context: Context) -> CGSize? {
        let width = proposal.width ?? 300
        let line = font.lineHeight
        let used = view.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height
        view.isScrollEnabled = used > line * CGFloat(lines)
        return CGSize(width: width, height: min(max(used, line), line * CGFloat(lines)))
    }
}
#endif
