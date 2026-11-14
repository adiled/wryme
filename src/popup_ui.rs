use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use crate::app::App;
use crate::popup::{self, Tab};

const MODAL_WIDTH_RATIO: f32 = 0.60;
const MODAL_MIN_WIDTH: u16 = 50;
const BORDER_ROWS: u16 = 2;
const SCROLLBAR_HEIGHT_RATIO: f32 = 0.3;
const SCROLLBAR_MIN_HEIGHT: f32 = 1.0;
const SCROLLBAR_INSET: u16 = 2;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();

    let sheet = crate::scene::sheet(app);
    let (lines, hint) = (&sheet.lines, sheet.hint);

    let modal_w = (area.width as f32 * MODAL_WIDTH_RATIO)
        .max(MODAL_MIN_WIDTH as f32)
        .min(area.width as f32) as u16;
    let modal_h = ((area.height as f32 * 0.70) as u16)
        .max(10)
        .min(area.height.saturating_sub(4));
    let modal_x = area.x + (area.width - modal_w) / 2;
    let modal_y = area.y + (area.height.saturating_sub(modal_h)) / 2;
    let modal_area = Rect {
        x: modal_x,
        y: modal_y,
        width: modal_w,
        height: modal_h,
    };

    let body_h = modal_h.saturating_sub(BORDER_ROWS) as usize;
    let max_scroll = lines.len().saturating_sub(body_h);
    if app.popup.scroll > max_scroll {
        app.popup.scroll = max_scroll;
    }
    if app.popup.mode == popup::Mode::Browse
        && matches!(app.popup.tab, Tab::Station | Tab::Pref)
    {
        let sel = app.popup.selected;
        let last = app.popup.scroll + body_h;
        app.popup.scroll = if sel < app.popup.scroll {
            sel
        } else if sel >= last {
            sel.saturating_sub(body_h).saturating_add(1)
        } else {
            app.popup.scroll
        };
        app.popup.scroll = app.popup.scroll.min(lines.len().saturating_sub(body_h));
    }
    let scroll = app.popup.scroll;

    let visible: Vec<Line<'static>> = lines.iter().skip(scroll).take(body_h).cloned().collect();

    f.render_widget(Clear, modal_area);

    let tab_spans: Vec<Span<'static>> = sheet
        .tabs
        .iter()
        .map(|(label, on)| {
            let style = if *on {
                Style::default()
                    .fg(Color::Black)
                    .bg(app.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Span::styled(format!(" {label} "), style)
        })
        .collect();
    let title = Line::from(tab_spans);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app.accent))
        .title(title.right_aligned())
        .title_bottom(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        ))
        .right_aligned());
    let widget = Paragraph::new(Text::from(visible))
        .block(block)
        .wrap(Wrap { trim: false });
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
        f.render_widget(
            Block::default().style(Style::default().bg(app.accent)),
            bar_rect,
        );
    }

    if let Some(prompt) = &sheet.prompt {
        let input = match app.popup.mode {
            popup::Mode::SaveAs => &app.popup.name_input,
            _ => &app.popup.dial_input,
        };
        let row = lines.len().saturating_sub(1).saturating_sub(scroll) as u16;
        f.set_cursor_position(Position {
            x: modal_area.x + prompt.len() as u16 + input.display_col(),
            y: modal_area.y + 1 + row,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        term.draw(|f| draw(f, app)).unwrap();
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
}
