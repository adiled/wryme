use super::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use futures_util::{Stream, StreamExt};
use std::time::Duration;

const GAP: Duration = Duration::from_millis(50);

pub fn rejoin<S>(events: S) -> impl Stream<Item = Event> + Unpin
where
    S: Stream<Item = Event> + Unpin,
{
    Box::pin(futures_util::stream::unfold(
        Rejoin { events, next: None },
        |mut r| async move { r.event().await.map(|ev| (ev, r)) },
    ))
}

struct Rejoin<S> {
    events: S,
    next: Option<Event>,
}

impl<S: Stream<Item = Event> + Unpin> Rejoin<S> {
    async fn soon(&mut self) -> Option<Event> {
        tokio::time::timeout(GAP, self.events.next()).await.ok().flatten()
    }

    async fn event(&mut self) -> Option<Event> {
        loop {
            let ev = match self.next.take() {
                Some(ev) => ev,
                None => self.events.next().await?,
            };
            if !matches!(&ev, Event::Key(k) if k.code == KeyCode::Esc && k.modifiers.is_empty()) {
                return Some(ev);
            }
            let Some(after) = self.soon().await else { return Some(ev) };
            let intro = match char_of(&after) {
                Some(c @ ('[' | 'O')) => c,
                _ => {
                    self.next = Some(after);
                    return Some(ev);
                }
            };
            let mut body = String::new();
            let fin = loop {
                let Some(e) = self.soon().await else { break None };
                match char_of(&e) {
                    Some(c) if intro == '[' && (' '..='?').contains(&c) => body.push(c),
                    Some(c) if ('@'..='~').contains(&c) => break Some(c),
                    _ => {
                        self.next = Some(e);
                        break None;
                    }
                }
            };
            match fin.and_then(|f| decode(intro, &body, f)) {
                Some(Event::Paste(mut text)) => {
                    while let Some(e) = self.soon().await {
                        match pasted(&e) {
                            Some(c) => text.push(c),
                            None => {
                                self.next = Some(e);
                                break;
                            }
                        }
                    }
                    return Some(Event::Paste(text));
                }
                Some(ev) => return Some(ev),
                None => {}
            }
        }
    }
}

fn char_of(ev: &Event) -> Option<char> {
    match ev {
        Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers, .. })
            if matches!(*modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) =>
        {
            Some(*c)
        }
        _ => None,
    }
}

fn pasted(ev: &Event) -> Option<char> {
    match ev {
        Event::Key(k) if k.code == KeyCode::Enter => Some('\n'),
        Event::Key(k) if k.code == KeyCode::Tab => Some('\t'),
        Event::Key(k) if k.code == KeyCode::Char('j') && k.modifiers == KeyModifiers::CONTROL => {
            Some('\n')
        }
        _ => char_of(ev),
    }
}

fn decode(intro: char, body: &str, fin: char) -> Option<Event> {
    if let Some(rest) = body.strip_prefix('<') {
        return mouse(rest, fin);
    }
    let mut nums = body.split(';').map(|n| n.parse::<u16>().ok());
    let first = nums.next().flatten();
    let mods = nums.next().flatten().map(modifiers).unwrap_or(KeyModifiers::NONE);
    let (code, mods) = match (intro, fin, first) {
        ('[', '~', Some(200)) => return Some(Event::Paste(String::new())),
        ('[', '~', Some(n)) => (tilde(n)?, mods),
        ('[', 'Z', _) => (KeyCode::BackTab, KeyModifiers::SHIFT),
        (_, f, _) => (letter(f)?, mods),
    };
    Some(Event::Key(KeyEvent::new(code, mods)))
}

fn letter(f: char) -> Option<KeyCode> {
    Some(match f {
        'A' => KeyCode::Up,
        'B' => KeyCode::Down,
        'C' => KeyCode::Right,
        'D' => KeyCode::Left,
        'H' => KeyCode::Home,
        'F' => KeyCode::End,
        'P'..='S' => KeyCode::F(f as u8 - b'P' + 1),
        _ => return None,
    })
}

fn tilde(n: u16) -> Option<KeyCode> {
    Some(match n {
        1 | 7 => KeyCode::Home,
        2 => KeyCode::Insert,
        3 => KeyCode::Delete,
        4 | 8 => KeyCode::End,
        5 => KeyCode::PageUp,
        6 => KeyCode::PageDown,
        11..=15 => KeyCode::F(n as u8 - 10),
        17..=21 => KeyCode::F(n as u8 - 11),
        23 | 24 => KeyCode::F(n as u8 - 12),
        _ => return None,
    })
}

fn modifiers(m: u16) -> KeyModifiers {
    let m = m.saturating_sub(1);
    let mut k = KeyModifiers::NONE;
    if m & 1 != 0 {
        k |= KeyModifiers::SHIFT;
    }
    if m & 2 != 0 {
        k |= KeyModifiers::ALT;
    }
    if m & 4 != 0 {
        k |= KeyModifiers::CONTROL;
    }
    k
}

fn mouse(body: &str, fin: char) -> Option<Event> {
    let mut nums = body.split(';').map(|n| n.parse::<u16>().ok());
    let (cb, cx, cy) = (nums.next()??, nums.next()??, nums.next()??);
    let button = (cb & 3) | ((cb & 0b1100_0000) >> 4);
    let pick = || match button {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Right),
        _ => None,
    };
    let kind = match (button, cb & 32 != 0) {
        (0..=2, false) if fin == 'm' => MouseEventKind::Up(pick()?),
        (0..=2, false) => MouseEventKind::Down(pick()?),
        (0..=2, true) => MouseEventKind::Drag(pick()?),
        (3, false) => MouseEventKind::Up(MouseButton::Left),
        (3..=5, true) => MouseEventKind::Moved,
        (4, false) => MouseEventKind::ScrollUp,
        (5, false) => MouseEventKind::ScrollDown,
        (6, false) => MouseEventKind::ScrollLeft,
        (7, false) => MouseEventKind::ScrollRight,
        _ => return None,
    };
    Some(Event::Mouse(MouseEvent {
        kind,
        column: cx.saturating_sub(1),
        row: cy.saturating_sub(1),
        modifiers: modifiers(((cb >> 2) & 7) + 1),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(s: &str) -> Vec<Event> {
        s.chars()
            .map(|c| {
                let mods = if c.is_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
                Event::Key(KeyEvent::new(KeyCode::Char(c), mods))
            })
            .collect()
    }

    fn split(rest: &str) -> Vec<Event> {
        let mut v = vec![key(KeyCode::Esc)];
        v.extend(typed(rest));
        v
    }

    async fn run(input: Vec<Event>) -> Vec<Event> {
        rejoin(futures_util::stream::iter(input)).collect().await
    }

    fn at(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE })
    }

    #[tokio::test]
    async fn a_mouse_report_split_after_esc_comes_back_as_mouse_not_text() {
        let out = run([split("[<35;128;2M"), split("[<64;10;5M"), typed("ok")].concat()).await;
        assert_eq!(
            out,
            [
                vec![at(MouseEventKind::Moved, 127, 1), at(MouseEventKind::ScrollUp, 9, 4)],
                typed("ok"),
            ]
            .concat()
        );
    }

    #[tokio::test]
    async fn a_lone_esc_is_still_esc() {
        assert_eq!(run(vec![key(KeyCode::Esc)]).await, vec![key(KeyCode::Esc)]);
        assert_eq!(run(split("x")).await, [vec![key(KeyCode::Esc)], typed("x")].concat());
    }

    #[tokio::test]
    async fn split_keys_and_clicks_decode() {
        let ctrl_right = Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(run(split("[1;5C")).await, vec![ctrl_right]);
        assert_eq!(run(split("OP")).await, vec![key(KeyCode::F(1))]);
        assert_eq!(run(split("[3~")).await, vec![key(KeyCode::Delete)]);
        assert_eq!(
            run([split("[<0;3;4M"), split("[<0;3;4m")].concat()).await,
            vec![
                at(MouseEventKind::Down(MouseButton::Left), 2, 3),
                at(MouseEventKind::Up(MouseButton::Left), 2, 3),
            ]
        );
    }

    #[tokio::test]
    async fn a_split_paste_stays_a_paste() {
        let mut input = split("[200~hi");
        input.push(key(KeyCode::Enter));
        input.extend(typed("there"));
        assert_eq!(run(input).await, vec![Event::Paste("hi\nthere".into())]);
    }

    #[tokio::test]
    async fn a_dead_or_unknown_prefix_is_dropped_not_typed() {
        let click = at(MouseEventKind::Down(MouseButton::Left), 0, 0);
        let mut input = split("[<35;12");
        input.push(click.clone());
        assert_eq!(run(input).await, vec![click]);
        assert_eq!(run(split("[99q")).await, vec![]);
    }

    #[tokio::test]
    async fn esc_then_a_late_bracket_is_two_keys() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let events = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|e| (e, rx))
        });
        tx.send(key(KeyCode::Esc)).unwrap();
        let sender = tokio::spawn(async move {
            tokio::time::sleep(GAP * 2).await;
            tx.send(typed("[")[0].clone()).unwrap();
        });
        let out: Vec<Event> = rejoin(Box::pin(events)).take(2).collect().await;
        sender.await.unwrap();
        assert_eq!(out, [vec![key(KeyCode::Esc)], typed("[")].concat());
    }
}
