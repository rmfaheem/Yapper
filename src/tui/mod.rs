mod app;
mod dashboard;
mod theme;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{Event, EventStream, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::execute;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::db::Db;
use crate::metrics::Metrics;
use crate::stats::StatsClient;

use app::{App, AppEvent, Outcome};

/// Launch the interactive TUI.
pub async fn run(config: Config) -> Result<()> {
    let db = Db::new(&config)?;
    let stats_client = StatsClient::new(
        config.http_stats_url(),
        config.username.clone(),
        config.password.clone(),
    );
    let metrics = Metrics::new();

    // Terminal setup.
    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("entering alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("creating terminal")?;

    let result = event_loop(&mut terminal, db, stats_client, metrics).await;

    // Terminal teardown (always runs).
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    result
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    db: Db,
    stats_client: StatsClient,
    metrics: std::sync::Arc<Metrics>,
) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let mut app = App::new(metrics);

    // Input task: forward key presses.
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut reader = EventStream::new();
            while let Some(Ok(event)) = reader.next().await {
                if let Event::Key(key) = event {
                    if key.kind != KeyEventKind::Release && tx.send(AppEvent::Key(key)).is_err() {
                        break;
                    }
                }
            }
        });
    }

    // Tick task: drives client-side throughput/latency sampling.
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            loop {
                interval.tick().await;
                if tx.send(AppEvent::Tick).is_err() {
                    break;
                }
            }
        });
    }

    // Stats poller task: polls KurrentDB /stats once a second.
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                let ev = match stats_client.poll().await {
                    Ok(stats) => AppEvent::Stats(stats),
                    Err(e) => AppEvent::StatsError(format!("{e:#}")),
                };
                if tx.send(ev).is_err() {
                    break;
                }
            }
        });
    }

    let mut stats_error_logged = false;

    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        let Some(event) = rx.recv().await else {
            break;
        };
        match event {
            AppEvent::Key(key) => match app.handle_key(key) {
                Outcome::Quit => break,
                Outcome::None => {}
                Outcome::StartFlood {
                    write,
                    params,
                    label,
                } => {
                    app.metrics.reset();
                    app.throughput.clear();
                    app.flood_running = true;
                    app.current_flood = label.clone();

                    let db = db.clone();
                    let metrics = app.metrics.clone();
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let res = if write {
                            db.write_flood(params, metrics).await
                        } else {
                            db.read_flood(params, metrics).await
                        };
                        let msg = match res {
                            Ok(()) => format!("Finished: {label}"),
                            Err(e) => format!("Flood error ({label}): {e:#}"),
                        };
                        let _ = tx.send(AppEvent::FloodFinished(msg));
                    });
                }
            },
            AppEvent::Tick => app.on_tick(),
            AppEvent::Stats(stats) => app.on_stats(stats),
            AppEvent::StatsError(e) => {
                if !stats_error_logged {
                    app.push_log(format!("stats poller: {e}"));
                    stats_error_logged = true;
                }
            }
            AppEvent::FloodFinished(msg) => {
                app.flood_running = false;
                app.push_log(msg);
            }
        }
    }

    Ok(())
}
