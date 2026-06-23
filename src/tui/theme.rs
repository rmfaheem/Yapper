//! Shared colours and widget helpers giving Yapper a `bottom`-inspired look:
//! rounded grey widget boxes, inset top-left titles, braille line graphs with
//! axis scales, and corner legends.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, BorderType, Borders, Chart, Dataset, GraphType, Paragraph, Sparkline,
};

/// Resting widget border colour (bottom's calm grey frame).
pub const BORDER: Color = Color::Rgb(99, 110, 123);
/// Accent used for titles, the selected tab and other highlights.
pub const ACCENT: Color = Color::Rgb(108, 181, 230);
/// Dimmed text for labels, scales and hints.
pub const MUTED: Color = Color::Rgb(120, 130, 140);

// Graph series colours, picked to read clearly side by side like bottom's.
pub const C_THROUGHPUT: Color = Color::Rgb(97, 214, 214);
pub const C_CPU: Color = Color::Rgb(235, 110, 110);
pub const C_MEM: Color = Color::Rgb(122, 162, 247);
pub const C_DISK_READ: Color = Color::Rgb(229, 192, 123);
pub const C_DISK_WRITE: Color = Color::Rgb(126, 199, 124);
pub const C_READER: Color = C_THROUGHPUT;
pub const C_WRITER: Color = C_DISK_WRITE;
pub const C_CACHE_HIT: Color = C_DISK_WRITE;
pub const C_CACHE_MISS: Color = C_CPU;
pub const C_BARS: Color = ACCENT;

/// A rounded, grey-bordered widget box with a `bottom`-style inset title.
///
/// The title renders as ` Label ` in the accent colour so it sits in the top
/// border like `── Label ──`.
pub fn widget_block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
}

/// How time-series widgets are drawn. Toggled live at runtime; not persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartStyle {
    /// Braille line graphs with axis scales (the default, bottom-style).
    Lines,
    /// Compact sparkline bars.
    Bars,
}

impl ChartStyle {
    pub fn toggle(self) -> Self {
        match self {
            ChartStyle::Lines => ChartStyle::Bars,
            ChartStyle::Bars => ChartStyle::Lines,
        }
    }

    /// Short label naming the *current* style, for the hint bar.
    pub fn label(self) -> &'static str {
        match self {
            ChartStyle::Lines => "lines",
            ChartStyle::Bars => "bars",
        }
    }
}

/// How a chart's latest value / axis scale should be formatted.
#[derive(Clone, Copy)]
pub enum Unit {
    /// Plain counts (e.g. ops/sec, items/sec).
    Count,
    /// Byte rates / sizes, rendered with a binary unit suffix.
    Bytes,
    /// A 0..=100 percentage.
    Percent,
}

impl Unit {
    fn fmt(self, value: f64) -> String {
        match self {
            Unit::Count => fmt_count(value),
            Unit::Bytes => fmt_bytes(value as u64),
            Unit::Percent => format!("{value:.0}%"),
        }
    }
}

/// Render a time-series `data` into `block` using the caller's chosen `style`.
#[allow(clippy::too_many_arguments)]
pub fn draw_series(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    block: Block<'static>,
    label: &str,
    color: Color,
    data: &[u64],
    unit: Unit,
    window_secs: u64,
    style: ChartStyle,
) {
    match style {
        ChartStyle::Lines => line_chart(frame, area, block, label, color, data, unit, window_secs),
        ChartStyle::Bars => bar_spark(frame, area, block, label, color, data, unit, window_secs),
    }
}

/// Render `data` as sparkline bars with the same y/x scales as the line charts:
/// a left gutter labelling `0..=peak` and a bottom row labelling the time window.
#[allow(clippy::too_many_arguments)]
fn bar_spark(
    frame: &mut ratatui::Frame,
    area: Rect,
    block: Block<'static>,
    label: &str,
    color: Color,
    data: &[u64],
    unit: Unit,
    window_secs: u64,
) {
    let latest = data.last().copied().unwrap_or(0);
    let peak = match unit {
        Unit::Percent => 100,
        _ => data.iter().copied().max().unwrap_or(0).max(1),
    };

    // Top-right legend echoing the line chart's `label  value`.
    let legend = Line::from(Span::styled(
        format!(" {label}  {} ", unit.fmt(latest as f64)),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
    .right_aligned();
    let block = block.title(legend);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let peak_label = unit.fmt(peak as f64);
    let gutter_w = peak_label.len().max(1) as u16;

    // Reserve a bottom row for the x-axis and a left gutter for the y-axis.
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(gutter_w), Constraint::Min(1)])
        .split(vert[0]);
    let gutter = cols[0];
    let plot = cols[1];

    let axis = Style::default().fg(MUTED);
    // y-axis: peak at the top of the gutter, 0 at the bottom.
    if gutter.height >= 1 {
        frame.render_widget(
            Paragraph::new(Span::styled(peak_label, axis)),
            Rect { height: 1, ..gutter },
        );
        frame.render_widget(
            Paragraph::new(Span::styled("0", axis)),
            Rect { y: gutter.y + gutter.height - 1, height: 1, ..gutter },
        );
    }

    frame.render_widget(
        Sparkline::default()
            .data(data)
            .max(peak)
            .style(Style::default().fg(color)),
        plot,
    );

    // x-axis: window span on the left, "now" on the right, aligned under the plot.
    frame.render_widget(
        Paragraph::new(Span::styled(format!("{window_secs}s"), axis)),
        Rect { x: plot.x, ..vert[1] },
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled("now", axis)).alignment(Alignment::Right)),
        vert[1],
    );
}

/// Render a `bottom`-style braille line graph for `data` into `block`.
///
/// * `label` names the series; the legend shows `label` + the latest value.
/// * the y-axis is scaled `0..=max` with `0` and the peak labelled.
/// * the x-axis is labelled with the time window the samples span.
///
/// The caller owns `block` so widgets can share the title/border helpers.
#[allow(clippy::too_many_arguments)]
pub fn line_chart(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    block: Block<'static>,
    label: &str,
    color: Color,
    data: &[u64],
    unit: Unit,
    window_secs: u64,
) {
    // Percentages always scale to a full 0..100 axis; everything else scales to
    // its own peak (with a little headroom so the line never hugs the top).
    let peak = data.iter().copied().max().unwrap_or(0) as f64;
    let y_max = match unit {
        Unit::Percent => 100.0,
        _ => (peak * 1.15).max(1.0),
    };
    let latest = data.last().copied().unwrap_or(0) as f64;

    let points: Vec<(f64, f64)> = data
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as f64, v as f64))
        .collect();
    let x_max = (data.len().max(1) - 1).max(1) as f64;

    let datasets = vec![Dataset::default()
        .name(format!("{label}  {}", unit.fmt(latest)))
        .marker(Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(color))
        .data(&points)];

    let axis = Style::default().fg(MUTED);
    let chart = Chart::new(datasets)
        .block(block)
        .style(Style::default())
        .x_axis(
            Axis::default()
                .style(axis)
                .bounds([0.0, x_max])
                .labels(vec![
                    Span::styled(format!("{window_secs}s"), axis),
                    Span::styled("now", axis),
                ]),
        )
        .y_axis(
            Axis::default()
                .style(axis)
                .bounds([0.0, y_max])
                .labels(vec![
                    Span::styled("0", axis),
                    Span::styled(unit.fmt(y_max), axis),
                ]),
        )
        .legend_position(Some(ratatui::widgets::LegendPosition::TopRight))
        .hidden_legend_constraints((
            ratatui::layout::Constraint::Ratio(1, 1),
            ratatui::layout::Constraint::Ratio(1, 1),
        ));
    frame.render_widget(chart, area);
}

pub fn tab_titles_line() -> Style {
    Style::default().fg(MUTED)
}

pub fn tab_highlight() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn fmt_count(value: f64) -> String {
    if value >= 1_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.1}k", value / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}

pub fn fmt_bytes(bytes: u64) -> String {
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
