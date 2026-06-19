use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{BarChart, Block, Borders, Gauge, Paragraph, Sparkline, Tabs};
use ratatui::Frame;

use super::app::{App, DashTab};

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
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Green));
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
        kv("transferred", &fmt_bytes(app.metrics.total_bytes())),
    ];
    frame.render_widget(Paragraph::new(text), rows[0]);

    let data: Vec<u64> = app.throughput.iter().copied().collect();
    let spark = Sparkline::default()
        .block(
            Block::default()
                .borders(Borders::TOP)
                .title("throughput (ops/sec)"),
        )
        .data(&data)
        .style(Style::default().fg(Color::Cyan));
    frame.render_widget(spark, rows[1]);
}

fn render_server(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Server — KurrentDB /stats   (Tab ↹ switches)")
        .border_style(Style::default().fg(Color::LightBlue));
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
            .style(Style::default().fg(Color::DarkGray))
            .highlight_style(
                Style::default()
                    .fg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD),
            )
            .divider(" "),
        rows[0],
    );

    let content = rows[1];
    if app.stats.is_none() {
        frame.render_widget(Paragraph::new("Waiting for server stats…"), content);
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
    let Some(stats) = &app.stats else { return };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // cpu gauge
            Constraint::Length(3), // mem gauge
            Constraint::Min(3),    // cpu history
        ])
        .split(area);

    let cpu = stats.proc_cpu.clamp(0.0, 100.0) / 100.0;
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::NONE).title("proc CPU"))
            .gauge_style(Style::default().fg(Color::Red))
            .ratio(cpu)
            .label(format!("{:.1}%", stats.proc_cpu)),
        rows[0],
    );

    let (used, total) = (stats.mem_used(), stats.sys_total_mem);
    let mem_ratio = if total > 0 {
        (used as f64 / total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::NONE).title("system memory"))
            .gauge_style(Style::default().fg(Color::Magenta))
            .ratio(mem_ratio)
            .label(format!("{} / {}", fmt_bytes(used), fmt_bytes(total))),
        rows[1],
    );

    let cpu_data: Vec<u64> = app.cpu_hist.iter().copied().collect();
    spark(frame, rows[2], "proc CPU %", Color::Red, &cpu_data, Fmt::Count);
}

fn render_disk_tab(frame: &mut Frame, area: Rect, app: &App) {
    let read_b: Vec<u64> = app.disk_read_hist.iter().copied().collect();
    let write_b: Vec<u64> = app.disk_write_hist.iter().copied().collect();
    let read_o: Vec<u64> = app.disk_read_ops_hist.iter().copied().collect();
    let write_o: Vec<u64> = app.disk_write_ops_hist.iter().copied().collect();

    let cells = grid(area, 4);
    spark(frame, cells[0], "disk read/sec", Color::Yellow, &read_b, Fmt::Bytes);
    spark(frame, cells[1], "disk write/sec", Color::Green, &write_b, Fmt::Bytes);
    spark(frame, cells[2], "read ops/sec", Color::Cyan, &read_o, Fmt::Count);
    spark(frame, cells[3], "write ops/sec", Color::LightMagenta, &write_o, Fmt::Count);
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

    spark(frame, rows[0], "reader threads (items/sec)", Color::Cyan, &reader, Fmt::Count);
    spark(frame, rows[1], "writer threads (items/sec)", Color::Green, &writer, Fmt::Count);

    // Top queues by length, charted as a bar chart.
    let stats = app.stats.as_ref();
    let qs = stats.map(|s| &s.queues[..s.queues.len().min(6)]).unwrap_or(&[]);
    let names: Vec<String> = qs.iter().map(|(name, _, _)| short(name)).collect();
    let bars: Vec<(&str, u64)> = names
        .iter()
        .zip(qs.iter())
        .map(|(name, (_, len, _))| (name.as_str(), *len))
        .collect();

    if bars.is_empty() {
        frame.render_widget(
            Paragraph::new("No queue stats.")
                .block(Block::default().borders(Borders::TOP).title("queue length")),
            rows[2],
        );
    } else {
        frame.render_widget(
            BarChart::default()
                .block(Block::default().borders(Borders::TOP).title("queue length"))
                .data(bars.as_slice())
                .bar_width(7)
                .bar_gap(1)
                .bar_style(Style::default().fg(Color::LightBlue))
                .value_style(Style::default().add_modifier(Modifier::BOLD)),
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

    spark(frame, rows[0], "read-index cache hits/sec", Color::Green, &hits, Fmt::Count);
    spark(frame, rows[1], "read-index cache misses/sec", Color::Red, &misses, Fmt::Count);
}

/// How to render the latest-value annotation in a sparkline title.
#[derive(Clone, Copy)]
enum Fmt {
    Bytes,
    Count,
}

/// Split `area` into `n` equal vertical rows.
fn grid(area: Rect, n: usize) -> std::rc::Rc<[Rect]> {
    let constraints: Vec<Constraint> = (0..n).map(|_| Constraint::Ratio(1, n as u32)).collect();
    Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area)
}

/// Render a top-bordered sparkline whose title shows the most recent value.
fn spark(frame: &mut Frame, area: Rect, label: &str, color: Color, data: &[u64], fmt: Fmt) {
    let latest = data.last().copied().unwrap_or(0);
    let shown = match fmt {
        Fmt::Bytes => fmt_bytes(latest),
        Fmt::Count => latest.to_string(),
    };
    frame.render_widget(
        Sparkline::default()
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .title(format!("{label} ({shown})")),
            )
            .data(data)
            .style(Style::default().fg(color)),
        area,
    );
}

fn kv<'a>(key: &'a str, value: &str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{key:>12}: "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(value.to_string(), Style::default().fg(Color::White)),
    ])
}

/// Shorten a queue name for the bar chart label.
fn short(name: &str) -> String {
    let trimmed: String = name.chars().take(7).collect();
    trimmed
}

fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
