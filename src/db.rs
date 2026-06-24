use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use kurrentdb::{
    AppendToStreamOptions, Client, ClientSettings, DeletePersistentSubscriptionOptions,
    Error as KurrentError, EventData, NakAction, PersistentSubscriptionOptions, ReadAllOptions,
    ReadStreamOptions, StreamPosition, SubscribeToAllOptions,
    SubscribeToPersistentSubscriptionOptions, SubscribeToStreamOptions,
};
use rand::distributions::{Alphanumeric, DistString};
use uuid::Uuid;

use crate::config::Config;
use crate::metrics::Metrics;

/// Convert a `Result` whose error only implements `Display` (e.g. KurrentDB's
/// `eyre::Report`) into an `anyhow::Result` with added context.
trait EyreCtx<T> {
    fn ctx(self, msg: &str) -> Result<T>;
}

impl<T, E: std::fmt::Display> EyreCtx<T> for std::result::Result<T, E> {
    fn ctx(self, msg: &str) -> Result<T> {
        self.map_err(|e| anyhow::anyhow!("{msg}: {e}"))
    }
}

/// A sink for human-readable stage messages emitted by a long-running job, so
/// both front-ends can surface *what stage the run has reached* (e.g. "Populating
/// streams…", "Subscribing 12 consumers…", "Deleting groups…"). The CLI prints
/// them to stdout; the TUI logs them to the console and shows the latest as the
/// live status.
#[derive(Clone)]
pub struct Reporter {
    sink: Arc<dyn Fn(String) + Send + Sync>,
}

impl Reporter {
    pub fn new(sink: impl Fn(String) + Send + Sync + 'static) -> Self {
        Reporter { sink: Arc::new(sink) }
    }

    /// A reporter that discards every message (tests, or callers that don't care).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn silent() -> Self {
        Reporter::new(|_| {})
    }

    /// Report that the run has reached a new stage.
    pub fn stage(&self, msg: impl Into<String>) {
        (self.sink)(msg.into());
    }
}

impl std::fmt::Debug for Reporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reporter")
    }
}

/// Parameters for a write/read flood, shared by the CLI and TUI front-ends.
#[derive(Debug, Clone)]
pub struct FloodParams {
    pub clients: usize,
    pub requests: usize,
    pub streams: usize,
    pub event_size: usize,
    pub batch_size: usize,
    pub stream_prefix: String,
}

/// What a persistent-subscription consumer does with each message it receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AckMode {
    /// Acknowledge every message.
    Ack,
    /// Negative-acknowledge every message (using the configured nack action).
    Nack,
    /// Alternate ack / nack message by message.
    Mix,
    /// Never settle messages, causing the server to time them out and redeliver.
    None,
}

/// The action sent when nacking a message. Mirrors `kurrentdb::NakAction` but is
/// `Copy` and derives `ValueEnum` so it can be threaded through tasks and parsed
/// directly by clap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum NackAction {
    /// Move the message to the parked/poison queue; do not redeliver.
    Park,
    /// Redeliver the message (retry counter climbs).
    Retry,
    /// Drop the message; do not redeliver or park.
    Skip,
    /// Stop the subscription.
    Stop,
}

impl From<NackAction> for NakAction {
    fn from(a: NackAction) -> Self {
        match a {
            NackAction::Park => NakAction::Park,
            NackAction::Retry => NakAction::Retry,
            NackAction::Skip => NakAction::Skip,
            NackAction::Stop => NakAction::Stop,
        }
    }
}

/// Parameters for a persistent-subscription load test (flood).
#[derive(Debug, Clone)]
pub struct SubFloodParams {
    /// Number of subscription groups; one per stream `{stream_prefix}{i}`.
    pub subscriptions: usize,
    /// Competing consumer clients per group.
    pub clients: usize,
    /// Persistent subscription group name (shared across streams).
    pub group: String,
    /// Stream name prefix; streams are `{stream_prefix}{i}`.
    pub stream_prefix: String,
    /// Per-message action each consumer takes.
    pub ack_mode: AckMode,
    /// Action used when nacking (for `Nack` / `Mix` modes).
    pub nack_action: NackAction,
    /// Populate streams first if missing/empty (otherwise the run aborts).
    pub create_streams: bool,
    /// Events to write per stream when creating.
    pub stream_length: usize,
    /// Event payload size in bytes when creating streams.
    pub event_size: usize,
    /// Keep subscription groups on exit instead of deleting them.
    pub keep: bool,
}

/// Thin wrapper around the KurrentDB client plus the config it was built from.
#[derive(Clone)]
pub struct Db {
    client: Client,
}

impl Db {
    pub fn new(config: &Config) -> Result<Self> {
        let conn = config.build_connection_string();
        let settings: ClientSettings = conn
            .parse()
            .map_err(|e| anyhow::anyhow!("parsing connection string `{conn}`: {e}"))?;
        let client = Client::new(settings).ctx("creating KurrentDB client")?;
        Ok(Db { client })
    }

    /// Append a single event. `data` is written as JSON; if it is not valid JSON
    /// it is wrapped as `{ "raw": <data> }`.
    pub async fn write_single(
        &self,
        stream: &str,
        event_type: &str,
        data: &str,
    ) -> Result<u64> {
        let payload: serde_json::Value = serde_json::from_str(data)
            .unwrap_or_else(|_| serde_json::json!({ "raw": data }));
        let event = EventData::json(event_type, &payload)
            .ctx("serialising event data")?
            .id(Uuid::new_v4());

        let result = self
            .client
            .append_to_stream(stream, &AppendToStreamOptions::default(), event)
            .await
            .ctx("appending to stream")?;
        Ok(result.next_expected_version)
    }

    /// Read up to `count` events from `stream` and return them formatted for display.
    pub async fn read_single(
        &self,
        stream: &str,
        count: usize,
        backwards: bool,
    ) -> Result<Vec<String>> {
        let mut options = ReadStreamOptions::default().max_count(count);
        options = if backwards {
            options.position(StreamPosition::End).backwards()
        } else {
            options.position(StreamPosition::Start).forwards()
        };

        let mut read = self
            .client
            .read_stream(stream, &options)
            .await
            .ctx("reading stream")?;

        let mut out = Vec::new();
        while let Some(event) = read.next().await.ctx("reading next event")? {
            let e = event.get_original_event();
            let body = String::from_utf8_lossy(&e.data);
            out.push(format!(
                "#{:<6} {:<24} {}",
                e.revision,
                e.event_type,
                truncate(&body, 120)
            ));
        }
        Ok(out)
    }

    /// Run a write flood, updating `metrics` as events are appended.
    pub async fn write_flood(
        &self,
        p: FloodParams,
        metrics: Arc<Metrics>,
        reporter: &Reporter,
    ) -> Result<()> {
        metrics.set_active(true);
        let batch_size = p.batch_size.max(1);
        reporter.stage(format!(
            "Write flood: {} client(s) × {} stream(s) × {} request(s), batch {batch_size}…",
            p.clients.max(1),
            p.streams.max(1),
            p.requests.max(1),
        ));

        let mut handles = Vec::new();
        for _client in 0..p.clients.max(1) {
            for _stream in 0..p.streams.max(1) {
                let db = self.clone();
                let metrics = metrics.clone();
                let prefix = p.stream_prefix.clone();
                let requests = p.requests.max(1);
                let event_size = p.event_size;

                handles.push(tokio::spawn(async move {
                    let stream_name = format!("{prefix}{}", Uuid::new_v4());
                    let mut written = 0usize;
                    while written < requests {
                        let this_batch = batch_size.min(requests - written);
                        let mut events = Vec::with_capacity(this_batch);
                        let mut batch_bytes = 0u64;
                        for _ in 0..this_batch {
                            let payload = random_payload(event_size);
                            batch_bytes += payload.len() as u64;
                            match EventData::json(
                                "FloodEvent",
                                &serde_json::json!({ "id": Uuid::new_v4(), "payload": payload }),
                            ) {
                                Ok(ev) => events.push(ev.id(Uuid::new_v4())),
                                Err(_) => metrics.record_error(),
                            }
                        }

                        let start = Instant::now();
                        let result = db
                            .client
                            .append_to_stream(
                                stream_name.as_str(),
                                &AppendToStreamOptions::default(),
                                events,
                            )
                            .await;
                        let latency_us = start.elapsed().as_micros() as u32;
                        match result {
                            Ok(_) => metrics.record_op(latency_us, batch_bytes),
                            Err(_) => metrics.record_error(),
                        }
                        written += this_batch;
                    }
                }));
            }
        }

        reporter.stage("Running…");
        for h in handles {
            let _ = h.await;
        }
        metrics.set_active(false);
        Ok(())
    }

    /// Run a read flood by paging through `$all`, updating `metrics`. This is what
    /// the live dashboard visualizes against the server stats.
    pub async fn read_flood(
        &self,
        p: FloodParams,
        metrics: Arc<Metrics>,
        reporter: &Reporter,
    ) -> Result<()> {
        metrics.set_active(true);
        let page = p.batch_size.max(1);
        reporter.stage(format!(
            "Read flood: {} client(s) paging $all × {} pass(es), page {page}…",
            p.clients.max(1),
            p.requests.max(1),
        ));

        let mut handles = Vec::new();
        for _client in 0..p.clients.max(1) {
            let db = self.clone();
            let metrics = metrics.clone();
            let requests = p.requests.max(1);

            handles.push(tokio::spawn(async move {
                for _ in 0..requests {
                    let options = ReadAllOptions::default()
                        .position(StreamPosition::Start)
                        .forwards()
                        .max_count(page);

                    let start = Instant::now();
                    match db.client.read_all(&options).await {
                        Ok(mut read) => {
                            let mut bytes = 0u64;
                            let mut ok = true;
                            loop {
                                match read.next().await {
                                    Ok(Some(ev)) => {
                                        bytes += ev.get_original_event().data.len() as u64;
                                    }
                                    Ok(None) => break,
                                    Err(_) => {
                                        ok = false;
                                        break;
                                    }
                                }
                            }
                            let latency_us = start.elapsed().as_micros() as u32;
                            if ok {
                                metrics.record_op(latency_us, bytes);
                            } else {
                                metrics.record_error();
                            }
                        }
                        Err(_) => metrics.record_error(),
                    }
                }
            }));
        }

        reporter.stage("Running…");
        for h in handles {
            let _ = h.await;
        }
        metrics.set_active(false);
        Ok(())
    }

    /// Run a catch-up read-load test: spawn `clients` catch-up subscribers, each
    /// reading `stream` (or `$all`) from the start and on through live events,
    /// recording every event as an op in `metrics`. Runs for `duration` seconds
    /// (0 = until cancelled), then aborts the tasks — dropping each subscription
    /// and unsubscribing — before returning.
    pub async fn catchup_flood(
        &self,
        stream: &str,
        clients: usize,
        metrics: Arc<Metrics>,
        duration: u64,
        reporter: &Reporter,
    ) -> Result<()> {
        let all = stream.is_empty() || stream == "$all" || stream == "all";
        metrics.set_active(true);
        let target = if all { "$all" } else { stream };
        reporter.stage(format!(
            "Catch-up flood: subscribing {} reader(s) to '{target}'…",
            clients.max(1),
        ));

        let mut handles = Vec::with_capacity(clients.max(1));
        for _ in 0..clients.max(1) {
            let db = self.clone();
            let metrics = metrics.clone();
            let stream = stream.to_string();
            handles.push(tokio::spawn(async move {
                let mut sub = if all {
                    db.client
                        .subscribe_to_all(
                            &SubscribeToAllOptions::default().position(StreamPosition::Start),
                        )
                        .await
                } else {
                    db.client
                        .subscribe_to_stream(
                            stream.as_str(),
                            &SubscribeToStreamOptions::default().start_from(StreamPosition::Start),
                        )
                        .await
                };
                loop {
                    match sub.next().await {
                        Ok(event) => {
                            let bytes = event.get_original_event().data.len() as u64;
                            // A streaming subscription has no request/response
                            // latency, so only throughput and bytes are recorded.
                            metrics.record_op(0, bytes);
                        }
                        Err(_) => {
                            metrics.record_error();
                            return;
                        }
                    }
                }
            }));
        }

        reporter.stage(running_note(duration));
        if duration > 0 {
            tokio::time::sleep(Duration::from_secs(duration)).await;
        } else {
            futures::future::pending::<()>().await;
        }
        reporter.stage("Stopping subscribers…");
        for h in &handles {
            h.abort();
        }
        metrics.set_active(false);
        Ok(())
    }

    /// Catch-up subscribe to a stream (or `$all`), invoking `on_event` for each
    /// event until the future is dropped / cancelled.
    pub async fn subscribe_catchup<F>(&self, stream: &str, mut on_event: F) -> Result<()>
    where
        F: FnMut(String),
    {
        let all = stream.is_empty() || stream == "$all" || stream == "all";
        // Catch-up from the beginning so existing events are replayed.
        let mut sub = if all {
            self.client
                .subscribe_to_all(
                    &SubscribeToAllOptions::default().position(StreamPosition::Start),
                )
                .await
        } else {
            self.client
                .subscribe_to_stream(
                    stream,
                    &SubscribeToStreamOptions::default().start_from(StreamPosition::Start),
                )
                .await
        };

        loop {
            let event = sub.next().await.ctx("reading subscription event")?;
            let e = event.get_original_event();
            on_event(format!(
                "{}@{} {} {}",
                e.stream_id(),
                e.revision,
                e.event_type,
                truncate(&String::from_utf8_lossy(&e.data), 120)
            ));
        }
    }

    /// Subscribe to a persistent subscription group, acking each event. Creates the
    /// group first when `create` is set, and deletes it on exit unless `keep`.
    pub async fn subscribe_persistent<F>(
        &self,
        stream: &str,
        group: &str,
        create: bool,
        keep: bool,
        mut on_event: F,
    ) -> Result<()>
    where
        F: FnMut(String),
    {
        if create {
            // Ignore "already exists" so repeated runs are idempotent.
            if let Err(e) = self
                .client
                .create_persistent_subscription(
                    stream,
                    group,
                    &PersistentSubscriptionOptions::default(),
                )
                .await
            {
                on_event(format!("(create group: {e})"));
            }
        }

        let mut sub = self
            .client
            .subscribe_to_persistent_subscription(
                stream,
                group,
                &SubscribeToPersistentSubscriptionOptions::default(),
            )
            .await
            .ctx("subscribing to persistent subscription")?;

        let result = async {
            loop {
                let event = sub.next().await.ctx("reading persistent event")?;
                let line = {
                    let e = event.get_original_event();
                    format!(
                        "{}@{} {} {}",
                        e.stream_id(),
                        e.revision,
                        e.event_type,
                        truncate(&String::from_utf8_lossy(&e.data), 120)
                    )
                };
                on_event(line);
                sub.ack(&event).await.ctx("acking event")?;
            }
        }
        .await;

        if !keep {
            let _ = self
                .client
                .delete_persistent_subscription(
                    stream,
                    group,
                    &DeletePersistentSubscriptionOptions::default(),
                )
                .await;
        }
        result
    }

    /// Number of events in `stream`, or `None` if the stream does not exist.
    /// Reads the last event (backwards, count 1) and derives the count from its
    /// revision. A stream that exists but is empty reports `Some(0)`.
    async fn stream_event_count(&self, stream: &str) -> Result<Option<u64>> {
        let opts = ReadStreamOptions::default()
            .position(StreamPosition::End)
            .backwards()
            .max_count(1);
        let mut read = match self.client.read_stream(stream, &opts).await {
            Ok(r) => r,
            Err(KurrentError::ResourceNotFound) => return Ok(None),
            Err(e) => return Err(anyhow::anyhow!("reading {stream}: {e}")),
        };
        match read.next().await {
            Ok(Some(ev)) => Ok(Some(ev.get_original_event().revision + 1)),
            Ok(None) => Ok(Some(0)),
            Err(KurrentError::ResourceNotFound) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("reading {stream}: {e}")),
        }
    }

    /// Append `count` random events to `stream` in batches, used to pre-populate
    /// streams for a persistent-subscription load test.
    async fn populate_stream(&self, stream: &str, count: usize, event_size: usize) -> Result<()> {
        const BATCH: usize = 500;
        let mut written = 0usize;
        while written < count {
            let this_batch = BATCH.min(count - written);
            let mut events = Vec::with_capacity(this_batch);
            for _ in 0..this_batch {
                let payload = random_payload(event_size);
                let ev = EventData::json(
                    "FloodEvent",
                    &serde_json::json!({ "id": Uuid::new_v4(), "payload": payload }),
                )
                .ctx("serialising event data")?
                .id(Uuid::new_v4());
                events.push(ev);
            }
            self.client
                .append_to_stream(stream, &AppendToStreamOptions::default(), events)
                .await
                .ctx("populating stream")?;
            written += this_batch;
        }
        Ok(())
    }

    /// Run a persistent-subscription load test: ensure `p.subscriptions` streams
    /// are populated, create one group per stream, then run `p.clients` competing
    /// consumers per group for `duration` seconds (0 = until cancelled), each
    /// applying `p.ack_mode` to every message. Updates `metrics` as messages are
    /// processed and tears the groups down on exit unless `p.keep`.
    pub async fn subscribe_flood(
        &self,
        p: SubFloodParams,
        metrics: Arc<Metrics>,
        duration: u64,
        reporter: &Reporter,
    ) -> Result<()> {
        let n = p.subscriptions.max(1);
        let streams: Vec<String> = (0..n).map(|i| format!("{}{i}", p.stream_prefix)).collect();

        // Phase 1: make sure every target stream is populated, failing fast (before
        // any subscribing) if streams are missing and we weren't asked to create them.
        reporter.stage(format!("Checking {n} stream(s)…"));
        for stream in &streams {
            let count = self.stream_event_count(stream).await?;
            let empty = matches!(count, None | Some(0));
            if empty {
                if !p.create_streams {
                    let what = if count.is_none() {
                        "does not exist"
                    } else {
                        "is empty"
                    };
                    anyhow::bail!(
                        "stream '{stream}' {what}; pass --create-streams (with --stream-length) \
                         to populate {n} stream(s) before subscribing"
                    );
                }
                reporter.stage(format!(
                    "Populating '{stream}' (up to {} events)…",
                    p.stream_length
                ));
                self.populate_stream(stream, p.stream_length, p.event_size)
                    .await?;
            }
        }

        // Phase 2: create one persistent group per stream (idempotent). Start from
        // the beginning so consumers replay the populated history.
        reporter.stage(format!(
            "Creating {n} subscription group(s) on '{}'…",
            p.group
        ));
        for stream in &streams {
            let opts = PersistentSubscriptionOptions::default().start_from(StreamPosition::Start);
            // Tolerate "already exists" so repeated runs are idempotent. Kept
            // silent so this is safe to call from the TUI's alternate screen.
            let _ = self
                .client
                .create_persistent_subscription(stream.as_str(), &p.group, &opts)
                .await;
        }

        // Phase 3: spawn subscriptions × clients competing consumers.
        metrics.set_active(true);
        let clients = p.clients.max(1);
        reporter.stage(format!(
            "Subscribing {} consumer(s) (ack-mode {:?})…",
            n * clients,
            p.ack_mode,
        ));
        let mut handles = Vec::with_capacity(n * clients);
        for stream in &streams {
            for _ in 0..clients {
                let db = self.clone();
                let metrics = metrics.clone();
                let stream = stream.clone();
                let group = p.group.clone();
                let ack_mode = p.ack_mode;
                let nack_action = p.nack_action;
                handles.push(tokio::spawn(async move {
                    db.consume(&stream, &group, ack_mode, nack_action, metrics)
                        .await;
                }));
            }
        }

        // Phase 4: run for the requested window. When duration is 0 we wait forever;
        // the CLI cancels this future on Ctrl-C.
        reporter.stage(running_note(duration));
        if duration > 0 {
            tokio::time::sleep(Duration::from_secs(duration)).await;
        } else {
            futures::future::pending::<()>().await;
        }

        // Phase 5: unsubscribe every consumer (aborting drops its subscription)
        // and tear down the groups unless asked to keep them.
        reporter.stage("Stopping consumers…");
        for h in &handles {
            h.abort();
        }
        metrics.set_active(false);

        if p.keep {
            reporter.stage("Keeping subscription groups.");
        } else {
            reporter.stage(format!("Deleting {n} subscription group(s)…"));
            for stream in &streams {
                let _ = self
                    .client
                    .delete_persistent_subscription(
                        stream.as_str(),
                        &p.group,
                        &DeletePersistentSubscriptionOptions::default(),
                    )
                    .await;
            }
        }
        Ok(())
    }

    /// One consumer of a persistent subscription: read messages and settle each
    /// per `ack_mode`, recording metrics, until the task is aborted or the
    /// subscription dies.
    async fn consume(
        &self,
        stream: &str,
        group: &str,
        ack_mode: AckMode,
        nack_action: NackAction,
        metrics: Arc<Metrics>,
    ) {
        let mut sub = match self
            .client
            .subscribe_to_persistent_subscription(
                stream,
                group,
                &SubscribeToPersistentSubscriptionOptions::default(),
            )
            .await
        {
            Ok(s) => s,
            Err(_) => {
                metrics.record_error();
                return;
            }
        };

        let mut seen = 0u64;
        loop {
            let event = match sub.next().await {
                Ok(ev) => ev,
                Err(_) => {
                    metrics.record_error();
                    return;
                }
            };
            let bytes = event.get_original_event().data.len() as u64;
            let start = Instant::now();
            let result = match ack_mode {
                AckMode::Ack => sub.ack(&event).await,
                AckMode::Nack => sub.nack(&event, nack_action.into(), "yapper").await,
                AckMode::Mix => {
                    if seen.is_multiple_of(2) {
                        sub.ack(&event).await
                    } else {
                        sub.nack(&event, nack_action.into(), "yapper").await
                    }
                }
                // Never settle: count the receive and let the server time it out.
                AckMode::None => Ok(()),
            };
            seen += 1;
            match result {
                Ok(_) => metrics.record_op(start.elapsed().as_micros() as u32, bytes),
                Err(_) => metrics.record_error(),
            }
        }
    }
}

/// Stage message for the run window of a timed job (0 = runs until cancelled).
fn running_note(duration: u64) -> String {
    if duration > 0 {
        format!("Running for {duration}s…")
    } else {
        "Running until stopped…".to_string()
    }
}

/// Generate a random alphanumeric payload of `size` bytes (minimum 1).
fn random_payload(size: usize) -> String {
    Alphanumeric.sample_string(&mut rand::thread_rng(), size.max(1))
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() > max {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_appends_ellipsis_when_too_long() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 3), "hel…");
        // Newlines are flattened to spaces.
        assert_eq!(truncate("a\nb", 10), "a b");
    }

    #[test]
    fn random_payload_respects_size() {
        assert_eq!(random_payload(32).len(), 32);
        // A zero size is clamped to at least one byte.
        assert_eq!(random_payload(0).len(), 1);
    }

    #[test]
    fn nack_action_maps_to_sdk() {
        assert_eq!(NakAction::from(NackAction::Park), NakAction::Park);
        assert_eq!(NakAction::from(NackAction::Retry), NakAction::Retry);
        assert_eq!(NakAction::from(NackAction::Skip), NakAction::Skip);
        assert_eq!(NakAction::from(NackAction::Stop), NakAction::Stop);
    }

    #[test]
    fn ack_and_nack_modes_parse_from_cli() {
        use clap::ValueEnum;
        assert_eq!(AckMode::from_str("ack", true).unwrap(), AckMode::Ack);
        assert_eq!(AckMode::from_str("mix", true).unwrap(), AckMode::Mix);
        assert_eq!(AckMode::from_str("none", true).unwrap(), AckMode::None);
        assert!(AckMode::from_str("bogus", true).is_err());
        assert_eq!(
            NackAction::from_str("retry", true).unwrap(),
            NackAction::Retry
        );
    }

    // --- Integration tests (require a running KurrentDB) ---
    //
    // These hit a real node and self-skip (early return) unless `YAPPER_TEST_DB` is
    // set, so the default `cargo test` stays hermetic. Run them with:
    //   YAPPER_TEST_DB=1 cargo test
    // against a node reachable via the default config (127.0.0.1:2113, insecure).

    fn it_db() -> Option<Db> {
        if std::env::var("YAPPER_TEST_DB").is_err() {
            return None;
        }
        Db::new(&crate::config::Config::default()).ok()
    }

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let Some(db) = it_db() else {
            return;
        };
        let stream = format!("yapper-it-{}", Uuid::new_v4());
        let version = db
            .write_single(&stream, "ItEvent", "{\"n\":1}")
            .await
            .unwrap();
        assert_eq!(version, 0);

        let events = db.read_single(&stream, 10, false).await.unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("ItEvent"));
        assert!(events[0].contains("\"n\":1"));
    }

    #[tokio::test]
    async fn write_flood_records_expected_op_count() {
        let Some(db) = it_db() else {
            return;
        };
        let metrics = Metrics::new();
        // 2 clients * 2 streams = 4 tasks, each 20 requests in batches of 5 = 4 appends.
        let params = FloodParams {
            clients: 2,
            requests: 20,
            streams: 2,
            event_size: 16,
            batch_size: 5,
            stream_prefix: "yapper-it-".to_string(),
        };
        db.write_flood(params, metrics.clone(), &Reporter::silent())
            .await
            .unwrap();
        assert_eq!(metrics.total_ops(), 16);
        assert_eq!(metrics.total_errors(), 0);
    }

    #[tokio::test]
    async fn subscribe_flood_consumes_populated_stream() {
        let Some(db) = it_db() else {
            return;
        };
        let metrics = Metrics::new();
        let params = SubFloodParams {
            subscriptions: 1,
            clients: 2,
            group: format!("yapper-it-{}", Uuid::new_v4()),
            // Unique prefix so the stream starts empty and is created here.
            stream_prefix: format!("yapper-it-ps-{}-", Uuid::new_v4()),
            ack_mode: AckMode::Ack,
            nack_action: NackAction::Park,
            create_streams: true,
            stream_length: 200,
            event_size: 16,
            keep: false,
        };
        db.subscribe_flood(params, metrics.clone(), 2, &Reporter::silent())
            .await
            .unwrap();
        // The two consumers should have processed at least some of the 200 events.
        assert!(metrics.total_ops() > 0, "expected some messages processed");
    }

    #[tokio::test]
    async fn subscribe_flood_aborts_when_streams_missing() {
        let Some(db) = it_db() else {
            return;
        };
        let metrics = Metrics::new();
        let params = SubFloodParams {
            subscriptions: 1,
            clients: 1,
            group: format!("yapper-it-{}", Uuid::new_v4()),
            stream_prefix: format!("yapper-it-missing-{}-", Uuid::new_v4()),
            ack_mode: AckMode::Ack,
            nack_action: NackAction::Park,
            create_streams: false,
            stream_length: 100,
            event_size: 16,
            keep: false,
        };
        // Without --create-streams the run must fail fast on the missing stream.
        let err = db
            .subscribe_flood(params, metrics, 2, &Reporter::silent())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("--create-streams"));
    }
}
