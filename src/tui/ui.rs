use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::app::App;
use super::dashboard;

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(15),   // dashboard (always visible)
            Constraint::Length(8), // console log
            Constraint::Length(3), // input
        ])
        .split(frame.area());

    draw_header(frame, chunks[0], app);
    dashboard::render(frame, chunks[1], app);
    draw_console(frame, chunks[2], app);
    draw_input(frame, chunks[3], app);

    // Help is an overlay so the dashboard stays live underneath it.
    if app.show_help {
        draw_help_overlay(frame, frame.area());
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let status = if app.flood_running {
        Span::styled(
            "● running",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("○ idle", Style::default().fg(Color::DarkGray))
    };

    let header = Line::from(vec![
        Span::styled(
            " Yapper 🗣 ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("— KurrentDB test client   ", Style::default().fg(Color::LightBlue)),
        status,
        Span::styled("   ·  `help` for commands", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(header), area);
}

fn draw_console(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Console")
        .border_style(Style::default().fg(Color::LightBlue));
    let inner_height = block.inner(area).height as usize;

    // Auto-scroll: show the last lines that fit.
    let start = app.output.len().saturating_sub(inner_height);
    let lines: Vec<Line> = app.output[start..]
        .iter()
        .map(|l| Line::from(l.as_str()))
        .collect();

    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_input(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));
    let inner = block.inner(area);

    let line = Line::from(vec![
        Span::styled("> ", Style::default().fg(Color::Green)),
        Span::raw(app.input.as_str()),
    ]);
    frame.render_widget(Paragraph::new(line).block(block), area);

    // Position the real terminal cursor: "> " prefix (2 cols) + cursor offset.
    let cursor_x = inner.x + 2 + app.cursor as u16;
    let cursor_x = cursor_x.min(inner.x + inner.width.saturating_sub(1));
    frame.set_cursor_position((cursor_x, inner.y));
}

fn draw_help_overlay(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(72, 80, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Help — Esc to close")
        .border_style(Style::default().fg(Color::Green));
    frame.render_widget(
        Paragraph::new(HELP_TEXT)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}

/// A rectangle centered in `area`, sized as a percentage of width/height.
fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(vertical[1])[1]
}

const HELP_TEXT: &str = "\
The dashboard is always live above. Just type a command and press Enter.

Commands:

  help               Show this help overlay
  clear              Clear the console log
  quit / exit        Quit Yapper

  wrfl [flags]       Write flood
  rdfl [flags]       Read flood

  Flood flags:
    -c, --clients <n>        Number of concurrent clients
    -r, --requests <n>       Requests per client (wrfl: events per stream)
    -s, --streams <n>        Streams per client (wrfl only)
    -e, --event-size <n>     Event payload size in bytes (wrfl only)
    -b, --batch-size <n>     Batch / page size
    -p, --stream-prefix <s>  Prefix for generated streams

  Examples:
    wrfl -c 4 -r 1000 -s 10 -e 50 -b 5 -p yap
    rdfl -c 4 -r 200 -b 100

Server dashboard tabs: System · Disk · Queues · Cache
  Tab ↹ next tab · Shift+Tab previous tab

Editing:  ←/→ move · Home/End · ↑/↓ command history · Backspace/Del
Keys:     Enter run · Esc close help · Ctrl+C / Ctrl+D quit";
