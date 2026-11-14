use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Rect;

use crate::action::{self, Action, Chord, Core, Edit, Key, Pane, Scope};
use crate::app::ViewMode;
use crate::control::Intent;
use crate::input::Input;
use crate::platform::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use crate::scene::Scene;
use crate::select::{Point, Screen, Selection};

const SHEET_ROWS_PER_TICK: usize = 2;
const ROWS_PER_TICK: usize = 2;
const TICKS_PER_PAGE: i32 = 3;

#[derive(Default)]
pub struct View {
    pub input: Input,
    pub field: Option<Input>,
    pub page: usize,
    pub scroll_row: usize,
    pub page_base: usize,
    pub wheel_accum: i32,
    pub viewport_h: usize,
    pub selection: Option<Selection>,
    pub screen: Screen,
    pub sheet_scroll: usize,
    pub sheet_body: Rect,
    pub tab_hits: Vec<(u16, u16, u16)>,
    seen: Option<(u64, ViewMode)>,
    seen_tab: Option<usize>,
}

pub struct Term<B: Backend> {
    pub terminal: Terminal<B>,
    pub view: View,
}

impl<B: Backend> Term<B> {
    pub fn new(terminal: Terminal<B>) -> Self {
        Self {
            terminal,
            view: View::default(),
        }
    }
}

impl<B: Backend<Error = std::io::Error>> crate::Surface for Term<B> {
    type Event = Event;

    fn present(&mut self, scene: &Scene) -> Result<()> {
        self.view.sync(scene);
        let view = &mut self.view;
        self.terminal
            .draw(|f| crate::ui::draw(f, scene, view))
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("draw: {e}"))
    }

    fn feed(&mut self, ev: Event, scene: &Scene) -> Vec<Intent> {
        self.view.feed(ev, scene)
    }
}

fn key_of(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    })
}

impl View {
    pub fn sync(&mut self, scene: &Scene) {
        let now = (scene.turn, scene.view);
        if self.seen != Some(now) {
            self.seen = Some(now);
            self.page = 0;
            self.scroll_row = 0;
            self.wheel_accum = 0;
        }
        let tab = scene
            .sheet
            .as_ref()
            .and_then(|s| s.tabs.iter().position(|t| t.on));
        if tab != self.seen_tab {
            self.seen_tab = tab;
            self.sheet_scroll = 0;
        }
        let field = scene.sheet.as_ref().and_then(|s| s.field.as_ref());
        match (field, self.field.is_some()) {
            (Some(f), false) => {
                let mut input = Input::new();
                input.text = f.text.clone();
                input.end();
                self.field = Some(input);
            }
            (None, true) => self.field = None,
            _ => {}
        }
    }

    pub fn feed(&mut self, ev: Event, scene: &Scene) -> Vec<Intent> {
        match ev {
            Event::Key(k) => self.key(k, scene),
            Event::Mouse(m) => self.mouse(m, scene),
            Event::Paste(text) => {
                if let Some(input) = self.typing(scene.scope) {
                    input.insert_paste(&text);
                }
                Vec::new()
            }
            #[cfg(target_arch = "wasm32")]
            Event::GuestNote(note) => vec![Intent::Note(note)],
            #[cfg(target_arch = "wasm32")]
            Event::ReloadConfig => vec![Intent::Reload],
            _ => Vec::new(),
        }
    }

    fn typing(&mut self, scope: Scope) -> Option<&mut Input> {
        match scope {
            Scope::Chat => Some(&mut self.input),
            Scope::Field => self.field.as_mut(),
            Scope::Sheet | Scope::Any => None,
        }
    }

    fn key(&mut self, k: KeyEvent, scene: &Scene) -> Vec<Intent> {
        if k.kind == KeyEventKind::Release {
            return Vec::new();
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let cmd = k.modifiers.contains(KeyModifiers::SUPER);
        if shift
            && scene.scope == Scope::Chat
            && let Some(sel) = self.selection.as_mut()
        {
            let delta = match k.code {
                KeyCode::Left => Some((-1, 0)),
                KeyCode::Right => Some((1, 0)),
                KeyCode::Up => Some((0, -1)),
                KeyCode::Down => Some((0, 1)),
                _ => None,
            };
            if let Some((dc, dr)) = delta {
                sel.move_head(dc, dr);
                return Vec::new();
            }
        }
        let Some(key) = key_of(k.code) else {
            return Vec::new();
        };
        if let Some(action) = action::lookup(scene.scope, Chord { key, ctrl, shift, cmd }) {
            return self.act(action, scene);
        }
        match (k.code, scene.scope) {
            (KeyCode::Char(_), _) if ctrl || cmd => Vec::new(),
            (KeyCode::Char(c), Scope::Sheet) => vec![Intent::Search(c)],
            (KeyCode::Char(c), scope) => {
                if let Some(input) = self.typing(scope) {
                    input.insert_char(c);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn act(&mut self, action: Action, scene: &Scene) -> Vec<Intent> {
        match action {
            Action::Core(Core::Interrupt) if self.selection.take().is_some() => Vec::new(),
            Action::Core(_) => vec![Intent::Act(action)],
            Action::Edit(e) => self.edit(e, scene.scope),
            Action::Pane(p) => self.pane(p, scene),
        }
    }

    fn edit(&mut self, e: Edit, scope: Scope) -> Vec<Intent> {
        let Some(input) = self.typing(scope) else {
            return Vec::new();
        };
        match e {
            Edit::Send => {
                let text = match scope {
                    Scope::Field => input.text.clone(),
                    _ => input.take(),
                };
                return vec![Intent::Submit(text)];
            }
            Edit::Left => input.move_left(),
            Edit::Right => input.move_right(),
            Edit::Home => input.home(),
            Edit::End => input.end(),
            Edit::Backspace => input.backspace(),
            Edit::Delete => input.delete_forward(),
            Edit::KillToStart => input.kill_to_start(),
            Edit::KillToEnd => input.kill_to_end(),
            Edit::KillWord => input.kill_prev_word(),
        }
        Vec::new()
    }

    fn pane(&mut self, p: Pane, scene: &Scene) -> Vec<Intent> {
        let sheet = scene.sheet.is_some();
        match p {
            Pane::Copy => return self.finish_selection(),
            Pane::PageUp if sheet => self.sheet_scroll = self.sheet_scroll.saturating_add(1),
            Pane::PageDown if sheet => self.sheet_scroll = self.sheet_scroll.saturating_sub(1),
            Pane::PageUp => self.page_up(scene.view),
            Pane::PageDown => self.page_down(scene.view),
        }
        Vec::new()
    }

    fn finish_selection(&mut self) -> Vec<Intent> {
        let Some(sel) = self.selection.take() else {
            return Vec::new();
        };
        let text = sel.extract(&self.screen);
        if text.trim().is_empty() {
            return Vec::new();
        }
        crate::platform::copy(&text);
        vec![Intent::Say("copied")]
    }

    fn step(&self) -> usize {
        self.viewport_h.saturating_sub(1).max(1)
    }

    fn trigger(&self) -> usize {
        self.page_base.saturating_sub(self.viewport_h / 2)
    }

    fn page_up(&mut self, view: ViewMode) {
        match view {
            ViewMode::Page if self.page_base > 0 && self.page == 0 => {
                let cur = self.scroll_row.min(self.trigger());
                if cur.saturating_add(self.step()) >= self.trigger() {
                    self.scroll_row = self.trigger();
                    self.page = 1;
                } else {
                    self.scroll_row = cur.saturating_add(self.step());
                }
                self.wheel_accum = 0;
            }
            ViewMode::Page => self.page = self.page.saturating_add(1),
            ViewMode::Scroll => self.scroll_row = self.scroll_row.saturating_add(self.step()),
        }
    }

    fn page_down(&mut self, view: ViewMode) {
        match view {
            ViewMode::Page if self.page_base > 0 => {
                if self.page >= 2 {
                    self.page -= 1;
                } else if self.page == 1 {
                    self.page = 0;
                    self.scroll_row = self.trigger();
                    self.wheel_accum = 0;
                } else {
                    self.scroll_row = self.scroll_row.saturating_sub(self.step());
                    self.wheel_accum = 0;
                }
            }
            ViewMode::Page => self.page = self.page.saturating_sub(1),
            ViewMode::Scroll => self.scroll_row = self.scroll_row.saturating_sub(self.step()),
        }
    }

    fn wheel(&mut self, up: bool, view: ViewMode) {
        match view {
            ViewMode::Page if self.page_base > 0 && self.page == 0 => {
                let trigger = self.trigger();
                if !up {
                    self.scroll_row = self.scroll_row.saturating_sub(ROWS_PER_TICK);
                } else if self.scroll_row.min(trigger) >= trigger {
                    self.scroll_row = trigger;
                    self.page = 1;
                    self.wheel_accum = 0;
                } else {
                    self.scroll_row = self.scroll_row.min(trigger).saturating_add(ROWS_PER_TICK).min(trigger);
                }
            }
            ViewMode::Page => {
                self.wheel_accum += if up { 1 } else { -1 };
                while self.wheel_accum >= TICKS_PER_PAGE {
                    self.page = self.page.saturating_add(1);
                    self.wheel_accum -= TICKS_PER_PAGE;
                }
                while self.wheel_accum <= -TICKS_PER_PAGE {
                    self.page = self.page.saturating_sub(1);
                    self.wheel_accum += TICKS_PER_PAGE;
                }
                if self.page == 0 && self.page_base > 0 {
                    self.scroll_row = self.trigger();
                }
            }
            ViewMode::Scroll if up => self.scroll_row = self.scroll_row.saturating_add(ROWS_PER_TICK),
            ViewMode::Scroll => self.scroll_row = self.scroll_row.saturating_sub(ROWS_PER_TICK),
        }
    }

    fn mouse(&mut self, m: MouseEvent, scene: &Scene) -> Vec<Intent> {
        let point = Point {
            col: m.column,
            row: m.row,
        };
        let up = matches!(m.kind, MouseEventKind::ScrollUp);
        let wheel = up || matches!(m.kind, MouseEventKind::ScrollDown);
        match (scene.scope, m.kind) {
            (Scope::Chat, _) if wheel => self.wheel(up, scene.view),
            (_, _) if wheel && up => {
                self.sheet_scroll = self.sheet_scroll.saturating_add(SHEET_ROWS_PER_TICK)
            }
            (_, _) if wheel => {
                self.sheet_scroll = self.sheet_scroll.saturating_sub(SHEET_ROWS_PER_TICK)
            }
            (Scope::Chat, MouseEventKind::Down(MouseButton::Left)) => {
                self.selection = Some(Selection::new(point));
            }
            (Scope::Chat, MouseEventKind::Drag(MouseButton::Left)) => {
                if let Some(sel) = self.selection.as_mut() {
                    sel.extend(point);
                }
            }
            (Scope::Chat, MouseEventKind::Up(MouseButton::Left)) => return self.finish_selection(),
            (Scope::Sheet, MouseEventKind::Down(MouseButton::Left)) => return self.click(point, scene),
            _ => {}
        }
        Vec::new()
    }

    fn click(&self, p: Point, scene: &Scene) -> Vec<Intent> {
        if let Some(i) = self
            .tab_hits
            .iter()
            .position(|&(y, x0, x1)| p.row == y && (x0..x1).contains(&p.col))
        {
            return vec![Intent::Tab(i)];
        }
        let body = self.sheet_body;
        let inside = (body.x..body.x + body.width).contains(&p.col)
            && (body.y..body.y + body.height).contains(&p.row);
        if !inside {
            return Vec::new();
        }
        let line = self.sheet_scroll + usize::from(p.row - body.y);
        match &scene.sheet {
            Some(s) if s.picks.contains(&line) => vec![Intent::Pick(line)],
            _ => Vec::new(),
        }
    }
}
