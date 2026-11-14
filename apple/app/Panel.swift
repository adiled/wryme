import SwiftUI

struct Panel: View {
    let sheet: Sheet
    let stage: Stage
    let accent: Color
    @EnvironmentObject var core: Core
    @FocusState private var editing: Bool
    @State private var position = ScrollPosition(idType: Int.self)
    @State private var geo = GeoBox()
    #if os(iOS)
    @State private var query = ""
    #endif

    var body: some View {
        ZStack {
            Color.black.opacity(0.45)
                .ignoresSafeArea()
                .onTapGesture { core.act("close") }
            VStack(alignment: .leading, spacing: 12) {
                tabs
                #if os(iOS)
                if sheet.tabs.first?.on == true, sheet.field == nil {
                    TextField("search stations", text: $query)
                        .textFieldStyle(.roundedBorder)
                        .onChange(of: query) { _, text in core.find(text) }
                }
                #endif
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 3) {
                        ForEach(Array(sheet.lines.enumerated()), id: \.offset) { i, row in
                            line(i, row).id(i)
                        }
                    }
                    .scrollTargetLayout()
                }
                .scrollPosition($position)
                .onScrollGeometryChange(for: Geo.self, of: Geo.init) { _, now in geo.value = now }
                .onReceive(core.sheetMoves) { move in
                    switch move {
                    case .top: position.scrollTo(edge: .top)
                    case .by(let dir): withAnimation { position.scrollTo(y: geo.value.target(dir)) }
                    }
                }
                .onChange(of: sheet.selected) { _, row in
                    if let row { withAnimation { position.scrollTo(id: row) } }
                }
                if let field = sheet.field {
                    HStack(spacing: 8) {
                        Text("\(field.label):").foregroundStyle(accent)
                        editor
                            .textFieldStyle(.plain)
                            .focused($editing)
                            .onAppear { editing = true }
                    }
                    .font(Theme.font(stage.font, size: 13, mono: true))
                }
                Text(sheet.hint.trimmingCharacters(in: .whitespaces))
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.faint)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            .padding(18)
            .frame(maxWidth: 640, maxHeight: 500)
            .background(RoundedRectangle(cornerRadius: 14).fill(Theme.panel))
            .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(accent.opacity(0.7), lineWidth: 1))
            .shadow(color: .black.opacity(0.5), radius: 30, y: 10)
            .padding(24)
        }
    }

    private var tabs: some View {
        HStack(spacing: 4) {
            Spacer()
            ForEach(Array(sheet.tabs.enumerated()), id: \.offset) { i, tab in
                Button {
                    core.tab(i)
                } label: {
                    Text(tab.label)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(tab.on ? Theme.bg : Theme.fg)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background(RoundedRectangle(cornerRadius: 6).fill(tab.on ? accent : .clear))
                }
                .buttonStyle(.plain)
            }
        }
    }

    @ViewBuilder
    private var editor: some View {
        #if os(iOS)
        TextField("", text: $core.fieldText, selection: $core.fieldSelection)
        #else
        TextField("", text: $core.fieldText)
        #endif
    }

    @ViewBuilder
    private func line(_ i: Int, _ row: Row) -> some View {
        let text = Text(Theme.paint([row], face: stage.font, size: 13, mono: true))
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        if sheet.picks.contains(i) {
            text.onTapGesture { core.pick(i) }
        } else {
            text
        }
    }
}
