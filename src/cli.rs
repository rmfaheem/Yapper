use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::config;
use crate::db::{Db, FloodParams};
use crate::metrics::Metrics;
use crate::tui;

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

#[derive(Subcommand)]
pub enum Commands {
    /// Show the current configuration and its path
    Config,
    /// Write to the database
    Write {
        #[command(subcommand)]
        mode: WriteMode,
    },
    /// Read from the database
    Read {
        #[command(subcommand)]
        mode: ReadMode,
    },
    /// Subscribe to the streams in the database
    Subscribe {
        #[command(subcommand)]
        mode: SubscribeMode,
    },
    /// Launch Yapper with the TUI (interactive dashboard)
    Tui,
}

#[derive(Subcommand)]
pub enum WriteMode {
    /// Send a single write request to the database
    Single {
        #[arg(short = 's', long = "stream")]
        stream: String,
        #[arg(short = 'e', long = "event-data", default_value = "")]
        event_data: String,
        #[arg(short = 't', long = "type", default_value = "yapper")]
        event_type: String,
        /// Read event data from a file instead of --event-data
        #[arg(short = 'f', long = "file")]
        file: Option<PathBuf>,
    },
    /// Send a flood of write requests to the database
    Flood {
        #[arg(short = 'c', long = "clients", default_value_t = 1)]
        clients: usize,
        #[arg(short = 'r', long = "requests", default_value_t = 1)]
        requests: usize,
        #[arg(short = 's', long = "streams", default_value_t = 1)]
        streams: usize,
        #[arg(short = 'e', long = "event-size", default_value_t = 10)]
        event_size: usize,
        #[arg(short = 'b', long = "batch-size", default_value_t = 1)]
        batch_size: usize,
        #[arg(short = 'p', long = "stream-prefix", default_value = "")]
        stream_prefix: String,
    },
}

#[derive(Subcommand)]
pub enum ReadMode {
    /// Send a single read request to the database
    Single {
        #[arg(short = 's', long = "stream")]
        stream: String,
        #[arg(short = 'n', long = "count", default_value_t = 50)]
        count: usize,
        #[arg(short = 'b', long = "backwards", default_value_t = false)]
        backwards: bool,
    },
    /// Send a flood of read requests to the database (pages through $all)
    Flood {
        #[arg(short = 'c', long = "clients", default_value_t = 1)]
        clients: usize,
        #[arg(short = 'r', long = "requests", default_value_t = 1)]
        requests: usize,
        #[arg(short = 'b', long = "batch-size", default_value_t = 100)]
        batch_size: usize,
        #[arg(short = 'p', long = "stream-prefix", default_value = "")]
        stream_prefix: String,
    },
}

#[derive(Subcommand)]
pub enum SubscribeMode {
    /// Catch-up subscription (prints events until Ctrl-C)
    Catchup {
        /// Stream to subscribe to. Empty or "$all" subscribes to $all.
        #[arg(short = 's', long = "stream", default_value = "$all")]
        stream: String,
    },
    /// Persistent subscription
    Persistent {
        #[arg(short = 's', long = "stream")]
        stream: String,
        #[arg(short = 'g', long = "group")]
        group: String,
        /// Create the subscription group first
        #[arg(long = "create", default_value_t = false)]
        create: bool,
        /// Keep the subscription group on exit (don't delete it)
        #[arg(long = "keep", default_value_t = false)]
        keep: bool,
    },
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
        Some(Commands::Write { mode }) => {
            let db = Db::new(&config)?;
            match mode {
                WriteMode::Single {
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
                WriteMode::Flood {
                    clients,
                    requests,
                    streams,
                    event_size,
                    batch_size,
                    stream_prefix,
                } => {
                    let params = FloodParams {
                        clients,
                        requests,
                        streams,
                        event_size,
                        batch_size,
                        stream_prefix,
                    };
                    run_flood(&db, params, true).await
                }
            }
        }
        Some(Commands::Read { mode }) => {
            let db = Db::new(&config)?;
            match mode {
                ReadMode::Single {
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
                ReadMode::Flood {
                    clients,
                    requests,
                    batch_size,
                    stream_prefix,
                } => {
                    let params = FloodParams {
                        clients,
                        requests,
                        streams: 1,
                        event_size: 0,
                        batch_size,
                        stream_prefix,
                    };
                    run_flood(&db, params, false).await
                }
            }
        }
        Some(Commands::Subscribe { mode }) => {
            let db = Db::new(&config)?;
            match mode {
                SubscribeMode::Catchup { stream } => {
                    println!("Subscribing to '{stream}' (Ctrl-C to stop)...");
                    tokio::select! {
                        res = db.subscribe_catchup(&stream, |line| println!("{line}")) => res,
                        _ = tokio::signal::ctrl_c() => {
                            println!("\nStopped.");
                            Ok(())
                        }
                    }
                }
                SubscribeMode::Persistent {
                    stream,
                    group,
                    create,
                    keep,
                } => {
                    println!("Subscribing to '{stream}' / group '{group}' (Ctrl-C to stop)...");
                    tokio::select! {
                        res = db.subscribe_persistent(&stream, &group, create, keep, |line| println!("{line}")) => res,
                        _ = tokio::signal::ctrl_c() => {
                            println!("\nStopped.");
                            Ok(())
                        }
                    }
                }
            }
        }
        Some(Commands::Tui) => tui::run(config).await,
    }
}

/// Run a write/read flood from the CLI and print a summary.
async fn run_flood(db: &Db, params: FloodParams, write: bool) -> Result<()> {
    let metrics = Metrics::new();
    let kind = if write { "write" } else { "read" };
    println!(
        "Starting {kind} flood: {} clients × {} requests (batch {})...",
        params.clients, params.requests, params.batch_size
    );

    let start = Instant::now();
    if write {
        db.write_flood(params, metrics.clone()).await?;
    } else {
        db.read_flood(params, metrics.clone()).await?;
    }
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
