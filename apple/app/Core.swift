import SwiftUI

struct Run: Decodable {
    let t: String
    let fg: String?
    let bg: String?
    let b: Bool
    let i: Bool
    let d: Bool
}

struct Row: Decodable {
    let mono: Bool
    let runs: [Run]
}

struct Bubble: Decodable, Identifiable {
    let id: String
    let user: Bool
    let name: String
    let activity: String?
    let tool: String?
    let waiting: Bool
    let live: Bool
    let time: String
    let images: [String]
    let reply: [Row]
    let brain: [Row]
    let heart: [Row]
}

struct Bar: Decodable {
    let state: String
    let esc: String
    let busy: Bool
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

struct Sheet: Decodable {
    let tabs: [Pill]
    let lines: [Row]
    let hint: String
}

struct Stage: Decodable {
    let accent: String?
    let bar: Bar
    let bubbles: [Bubble]
    let status: Status
    let error: String?
    let sheet: Sheet?
}

struct Fatal: Decodable {
    let fatal: String
}

final class Core: ObservableObject {
    static let shared = Core()
    @Published var stage: Stage?
    @Published var fatal: String?

    func start() {
        wryme_start { text in
            guard let text else { return }
            let data = Data(bytes: text, count: strlen(text))
            let decoder = JSONDecoder()
            if let stage = try? decoder.decode(Stage.self, from: data) {
                DispatchQueue.main.async { Core.shared.stage = stage }
            } else if let fatal = try? decoder.decode(Fatal.self, from: data) {
                DispatchQueue.main.async { Core.shared.fatal = fatal.fatal }
            }
        }
    }

    func submit(_ text: String) {
        send(["submit": text])
    }

    func key(_ name: String, ctrl: Bool = false) {
        send(["key": name, "ctrl": ctrl])
    }

    private func send(_ msg: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: msg),
            let text = String(data: data, encoding: .utf8)
        else { return }
        wryme_send(text)
    }
}
