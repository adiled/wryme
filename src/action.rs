use serde::{Deserialize, Serialize, Serializer};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Core {
    Quit,
    Help,
    Voice,
    Settings,
    ViewMode,
    Interrupt,
    Close,
    Cancel,
    NextTab,
    Up,
    Down,
    Less,
    More,
    Activate,
    SearchBack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Edit {
    Send,
    Left,
    Right,
    Home,
    End,
    Backspace,
    Delete,
    KillToStart,
    KillToEnd,
    KillWord,
    Picture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pane {
    PageUp,
    PageDown,
    Copy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Action {
    Core(Core),
    Edit(Edit),
    Pane(Pane),
}

impl Edit {
    pub const ALL: &[Edit] = &[
        Edit::Send,
        Edit::Left,
        Edit::Right,
        Edit::Home,
        Edit::End,
        Edit::Backspace,
        Edit::Delete,
        Edit::KillToStart,
        Edit::KillToEnd,
        Edit::KillWord,
        Edit::Picture,
    ];
}

impl Pane {
    pub const ALL: &[Pane] = &[Pane::PageUp, Pane::PageDown, Pane::Copy];
}

impl Action {
    fn owner(self) -> &'static str {
        match self {
            Action::Core(_) => "core",
            Action::Edit(_) => "edit",
            Action::Pane(_) => "pane",
        }
    }

    fn about(self, scope: Scope) -> &'static str {
        match self {
            Action::Core(c) => match c {
                Core::Quit => "quit",
                Core::Help => "open Help",
                Core::Voice => "read replies aloud on / off",
                Core::Settings => "open settings",
                Core::ViewMode => "toggle page / scroll view",
                Core::Interrupt => "stop a reply, quiet voice, clear note",
                Core::Close => "close settings",
                Core::Cancel => "cancel editing",
                Core::NextTab => "next tab",
                Core::Up => "previous row",
                Core::Down => "next row",
                Core::Less => "adjust down",
                Core::More => "adjust up",
                Core::Activate => "load, toggle or edit the row",
                Core::SearchBack => "erase search",
            },
            Action::Edit(e) => match e {
                Edit::Send if scope == Scope::Field => "save",
                Edit::Send => "send",
                Edit::Left => "cursor left",
                Edit::Right => "cursor right",
                Edit::Home => "jump to start",
                Edit::End => "jump to end",
                Edit::Backspace => "delete before cursor",
                Edit::Delete => "delete after cursor",
                Edit::KillToStart => "delete to start",
                Edit::KillToEnd => "delete to end",
                Edit::KillWord => "delete previous word",
                Edit::Picture => "attach the clipboard image",
            },
            Action::Pane(p) => match p {
                Pane::PageUp => "page up",
                Pane::PageDown => "page down",
                Pane::Copy => "copy the selection",
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Any,
    Chat,
    Sheet,
    Field,
}

impl Scope {
    pub const ALL: &[Scope] = &[Scope::Any, Scope::Chat, Scope::Sheet, Scope::Field];

    fn title(self) -> &'static str {
        match self {
            Scope::Any => "Anywhere:",
            Scope::Chat => "In the chat:",
            Scope::Sheet => "In settings:",
            Scope::Field => "Editing a name or value:",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    F(u8),
}

impl Key {
    pub fn name(self) -> String {
        match self {
            Key::Char(c) => c.to_string(),
            Key::Enter => "enter".into(),
            Key::Esc => "esc".into(),
            Key::Tab => "tab".into(),
            Key::Up => "up".into(),
            Key::Down => "down".into(),
            Key::Left => "left".into(),
            Key::Right => "right".into(),
            Key::Home => "home".into(),
            Key::End => "end".into(),
            Key::PageUp => "page_up".into(),
            Key::PageDown => "page_down".into(),
            Key::Backspace => "backspace".into(),
            Key::Delete => "delete".into(),
            Key::F(n) => format!("f{n}"),
        }
    }

    fn label(self) -> String {
        match self {
            Key::Char(c) => c.to_ascii_uppercase().to_string(),
            Key::Enter => "Enter".into(),
            Key::Esc => "Esc".into(),
            Key::Tab => "Tab".into(),
            Key::Up => "↑".into(),
            Key::Down => "↓".into(),
            Key::Left => "←".into(),
            Key::Right => "→".into(),
            Key::Home => "Home".into(),
            Key::End => "End".into(),
            Key::PageUp => "PgUp".into(),
            Key::PageDown => "PgDn".into(),
            Key::Backspace => "Backspace".into(),
            Key::Delete => "Delete".into(),
            Key::F(n) => format!("F{n}"),
        }
    }
}

impl Serialize for Key {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.name())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Chord {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
    pub cmd: bool,
}

impl Chord {
    pub fn label(self) -> String {
        let mut out = String::new();
        if self.cmd {
            out.push_str("Cmd-");
        }
        if self.ctrl {
            out.push_str("Ctrl-");
        }
        if self.shift {
            out.push_str("Shift-");
        }
        out.push_str(&self.key.label());
        out
    }

    fn normal(self) -> Self {
        match self.key {
            Key::Char(c) => Self {
                key: Key::Char(c.to_ascii_lowercase()),
                ..self
            },
            _ => self,
        }
    }

    fn fits(self, pressed: Chord, relax: bool) -> bool {
        self.key == pressed.key
            && self.ctrl == pressed.ctrl
            && self.cmd == pressed.cmd
            && (self.shift == pressed.shift || (relax && !self.shift))
    }
}

pub struct Binding {
    pub scope: Scope,
    pub chord: Chord,
    pub action: Action,
}

const fn key(key: Key) -> Chord {
    Chord {
        key,
        ctrl: false,
        shift: false,
        cmd: false,
    }
}

const fn ctrl(c: char) -> Chord {
    Chord {
        key: Key::Char(c),
        ctrl: true,
        shift: false,
        cmd: false,
    }
}

const fn bind(scope: Scope, chord: Chord, action: Action) -> Binding {
    Binding {
        scope,
        chord,
        action,
    }
}

const fn core(scope: Scope, chord: Chord, c: Core) -> Binding {
    bind(scope, chord, Action::Core(c))
}

const fn edit(scope: Scope, chord: Chord, e: Edit) -> Binding {
    bind(scope, chord, Action::Edit(e))
}

const fn pane(scope: Scope, chord: Chord, p: Pane) -> Binding {
    bind(scope, chord, Action::Pane(p))
}

const COPY_SHIFT: Chord = Chord {
    key: Key::Char('c'),
    ctrl: true,
    shift: true,
    cmd: false,
};

const COPY_CMD: Chord = Chord {
    key: Key::Char('c'),
    ctrl: false,
    shift: false,
    cmd: true,
};

macro_rules! typing {
    ($scope:expr) => {
        [
            edit($scope, key(Key::Left), Edit::Left),
            edit($scope, key(Key::Right), Edit::Right),
            edit($scope, key(Key::Home), Edit::Home),
            edit($scope, ctrl('a'), Edit::Home),
            edit($scope, key(Key::End), Edit::End),
            edit($scope, ctrl('e'), Edit::End),
            edit($scope, key(Key::Backspace), Edit::Backspace),
            edit($scope, key(Key::Delete), Edit::Delete),
            edit($scope, ctrl('u'), Edit::KillToStart),
            edit($scope, ctrl('k'), Edit::KillToEnd),
            edit($scope, ctrl('w'), Edit::KillWord),
        ]
    };
}

const ANY: [Binding; 5] = [
    pane(Scope::Any, COPY_SHIFT, Pane::Copy),
    pane(Scope::Any, COPY_CMD, Pane::Copy),
    core(Scope::Any, ctrl('c'), Core::Quit),
    core(Scope::Any, key(Key::F(1)), Core::Help),
    core(Scope::Any, ctrl('v'), Core::Voice),
];

const CHAT: [Binding; 7] = [
    edit(Scope::Chat, key(Key::Enter), Edit::Send),
    edit(Scope::Chat, ctrl('y'), Edit::Picture),
    core(Scope::Chat, key(Key::Esc), Core::Interrupt),
    core(Scope::Chat, ctrl('s'), Core::Settings),
    core(Scope::Chat, ctrl('t'), Core::ViewMode),
    pane(Scope::Chat, key(Key::PageUp), Pane::PageUp),
    pane(Scope::Chat, key(Key::PageDown), Pane::PageDown),
];

const CHAT_TYPING: [Binding; 11] = typing!(Scope::Chat);

const SHEET: [Binding; 11] = [
    core(Scope::Sheet, key(Key::Up), Core::Up),
    core(Scope::Sheet, key(Key::Down), Core::Down),
    core(Scope::Sheet, key(Key::Left), Core::Less),
    core(Scope::Sheet, key(Key::Right), Core::More),
    core(Scope::Sheet, key(Key::Enter), Core::Activate),
    core(Scope::Sheet, key(Key::Tab), Core::NextTab),
    core(Scope::Sheet, key(Key::Backspace), Core::SearchBack),
    pane(Scope::Sheet, key(Key::PageUp), Pane::PageUp),
    pane(Scope::Sheet, key(Key::PageDown), Pane::PageDown),
    core(Scope::Sheet, key(Key::Esc), Core::Close),
    core(Scope::Sheet, ctrl('s'), Core::Close),
];

const FIELD: [Binding; 3] = [
    edit(Scope::Field, key(Key::Enter), Edit::Send),
    core(Scope::Field, key(Key::Esc), Core::Cancel),
    core(Scope::Field, ctrl('s'), Core::Close),
];

const FIELD_TYPING: [Binding; 11] = typing!(Scope::Field);

pub fn bindings() -> impl Iterator<Item = &'static Binding> {
    ANY.iter()
        .chain(CHAT.iter())
        .chain(CHAT_TYPING.iter())
        .chain(SHEET.iter())
        .chain(FIELD.iter())
        .chain(FIELD_TYPING.iter())
}

pub fn lookup(scope: Scope, pressed: Chord) -> Option<Action> {
    let pressed = pressed.normal();
    let hit = |relax: bool| {
        bindings()
            .find(|b| {
                (b.scope == Scope::Any || b.scope == scope) && b.chord.fits(pressed, relax)
            })
            .map(|b| b.action)
    };
    hit(false).or_else(|| hit(true))
}

pub fn help() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for &scope in Scope::ALL {
        if !out.is_empty() {
            out.push((String::new(), String::new()));
        }
        out.push((scope.title().to_string(), String::new()));
        let mut seen: Vec<Action> = Vec::new();
        for b in bindings().filter(|b| b.scope == scope) {
            if seen.contains(&b.action) {
                continue;
            }
            seen.push(b.action);
            let keys: Vec<String> = bindings()
                .filter(|o| o.scope == scope && o.action == b.action)
                .map(|o| o.chord.label())
                .collect();
            out.push((keys.join(" / "), b.action.about(scope).to_string()));
        }
        if scope == Scope::Sheet {
            out.push(("letters".into(), "search saved stations".into()));
        }
    }
    out
}

pub fn table() -> serde_json::Value {
    bindings()
        .map(|b| {
            serde_json::json!({
                "scope": b.scope,
                "chord": b.chord,
                "action": b.action,
                "owner": b.action.owner(),
                "about": b.action.about(b.scope),
                "label": b.chord.label(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key, ctrl: bool, shift: bool) -> Chord {
        Chord {
            key,
            ctrl,
            shift,
            cmd: false,
        }
    }

    #[test]
    fn ctrl_s_opens_in_chat_and_closes_in_sheet() {
        let s = press(Key::Char('s'), true, false);
        assert_eq!(lookup(Scope::Chat, s), Some(Action::Core(Core::Settings)));
        assert_eq!(lookup(Scope::Sheet, s), Some(Action::Core(Core::Close)));
        assert_eq!(lookup(Scope::Field, s), Some(Action::Core(Core::Close)));
    }

    #[test]
    fn ctrl_c_quits_and_ctrl_shift_c_copies() {
        assert_eq!(
            lookup(Scope::Chat, press(Key::Char('c'), true, false)),
            Some(Action::Core(Core::Quit))
        );
        assert_eq!(
            lookup(Scope::Chat, press(Key::Char('C'), true, true)),
            Some(Action::Pane(Pane::Copy))
        );
    }

    #[test]
    fn shift_is_relaxed_only_when_nothing_wants_it() {
        assert_eq!(
            lookup(Scope::Chat, press(Key::Enter, false, true)),
            Some(Action::Edit(Edit::Send))
        );
        assert_eq!(lookup(Scope::Chat, press(Key::Char('x'), false, false)), None);
    }

    #[test]
    fn any_scope_wins_everywhere() {
        for &scope in &[Scope::Chat, Scope::Sheet, Scope::Field] {
            assert_eq!(
                lookup(scope, press(Key::F(1), false, false)),
                Some(Action::Core(Core::Help))
            );
        }
    }

    #[test]
    fn every_chord_is_unique_within_its_reach() {
        for a in bindings() {
            for b in bindings() {
                if std::ptr::eq(a, b) || a.chord != b.chord {
                    continue;
                }
                let overlap = a.scope == b.scope || a.scope == Scope::Any || b.scope == Scope::Any;
                assert!(!overlap, "{} bound twice", a.chord.label());
            }
        }
    }

    #[test]
    fn help_lists_every_binding() {
        let rows = help();
        for b in bindings() {
            assert!(
                rows.iter().any(|(k, _)| k.split(" / ").any(|c| c == b.chord.label())),
                "help is missing {}",
                b.chord.label()
            );
        }
    }

    #[test]
    fn table_names_actions_flat() {
        let t = table();
        let first = &t[0];
        assert_eq!(first["action"], "copy");
        assert_eq!(first["owner"], "pane");
        assert_eq!(first["chord"]["key"], "c");
    }
}
