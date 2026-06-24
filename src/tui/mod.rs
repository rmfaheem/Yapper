mod app;
mod dashboard;
mod theme;
mod ui;

use std::io;
use std::time::{Duration, Instant};

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

use std::sync::Arc;

use crate::cli::Job;
use crate::config::Config;
use crate::db::{Db, Reporter};
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
    // Cadence + running totals for the periodic progress line logged while a job runs.
    let mut progress = ProgressTrail::default();

    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        let Some(event) = rx.recv().await else {
            break;
        };
        match event {
            AppEvent::Key(key) => match app.handle_key(key) {
                Outcome::Quit => break,
                Outcome::None => {}
                Outcome::Run { job, label } => {
                    app.metrics.reset();
                    app.throughput.clear();
                    app.flood_running = true;
                    app.current_flood = label.clone();
                    app.current_stage.clear();
                    progress.reset();
                    spawn_job(db.clone(), job, app.metrics.clone(), tx.clone(), label);
                }
            },
            AppEvent::Tick => {
                app.on_tick();
                // Mirror the CLI's periodic progress in the console while a job runs.
                if app.flood_running {
                    if let Some(line) = progress.tick(&app.metrics) {
                        app.push_log(line);
                    }
                }
            }
            AppEvent::Stats(stats) => app.on_stats(stats),
            AppEvent::StatsError(e) => {
                if !stats_error_logged {
                    app.push_log(format!("stats poller: {e}"));
                    stats_error_logged = true;
                }
            }
            AppEvent::Log(line) => app.push_log(line),
            AppEvent::Stage(msg) => {
                // Show the stage live in the Client panel and keep a trail in the log.
                app.current_stage = msg.clone();
                app.push_log(msg);
            }
            AppEvent::FloodFinished(msg) => {
                app.flood_running = false;
                app.current_stage.clear();
                app.push_log(msg);
            }
        }
    }

    Ok(())
}

/// Tracks the cadence and last-seen counters for the periodic progress line the
/// TUI logs while a job runs. Mirrors the CLI's 2-second progress print, but only
/// emits when the counters actually advanced, so a quiet catch-up tail or an idle
/// single subscription doesn't spam the console with zeros.
struct ProgressTrail {
    since: Instant,
    last_ops: u64,
    last_errors: u64,
}

impl Default for ProgressTrail {
    fn default() -> Self {
        ProgressTrail {
            since: Instant::now(),
            last_ops: 0,
            last_errors: 0,
        }
    }
}

impl ProgressTrail {
    const INTERVAL: Duration = Duration::from_secs(2);

    /// Reset at the start of a run.
    fn reset(&mut self) {
        *self = ProgressTrail::default();
    }

    /// Returns a progress line once per [`INTERVAL`], if the counters advanced.
    fn tick(&mut self, metrics: &Metrics) -> Option<String> {
        if self.since.elapsed() < Self::INTERVAL {
            return None;
        }
        let dt = self.since.elapsed().as_secs_f64();
        let ops = metrics.total_ops();
        let errors = metrics.total_errors();
        self.since = Instant::now();
        if ops == self.last_ops && errors == self.last_errors {
            return None;
        }
        let rate = (ops.saturating_sub(self.last_ops) as f64 / dt).round() as u64;
        self.last_ops = ops;
        self.last_errors = errors;
        Some(format!("  {ops} ops · {rate}/s · {errors} errors"))
    }
}

/// Spawn a parsed [`Job`] on a background task, feeding the live dashboard via
/// `metrics` and streaming any textual output to the console over `tx`. A final
/// `FloodFinished` event clears the running flag and logs the outcome.
fn spawn_job(
    db: Db,
    job: Job,
    metrics: Arc<Metrics>,
    tx: mpsc::UnboundedSender<AppEvent>,
    label: String,
) {
    // Stage messages from long-running jobs flow back as their own event so the
    // dashboard can show the current stage and keep a trail in the console.
    let reporter = {
        let tx = tx.clone();
        Reporter::new(move |msg| {
            let _ = tx.send(AppEvent::Stage(msg));
        })
    };
    tokio::spawn(async move {
        let result: anyhow::Result<()> = match job {
            Job::WriteSingle {
                stream,
                event_data,
                event_type,
                file,
            } => {
                let data = match file {
                    Some(path) => match std::fs::read_to_string(&path) {
                        Ok(d) => d,
                        Err(e) => {
                            let _ = tx.send(AppEvent::FloodFinished(format!(
                                "Error ({label}): reading {}: {e}",
                                path.display()
                            )));
                            return;
                        }
                    },
                    None => event_data,
                };
                db.write_single(&stream, &event_type, &data).await.map(|v| {
                    let _ = tx.send(AppEvent::Log(format!(
                        "Wrote to '{stream}' (next expected version: {v})"
                    )));
                })
            }
            Job::WriteFlood(params) => db.write_flood(params, metrics, &reporter).await,
            Job::ReadSingle {
                stream,
                count,
                backwards,
            } => db.read_single(&stream, count, backwards).await.map(|lines| {
                if lines.is_empty() {
                    let _ = tx.send(AppEvent::Log(format!("No events in '{stream}'.")));
                } else {
                    for line in lines {
                        let _ = tx.send(AppEvent::Log(line));
                    }
                }
            }),
            Job::ReadFlood(params) => db.read_flood(params, metrics, &reporter).await,
            Job::Catchup { stream } => {
                let tx = tx.clone();
                db.subscribe_catchup(&stream, move |line| {
                    let _ = tx.send(AppEvent::Log(line));
                })
                .await
            }
            Job::CatchupFlood {
                stream,
                clients,
                duration,
            } => {
                db.catchup_flood(&stream, clients, metrics, duration, &reporter)
                    .await
            }
            Job::PsubSingle {
                stream,
                group,
                create,
                keep,
            } => {
                let tx = tx.clone();
                db.subscribe_persistent(&stream, &group, create, keep, move |line| {
                    let _ = tx.send(AppEvent::Log(line));
                })
                .await
            }
            Job::PsubFlood { params, duration } => {
                db.subscribe_flood(params, metrics, duration, &reporter).await
            }
        };
        let msg = match result {
            Ok(()) => format!("Finished: {label}"),
            Err(e) => format!("Error ({label}): {e:#}"),
        };
        let _ = tx.send(AppEvent::FloodFinished(msg));
    });
}
