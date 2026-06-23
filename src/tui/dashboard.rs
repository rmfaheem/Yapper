use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{BarChart, Paragraph, Tabs};
use ratatui::Frame;

use super::app::{App, DashTab};
use super::theme::{self, Unit};

/// Time window (seconds) the sparkline buffers span, used to label the x-axis.
/// Client throughput is sampled every 250ms; server stats every second.
const CLIENT_WINDOW_S: u64 = 30;
const SERVER_WINDOW_S: u64 = 120;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_client(frame, cols[0], app);
    render_server(frame, cols[1], app);
}

fn render_client(frame: &mut Frame, area: Rect, app: &App) {
    let title = if app.current_flood.is_empty() {
        "Client".to_string()
    } else {
        format!("Client — {}", app.current_flood)
    };
    let block = theme::widget_block(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(3)])
        .split(inner);

    let ops = app.metrics.total_ops();
    let errors = app.metrics.total_errors();
    let now_rate = app.throughput.back().copied().unwrap_or(0);
    let status = if app.flood_running { "running" } else { "idle" };

    let text = vec![
        kv("status", status),
        kv("ops", &ops.to_string()),
        kv("errors", &errors.to_string()),
        kv("ops/sec", &now_rate.to_string()),
        kv("p50 / p99", &format!("{:.1} ms / {:.1} ms", app.p50, app.p99)),
        kv("transferred", &theme::fmt_bytes(app.metrics.total_bytes())),
    ];
    frame.render_widget(Paragraph::new(text), rows[0]);

    let data: Vec<u64> = app.throughput.iter().copied().collect();
    theme::draw_series(
        frame,
        rows[1],
        theme::widget_block("throughput"),
        "ops/sec",
        theme::C_THROUGHPUT,
        &data,
        Unit::Count,
        CLIENT_WINDOW_S,
        app.chart_style,
    );
}

fn render_server(frame: &mut Frame, area: Rect, app: &App) {
    let block = theme::widget_block("Server — KurrentDB /stats");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Tab bar on top, the selected tab's charts below.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let titles: Vec<Line> = DashTab::ALL.iter().map(|t| Line::from(t.title())).collect();
    frame.render_widget(
        Tabs::new(titles)
            .select(app.active_tab)
            .style(theme::tab_titles_line())
            .highlight_style(theme::tab_highlight())
            .divider(Span::styled("│", Style::default().fg(theme::MUTED))),
        rows[0],
    );

    let content = rows[1];
    if app.stats.is_none() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "Waiting for server stats…",
                Style::default().fg(theme::MUTED),
            )),
            content,
        );
        return;
    }

    match DashTab::ALL[app.active_tab] {
        DashTab::System => render_system_tab(frame, content, app),
        DashTab::Disk => render_disk_tab(frame, content, app),
        DashTab::Queues => render_queues_tab(frame, content, app),
        DashTab::Cache => render_cache_tab(frame, content, app),
    }
}

fn render_system_tab(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let cpu: Vec<u64> = app.cpu_hist.iter().copied().collect();
    chart(frame, rows[0], "CPU", "proc", theme::C_CPU, &cpu, Unit::Percent, app.chart_style);

    let mem: Vec<u64> = app.mem_hist.iter().copied().collect();
    chart(frame, rows[1], "Memory", "system", theme::C_MEM, &mem, Unit::Percent, app.chart_style);
}

fn render_disk_tab(frame: &mut Frame, area: Rect, app: &App) {
    let read_b: Vec<u64> = app.disk_read_hist.iter().copied().collect();
    let write_b: Vec<u64> = app.disk_write_hist.iter().copied().collect();
    let read_o: Vec<u64> = app.disk_read_ops_hist.iter().copied().collect();
    let write_o: Vec<u64> = app.disk_write_ops_hist.iter().copied().collect();

    let cells = grid_2x2(area);
    let s = app.chart_style;
    chart(frame, cells[0], "Disk Read", "B/s", theme::C_DISK_READ, &read_b, Unit::Bytes, s);
    chart(frame, cells[1], "Disk Write", "B/s", theme::C_DISK_WRITE, &write_b, Unit::Bytes, s);
    chart(frame, cells[2], "Read Ops", "ops/s", theme::C_THROUGHPUT, &read_o, Unit::Count, s);
    chart(frame, cells[3], "Write Ops", "ops/s", theme::C_WRITER, &write_o, Unit::Count, s);
}

fn render_queues_tab(frame: &mut Frame, area: Rect, app: &App) {
    let reader: Vec<u64> = app.reader_ips_hist.iter().copied().collect();
    let writer: Vec<u64> = app.writer_ips_hist.iter().copied().collect();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(area);

    chart(frame, rows[0], "Reader", "items/s", theme::C_READER, &reader, Unit::Count, app.chart_style);
    chart(frame, rows[1], "Writer", "items/s", theme::C_WRITER, &writer, Unit::Count, app.chart_style);

    // Top queues by length, charted as a bar chart.
    let stats = app.stats.as_ref();
    let qs = stats.map(|s| &s.queues[..s.queues.len().min(6)]).unwrap_or(&[]);
    let names: Vec<String> = qs.iter().map(|(name, _, _)| short(name)).collect();
    let bars: Vec<(&str, u64)> = names
        .iter()
        .zip(qs.iter())
        .map(|(name, (_, len, _))| (name.as_str(), *len))
        .collect();

    let block = theme::widget_block("Queue Length");
    if bars.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("No queue stats.", Style::default().fg(theme::MUTED)))
                .block(block),
            rows[2],
        );
    } else {
        frame.render_widget(
            BarChart::default()
                .block(block)
                .data(bars.as_slice())
                .bar_width(7)
                .bar_gap(1)
                .bar_style(Style::default().fg(theme::C_BARS))
                .value_style(
                    Style::default()
                        .fg(ratatui::style::Color::Black)
                        .bg(theme::C_BARS)
                        .add_modifier(Modifier::BOLD),
                ),
            rows[2],
        );
    }
}

fn render_cache_tab(frame: &mut Frame, area: Rect, app: &App) {
    let hits: Vec<u64> = app.cache_hit_hist.iter().copied().collect();
    let misses: Vec<u64> = app.cache_miss_hist.iter().copied().collect();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    chart(frame, rows[0], "Cache Hits", "rec/s", theme::C_CACHE_HIT, &hits, Unit::Count, app.chart_style);
    chart(frame, rows[1], "Cache Misses", "rec/s", theme::C_CACHE_MISS, &misses, Unit::Count, app.chart_style);
}

/// Render a titled, rounded-border server-stats series in the active style.
#[allow(clippy::too_many_arguments)]
fn chart(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    label: &str,
    color: ratatui::style::Color,
    data: &[u64],
    unit: Unit,
    style: theme::ChartStyle,
) {
    theme::draw_series(
        frame,
        area,
        theme::widget_block(title),
        label,
        color,
        data,
        unit,
        SERVER_WINDOW_S,
        style,
    );
}

/// Split `area` into a 2x2 grid of equal cells.
fn grid_2x2(area: Rect) -> Vec<Rect> {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let mut cells = Vec::with_capacity(4);
    for row in rows.iter() {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(*row);
        cells.push(cols[0]);
        cells.push(cols[1]);
    }
    cells
}

fn kv<'a>(key: &'a str, value: &str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{key:>12}: "), Style::default().fg(theme::MUTED)),
        Span::styled(value.to_string(), Style::default().fg(ratatui::style::Color::White)),
    ])
}

/// Shorten a queue name for the bar chart label.
fn short(name: &str) -> String {
    name.chars().take(7).collect()
}
