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

    let mut spans = vec![
        key("Tab"),
        desc(" switch panel"),
        sep(),
        key("Ctrl+B"),
        desc(&format!(" view: {}", app.chart_style.label())),
        sep(),
        key("Ctrl+H"),
        desc(" help"),
        sep(),
    ];
    // Surface the cancel key only while a command is running.
    if app.flood_running {
        spans.push(key("Esc"));
        spans.push(desc(" cancel"));
        spans.push(sep());
    }
    spans.push(key("Ctrl+C"));
    spans.push(desc(" quit"));

    let line = Line::from(spans).alignment(Alignment::Center);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_help_overlay(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(72, 80, area);
    frame.render_widget(Clear, popup);
    let block = theme::widget_block("Help — Ctrl+H or Esc to close");
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
    use crate::stats::{QueueStat, ServerStats};
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
            reader_queue_peak: 7,
            writer_queue_peak: 31,
            tcp_connections: 42,
            queues: vec![
                QueueStat {
                    name: "persistentSubscriptions".into(),
                    length_current_try_peak: 55,
                },
                QueueStat {
                    name: "Writer".into(),
                    length_current_try_peak: 31,
                },
            ],
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

    /// The Client panel folds the running job's current stage into its status line.
    #[test]
    fn client_status_shows_current_stage() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(Metrics::new());
        app.flood_running = true;
        app.current_flood = "psub flood -n 2".into();
        app.current_stage = "Subscribing 4 consumer(s) (ack-mode Ack)…".into();

        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        // The trailing detail may be clipped at the panel width; the stage prefix
        // is what matters for the "what stage are we at" status.
        let dump = format!("{}", terminal.backend());
        assert!(
            dump.contains("running — Subscribing 4 consumer"),
            "status line missing stage:\n{dump}"
        );
    }

    /// Smoke-render the Subs and Conns tabs so they can be eyeballed (--nocapture)
    /// and to guard against layout panics on the new tabs.
    #[test]
    fn subs_and_conns_tabs_render() {
        let mut app = App::new(Metrics::new());
        for i in 0..60u64 {
            app.psub_peak_hist.push_back((i * 3) % 120);
            app.reader_peak_hist.push_back((i * 2) % 50);
            app.writer_peak_hist.push_back((i * 5) % 90);
            app.conn_hist.push_back(10 + (i % 25));
            app.tcp_recv_hist.push_back((i * 1024 * 7) % (2 * 1024 * 1024));
            app.tcp_send_hist.push_back((i * 1024 * 3) % (1024 * 1024));
        }
        app.stats = Some(ServerStats {
            reader_queue_peak: 7,
            writer_queue_peak: 31,
            tcp_connections: 23,
            queues: vec![
                QueueStat { name: "persistentSubscriptions".into(), length_current_try_peak: 55 },
                QueueStat { name: "Subscriptions".into(), length_current_try_peak: 4 },
            ],
            ..Default::default()
        });

        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        for (tab, name) in [(2, "QUEUES"), (3, "SUBS"), (4, "CONNS")] {
            app.active_tab = tab;
            terminal.draw(|f| draw(f, &app)).unwrap();
            println!("=== {name} ===\n{}", terminal.backend());
        }
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
The dashboard is always live above. Type a command and press Enter — the same
commands work on the `yapper` CLI (e.g. `yapper write flood -c 8`).

Each command runs in single mode by default; append `flood` for many concurrent
clients. `-c/--clients` is a flood-only flag (single mode is always one client).

Commands:

  clear              Clear the console log
  stop / cancel      Gracefully stop the running command (same as Esc)
  quit / exit        Quit Yapper

  write [flood]      Append one event (-s stream -e data -t type -f file),
                       or `write flood` for a multi-client write load test
  read  [flood]      Read one stream (-s stream -n count -b backwards),
                       or `read flood` to page through $all under load
  csub  [flood]      Catch-up subscribe / live tail (-s stream, default $all),
                       or `csub flood` for many concurrent readers
  psub  [flood]      Persistent subscribe (-s stream -g group --create --keep),
                       or `psub flood` for many groups × competing clients

  write/read flood flags:
    -c, --clients <n>        Concurrent clients (default 4)
    -r, --requests <n>       Requests per client (write: events per stream)
    -s, --streams <n>        Streams per client (write only)
    -e, --event-size <n>     Event payload size in bytes (write only)
    -b, --batch-size <n>     Batch / page size
    -p, --stream-prefix <s>  Prefix for generated streams
    -d, --duration <secs>    Run for N seconds of sustained load (ignores --requests)

  csub flood flags (mirror psub; reads each stream to its end):
    -n, --subscriptions <n>  Streams to read (one set of readers per {prefix}{i})
    -c, --clients <n>        Concurrent catch-up readers per stream (default 4)
    -p, --stream-prefix <s>  Stream prefix (default yapper-cs-)
        --create-streams     Populate streams first if missing/empty
        --stream-length <n>  Events per stream when creating (default 10000)
    -e, --event-size <n>     Payload size when creating (default 64)
    -d, --duration <secs>    Timeout: stop if not caught up first (0 = no timeout)
        --delete-streams     Also delete created streams on a clean exit (kept by default)
  Exits when every reader reaches the end of its stream, or when the timeout fires.

  psub flood flags:
    -n, --subscriptions <n>  Subscription groups (one per stream {prefix}{i})
    -c, --clients <n>        Competing consumers per group (default 4)
        --ack-mode <m>       ack | nack | mix | none
        --nack-action <a>    park | retry | skip | stop (default park)
    -g, --group <s>          Group name (default yapper)
    -p, --stream-prefix <s>  Stream prefix (default yapper-ps-)
        --create-streams     Populate streams first if missing/empty
        --stream-length <n>  Events per stream when creating (default 10000)
    -e, --event-size <n>     Payload size when creating (default 64)
    -d, --duration <secs>    Timeout: stop if not drained first (0 = no timeout)
        --keep               Keep groups (and created streams) on exit
        --delete-streams     Also delete created streams on a clean exit (kept by default)
  Exits when the streams are drained, or when the timeout fires — whichever is first.
  Persistent-sub groups are unsubscribed and deleted on exit unless --keep.

  Examples:
    write -s orders -e '{\"id\":1}' -t OrderPlaced
    write flood -c 8 -r 1000 -s 10 -e 50 -b 5 -p yap
    write flood -c 8 -d 30          (sustained writes for 30s)
    read flood -c 4 -r 200 -b 100
    csub flood -n 4 -c 3 --create-streams --stream-length 50000 -d 120
    psub flood -n 4 -c 3 --create-streams --stream-length 50000 --ack-mode mix -d 120

Server dashboard tabs: System · Disk · Queues · Subs · Conns · Cache
  Subs  = persistent subscriptions (per-group peak backlog)
  Conns = TCP connection count + send/receive throughput
  Tab ↹ next tab · Shift+Tab previous tab

View:     Ctrl+B toggles line charts vs sparkline bars
Help:     Ctrl+H toggles this overlay
Cancel:   Esc (or `stop`) gracefully stops the running command — any groups or
          streams it created are deleted first. Quitting cancels the same way.

Editing:  ←/→ move · Home/End · ↑/↓ command history · Backspace/Del
Keys:     Enter run · Ctrl+H close help · Esc cancel/close help · Ctrl+C / Ctrl+D quit";
