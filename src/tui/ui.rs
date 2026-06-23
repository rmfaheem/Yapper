use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::app::App;
use super::dashboard;
use super::theme;

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(15),   // dashboard (always visible)
            Constraint::Length(8), // console log
            Constraint::Length(3), // input
            Constraint::Length(1), // keybinding hint bar
        ])
        .split(frame.area());

    draw_header(frame, chunks[0], app);
    dashboard::render(frame, chunks[1], app);
    draw_console(frame, chunks[2], app);
    draw_input(frame, chunks[3], app);
    draw_hint_bar(frame, chunks[4], app);

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
        Span::styled("○ idle", Style::default().fg(theme::MUTED))
    };

    let header = Line::from(vec![
        Span::styled(
            " Yapper 🗣 ",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("— KurrentDB test client   ", Style::default().fg(theme::MUTED)),
        status,
    ]);
    frame.render_widget(Paragraph::new(header), area);
}

fn draw_console(frame: &mut Frame, area: Rect, app: &App) {
    let block = theme::widget_block("Console");
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
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::ACCENT));
    let inner = block.inner(area);

    let line = Line::from(vec![
        Span::styled("> ", Style::default().fg(theme::ACCENT)),
        Span::raw(app.input.as_str()),
    ]);
    frame.render_widget(Paragraph::new(line).block(block), area);

    // Position the real terminal cursor: "> " prefix (2 cols) + cursor offset.
    let cursor_x = inner.x + 2 + app.cursor as u16;
    let cursor_x = cursor_x.min(inner.x + inner.width.saturating_sub(1));
    frame.set_cursor_position((cursor_x, inner.y));
}

/// `bottom`-style keybinding hint bar pinned to the very bottom.
fn draw_hint_bar(frame: &mut Frame, area: Rect, app: &App) {
    let key = |k: &str| Span::styled(k.to_string(), Style::default().fg(theme::ACCENT));
    let sep = || Span::styled("   ", Style::default().fg(theme::MUTED));
    let desc = |d: &str| Span::styled(d.to_string(), Style::default().fg(theme::MUTED));

    let line = Line::from(vec![
        key("Tab"),
        desc(" switch panel"),
        sep(),
        key("Ctrl+B"),
        desc(&format!(" view: {}", app.chart_style.label())),
        sep(),
        key("help"),
        desc(" commands"),
        sep(),
        key("Ctrl+C"),
        desc(" quit"),
    ])
    .alignment(Alignment::Center);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_help_overlay(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(72, 80, area);
    frame.render_widget(Clear, popup);
    let block = theme::widget_block("Help — Esc to close");
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

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::metrics::Metrics;
    use crate::stats::ServerStats;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Smoke-render the whole TUI with synthetic data and dump it so the
    /// charts, axis scales and legends can be eyeballed with --nocapture.
    #[test]
    fn dashboard_renders_with_scales() {
        let mut app = App::new(Metrics::new());
        for i in 0..120u64 {
            app.throughput.push_back((i * 37 % 900) + 50);
            app.cpu_hist.push_back((i * 7 % 100).max(3));
            app.mem_hist.push_back(40 + (i % 30));
            app.disk_read_hist.push_back((i * 1024 * 13) % (5 * 1024 * 1024));
            app.disk_write_hist.push_back((i * 1024 * 9) % (3 * 1024 * 1024));
        }
        app.p50 = 1.4;
        app.p99 = 12.8;
        app.stats = Some(ServerStats {
            sys_total_mem: 16 * 1024 * 1024 * 1024,
            sys_free_mem: 9 * 1024 * 1024 * 1024,
            queues: vec![("Writer".into(), 12, 8.0), ("Reader".into(), 3, 2.0)],
            ..Default::default()
        });

        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();

        app.chart_style = super::theme::ChartStyle::Lines;
        terminal.draw(|f| draw(f, &app)).unwrap();
        println!("=== LINES ===\n{}", terminal.backend());

        app.chart_style = super::theme::ChartStyle::Bars;
        terminal.draw(|f| draw(f, &app)).unwrap();
        println!("=== BARS ===\n{}", terminal.backend());

        assert_eq!(terminal.backend().buffer().area.width, 120);
    }

    /// Dump the Disk tab in bars mode so the y/x scales on bar charts can be
    /// eyeballed with --nocapture.
    #[test]
    fn bars_have_scales() {
        let mut app = App::new(Metrics::new());
        for i in 0..40u64 {
            app.disk_read_hist.push_back((i % 8) * 256 * 1024);
            app.disk_write_hist.push_back((i % 5) * 128 * 1024);
            app.disk_read_ops_hist.push_back((i % 6) * 40);
            app.disk_write_ops_hist.push_back((i % 4) * 25);
        }
        app.stats = Some(ServerStats::default());
        app.active_tab = 1; // Disk
        app.chart_style = super::theme::ChartStyle::Bars;

        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        println!("=== DISK / BARS ===\n{}", terminal.backend());
    }
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

View:     Ctrl+B toggles line charts vs sparkline bars

Editing:  ←/→ move · Home/End · ↑/↓ command history · Backspace/Del
Keys:     Enter run · Esc close help · Ctrl+C / Ctrl+D quit";
