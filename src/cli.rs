use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use tokio_util::sync::CancellationToken;

use crate::config;
use crate::db::{
    AckMode, CatchupFloodParams, Db, FloodParams, NackAction, Reporter, SubFloodParams,
};
use crate::metrics::Metrics;
use crate::tui;

/// Run `fut` to completion, but cancel `cancel` on Ctrl-C so the operation can
/// tear down anything it created and return gracefully — rather than being
/// dropped mid-flight (which would skip its cleanup).
async fn with_ctrl_c_cancel<F>(cancel: CancellationToken, fut: F) -> Result<()>
where
    F: std::future::Future<Output = Result<()>>,
{
    let watcher = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                println!("\nCancelling… cleaning up…");
                cancel.cancel();
            }
        })
    };
    let res = fut.await;
    watcher.abort();
    res
}

/// A reporter that prints each stage to stdout, for the CLI front-end.
fn stdout_reporter() -> Reporter {
    Reporter::new(|msg| println!("{msg}"))
}

/// Spawn a task that prints a periodic progress line to stdout every ~2s while a
/// job runs (matching the TUI console). Abort the handle once the job finishes.
fn spawn_progress(metrics: std::sync::Arc<Metrics>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last = 0u64;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let ops = metrics.total_ops();
            let errors = metrics.total_errors();
            // Stay quiet when nothing moved (e.g. a quiet catch-up tail).
            if ops != last {
                let rate = (ops - last) / 2;
                println!("  {ops} ops · {rate}/s · {errors} errors");
                last = ops;
            }
        }
    })
}

/// Default concurrent-client count for `flood` modes when `-c` is omitted.
/// A flood is "many clients" by definition, so this is > 1.
pub const DEFAULT_FLOOD_CLIENTS: usize = 4;

#[derive(Parser)]
#[command(
    name = "yapper",
    about = "Yapper is a test client for KurrentDB",
    long_about = "Yapper is a test/load client for KurrentDB.\nIt can read, write and subscribe to the database, and run load floods.",
    version
)]
pub struct Cli {
    /// Config file (default is $HOME/.yapper.json)
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// The command grammar, shared verbatim by the CLI and the TUI command line.
///
/// Each data command runs in single mode by default (one client / subscriber);
/// appending the `flood` subcommand switches it to multiple concurrent clients.
/// `-c/--clients` is a flood-only flag, so passing it in single mode is rejected
/// by the parser ("unexpected argument").
#[derive(Subcommand)]
pub enum Commands {
    /// Show the current configuration and its path
    Config,
    /// Write an event (or `write flood` for a multi-client write load test)
    Write(WriteArgs),
    /// Read a stream (or `read flood` to page through $all under load)
    Read(ReadArgs),
    /// Catch-up subscribe / live tail (or `csub flood` for many concurrent readers)
    Csub(CsubArgs),
    /// Persistent subscribe (or `psub flood` for many groups × competing clients)
    Psub(PsubArgs),
    /// Launch Yapper with the TUI (interactive dashboard)
    Tui,
}

// --- write ---------------------------------------------------------------

#[derive(Args)]
pub struct WriteArgs {
    /// Stream to append the event to (required in single mode)
    #[arg(short = 's', long = "stream")]
    pub stream: Option<String>,
    /// Inline event data (JSON; wrapped as {"raw": ...} if not valid JSON)
    #[arg(short = 'e', long = "event-data", default_value = "")]
    pub event_data: String,
    /// Event type
    #[arg(short = 't', long = "type", default_value = "yapper")]
    pub event_type: String,
    /// Read event data from a file instead of --event-data
    #[arg(short = 'f', long = "file")]
    pub file: Option<PathBuf>,
    #[command(subcommand)]
    pub flood: Option<WriteFlood>,
}

#[derive(Subcommand)]
pub enum WriteFlood {
    /// Flood the database with writes from multiple concurrent clients
    Flood(WriteFloodArgs),
}

#[derive(Args)]
pub struct WriteFloodArgs {
    /// Number of concurrent clients
    #[arg(short = 'c', long = "clients", default_value_t = DEFAULT_FLOOD_CLIENTS)]
    pub clients: usize,
    /// Events to append per stream
    #[arg(short = 'r', long = "requests", default_value_t = 1)]
    pub requests: usize,
    /// Streams per client
    #[arg(short = 's', long = "streams", default_value_t = 1)]
    pub streams: usize,
    /// Event payload size in bytes
    #[arg(short = 'e', long = "event-size", default_value_t = 10)]
    pub event_size: usize,
    /// Events appended per request
    #[arg(short = 'b', long = "batch-size", default_value_t = 1)]
    pub batch_size: usize,
    /// Prefix for generated stream names
    #[arg(short = 'p', long = "stream-prefix", default_value = "")]
    pub stream_prefix: String,
    /// Run for this many seconds instead of stopping after --requests (0 = use --requests)
    #[arg(short = 'd', long = "duration", default_value_t = 0)]
    pub duration: u64,
}

// --- read ----------------------------------------------------------------

#[derive(Args)]
pub struct ReadArgs {
    /// Stream to read (required in single mode)
    #[arg(short = 's', long = "stream")]
    pub stream: Option<String>,
    /// Maximum events to read
    #[arg(short = 'n', long = "count", default_value_t = 50)]
    pub count: usize,
    /// Read backwards from the end of the stream
    #[arg(short = 'b', long = "backwards", default_value_t = false)]
    pub backwards: bool,
    #[command(subcommand)]
    pub flood: Option<ReadFlood>,
}

#[derive(Subcommand)]
pub enum ReadFlood {
    /// Flood the database with reads (pages through $all) from multiple clients
    Flood(ReadFloodArgs),
}

#[derive(Args)]
pub struct ReadFloodArgs {
    /// Number of concurrent clients
    #[arg(short = 'c', long = "clients", default_value_t = DEFAULT_FLOOD_CLIENTS)]
    pub clients: usize,
    /// Number of full $all passes per client
    #[arg(short = 'r', long = "requests", default_value_t = 1)]
    pub requests: usize,
    /// Page size when reading $all
    #[arg(short = 'b', long = "batch-size", default_value_t = 100)]
    pub batch_size: usize,
    /// Stream prefix (reserved; reads page through $all)
    #[arg(short = 'p', long = "stream-prefix", default_value = "")]
    pub stream_prefix: String,
    /// Run for this many seconds instead of stopping after --requests (0 = use --requests)
    #[arg(short = 'd', long = "duration", default_value_t = 0)]
    pub duration: u64,
}

// --- csub (catch-up subscription) ----------------------------------------

#[derive(Args)]
pub struct CsubArgs {
    /// Stream to subscribe to ("$all" tails everything)
    #[arg(short = 's', long = "stream", default_value = "$all")]
    pub stream: String,
    #[command(subcommand)]
    pub flood: Option<CsubFlood>,
}

#[derive(Subcommand)]
pub enum CsubFlood {
    /// Catch-up read-load test: many concurrent readers across prefixed streams
    Flood(CsubFloodArgs),
}

#[derive(Args)]
pub struct CsubFloodArgs {
    /// Number of streams to read (one set of readers per stream {prefix}{i})
    #[arg(short = 'n', long = "subscriptions", default_value_t = 1)]
    pub subscriptions: usize,
    /// Concurrent catch-up readers per stream
    #[arg(short = 'c', long = "clients", default_value_t = DEFAULT_FLOOD_CLIENTS)]
    pub clients: usize,
    /// Stream name prefix; streams are {prefix}{i}
    #[arg(short = 'p', long = "stream-prefix", default_value = "yapper-cs-")]
    pub stream_prefix: String,
    /// Populate streams first if missing/empty (otherwise the run aborts)
    #[arg(long = "create-streams", default_value_t = false)]
    pub create_streams: bool,
    /// Events to write per stream when creating
    #[arg(long = "stream-length", default_value_t = 10_000)]
    pub stream_length: usize,
    /// Event payload size in bytes when creating streams
    #[arg(short = 'e', long = "event-size", default_value_t = 64)]
    pub event_size: usize,
    /// Timeout in seconds: stop if the readers haven't caught up first (0 = no timeout)
    #[arg(short = 'd', long = "duration", default_value_t = 0)]
    pub duration: u64,
    /// Also delete the streams created by --create-streams on a clean exit (kept by default)
    #[arg(long = "delete-streams", default_value_t = false)]
    pub delete_streams: bool,
}

// --- psub (persistent subscription) --------------------------------------

#[derive(Args)]
pub struct PsubArgs {
    /// Stream to subscribe to (required in single mode)
    #[arg(short = 's', long = "stream")]
    pub stream: Option<String>,
    /// Persistent subscription group name (required in single mode)
    #[arg(short = 'g', long = "group")]
    pub group: Option<String>,
    /// Create the subscription group first
    #[arg(long = "create", default_value_t = false)]
    pub create: bool,
    /// Keep the subscription group on exit (don't delete it)
    #[arg(long = "keep", default_value_t = false)]
    pub keep: bool,
    #[command(subcommand)]
    pub flood: Option<PsubFlood>,
}

#[derive(Subcommand)]
pub enum PsubFlood {
    /// Load-test persistent subscriptions: many groups × competing clients
    Flood(PsubFloodArgs),
}

#[derive(Args)]
pub struct PsubFloodArgs {
    /// Number of subscription groups (one per stream {prefix}{i})
    #[arg(short = 'n', long = "subscriptions", default_value_t = 1)]
    pub subscriptions: usize,
    /// Competing consumer clients per group
    #[arg(short = 'c', long = "clients", default_value_t = DEFAULT_FLOOD_CLIENTS)]
    pub clients: usize,
    /// What each client does with a message
    #[arg(long = "ack-mode", value_enum, default_value_t = AckMode::Ack)]
    pub ack_mode: AckMode,
    /// Action used when nacking (ack-mode nack / mix)
    #[arg(long = "nack-action", value_enum, default_value_t = NackAction::Park)]
    pub nack_action: NackAction,
    /// Persistent subscription group name (shared across streams)
    #[arg(short = 'g', long = "group", default_value = "yapper")]
    pub group: String,
    /// Stream name prefix; streams are {prefix}{i}
    #[arg(short = 'p', long = "stream-prefix", default_value = "yapper-ps-")]
    pub stream_prefix: String,
    /// Populate streams first if missing/empty (otherwise the run aborts)
    #[arg(long = "create-streams", default_value_t = false)]
    pub create_streams: bool,
    /// Events to write per stream when creating
    #[arg(long = "stream-length", default_value_t = 10_000)]
    pub stream_length: usize,
    /// Event payload size in bytes when creating streams
    #[arg(short = 'e', long = "event-size", default_value_t = 64)]
    pub event_size: usize,
    /// Timeout in seconds: stop if the streams haven't drained first (0 = no timeout)
    #[arg(short = 'd', long = "duration", default_value_t = 0)]
    pub duration: u64,
    /// Keep subscription groups (and created streams) on exit (don't delete them)
    #[arg(long = "keep", default_value_t = false)]
    pub keep: bool,
    /// Also delete the streams created by --create-streams on a clean exit (kept by default)
    #[arg(long = "delete-streams", default_value_t = false)]
    pub delete_streams: bool,
}

/// A runnable unit of work, produced by parsing a command and executed by
/// either front-end: the CLI runs it in the foreground and prints a summary;
/// the TUI runs it in the background and feeds the live dashboard.
#[derive(Debug, Clone)]
pub enum Job {
    WriteSingle {
        stream: String,
        event_data: String,
        event_type: String,
        file: Option<PathBuf>,
    },
    WriteFlood(FloodParams),
    ReadSingle {
        stream: String,
        count: usize,
        backwards: bool,
    },
    ReadFlood(FloodParams),
    Catchup {
        stream: String,
    },
    CatchupFlood {
        params: CatchupFloodParams,
        duration: u64,
    },
    PsubSingle {
        stream: String,
        group: String,
        create: bool,
        keep: bool,
    },
    PsubFlood {
        params: SubFloodParams,
        duration: u64,
    },
}

/// Turn a parsed data command into an executable [`Job`], validating the
/// single-mode requirements (stream/group names). `Config`/`Tui` are not jobs.
pub fn build_job(command: Commands) -> Result<Job> {
    match command {
        Commands::Write(a) => match a.flood {
            Some(WriteFlood::Flood(f)) => Ok(Job::WriteFlood(FloodParams {
                clients: f.clients,
                requests: f.requests,
                streams: f.streams,
                event_size: f.event_size,
                batch_size: f.batch_size,
                stream_prefix: f.stream_prefix,
                duration: f.duration,
            })),
            None => Ok(Job::WriteSingle {
                stream: a
                    .stream
                    .context("write requires --stream (or use `write flood`)")?,
                event_data: a.event_data,
                event_type: a.event_type,
                file: a.file,
            }),
        },
        Commands::Read(a) => match a.flood {
            Some(ReadFlood::Flood(f)) => Ok(Job::ReadFlood(FloodParams {
                clients: f.clients,
                requests: f.requests,
                streams: 1,
                event_size: 0,
                batch_size: f.batch_size,
                stream_prefix: f.stream_prefix,
                duration: f.duration,
            })),
            None => Ok(Job::ReadSingle {
                stream: a
                    .stream
                    .context("read requires --stream (or use `read flood`)")?,
                count: a.count,
                backwards: a.backwards,
            }),
        },
        Commands::Csub(a) => match a.flood {
            Some(CsubFlood::Flood(f)) => Ok(Job::CatchupFlood {
                params: CatchupFloodParams {
                    subscriptions: f.subscriptions,
                    clients: f.clients,
                    stream_prefix: f.stream_prefix,
                    create_streams: f.create_streams,
                    stream_length: f.stream_length,
                    event_size: f.event_size,
                    delete_streams: f.delete_streams,
                },
                duration: f.duration,
            }),
            None => Ok(Job::Catchup { stream: a.stream }),
        },
        Commands::Psub(a) => match a.flood {
            Some(PsubFlood::Flood(f)) => Ok(Job::PsubFlood {
                params: SubFloodParams {
                    subscriptions: f.subscriptions,
                    clients: f.clients,
                    group: f.group,
                    stream_prefix: f.stream_prefix,
                    ack_mode: f.ack_mode,
                    nack_action: f.nack_action,
                    create_streams: f.create_streams,
                    stream_length: f.stream_length,
                    event_size: f.event_size,
                    keep: f.keep,
                    delete_streams: f.delete_streams,
                },
                duration: f.duration,
            }),
            None => Ok(Job::PsubSingle {
                stream: a
                    .stream
                    .context("psub requires --stream (or use `psub flood`)")?,
                group: a
                    .group
                    .context("psub requires --group (or use `psub flood`)")?,
                create: a.create,
                keep: a.keep,
            }),
        },
        Commands::Config | Commands::Tui => {
            anyhow::bail!("not available here")
        }
    }
}

/// Split a command line into argv-style tokens, honouring single and double
/// quotes so values such as `-e '{"a": 1}'` survive intact. Used to feed the
/// TUI's input line through the same clap parser the CLI uses.
pub fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let (mut in_single, mut in_double) = (false, false);
    for c in line.chars() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                started = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                started = true;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if started {
                    tokens.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        tokens.push(cur);
    }
    tokens
}

/// Parse a TUI command line into a [`Commands`] using the shared clap grammar.
/// On a parse error (or `--help`/`--version`), returns the rendered message so
/// the caller can show it in the console log.
pub fn parse_command_line(line: &str) -> std::result::Result<Commands, String> {
    let tokens = tokenize(line);
    let argv = std::iter::once("yapper".to_string()).chain(tokens);
    match Cli::try_parse_from(argv) {
        Ok(cli) => cli.command.ok_or_else(|| "no command given".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

pub async fn run(cli: Cli) -> Result<()> {
    let config = config::load(cli.config.as_deref())?;

    match cli.command {
        None => {
            // Mirror the Go root command: print help when no subcommand is given.
            use clap::CommandFactory;
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
        Some(Commands::Config) => {
            let path = match cli.config {
                Some(p) => p,
                None => config::default_config_path()?,
            };
            println!("Config file: {}", path.display());
            println!("{}", serde_json::to_string_pretty(&config)?);
            println!("Connection string: {}", config.build_connection_string());
            Ok(())
        }
        Some(Commands::Tui) => tui::run(config).await,
        Some(command) => {
            let db = Db::new(&config)?;
            let job = build_job(command)?;
            run_job(&db, job).await
        }
    }
}

/// Execute a [`Job`] in the foreground, printing results / a summary.
async fn run_job(db: &Db, job: Job) -> Result<()> {
    match job {
        Job::WriteSingle {
            stream,
            event_data,
            event_type,
            file,
        } => {
            let data = match file {
                Some(path) => std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?,
                None => event_data,
            };
            let version = db.write_single(&stream, &event_type, &data).await?;
            println!("Wrote event to '{stream}' (next expected version: {version})");
            Ok(())
        }
        Job::WriteFlood(params) => run_flood(db, params, true).await,
        Job::ReadSingle {
            stream,
            count,
            backwards,
        } => {
            let events = db.read_single(&stream, count, backwards).await?;
            if events.is_empty() {
                println!("No events in '{stream}'.");
            } else {
                for line in events {
                    println!("{line}");
                }
            }
            Ok(())
        }
        Job::ReadFlood(params) => run_flood(db, params, false).await,
        Job::Catchup { stream } => {
            println!("Subscribing to '{stream}' (Ctrl-C to stop)...");
            let cancel = CancellationToken::new();
            with_ctrl_c_cancel(
                cancel.clone(),
                db.subscribe_catchup(&stream, cancel.clone(), |line| println!("{line}")),
            )
            .await
        }
        Job::CatchupFlood { params, duration } => run_catchup_flood(db, params, duration).await,
        Job::PsubSingle {
            stream,
            group,
            create,
            keep,
        } => {
            println!("Subscribing to '{stream}' / group '{group}' (Ctrl-C to stop)...");
            let cancel = CancellationToken::new();
            with_ctrl_c_cancel(
                cancel.clone(),
                db.subscribe_persistent(&stream, &group, create, keep, cancel.clone(), |line| {
                    println!("{line}")
                }),
            )
            .await
        }
        Job::PsubFlood { params, duration } => run_sub_flood(db, params, duration).await,
    }
}

/// Run a write/read flood from the CLI and print a summary.
async fn run_flood(db: &Db, params: FloodParams, write: bool) -> Result<()> {
    let metrics = Metrics::new();
    let reporter = stdout_reporter();
    let progress = spawn_progress(metrics.clone());
    let cancel = CancellationToken::new();

    let start = Instant::now();
    let res = with_ctrl_c_cancel(cancel.clone(), async {
        if write {
            db.write_flood(params, metrics.clone(), &reporter, cancel.clone()).await
        } else {
            db.read_flood(params, metrics.clone(), &reporter, cancel.clone()).await
        }
    })
    .await;
    progress.abort();
    res?;
    let elapsed = start.elapsed().as_secs_f64().max(0.001);

    let (p50, p99) = metrics.drain_latency_percentiles();
    let ops = metrics.total_ops();
    println!(
        "Done in {elapsed:.2}s — {ops} ops, {} errors, {:.0} ops/s, {:.2} MB, p50 {p50:.1}ms p99 {p99:.1}ms",
        metrics.total_errors(),
        ops as f64 / elapsed,
        metrics.total_bytes() as f64 / 1_048_576.0,
    );
    Ok(())
}

/// Run a catch-up subscription read-load test from the CLI and print a summary.
async fn run_catchup_flood(db: &Db, params: CatchupFloodParams, duration: u64) -> Result<()> {
    let metrics = Metrics::new();
    let reporter = stdout_reporter();
    let progress = spawn_progress(metrics.clone());
    let cancel = CancellationToken::new();

    let start = Instant::now();
    let res = with_ctrl_c_cancel(
        cancel.clone(),
        db.catchup_flood(params, metrics.clone(), duration, &reporter, cancel.clone()),
    )
    .await;
    progress.abort();
    let elapsed = start.elapsed().as_secs_f64().max(0.001);

    let ops = metrics.total_ops();
    println!(
        "Done in {elapsed:.2}s — {ops} events, {} errors, {:.0} events/s, {:.2} MB",
        metrics.total_errors(),
        ops as f64 / elapsed,
        metrics.total_bytes() as f64 / 1_048_576.0,
    );
    res
}

/// Run a persistent-subscription load test from the CLI and print a summary.
async fn run_sub_flood(db: &Db, params: SubFloodParams, duration: u64) -> Result<()> {
    let metrics = Metrics::new();
    let reporter = stdout_reporter();
    let progress = spawn_progress(metrics.clone());

    let start = Instant::now();
    let cancel = CancellationToken::new();
    let res = with_ctrl_c_cancel(
        cancel.clone(),
        db.subscribe_flood(params, metrics.clone(), duration, &reporter, cancel.clone()),
    )
    .await;
    progress.abort();
    let elapsed = start.elapsed().as_secs_f64().max(0.001);

    let (p50, p99) = metrics.drain_latency_percentiles();
    let ops = metrics.total_ops();
    println!(
        "Done in {elapsed:.2}s — {ops} messages, {} errors, {:.0} msg/s, p50 {p50:.1}ms p99 {p99:.1}ms",
        metrics.total_errors(),
        ops as f64 / elapsed,
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a TUI-style line all the way to a Job (the path the TUI uses).
    fn job(line: &str) -> Result<Job, String> {
        let command = parse_command_line(line)?;
        build_job(command).map_err(|e| e.to_string())
    }

    #[test]
    fn tokenize_honours_quotes() {
        assert_eq!(tokenize("write -s s"), ["write", "-s", "s"]);
        assert_eq!(
            tokenize(r#"write -e '{"a": 1}'"#),
            ["write", "-e", r#"{"a": 1}"#]
        );
        assert_eq!(tokenize("   spaced   out  "), ["spaced", "out"]);
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn write_single_requires_stream() {
        match job("write -s orders -e hi -t Order") {
            Ok(Job::WriteSingle { stream, event_data, event_type, .. }) => {
                assert_eq!(stream, "orders");
                assert_eq!(event_data, "hi");
                assert_eq!(event_type, "Order");
            }
            other => panic!("expected WriteSingle, got {other:?}"),
        }
        // Bare `write` has no stream → error, not a panic.
        assert!(job("write").is_err());
    }

    #[test]
    fn write_flood_parses_clients() {
        match job("write flood -c 8 -r 1000 -s 10 -e 50 -b 5 -p yap") {
            Ok(Job::WriteFlood(p)) => {
                assert_eq!(p.clients, 8);
                assert_eq!(p.requests, 1000);
                assert_eq!(p.streams, 10);
                assert_eq!(p.event_size, 50);
                assert_eq!(p.batch_size, 5);
                assert_eq!(p.stream_prefix, "yap");
            }
            other => panic!("expected WriteFlood, got {other:?}"),
        }
    }

    #[test]
    fn write_flood_defaults_to_many_clients() {
        match job("write flood") {
            Ok(Job::WriteFlood(p)) => assert_eq!(p.clients, DEFAULT_FLOOD_CLIENTS),
            other => panic!("expected WriteFlood, got {other:?}"),
        }
    }

    #[test]
    fn write_read_flood_duration_is_optional() {
        // Absent -> 0 (run until --requests); present -> the given seconds.
        match job("write flood -c 2") {
            Ok(Job::WriteFlood(p)) => assert_eq!(p.duration, 0),
            other => panic!("expected WriteFlood, got {other:?}"),
        }
        match job("write flood -c 2 -d 45") {
            Ok(Job::WriteFlood(p)) => assert_eq!(p.duration, 45),
            other => panic!("expected WriteFlood, got {other:?}"),
        }
        match job("read flood -d 10") {
            Ok(Job::ReadFlood(p)) => assert_eq!(p.duration, 10),
            other => panic!("expected ReadFlood, got {other:?}"),
        }
    }

    #[test]
    fn clients_flag_is_rejected_in_single_mode() {
        // -c only exists under `flood`; in single mode it's an unexpected arg.
        assert!(job("write -c 4").is_err());
        assert!(job("read -c 4").is_err());
        assert!(job("psub -c 4").is_err());
    }

    #[test]
    fn read_single_and_flood() {
        match job("read -s orders -n 20 -b") {
            Ok(Job::ReadSingle { stream, count, backwards }) => {
                assert_eq!(stream, "orders");
                assert_eq!(count, 20);
                assert!(backwards);
            }
            other => panic!("expected ReadSingle, got {other:?}"),
        }
        match job("read flood -c 4 -r 200 -b 100") {
            Ok(Job::ReadFlood(p)) => {
                assert_eq!(p.clients, 4);
                assert_eq!(p.requests, 200);
                assert_eq!(p.batch_size, 100);
            }
            other => panic!("expected ReadFlood, got {other:?}"),
        }
    }

    #[test]
    fn csub_single_defaults_all_and_flood_mirrors_psub() {
        match job("csub") {
            Ok(Job::Catchup { stream }) => assert_eq!(stream, "$all"),
            other => panic!("expected Catchup, got {other:?}"),
        }
        match job("csub flood -n 4 -c 3 -p cs- --create-streams --stream-length 500 -e 32 -d 30") {
            Ok(Job::CatchupFlood { params, duration }) => {
                assert_eq!(params.subscriptions, 4);
                assert_eq!(params.clients, 3);
                assert_eq!(params.stream_prefix, "cs-");
                assert!(params.create_streams);
                assert_eq!(params.stream_length, 500);
                assert_eq!(params.event_size, 32);
                assert_eq!(duration, 30);
            }
            other => panic!("expected CatchupFlood, got {other:?}"),
        }
    }

    #[test]
    fn psub_single_requires_stream_and_group() {
        match job("psub -s orders -g g1 --create --keep") {
            Ok(Job::PsubSingle { stream, group, create, keep }) => {
                assert_eq!(stream, "orders");
                assert_eq!(group, "g1");
                assert!(create);
                assert!(keep);
            }
            other => panic!("expected PsubSingle, got {other:?}"),
        }
        assert!(job("psub -s orders").is_err()); // missing group
        assert!(job("psub").is_err()); // missing both
    }

    #[test]
    fn psub_flood_parses_all_flags() {
        let line = "psub flood -n 4 -c 3 --ack-mode mix --nack-action retry -g grp \
                    -p pre- --create-streams --stream-length 5000 -e 32 -d 120 --keep";
        match job(line) {
            Ok(Job::PsubFlood { params, duration }) => {
                assert_eq!(params.subscriptions, 4);
                assert_eq!(params.clients, 3);
                assert_eq!(params.ack_mode, AckMode::Mix);
                assert_eq!(params.nack_action, NackAction::Retry);
                assert_eq!(params.group, "grp");
                assert_eq!(params.stream_prefix, "pre-");
                assert!(params.create_streams);
                assert_eq!(params.stream_length, 5000);
                assert_eq!(params.event_size, 32);
                assert!(params.keep);
                assert_eq!(duration, 120);
            }
            other => panic!("expected PsubFlood, got {other:?}"),
        }
    }

    #[test]
    fn unknown_command_errors() {
        assert!(job("frobnicate").is_err());
        assert!(job("write flood --bogus zzz").is_err());
    }
}
