use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use crate::scene::Sheet;
use crate::term::View;

const MODAL_WIDTH_RATIO: f32 = 0.60;
const MODAL_MIN_WIDTH: u16 = 50;
const BORDER_ROWS: u16 = 2;
const SCROLLBAR_HEIGHT_RATIO: f32 = 0.3;
const SCROLLBAR_MIN_HEIGHT: f32 = 1.0;
const SCROLLBAR_INSET: u16 = 2;

pub fn draw(f: &mut Frame, sheet: &Sheet, view: &mut View, accent: Color) {
    let area = f.area();

    let mut lines = sheet.lines.clone();
    let modal_w = (area.width as f32 * MODAL_WIDTH_RATIO)
        .max(MODAL_MIN_WIDTH as f32)
        .min(area.width as f32) as u16;
    let modal_h = ((area.height as f32 * 0.70) as u16)
        .max(10)
        .min(area.height.saturating_sub(4));
    let modal_area = Rect {
        x: area.x + (area.width - modal_w) / 2,
        y: area.y + (area.height.saturating_sub(modal_h)) / 2,
        width: modal_w,
        height: modal_h,
    };

    let mut caret = None;
    if let (Some(field), Some(input)) = (&sheet.field, &view.field)
        && let Some(line) = lines.get_mut(field.row)
    {
        let at = line.width();
        let skip = input.scroll_offset((modal_w as usize).saturating_sub(at + 4).max(1));
        line.spans.push(Span::raw(from_col(&input.text, skip)));
        caret = Some((field.row, (at + 1 + usize::from(input.display_col())).saturating_sub(skip) as u16));
    }

    let body_h = modal_h.saturating_sub(BORDER_ROWS) as usize;
    let max_scroll = lines.len().saturating_sub(body_h);
    if sheet.selected != view.seen_sel {
        view.seen_sel = sheet.selected;
        if let Some(sel) = sheet.selected {
            if sel < view.sheet_scroll {
                let lead = (0..sel).rev().take_while(|i| !sheet.picks.contains(i)).count();
                view.sheet_scroll = sel - lead;
            } else if sel >= view.sheet_scroll + body_h {
                view.sheet_scroll = sel + 1 - body_h;
            }
        }
    }
    view.sheet_scroll = view.sheet_scroll.min(max_scroll);
    let scroll = view.sheet_scroll;

    let visible: Vec<Line<'static>> = lines.iter().skip(scroll).take(body_h).cloned().collect();

    f.render_widget(Clear, modal_area);

    let tab_spans: Vec<Span<'static>> = sheet
        .tabs
        .iter()
        .map(|pill| {
            let style = if pill.on {
                Style::default()
                    .fg(Color::Black)
                    .bg(accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Span::styled(format!(" {} ", pill.label), style)
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .title(Line::from(tab_spans).right_aligned())
        .title_bottom(
            Line::from(Span::styled(format!(" {} ", sheet.hint), Style::default().fg(Color::DarkGray)))
                .right_aligned(),
        );
    let widget = Paragraph::new(Text::from(visible)).block(block);
    let widget = if sheet.picks.is_empty() {
        widget.wrap(Wrap { trim: false })
    } else {
        widget
    };
    f.render_widget(widget, modal_area);

    if lines.len() > body_h {
        let ratio = scroll as f32 / max_scroll.max(1) as f32;
        let bar_h = (body_h as f32 * SCROLLBAR_HEIGHT_RATIO).max(SCROLLBAR_MIN_HEIGHT) as u16;
        let bar_y =
            modal_area.y + 1 + (ratio * (body_h.saturating_sub(bar_h as usize) as f32)) as u16;
        let bar_rect = Rect {
            x: modal_area.x + modal_area.width.saturating_sub(SCROLLBAR_INSET),
            y: bar_y,
            width: 1,
            height: bar_h.min(body_h as u16),
        };
        f.render_widget(Block::default().style(Style::default().bg(accent)), bar_rect);
    }

    view.sheet_body = Rect {
        x: modal_area.x + 1,
        y: modal_area.y + 1,
        width: modal_area.width.saturating_sub(2),
        height: body_h as u16,
    };
    view.tab_hits = tab_hits(f, sheet, modal_area);

    if let Some((row, col)) = caret
        && (scroll..scroll + body_h).contains(&row)
    {
        f.set_cursor_position(Position {
            x: modal_area.x + col,
            y: modal_area.y + 1 + (row - scroll) as u16,
        });
    }
}

fn from_col(text: &str, skip: usize) -> String {
    let mut seen = 0;
    text.chars()
        .skip_while(|c| {
            let before = seen < skip;
            seen += unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0);
            before
        })
        .collect()
}

fn tab_hits(f: &mut Frame, sheet: &Sheet, modal: Rect) -> Vec<(u16, u16, u16)> {
    sheet
        .tabs
        .iter()
        .map(|pill| {
            let (x0, x1) = crate::ui::spot(f, modal.y, modal.x..modal.right(), &format!(" {} ", pill.label))
                .unwrap_or((modal.x, modal.x));
            (modal.y, x0, x1)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::popup;
    use crate::shop::{Protocol, Shop, WindowMode};
    use crate::station::Station;

    fn shop(name: &str, models: &[&str]) -> Shop {
        Shop {
            name: name.into(),
            url: format!("http://{name}"),
            key: String::new(),
            protocol: Protocol::ChatCompletions,
            window: WindowMode::Full,
            models: models.iter().map(|m| m.to_string()).collect(),
            limit: Vec::new(),
            headers: Default::default(),
        }
    }

    fn station(name: &str, model: &str) -> Station {
        Station {
            name: name.into(),
            model: model.into(),
            ..Default::default()
        }
    }

    fn screen(app: &mut App) -> Vec<String> {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 40)).unwrap();
        let scene = crate::scene::build(app);
        let mut view = View::default();
        view.sync(&scene);
        let sheet = scene.sheet.expect("sheet open");
        term.draw(|f| draw(f, &sheet, &mut view, scene.accent)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn station_tab_shows_active_saved_and_unstationed_with_shops() {
        let hum = shop("hum", &["deepseek-v4-flash", "claude-sonnet-5-5"]);
        let opus = station("opus", "claude-opus-5-5");
        let mut app = App::new(
            None,
            vec![Shop::demo(), shop("dwarfstar", &["deepseek-v4-flash", "glm-5.3-flash"]), hum.clone()],
            vec![Station::demo(), opus.clone(), station("sonnet", "claude-sonnet-5-5")],
            opus,
            hum,
            Some("opus".into()),
        );
        app.memory.clear();
        app.popup.mode = popup::Mode::Browse;
        let lines = screen(&mut app);
        let has = |want: &str| lines.iter().any(|l| l.contains(want));
        for want in [
            "active station",
            "opus (claude-opus-5-5) • ?",
            "saved stations",
            "sonnet (claude-sonnet-5-5) • hum",
            "unstationed models",
            "deepseek-v4-flash • dwarfstar, hum",
            "glm-5.3-flash • dwarfstar",
        ] {
            assert!(has(want), "missing {want:?} in:\n{}", lines.join("\n"));
        }
        assert!(!has("claude-sonnet-5-5 • "), "a stationed model is listed as unstationed");
        app.memory.insert("claude-opus-5-5".into(), "hum".into());
        assert!(screen(&mut app).iter().any(|l| l.contains("opus (claude-opus-5-5) • hum")));
    }
    #[test]
    fn the_wheel_reaches_the_end_and_wrapping_shows_the_header() {
        use crate::platform::{Event, KeyModifiers, MouseEvent, MouseEventKind};
        let hum = shop("hum", &["m"]);
        let stations = std::iter::once(Station::demo()).chain((0..30).map(|i| station(&format!("st{i:02}"), "m")));
        let mut app = App::new(None, vec![hum.clone()], stations.collect(), Station::demo(), hum, None);
        popup::toggle(&mut app);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let mut view = View::default();
        let mut frame = |app: &App, view: &mut View| {
            let scene = crate::scene::build(app);
            view.sync(&scene);
            term.draw(|f| draw(f, scene.sheet.as_ref().unwrap(), view, scene.accent)).unwrap();
            let buf = term.backend().buffer().clone();
            let text = (0..24).map(|y| (0..80).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>());
            (scene, text.collect::<Vec<_>>().join("\n"))
        };
        let wheel = Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column: 0, row: 0, modifiers: KeyModifiers::NONE });
        for _ in 0..40 {
            let (scene, _) = frame(&app, &mut view);
            view.feed(wheel.clone(), &scene);
        }
        let (_, text) = frame(&app, &mut view);
        assert!(text.contains("save active as new…"), "{text}");
        for _ in 0..popup::rows(&app).len() {
            popup::move_selection(&mut app, 1);
            frame(&app, &mut view);
        }
        let (_, text) = frame(&app, &mut view);
        assert!(text.contains("active station"), "{text}");
    }

    #[test]
    fn shops_edit_like_stations() {
        use crate::shop_edit::{Draft, Key, Knob};
        let hum = shop("hum", &["deepseek-v4-flash"]);
        let mut app = App::new(None, vec![hum.clone()], vec![Station::demo()], Station::demo(), hum, None);
        let saved = |name: &str| Draft {
            name: name.into(),
            url: format!("http://{name}/v1"),
            key: Key::Env("HUM_KEY".into()),
            models: vec!["deepseek-v4-flash".into()],
            ..Draft::blank()
        };
        app.popup.saved_shops = vec![saved("hum"), saved("ollama")];
        app.popup.shop = Some(popup::Editing {
            draft: saved("hum"),
            base: saved("hum"),
            origin: Some("hum".into()),
        });
        app.popup.mode = popup::Mode::Browse;
        app.popup.tab = popup::Tab::Shops;
        let lines = screen(&mut app);
        let has = |lines: &[String], want: &str| lines.iter().any(|l| l.contains(want));
        for want in ["editing shop", "hum • in use", "url          http://hum/v1", "key          $HUM_KEY", "saved shops", "ollama (responses)", "save as new shop…"] {
            assert!(has(&lines, want), "missing {want:?} in:\n{}", lines.join("\n"));
        }
        assert!(!has(&lines, "update 'hum'"));

        let row = |app: &App, want: popup::Row| {
            popup::rows(app).iter().position(|r| format!("{r:?}") == format!("{want:?}")).unwrap()
        };
        let at = row(&app, popup::Row::ShopKnob(Knob::Protocol));
        popup::pick(&mut app, at);
        let at = row(&app, popup::Row::ShopKnob(Knob::Models));
        popup::pick(&mut app, at);
        assert_eq!(app.popup.mode, popup::Mode::ShopEdit);
        let field = crate::scene::build(&app).sheet.unwrap().field.unwrap();
        assert_eq!((field.label.as_str(), field.text.as_str()), ("models", "deepseek-v4-flash"));
        let lines = screen(&mut app);
        let editing: Vec<&String> = lines.iter().filter(|l| l.contains("models")).collect();
        assert_eq!(editing.len(), 1, "{}", lines.join("\n"));
        assert!(editing[0].contains("models       deepseek-v4-flash"));
        popup::commit_shop_edit(&mut app, "deepseek-v4-flash, glm-5");
        let lines = screen(&mut app);
        for want in ["protocol     chat-completions", "models       deepseek-v4-flash, glm-5", "hum • in use • changed", "update 'hum'"] {
            assert!(has(&lines, want), "missing {want:?} in:\n{}", lines.join("\n"));
        }
        let at = row(&app, popup::Row::SavedShop(1));
        popup::pick(&mut app, at);
        assert_eq!(app.popup.shop.as_ref().unwrap().origin.as_deref(), Some("ollama"));
    }
}
