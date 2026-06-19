use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use kurrentdb::{
    AppendToStreamOptions, Client, ClientSettings, DeletePersistentSubscriptionOptions, EventData,
    PersistentSubscriptionOptions, ReadAllOptions, ReadStreamOptions, StreamPosition,
    SubscribeToAllOptions, SubscribeToPersistentSubscriptionOptions, SubscribeToStreamOptions,
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
    pub async fn write_flood(&self, p: FloodParams, metrics: Arc<Metrics>) -> Result<()> {
        metrics.set_active(true);
        let batch_size = p.batch_size.max(1);

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

        for h in handles {
            let _ = h.await;
        }
        metrics.set_active(false);
        Ok(())
    }

    /// Run a read flood by paging through `$all`, updating `metrics`. This is what
    /// the live dashboard visualizes against the server stats.
    pub async fn read_flood(&self, p: FloodParams, metrics: Arc<Metrics>) -> Result<()> {
        metrics.set_active(true);
        let page = p.batch_size.max(1);

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

        for h in handles {
            let _ = h.await;
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
        db.write_flood(params, metrics.clone()).await.unwrap();
        assert_eq!(metrics.total_ops(), 16);
        assert_eq!(metrics.total_errors(), 0);
    }
}
